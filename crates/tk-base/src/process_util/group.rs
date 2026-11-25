//! **The process group, and what happens when WE die.**
//!
//! A child killed cleanly leaves its own children alive: it is the
//! group that must be targeted, not the pid. And the terminal has its say — if
//! it is the one owning the group, taking over would steal its
//! foreground.
//!
//! Signal relaying is the half people forget: receiving a Ctrl-C and
//! dying without forwarding it leaves what we launched running, out of reach
//! of the user who just asked for it to stop.

// No `use super::*;`: the group and signals are handled through the OS
// directly, borrowing nothing from the rest of the module.

/// True when stdin is a terminal, i.e. when the TERMINAL — not us — decides who
/// gets a signal.
///
/// Both halves of the mechanism consult this one predicate so they can never
/// disagree, and the reason it exists is a hazard in the fix itself: `wrap`
/// pipes stdout/stderr but INHERITS stdin. Moving the child into its own group
/// makes that group a BACKGROUND group for the controlling terminal, so a
/// wrapped command that reads the tty takes `SIGTTIN` and stops — a hang where
/// it used to work.
///
/// With a terminal there is nothing to fix: it delivers Ctrl-C to the whole
/// foreground group, children and grandchildren alike, which is precisely the
/// guarantee the new group was introduced to recover. Forwarding must be off
/// here too — `kill(-pgid)` would target OUR group and take out the terminal's
/// other jobs.
///
/// The original measurement ran with a non-tty stdin, the one case where
/// this hazard is invisible, so its green could not have caught it.
/// Public because the two decisions must be made by the CALLER, not coupled
/// behind a shared predicate: `sandbox` creates its group unconditionally (its
/// stdio is fully piped and `kill_tree` depends on the group existing), so
/// gating its forwarding on this would silently skip it there.
#[cfg(unix)]
pub fn terminal_owns_the_group() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// Off unix there is no group to own, so nothing is ever suppressed.
#[cfg(not(unix))]
pub fn terminal_owns_the_group() -> bool {
    false
}

/// No-op: process groups are a unix concept. Windows would need a Job Object,
/// which is NOT done — say so rather than imply the guarantee holds there.
#[cfg(not(unix))]
pub fn configure_own_group(_cmd: &mut std::process::Command) {}

/// Ask the kernel to signal this child when WE die — including deaths we can
/// never handle.
///
/// [`forward_termination_to`] relays SIGINT/SIGTERM/SIGHUP, which are catchable
/// by definition. `SIGKILL` is not, and an agent that times a command out
/// frequently sends exactly that: measured 2026-08-08, `kill -9` on `wrap` left
/// `sleep 3999` running, so the case most likely to happen in the field was the
/// one case the relay could not cover.
///
/// `PR_SET_PDEATHSIG` closes it for the DIRECT child: the kernel delivers the
/// signal when the parent dies, whatever killed it. What it does NOT cover is a
/// shell route's pipeline — `sh` dies, its children do not inherit the setting —
/// so this narrows the hole rather than sealing it, and the doc says so.
///
/// Linux-only (`prctl` is not portable); a documented no-op elsewhere.
#[cfg(target_os = "linux")]
pub fn configure_die_with_parent(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `pre_exec` runs in the forked child before `exec`, where only
    // async-signal-safe work is allowed. This is a single `prctl` syscall and
    // nothing else; its result is ignored because failure only costs us the
    // extra guarantee, never correctness.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
}

/// No-op off Linux — `PR_SET_PDEATHSIG` has no portable equivalent, so a
/// `SIGKILL`ed parent still orphans its child there. Stated, not implied.
#[cfg(not(target_os = "linux"))]
pub fn configure_die_with_parent(_cmd: &mut std::process::Command) {}

/// A living witness that we are still here. Dropping it without
/// [`GroupReaper::stand_down`] kills the guarded process group.
///
/// See [`guard_group_against_our_death`] for why this exists at all.
#[cfg(unix)]
pub struct GroupReaper {
    write_end: Option<std::os::unix::io::OwnedFd>,
}

#[cfg(unix)]
impl GroupReaper {
    /// The work is finished and reaped — tell the reaper to exit quietly.
    ///
    /// Deliberately consumes `self`: forgetting to call it is the SAFE
    /// direction (the group gets killed, and by then it is already dead), while
    /// calling it twice or after a partial write is not expressible.
    pub fn stand_down(mut self) {
        if let Some(fd) = self.write_end.take() {
            use std::io::Write;
            let mut f = std::fs::File::from(fd);
            let _ = f.write_all(b"ok\n");
        }
    }
}

