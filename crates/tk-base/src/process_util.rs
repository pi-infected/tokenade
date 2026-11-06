//! Process-spawning utilities shared by tokenade-cli and tokenade-mcp.
//!
//! The naïve `Command::output()` reads ALL bytes from child stdout/stderr
//! into memory. For a `cat /var/log/big.log` (multi-GB) or `journalctl -u
//! svc` (~500MB) that's an OOM bomb. The sequential `read_to_string()`
//! pattern after `try_wait()` is worse — the pipe buffer fills, the child
//! blocks on write, and `try_wait` polls forever until timeout.
//!
//! [`drain_capped`] solves both: a background thread reads up to `cap`
//! bytes, then keeps draining (discarding) so the child can finish
//! writing. Bounded memory, no deadlock.

mod decode;
mod detach;
mod drain;
mod group;
mod shell;
pub use decode::*;
pub use detach::*;
pub use drain::*;
pub use group::*;
pub use shell::*;

/// Read up to `cap` bytes from `r` in a background thread, draining
/// the rest into oblivion. Returns the buffer + a boolean indicating
/// whether the source produced more bytes than the cap.
///
/// **Why a thread**: the caller typically spawns a child with both
/// stdout and stderr piped. Reading them sequentially would deadlock
/// when one stream's pipe buffer fills while the caller is draining
/// the other. Each pipe gets its own `drain_capped` thread; the
/// caller `join()`s both after the child exits.
///
/// **Why "keep draining"**: if we stopped reading at `cap`, the child
/// would block on the next write past the pipe buffer (typically
/// 64 KB on Linux). The loop must keep consuming bytes for the child
/// to finish — we just stop COPYING them once `cap` is reached.
/// Is `pid` a live process? The liveness question, in ONE place.
///
/// Asked with `kill(pid, 0)` — nothing is delivered at signal 0, it only runs
/// the existence-and-permission checks — because the obvious alternative is
/// not portable. `Path::new("/proc").join(pid).exists()` is a LINUX question,
/// and `cfg(unix)` includes macOS, which has no `/proc` at all: there it
/// answers "dead" for every pid, including the caller's own.
///
/// MEASURED in the 1.1.7 release build, where both copies of that mistake were
/// live at once: `testtmp`'s sweep deleted the scratch of RUNNING tests on
/// macOS (`a_live_process_is_never_swept` failed on both macOS legs), and
/// `agent::worker_dead` — whose own doc says "Conservative: unknown → assume
/// alive" — reported every live worker as "demonstrably gone", so `follow`
/// would stop following a run that was working.
///
/// Errno decides: `ESRCH` is the only one that means gone. `EPERM` means the
/// process exists and belongs to someone else — alive, and the conservative
/// answer. Pid 0 is never a userland process on any platform we ship to, and
/// is not a pid to ask about either: POSIX reads `kill(0, …)` as "every
/// process in the CALLER's group", which answers success and reads as alive.
pub fn process_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    process_exists(pid)
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    // SAFETY: `kill` with signal 0 sends nothing. It only reports whether the
    // pid exists and whether we could signal it. No memory is touched.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(windows)]
fn process_exists(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: `OpenProcess` only asks the kernel for a handle; nothing is run
    // in the target and no memory of ours is touched. The handle is closed on
    // both paths out.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if !h.is_null() {
            CloseHandle(h);
            return true;
        }
        // The Windows twin of the errno rule above. `ERROR_INVALID_PARAMETER`
        // is the only answer that means "no process with that id";
        // `ERROR_ACCESS_DENIED` means it exists and belongs to someone else,
        // which is alive and the conservative answer.
        GetLastError() != ERROR_INVALID_PARAMETER
    }
}

/// Neither Unix nor Windows: no cheap liveness check, so keep the conservative
/// answer. Nothing we ship to lands here.
#[cfg(not(any(unix, windows)))]
fn process_exists(_pid: u32) -> bool {
    true
}

/// Marks a child process as TOKENADE'S OWN plumbing rather than work an agent
/// asked for.
///
/// Set by [`internal_command`] on every subprocess tokenade runs and then reads
/// ITSELF — `git ls-files` behind `pack`, `curl` behind `web`, `rg` behind the
/// grep hook, `ffprobe` behind a media read. Those bytes never reach an agent,
/// so compacting them would be worse than pointless twice over: the caller
/// parses what it asked for, and the ledger counts a saving nobody was ever
/// going to pay.
///
/// MEASURED 2026-09-04 with a `git` on PATH that wraps (the shape of the
/// pre-0.9.5 PATH shim, which the installer removes but which an install that
/// never upgraded still carries): one `tokenade pack` booked
/// `wrap / git-ls-files 8059 -> 249` plus a `recovery:offered` row — a saving
/// on 2,699 file paths no agent ever saw — and `pack`'s own answer came back
/// 57 bytes different, because it had parsed the FOLD instead of the file list.
pub const INTERNAL_SUBPROCESS_ENV: &str = "TOKENADE_INTERNAL_SUBPROCESS";

/// Build a [`std::process::Command`] for a program tokenade runs for ITSELF.
///
/// One helper so the marking cannot be half-applied: it stamps
/// [`INTERNAL_SUBPROCESS_ENV`], sets the shell proxy's own recursion guard
/// (`TOKENADE_WRAPPING`, which the emitted fragment checks before wrapping
/// anything), and suppresses the Windows console window every such call wanted
/// anyway.
///
/// Use it for a subprocess whose output TOKENADE reads. A subprocess that runs
/// what the AGENT asked for — `wrap`'s shell, `exec`'s script runner, the MCP
/// target — must NOT go through it: compacting those is the product.
pub fn internal_command<S: AsRef<std::ffi::OsStr>>(program: S) -> std::process::Command {
    let mut cmd = std::process::Command::new(program);
    cmd.env(INTERNAL_SUBPROCESS_ENV, "1")
        .env("TOKENADE_WRAPPING", "1");
    configure_no_window(&mut cmd);
    cmd
}

/// Is THIS process one tokenade spawned for its own plumbing?
///
/// True inside the child of an [`internal_command`], however many layers of
/// interception sit in between — a PATH shim, a shell function, a wrapper
/// script — because the marker travels in the environment.
pub fn running_as_internal_subprocess() -> bool {
    std::env::var(INTERNAL_SUBPROCESS_ENV)
        .map(|v| v.trim() == "1")
        .unwrap_or(false)
}

#[cfg(test)]
include!("process_util_tests.rs");
