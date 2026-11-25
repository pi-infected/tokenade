//! Process-wide fatal-signal guard.
//!
//! tokenade links several native components that can SIGSEGV/SIGBUS/SIGILL on
//! an exotic platform/libc: the bundled `sqlite-vec` C extension (ABI mismatch
//! at `sqlite3_auto_extension` time), the tree-sitter C grammars, and the
//! embedding runtime. A Rust binary that takes a fatal signal DUMPS CORE.
//!
//! Prod report (Fedora, no repro available — `core dump causing PC slowdown`):
//! a crash on EVERY hook invocation made systemd-coredump write a multi-hundred-
//! MB tokenade core per fire, dozens of times a minute, grinding the machine to
//! a halt. `std::panic::catch_unwind` does NOT help — these are signals, not
//! unwinds.
//!
//! This installs async-signal-safe handlers that, on a fatal signal:
//!   1. append a one-line breadcrumb to the debug log through a PRE-OPENED fd
//!      (raw `write(2)` only — no allocation, no lock, no Rust formatting), and
//!   2. `_exit(0)` — fail-open (tokenade's standing philosophy: when it can't
//!      run safely, get out of the agent's way) and, crucially, exit WITHOUT
//!      dumping core, so the machine stops being hammered.
//!
//! Unix-only; a no-op on other targets. Idempotent. Opt out with
//! `TOKENADE_NO_CRASH_GUARD=1` (e.g. when you actually want a core dump for
//! debugging).

/// Install the guard. Call once, as early as possible in `main`. Resolves the
/// debug-log path itself (same env var + default the logger uses) so the
/// breadcrumb lands wherever normal logs do. Safe to call when no debug log is
/// configured — the handler simply skips the breadcrumb and still prevents the
/// core dump.
/// Bytes to hand back on stdout if a fatal fault kills the process before the
/// normal path delivered them, plus the exit code to leave with.
///
/// WHY this exists rather than a `catch_unwind`: the release profile is
/// `panic = "abort"`, so **`catch_unwind` never fires in a shipped binary** — a
/// panic goes straight to `abort()`, which lands here as SIGABRT. The handler
/// answered every fault with `_exit(0)` and nothing on stdout, so a panic
/// anywhere in compaction turned `tokenade wrap '<cmd>'` into **"exit 0, no
/// output"** — a failure that reads as a command that succeeded and printed
/// nothing. That is the one outcome the product's contract rules out.
///
/// Arming with the command's ALREADY-REDACTED output makes the promise real for
/// every fatal fault, not only panics: a SIGSEGV in a bundled C extension is
/// salvaged the same way, which no unwinding scheme could do.
///
/// `write` and `_exit` are the two calls allowed in a handler, and the bytes are
/// borrowed, never copied — nothing is allocated on the path that works.
static SALVAGE_PTR: std::sync::atomic::AtomicPtr<u8> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
static SALVAGE_LEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static SALVAGE_CODE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Disarms on drop, and borrows the bytes so they cannot be freed while armed.
pub struct Salvage<'a>(std::marker::PhantomData<&'a str>);

impl Drop for Salvage<'_> {
    fn drop(&mut self) {
        SALVAGE_LEN.store(0, std::sync::atomic::Ordering::SeqCst);
        SALVAGE_PTR.store(std::ptr::null_mut(), std::sync::atomic::Ordering::SeqCst);
    }
}

/// Arm the salvage path for the lifetime of the returned guard. Use it around
/// work that can fault AFTER the output already exists — compaction — and let
/// the guard drop before the normal path writes its own bytes, so the two can
/// never both print.
pub fn arm_salvage(bytes: &str, exit_code: i32) -> Salvage<'_> {
    SALVAGE_CODE.store(exit_code, std::sync::atomic::Ordering::SeqCst);
    SALVAGE_LEN.store(bytes.len(), std::sync::atomic::Ordering::SeqCst);
    SALVAGE_PTR.store(
        bytes.as_ptr() as *mut u8,
        std::sync::atomic::Ordering::SeqCst,
    );
    Salvage(std::marker::PhantomData)
}

/// Armed length, or 0 — for tests, which cannot raise a real fatal signal
/// without taking the harness down with them.
pub fn salvage_armed_len() -> usize {
    if SALVAGE_PTR
        .load(std::sync::atomic::Ordering::SeqCst)
        .is_null()
    {
        0
    } else {
        SALVAGE_LEN.load(std::sync::atomic::Ordering::SeqCst)
    }
}