/// Off unix there is no process group to guard.
#[cfg(not(unix))]
pub struct GroupReaper;

#[cfg(not(unix))]
impl GroupReaper {
    /// No-op counterpart so call sites need no `cfg`.
    pub fn stand_down(self) {}
}

/// Post a tiny forked process outside our blast radius that kills the group led
/// by `pgid` if we die WITHOUT standing it down.
///
/// **Why the relay is not enough.** [`forward_termination_to`] can only relay
/// signals that are catchable, and an agent timing a command out sends
/// `SIGKILL`. [`configure_die_with_parent`] covers the DIRECT child on Linux,
/// but `PR_SET_PDEATHSIG` is not inherited, so a shell route's pipeline keeps
/// running after `sh` dies — measured 2026-08-08 on all three shell forms
/// (`a | b`, `a; b`, `(a)`).
///
/// **And the group move made one case worse, which is the real reason this
/// exists.** Before [`configure_own_group`], the work sat in OUR group, so a
/// harness that kills the whole process group on timeout — the common shape —
/// took the pipeline down with us. Putting the child in its own group moved the
/// work OUT of the group the agent aims at. Measured against 1.1.6 on the same
/// probe: `kill -9 -<pgid>` used to leave nothing behind and now leaves the
/// pipeline running. Taking a process group away from the terminal means owning
/// its death, including the deaths we cannot execute any code for.
///
/// **How.** A pipe whose write end we alone hold (`CLOEXEC`, so the work never
/// inherits it) is the reaper's stdin. It blocks on one `read`:
/// - we finish and [`GroupReaper::stand_down`] writes `ok` → it exits quietly;
/// - we die by any means, including `SIGKILL` → the write end closes, the read
///   hits EOF, and it sends `SIGKILL` to the group.
///
/// No polling, no timer, no signal to catch, and — unlike `PR_SET_PDEATHSIG` —
/// it works on macOS too. The reaper `setsid`s itself so neither a group kill
/// aimed at us nor a `SIGHUP` can take out the thing whose whole job is to
/// outlive us.
///
/// **Why a bare `fork` and not a small `sh -c`.** Both work; the shell costs
/// **2.2 ms** measured, on the path every hooked and shimmed command takes, and
/// against a `wrap` that runs in ~10 ms that is a fifth of the budget the
/// latency work of the same week had just won back. Forking without `exec`
/// costs **0.65 ms** on the same probe (interleaved A/B, 100 invocations each:
/// 42.6 ms → 43.2 ms), and that is measured against the 620 MB DEBUG binary,
/// where the page-table copy is at its most expensive. It also drops the
/// assumption that
/// `/bin/sh` exists and behaves, in the one code path whose entire purpose is
/// to still work when things are going wrong.
///
/// The kill names the group EXPLICITLY (`kill(-pgid)`) rather than saying
/// "my group": were the placement ever to fail, an implicit `kill 0` would
/// aim at whatever group the reaper landed in — potentially the agent's. The
/// blast radius must not depend on a syscall having succeeded.
///
/// PRECONDITION, and it is the CALLER's to check, exactly as for
/// [`forward_termination_to`]: `pgid` must actually lead a group. `wrap` only
/// creates one when stdin is not a terminal, so it must ask
/// [`terminal_owns_the_group`] first; `sandbox` creates its group
/// unconditionally, so it must NOT. Gating this behind the shared predicate
/// looks tidier and silently drops the guarantee for `tokenade exec` under a
/// terminal — the same coupling the doc on that predicate already warns about,
/// which did not stop me writing it.
///
/// Returns `None` — no guarantee, no pretence — when the pipe or the fork fails.
#[cfg(unix)]
pub fn guard_group_against_our_death(pgid: u32) -> Option<GroupReaper> {
    use std::os::unix::io::{FromRawFd, OwnedFd};
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `fds` is a valid 2-element array; both descriptors are wrapped in
    // `OwnedFd` immediately on success so neither can leak.
    let made = unsafe {
        #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "netbsd"))]
        {
            libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC)
        }
        // macOS has no `pipe2`; set the flag afterwards. The window between the
        // two calls is harmless here — this process spawns the reaper itself and
        // the work was spawned BEFORE the pipe existed, so nothing can inherit it.
        #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "netbsd")))]
        {
            let rc = libc::pipe(fds.as_mut_ptr());
            if rc == 0 {
                libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
                libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
            }
            rc
        }
    };
    if made != 0 {
        return None;
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);
    // SAFETY: `fork` is the only way to get a process that outlives a `SIGKILL`
    // of this one without paying for an `exec`. The child below calls NOTHING
    // that is not async-signal-safe — `close`, `setsid`, `read`, `kill`,
    // `_exit` — which is exactly the rule that makes forking a multithreaded
    // process safe. In particular it allocates nothing, takes no lock, runs no
    // destructor and never returns into Rust.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        // SAFETY: both are fds we own and have not wrapped yet.
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return None;
    }
    if pid == 0 {
        // SAFETY: see above — async-signal-safe calls only, ending in `_exit`.
        unsafe {
            // `fork` copies EVERY descriptor, and a pipe stays open as long as
            // ANY process holds a writer. That is not an optimisation detail —
            // it is a hang: the sandbox hands a >64 KiB script to `sh -s` on the
            // child's stdin and then CLOSES it so the shell sees EOF and runs.
            // A reaper holding a duplicate of that write end means the EOF never
            // arrives and the script never starts. Measured: the gate failed on
            // `e2e_a_huge_script_still_runs_when_stdin_is_piped` the first time
            // this was wired in.
            //
            // So keep exactly one descriptor. `dup2` puts the read end on fd 0
            // (closing our inherited stdin in the same call), then everything
            // above it goes — our stdout and stderr included, so a consumer
            // reading THEM sees EOF when we die rather than when the reaper does.
            libc::dup2(read_fd, 0);
            // Everything above fd 0 goes. `close_range` does it in ONE syscall
            // and is not bounded by a guess: the soft `RLIMIT_NOFILE` on this
            // machine is 524288, so a "close 1..1024" loop is a heuristic that
            // happens to hold rather than a guarantee — and this is the one code
            // path whose job is to work when things are already going wrong.
            // Falls back to the loop when the kernel is older than 5.9 or the
            // platform has no such call; 1024 covers every default soft limit
            // that ships (macOS is 256).
            #[cfg(target_os = "linux")]
            let swept = libc::syscall(
                libc::SYS_close_range,
                1 as libc::c_uint,
                libc::c_uint::MAX,
                0 as libc::c_uint,
            ) == 0;
            #[cfg(not(target_os = "linux"))]
            let swept = false;
            if !swept {
                let mut fd = 1;
                while fd < 1024 {
                    libc::close(fd);
                    fd += 1;
                }
            }
            // Out of our session, so the group kill aimed at us — or a SIGHUP —
            // cannot take out the one process whose job is to outlive us.
            libc::setsid();
            let read_fd = 0;
            let mut buf = [0u8; 2];
            let mut got = 0usize;
            while got < buf.len() {
                let n = libc::read(
                    read_fd,
                    buf.as_mut_ptr().add(got) as *mut libc::c_void,
                    buf.len() - got,
                );
                if n <= 0 {
                    break; // EOF (we died) or error — either way, no stand-down
                }
                got += n as usize;
            }
            if !(got == 2 && buf[0] == b'o' && buf[1] == b'k') {
                libc::kill(-(pgid as i32), libc::SIGKILL);
            }
            libc::_exit(0);
        }
    }
    // SAFETY: parent side. `read_fd` is ours to close; `write_fd` becomes the
    // handle whose closure IS the death signal, so it must outlive this scope.
    unsafe { libc::close(read_fd) };
    Some(GroupReaper {
        // SAFETY: `write_fd` came from pipe(2) above and nothing else owns it, so `OwnedFd` may
        // take it.
        write_end: Some(unsafe { OwnedFd::from_raw_fd(write_fd) }),
    })
}

/// No-op off unix: there is no process group to guard, and Windows would need a
/// Job Object. Returning `None` keeps the caller's `cfg`-free shape while
/// promising nothing.
#[cfg(not(unix))]
pub fn guard_group_against_our_death(_pgid: u32) -> Option<GroupReaper> {
    None
}

#[cfg(unix)]
mod term_forward {
    use std::sync::atomic::{AtomicI32, Ordering};

    /// Process GROUP to relay a termination signal to, or 0 when disarmed.
    static TARGET_PGID: AtomicI32 = AtomicI32::new(0);

    /// Whether WE must die once the signal has been relayed.
    ///
    /// `wrap`, `raw` and `exec` end at a `wait()`/poll that returns as soon as
    /// the child is gone, so staying alive one moment longer lets them deliver
    /// the partial output — that is why the relay does not re-raise by default.
    /// `mcp-proxy` is the exception and it HUNG: it joins its stdin pump first,
    /// and no child death ever closes stdin. Catching SIGTERM there turned a
    /// process that used to die into one that could not be killed (measured:
    /// 504 s and still running, gate stuck).
    static EXIT_AFTER_RELAY: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    /// The signals that mean "stop", as opposed to the faults `crash_guard`
    /// owns. The two never overlap, so both can be installed at once.
    const TERMINATING: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