pub fn install() {
    #[cfg(unix)]
    imp::install();
}

/// Path the breadcrumb fd is opened on: ONLY `$TOKENADE_DEBUG_LOG`, when set
/// and non-empty. Deliberately does NOT fall back to the default
/// `~/.tokenade/debug.log`: `debug_log` itself is a no-op unless the env var
/// is set, and creating the default file on every CLI start (even
/// `install --dry-run`) would touch HOME when the user never enabled logging.
/// Returns `None` otherwise — the guard still installs and still prevents the
/// core dump; it just skips the (best-effort) breadcrumb.
#[cfg(unix)]
fn breadcrumb_path() -> Option<std::path::PathBuf> {
    match std::env::var(crate::debug_log::ENV_VAR) {
        Ok(p) if !p.is_empty() => Some(std::path::PathBuf::from(p)),
        _ => None,
    }
}

#[cfg(unix)]
mod imp {
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::Once;

    /// Pre-opened append fd for the debug log, or -1. Read by the signal
    /// handler (which must not allocate or open files).
    static LOG_FD: AtomicI32 = AtomicI32::new(-1);

    /// The fatal signals we intercept. SIGSEGV/SIGBUS/SIGILL/SIGFPE are the
    /// hardware/native faults a mis-linked C extension throws; SIGABRT covers a
    /// C-side `abort()` (and Rust `panic=abort`) so those don't dump core either.
    const FATAL: [libc::c_int; 5] = [
        libc::SIGSEGV,
        libc::SIGBUS,
        libc::SIGILL,
        libc::SIGFPE,
        libc::SIGABRT,
    ];

    /// Async-signal-safe handler: one raw `write` of a fixed breadcrumb, then
    /// `_exit(0)`. NOTHING here may allocate, lock, or call back into Rust std
    /// I/O — only the listed libc primitives are signal-safe.
    extern "C" fn handler(sig: libc::c_int) {
        let fd = LOG_FD.load(Ordering::Relaxed);
        if fd >= 0 {
            // Fixed prefix + a single-digit-friendly signal suffix. Built on the
            // stack, no formatting machinery. Most fatal signals are < 100.
            let mut buf = [0u8; 64];
            let prefix = b"{\"component\":\"crash-guard\",\"event\":\"fatal_signal\",\"signal\":";
            let mut n = 0usize;
            for &b in prefix {
                buf[n] = b;
                n += 1;
            }
            // itoa for `sig` (non-negative), signal-safe.
            let mut s = if sig < 0 { 0 } else { sig as u32 };
            let mut digits = [0u8; 10];
            let mut d = 0usize;
            loop {
                digits[d] = b'0' + (s % 10) as u8;
                d += 1;
                s /= 10;
                if s == 0 {
                    break;
                }
            }
            while d > 0 {
                d -= 1;
                buf[n] = digits[d];
                n += 1;
            }
            for &b in b"}\n" {
                buf[n] = b;
                n += 1;
            }
            // SAFETY: `buf[..n]` is initialised stack memory and write(2) is async-signal-safe,
            // so it is sound inside this signal handler.
            unsafe {
                libc::write(fd, buf.as_ptr() as *const libc::c_void, n);
            }
        }
        // Hand back the output the run had already produced, if any was armed.
        // Partial writes are looped over; both calls are signal-safe.
        let ptr = super::SALVAGE_PTR.load(core::sync::atomic::Ordering::SeqCst);
        let len = super::SALVAGE_LEN.load(core::sync::atomic::Ordering::SeqCst);
        let code = if !ptr.is_null() && len > 0 {
            let mut off = 0usize;
            while off < len {
                // SAFETY: `ptr` is non-null and the salvage buffer it points to holds `len`
                // bytes, published before this handler was installed; write(2) is async-signal-
                // safe.
                let n = unsafe { libc::write(1, ptr.add(off) as *const libc::c_void, len - off) };
                if n <= 0 {
                    break;
                }
                off += n as usize;
            }
            super::SALVAGE_CODE.load(core::sync::atomic::Ordering::SeqCst)
        } else {
            // Nothing armed: the original fail-open, unchanged.
            0
        };
        // Fail-open: exit cleanly, NO core dump. _exit (not exit) skips atexit
        // handlers / buffer flushes that aren't signal-safe.
        // SAFETY: _exit(2) is async-signal-safe and touches no Rust state; it never returns.
        unsafe {
            libc::_exit(code);
        }
    }

    pub fn install() {
        if std::env::var_os("TOKENADE_NO_CRASH_GUARD").is_some() {
            return;
        }
        install_resolved(super::breadcrumb_path());
    }

    /// The half of [`install`] that reads no environment.
    ///
    /// Split out for `crash_guard_prevents_core_dump_in_child_on_sigsegv`,
    /// which raises the signal in a `fork()`ed child. A fork from a
    /// MULTI-THREADED process gives the child a copy of every lock in the
    /// state it happened to be in, with no thread alive to release it — and
    /// `std::env::var_os` takes a process-global one. With env-mutating tests
    /// running in parallel that is a coin flip, and the losing side hangs
    /// FOREVER: measured once at 2 h 36 min, the child parked in
    /// `futex_do_wait` and the whole `tokenade-core` suite parked on its
    /// `waitpid`. Resolving the env in the parent is what makes the child's
    /// call chain async-signal-safe (`sigaction` only, no allocation).
    pub fn install_resolved(breadcrumb: Option<std::path::PathBuf>) {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            if let Some(path) = breadcrumb {
                // Open for append WITHOUT create: the breadcrumb is best-effort
                // and must never mutate HOME on its own (a fresh CLI start —
                // including `install --dry-run` — would otherwise touch
                // ~/.tokenade/debug.log before anything actually logged). When
                // debug logging is active the file already exists (debug_log
                // created it on the first event), so we attach to it; if not,
                // we skip the breadcrumb and still install the signal handlers.
                if let Ok(f) = std::fs::OpenOptions::new().append(true).open(&path) {
                    use std::os::unix::io::IntoRawFd;
                    LOG_FD.store(f.into_raw_fd(), Ordering::Relaxed);
                }
            }
            // SAFETY: `handler` is an `extern "C"` function that only calls async-signal-safe
            // functions, which is all signal(2) requires of it.
            unsafe {
                for &sig in &FATAL {
                    // SIG_DFL handler replaced by ours. `signal` is portable
                    // enough for our "log + exit" use; we don't need siginfo.
                    libc::signal(sig, handler as *const () as libc::sighandler_t);
                }
            }
        });
    }
}

/// Restore the DEFAULT `SIGPIPE` disposition.
///
/// Rust's runtime sets `SIGPIPE` to `SIG_IGN` before `main`, so a write to a
/// closed pipe returns `EPIPE` and `println!` PANICS. Every ordinary shell
/// idiom that stops reading early — `tokenade map | head`, quitting a `less`,
/// `grep -m1` — therefore printed a Rust panic plus a backtrace note to
/// stderr (MEASURED: 252 bytes of noise, and a non-zero exit).
///
/// Restoring `SIG_DFL` gives the standard Unix behaviour: the process dies
/// quietly on `SIGPIPE`, which is exactly what every other CLI does. No-op on
/// non-Unix, where the problem does not exist.
pub fn restore_sigpipe_default() {
    #[cfg(unix)]
    // SAFETY: restoring the default disposition of SIGPIPE installs no handler and has no
    // memory-safety precondition.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// Write to stdout and die the way every other CLI dies when the reader quit.
///
/// A handful of subcommands are deliberately kept OUT of
/// `restore_sigpipe_default` because they also write to a CHILD's stdin, where
/// SIG_DFL would kill us on a script the interpreter refused and replace a real
/// diagnostic with a silent 141. That reasoning is about the pipe going DOWN to
/// the child; it was applied to the whole process, so the pipe going OUT to the
/// user inherited SIG_IGN too. Measured 2026-08-26:
/// `tokenade exec --lang bash --script 'seq 1 300000' | head -1` printed a Rust
/// runtime panic — `failed printing to stdout: Broken pipe (os error 32)` —
/// where `seq 1 300000 | head -1` exits silently.
///
/// 141 is 128 + SIGPIPE, which is exactly what a shell reports for a process
/// the signal killed, so a caller cannot tell this from the real thing.
pub fn write_stdout_or_exit_on_broken_pipe(text: &str) {
    use std::io::Write;
    let out = std::io::stdout();
    let mut h = out.lock();
    match h.write_all(text.as_bytes()).and_then(|_| h.flush()) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(141),
        _ => {}
    }
}