    /// Async-signal-safe: a single `kill(2)` and nothing else. No allocation,
    /// no lock, no Rust I/O.
    extern "C" fn relay(sig: libc::c_int) {
        let pgid = TARGET_PGID.load(Ordering::SeqCst);
        if pgid > 0 {
            // SAFETY: `kill` is async-signal-safe. A negative pid targets the
            // whole process group.
            unsafe {
                libc::kill(-pgid, sig);
            }
        }
        if EXIT_AFTER_RELAY.load(Ordering::SeqCst) {
            // Restore the default disposition and re-raise: both are
            // async-signal-safe, and the process dies with the right status.
            // SAFETY: signal(2) and raise(3) are both async-signal-safe.
            unsafe {
                libc::signal(sig, libc::SIG_DFL);
                libc::raise(sig);
            }
        }
    }

    /// Relay SIGINT/SIGTERM/SIGHUP to the group led by `pid` until disarmed.
    ///
    /// WHY this exists: `wrap` sits between the agent and the real command on
    /// ranks 1 and 2 of the ladder, so EVERY cancelled or timed-out command
    /// passes through it. It spawned the child and blocked in `wait()` with no
    /// handler installed, so killing the wrapper left the real work running.
    /// Measured 2026-08-08: `kill <wrap-pid>` during `sleep 293` left `sleep`
    /// alive, and on the shell route (`sleep 291 | cat`) the shell was never
    /// signalled at all, so the grandchild survived too. An orphaned build
    /// keeps its lock with nobody reading its output — that cost this project
    /// a stalled gate whose slowness read as slow tests.
    ///
    /// It deliberately does NOT re-raise. Letting `wait()` return means the
    /// normal path still delivers the partial, compacted output and exits with
    /// the child's status, which `cmd_wrap` already derives correctly for a
    /// child that died on a signal. Re-raising would trade that output for a
    /// marginally tidier exit status.
    /// PRECONDITION, enforced by the caller: `pid` leads its own process
    /// group. Arming without that would aim `kill(-pgid)` at OUR OWN group and
    /// take out the terminal's other jobs — which is why this is a bare
    /// mechanism and the decision lives at each call site.
    pub fn arm(pid: u32, exit_after: bool) {
        EXIT_AFTER_RELAY.store(exit_after, Ordering::SeqCst);
        TARGET_PGID.store(pid as i32, Ordering::SeqCst);
        // SAFETY: installing a handler whose body is a single `kill`.
        unsafe {
            for sig in TERMINATING {
                libc::signal(sig, relay as *const () as libc::sighandler_t);
            }
        }
    }

    /// Stop relaying — the child is gone and its pid could be recycled.
    pub fn disarm() {
        TARGET_PGID.store(0, Ordering::SeqCst);
    }
}

/// Relay termination signals to the group led by `pid`. See the unix module for
/// the measurement that motivates it. No-op off unix.
#[cfg(unix)]
pub fn forward_termination_to(pid: u32) {
    term_forward::arm(pid, false);
}

/// Relay, then DIE — for a caller whose main loop cannot notice the child's
/// death. `mcp-proxy` joins a stdin pump before waiting on the child, so
/// suppressing our own death makes it unkillable by SIGTERM. No-op off unix.
#[cfg(unix)]
pub fn forward_termination_and_die(pid: u32) {
    term_forward::arm(pid, true);
}

/// No-op off unix, like its sibling.
#[cfg(not(unix))]
pub fn forward_termination_and_die(_pid: u32) {}

/// No-op off unix: see [`configure_own_group`] — Windows needs a Job Object and
/// does not have one, so the orphaning behaviour there is UNCHANGED.
#[cfg(not(unix))]
pub fn forward_termination_to(_pid: u32) {}