/// Stderr twin. A closed stderr is rarer but costs the same panic, and the
/// diagnostic is worth even less than the output when nobody is reading.
pub fn write_stderr_or_exit_on_broken_pipe(text: &str) {
    use std::io::Write;
    let err = std::io::stderr();
    let mut h = err.lock();
    match h.write_all(text.as_bytes()).and_then(|_| h.flush()) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(141),
        _ => {}
    }
}

#[cfg(all(test, unix))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breadcrumb_path_prefers_env_var() {
        // Save/restore to avoid cross-test bleed — under THE lock, which this
        // test did not take. `debug_log`'s own tests mutate the same variable,
        // so the two raced: see `platform::ENV_LOCK`.
        let _g = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let saved = std::env::var(crate::debug_log::ENV_VAR).ok();
        std::env::set_var(crate::debug_log::ENV_VAR, "/tmp/tokenade-crash-test.log");
        assert_eq!(
            breadcrumb_path().unwrap(),
            std::path::PathBuf::from("/tmp/tokenade-crash-test.log")
        );
        match saved {
            Some(v) => std::env::set_var(crate::debug_log::ENV_VAR, v),
            None => std::env::remove_var(crate::debug_log::ENV_VAR),
        }
    }

    #[test]
    fn crash_guard_prevents_core_dump_in_child_on_sigsegv() {
        // End-to-end: prove a fatal signal exits cleanly (our handler) instead
        // of killing the process / dumping core.
        //
        // The install runs ONLY in the forked CHILD — never the shared test
        // process — so the handlers (which `_exit(0)` on SIGSEGV/SIGABRT/…)
        // can't mask a real crash in another parallel test. It also exercises
        // idempotency (install twice) before raising the signal.
        //
        // The child calls `install_resolved(None)`, NOT `install()`: see that
        // function for the fork-in-a-threaded-process deadlock the env read
        // used to cause. `None` is what the removed `TOKENADE_DEBUG_LOG` used
        // to produce, stated directly instead of via a process-global mutation.
        // SAFETY: test-only fork. The child runs only async-signal-safe calls before it _exits,
        // as fork in a threaded process requires.
        unsafe {
            let pid = libc::fork();
            if pid == 0 {
                imp::install_resolved(None);
                imp::install_resolved(None); // Once guard → no-op, must not panic.
                libc::raise(libc::SIGSEGV);
                // Unreachable if the handler fired; 42 would flag a miss.
                libc::_exit(42);
            } else if pid > 0 {
                let mut status: libc::c_int = 0;
                // BOUNDED wait. A blocking `waitpid` turns any child-side
                // deadlock into a suite that never finishes and reports
                // nothing — which is strictly worse than a red test, and is
                // how the hang above stayed invisible for hours. Poll instead
                // and fail loudly.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                let mut wedged = false;
                while libc::waitpid(pid, &mut status, libc::WNOHANG) != pid {
                    if std::time::Instant::now() >= deadline {
                        // Reap it before failing: a panic alone would leave the
                        // child parked for as long as the machine lives.
                        libc::kill(pid, libc::SIGKILL);
                        libc::waitpid(pid, &mut status, 0);
                        wedged = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                assert!(
                    !wedged,
                    "child never exited within 10s — wedged, not slow (a fork from this \
                     multi-threaded harness inherited a locked mutex)"
                );
                // WIFEXITED && WEXITSTATUS==0  → handler ran, exited clean.
                let exited = libc::WIFEXITED(status);
                let code = libc::WEXITSTATUS(status);
                let signaled = libc::WIFSIGNALED(status);
                assert!(
                    exited && code == 0,
                    "guard should exit(0) on SIGSEGV; exited={exited} code={code} signaled={signaled}"
                );
                assert!(
                    !signaled,
                    "process must NOT be killed by the signal (no core dump)"
                );
            } else {
                panic!("fork failed");
            }
        }
    }

    /// The guard borrows the bytes, so they cannot be freed while a signal
    /// handler might still read them, and it must disarm the moment the normal
    /// path takes over — otherwise a fault after delivery would print twice.
    #[test]
    fn salvage_is_armed_only_for_the_window_that_needs_it() {
        assert_eq!(super::salvage_armed_len(), 0, "armed before anything asked");
        let out = String::from("the command's output\n");
        {
            let _g = super::arm_salvage(&out, 3);
            assert_eq!(super::salvage_armed_len(), out.len());
        }
        assert_eq!(
            super::salvage_armed_len(),
            0,
            "still armed after the guard dropped: a later fault would print the output twice"
        );
    }
}