/// Stop relaying termination signals. Call once the child has been reaped.
#[cfg(unix)]
pub fn stop_forwarding_termination() {
    term_forward::disarm();
}

/// No-op off unix.
#[cfg(not(unix))]
pub fn stop_forwarding_termination() {}

/// Hide the console window Windows allocated **solely for this process**.
///
/// A GUI agent (Claude Code / an Electron or Node harness with no console)
/// that launches `tokenade.exe` — a console-subsystem binary — without
/// `CREATE_NO_WINDOW` makes Windows allocate and show a new console for
/// tokenade itself. For a millisecond-long hook invocation that console
/// pops up and closes: the exact "windows flashing in a loop" the beta
/// tester reported. We cannot change how the agent spawns us, so we hide
/// the window as the very first thing `main` does.
///
/// Guarded by the standard "am I the sole owner of this console" test so an
/// INTERACTIVE session is never touched: `GetConsoleProcessList == 1` means
/// the console is attached to *only* this process, i.e. it was freshly
/// allocated for us (spurious). A real terminal is ALWAYS shared with the
/// shell that launched us (`cmd`/`pwsh`/Windows-Terminal → count ≥ 2), so a
/// human running `tokenade …` by hand keeps their window; only the
/// agent/service case where tokenade is the lone console process is hidden.
/// (This is the same heuristic the Go toolchain and the Python `py` launcher
/// use.) Opt out entirely with `TOKENADE_KEEP_CONSOLE=1`.
///
/// The console is only HIDDEN, not freed: the (now invisible) screen buffer
/// and all stdio handles stay valid, so stdout/stderr the agent captures and
/// the exit code are unaffected, and children that inherit it don't allocate
/// their own windows either. No-op off Windows.
#[cfg(windows)]
pub fn hide_spurious_console() {
    extern "system" {
        fn GetConsoleWindow() -> *mut core::ffi::c_void;
        fn GetConsoleProcessList(lpdw_process_list: *mut u32, dw_count: u32) -> u32;
        fn ShowWindow(hwnd: *mut core::ffi::c_void, n_cmd_show: i32) -> i32;
    }
    const SW_HIDE: i32 = 0;
    // Env override: TOKENADE_KEEP_CONSOLE=1 disables the guard entirely
    // (debugging a hook by hand, or an unforeseen terminal we misjudge).
    if std::env::var("TOKENADE_KEEP_CONSOLE").as_deref() == Ok("1") {
        return;
    }
    unsafe {
        let hwnd = GetConsoleWindow();
        if hwnd.is_null() {
            return; // no console at all (already windowless) — nothing to do
        }
        let mut pids = [0u32; 4];
        let n = GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32);
        if n == 1 {
            // Console belongs to us alone → it was spawned for us → hide it.
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

/// No-op on non-Windows.
#[cfg(not(windows))]
pub fn hide_spurious_console() {}

/// Run `f` (a spawn/exec of a file this process just WROTE), retrying while the
/// OS reports `ETXTBSY` ("Text file busy").
///
/// Writing an executable and running it is racy in any MULTITHREADED process,
/// and it has nothing to do with the writer being careless: the write fd is
/// closed before the exec, but a SIBLING thread that `fork`s in that window
/// (tokenade runs background threads — the usage reporter, hook spawns)
/// inherits the still-open write fd, and the kernel refuses to exec a file any
/// process holds open for writing. `O_CLOEXEC` does not help: it fires at the
/// child's exec, not at fork, so the fd lives for the microseconds in between.
///
/// Measured: the e2e suite hit it about once per six full runs and never when
/// a test ran alone — the signature of exactly this window. The same shape
/// exists in [`crate::updater`], where the post-download `--version` probe execs
/// the freshly written binary; there ETXTBSY makes the probe fail and the
/// update get REFUSED, leaving the user pinned to an old build.
///
/// Retries are bounded and the error is returned unchanged when it is not
/// ETXTBSY, so a genuinely missing or non-executable file still fails fast.
pub fn retry_while_text_file_busy<T>(
    mut f: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    const MAX_ATTEMPTS: u32 = 50;
    const BACKOFF: std::time::Duration = std::time::Duration::from_millis(20);
    let mut attempt = 0;
    loop {
        match f() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                attempt += 1;
                if attempt >= MAX_ATTEMPTS {
                    return Err(e);
                }
                std::thread::sleep(BACKOFF);
            }
            other => return other,
        }
    }
}
