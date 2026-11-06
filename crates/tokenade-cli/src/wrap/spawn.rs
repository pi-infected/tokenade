//! **Launching the real program, and taking nothing from it.**
//!
//! The exit code is returned EXACTLY, and a process killed by a signal has no
//! code — confusing it with an ordinary failure passes an OOM kill off as a
//! test that failed correctly. Failure hints live here too: a command that
//! succeeds while printing errors, and one that fails while printing nothing,
//! are both misread unless they are named.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ShellKind {
    Posix,
    PowerShell,
    Cmd,
}

impl ShellKind {
    pub(crate) fn parse(s: &str) -> Option<ShellKind> {
        match s.to_ascii_lowercase().as_str() {
            "posix" | "sh" | "bash" => Some(ShellKind::Posix),
            "powershell" | "pwsh" | "ps" => Some(ShellKind::PowerShell),
            "cmd" | "cmd.exe" | "bat" => Some(ShellKind::Cmd),
            _ => None,
        }
    }

    /// Best-effort detection of the shell `tokenade wrap` was invoked FROM,
    /// for the MANUAL (non-hook) path where no `--shell` was passed. The
    /// Claude Code hooks always pass `--shell` explicitly, so this only
    /// affects a human / non-CC agent running `tokenade wrap` by hand.
    /// Defaults to Posix (the historical behaviour) when ambiguous.
    pub(crate) fn detect_from_env() -> ShellKind {
        // PowerShell sets PSModulePath (and $PSEdition); cmd.exe sets neither
        // but sets PROMPT=$P$G by default. A POSIX shell sets SHELL=/…/bash.
        if std::env::var_os("SHELL").is_some() {
            return ShellKind::Posix; // a POSIX shell exported SHELL
        }
        // Only on Windows: CI runners (GitHub's Ubuntu images) and pwsh
        // installs export PSModulePath to every process on Linux/macOS, where
        // running a POSIX command line through PowerShell breaks it.
        if cfg!(windows) && std::env::var_os("PSModulePath").is_some() {
            return ShellKind::PowerShell;
        }
        ShellKind::Posix
    }
}

/// Spawn the inner command under the requested shell, stdout+stderr piped.
/// Posix: `sh -c` (Windows fallback: `Git\bin\bash.exe -c`, which bootstraps
/// the MSYS coreutils onto PATH). PowerShell: `powershell.exe -NoProfile
/// -NonInteractive -Command` (fallback `pwsh` on PATH). Cmd: `cmd.exe /C`.
pub(crate) fn spawn_wrapped(
    shell_cmd: &str,
    kind: ShellKind,
) -> std::io::Result<std::process::Child> {
    let piped = || (std::process::Stdio::piped(), std::process::Stdio::piped());
    // The inner shell re-sources the tokenade proxy (BASH_ENV/ENV), which
    // re-aliases every proxyable command. FORCE the re-entry guard into the
    // child env here rather than trusting the shell to propagate the
    // `VAR=val cmd` prefix — a stripped/reset env in the parent shell would
    // otherwise let the child re-wrap and recurse (FUNCNEST report).
    match kind {
        ShellKind::Posix => {
            let (o, e) = piped();
            let mut c = Command::new("sh");
            c.arg("-c")
                .arg(shell_cmd)
                .env("TOKENADE_WRAPPING", "1")
                .stdout(o)
                .stderr(e);
            tokenade_core::process_util::configure_no_window(&mut c);
            // Own group: killing `wrap` must reach the PIPELINE, not just the
            // `sh` that hosts it. Measured — the grandchild used to survive.
            tokenade_core::process_util::configure_own_group(&mut c);
            tokenade_core::process_util::configure_die_with_parent(&mut c);
            let spawned = c.spawn();
            #[cfg(windows)]
            let spawned = spawned.or_else(|orig| {
                let Some(bash) = tokenade_core::process_util::git_bash() else {
                    return Err(orig);
                };
                let mut cmd = Command::new(bash);
                cmd.arg("-c")
                    .arg(shell_cmd)
                    .env("TOKENADE_WRAPPING", "1")
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                tokenade_core::process_util::configure_no_window(&mut cmd);
                cmd.spawn().or(Err(orig))
            });
            spawned
        }
        ShellKind::PowerShell => {
            // -NoProfile: don't load the user profile (faster, deterministic).
            // -NonInteractive: never prompt. -Command -: read the script from
            // the arg. `powershell.exe` is Windows PowerShell (always present
            // when the PowerShell tool is active); fall back to `pwsh` (PS 7+,
            // cross-platform) when the legacy launcher isn't on PATH.
            let try_one = |exe: &str| {
                let mut c = Command::new(exe);
                c.args(["-NoProfile", "-NonInteractive", "-Command", shell_cmd])
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                tokenade_core::process_util::configure_no_window(&mut c);
                tokenade_core::process_util::configure_own_group(&mut c);
                tokenade_core::process_util::configure_die_with_parent(&mut c);
                c.spawn()
            };
            try_one("powershell.exe").or_else(|_| try_one("pwsh"))
        }
        ShellKind::Cmd => {
            let (o, e) = piped();
            // Verbatim, not `args(["/C", …])`: Rust's quoting is not cmd's,
            // and a line with its own quotes ran nothing (#10).
            #[cfg(windows)]
            let mut c = tokenade_core::process_util::cmd_exe(shell_cmd);
            #[cfg(not(windows))]
            let mut c = {
                let mut c = Command::new("cmd.exe");
                c.args(["/C", shell_cmd]);
                c
            };
            c.stdout(o).stderr(e);
            tokenade_core::process_util::configure_no_window(&mut c);
            c.spawn()
        }
    }
}

/// Exit code for a finished child. `status.code()` is None when the child
/// died on a SIGNAL (SIGKILL/SIGSEGV/OOM-kill) — `unwrap_or(0)` reported
/// SUCCESS to the agent for a crashed command. Map to the shell convention
/// 128+signal (or 1 when the signal is unknown / non-unix).
pub(crate) fn exit_code_of(status: std::process::ExitStatus) -> i32 {
    if let Some(c) = status.code() {
        // A shell that does not `exec` its last command (dash, Debian/Ubuntu's
        // `sh`) reports its child's signal death as 128+signal, so the signal
        // never reaches us as one. Same convention, same notice.
        #[cfg(unix)]
        if (129..=159).contains(&c) {
            disclose_signal_death(c - 128);
        }
        return c;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            disclose_signal_death(sig);
            return 128 + sig;
        }
    }
    1
}

/// Name the signal, once, in the text the MODEL reads.
///
/// The number was already right — `exit_code_of` maps a signal death to
/// 128+signal, and its own comment records the day `unwrap_or(0)` reported
/// SUCCESS for a crashed command. What it never did was SAY so. Measured
/// 2026-08-27 through the binary: `sh -c 'kill -SEGV $$'` wrapped gives exit
/// 139 and **zero bytes of output**, and `timeout -s KILL 1 …` gives 137 with
/// a normal-looking fold banner. In both the model has only an exit code,
/// which many harnesses never surface as text — so a command that died halfway
/// reads exactly like one that finished quietly.
///
/// Worse is the shape where the shell survives the kill:
/// `timeout -s KILL 1 <loop>; echo DONE` exits **0** and ends on `DONE`. The
/// shell hides that as much as we do, but the fold banner ("1 912 788 tokens
/// folded away") invites reading the shortness as compaction rather than as a
/// death, so tokenade is the one adding the misleading explanation.
///
/// Emitted from `exit_code_of` rather than from a call site: this is the one
/// function that learns a signal killed the child, and it has six callers.
/// A notice at one of them is a notice the other five do not give.
///
/// `SIGPIPE` is deliberately silent — `… | head -20` closes the pipe on
/// purpose and the producer dies every time; naming it would be noise on the
/// most ordinary shell idiom there is.
#[cfg(unix)]
pub(super) fn disclose_signal_death(sig: i32) {
    const SIGPIPE: i32 = 13;
    if sig == SIGPIPE {
        return;
    }
    // At most once per process: several helpers call `exit_code_of` on the
    // same status.
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let name = match sig {
        2 => "SIGINT (interrupted)",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT (abort)",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL (killed outright — often the out-of-memory killer)",
        11 => "SIGSEGV (segmentation fault)",
        15 => "SIGTERM (asked to stop)",
        _ => "a signal",
    };
    eprintln!(
        "[tokenade] this command did not finish: it died on {name}, exit {}. \
         Whatever output you see above is what it managed to print before that, \
         not a result.",
        128 + sig
    );
}

/// Programs whose non-zero exit is the whole point of running them. Only a
/// command that RAN one of these can have its result contradicted below — a
/// `grep "test result: FAILED" build.log` legitimately prints the marker and
/// legitimately succeeds, and must stay silent.
/// Programs that RUN another program. The stage leader is theirs, not the
/// build's, so they are stepped over like a `VAR=val` prefix.
pub(super) const COMMAND_WRAPPERS: &[&str] = &[
    "timeout", "env", "nice", "stdbuf", "time", "command", "exec", "nohup", "sudo",
];

pub(super) const BUILD_OR_TEST_PROGRAMS: &[&str] = &[
    "cargo", "npm", "pnpm", "yarn", "pytest", "go", "tsc", "make", "mvn", "gradle", "jest",
    "vitest", "rustc", "cmake", "ctest", "dotnet", "just", "bazel", "tox", "nox",
];

/// True when some stage of `cmd` invokes a build or test runner.
///
/// Stage leaders only: the first real word after each `|`, `;`, `&&`, `||` or
/// newline, skipping `VAR=val` prefixes. `cargo` appearing as an ARGUMENT
/// (`grep cargo notes.txt`) is not a run of cargo.
pub(super) fn runs_a_build_or_test(cmd: &str) -> bool {
    cmd.split(['|', ';', '&', '\n']).any(|stage| {
        let mut words = stage.split_whitespace().peekable();
        // Skip what stands BEFORE the program: `VAR=val`, and a wrapper that
        // runs another command. `timeout 60 cargo test` reads `timeout` as the
        // stage leader, so the run of cargo behind it was invisible to this
        // gate — measured on the corpus, skipping wrappers opens it on 950
        // more commands, 110 of which carry evidence of a failure the exit
        // code did not report.
        while let Some(t) = words.peek() {
            if is_env_assignment(t) {
                words.next();
                continue;
            }
            let base = t.rsplit(['/', '\\']).next().unwrap_or(t);
            if !COMMAND_WRAPPERS.contains(&base) {
                return BUILD_OR_TEST_PROGRAMS.contains(&base);
            }
            words.next();
            // Its own arguments: flags, and the bare numbers they take —
            // `timeout`'s mandatory duration and `nice -n 19`'s level. A
            // program is never spelled as a number, so this cannot swallow
            // the thing being wrapped.
            while words.peek().is_some_and(|w| {
                w.starts_with('-') || {
                    let d = w.strip_suffix(['s', 'm', 'h', 'd']).unwrap_or(w);
                    !d.is_empty() && d.chars().all(|c| c.is_ascii_digit() || c == '.')
                }
            }) {
                words.next();
            }
        }
        false
    })
}

/// Text that only a FAILED build or test prints.
///
/// Every marker here is one the corpus actually produced. The first version of
/// this check also counted `\d+ failed`, and that is the trap: every GREEN
/// cargo summary ends `test result: ok. 123 passed; 0 failed`, so the count of
/// swallowed failures came out at 2797 against a true 830. A marker that
/// matches its own success case is not evidence.
pub(super) fn failure_evidence(text: &str) -> Option<&str> {
    // `error[E` is rustc's code-prefixed diagnostic; `error TS` is tsc's.
    for m in [
        "test result: FAILED",
        "error[E",
        "error TS",
        "npm ERR! code",
        "BUILD FAILURE",
        // A rust panic, a rustc error with no code (`error: could not compile`
        // closes every failing cargo build, coded diagnostic or not), and an
        // uncaught python exception. Together they add 312 disclosures to the
        // 2016 the list above already makes, measured over 86 996 successful
        // command results.
        "panicked at",
        "error: could not compile",
        "Traceback (most recent call last)",
    ] {
        if text.contains(m) {
            return Some(m);
        }
    }
    // Naming one cargo error at a time does not converge: re-measured over the
    // WHOLE transcript corpus (99 137 command results, where the earlier census
    // had reached 7.8 % of it), 233 results with no marker above carry a line
    // opening `error: ` and every recurring one is cargo refusing the command
    // it was given — `test failed, to rerun pass`, `no library targets found`,
    // `unexpected argument`, `unrecognized subcommand`, `no bin target named`,
    // `failed to run custom build command`. `error: could not compile` was one
    // member of that family; the prefix is the rule it belongs to. A further
    // 210 carry `FAIL ` opening a line, which is how `go test` and jest close a
    // failing run.
    //
    // Line START, never mid-line: `error:` inside a sentence is prose, and a
    // path like `src/error.rs` is a filename. The caller has already required a
    // build-or-test command and a zero exit, so what is left is a build tool
    // reporting its own refusal.
    // The LINE is the evidence here, not the prefix: quoting `"error: "` back
    // at the reader names the rule instead of what happened, which is the
    // defect this whole disclosure exists to avoid.
    text.lines().find(|l| line_opens_with_failure(l)).map(clip)
}

/// Does this line OPEN with a failure marker? See [`failure_evidence`].
pub(super) fn line_opens_with_failure(line: &str) -> bool {
    if line.starts_with("error: ") {
        return true;
    }
    // `FAIL` must open a word — `go test` writes `FAIL\tpkg`, jest `FAIL path`
    // — so a run whose own output mentions FAILSAFE is not a failure.
    line.strip_prefix("FAIL")
        .is_some_and(|rest| rest.starts_with(char::is_whitespace))
}

/// The first `MAX` characters of `line`, on a character boundary, so a
/// thousand-column build line does not become the whole message.
pub(super) fn clip(line: &str) -> &str {
    const MAX: usize = 96;
    let line = line.trim_end();
    match line.char_indices().nth(MAX) {
        Some((i, _)) => &line[..i],
        None => line,
    }
}

/// Say so when the exit code claims success and the output says otherwise.
///
/// A pipeline reports its LAST stage's status, so `cargo test | tail -40`
/// exits 0 however the tests went — and the operations an agent reaches for to
/// keep output small (`| tail`, `| head`, `| grep`) are exactly the ones that
/// discard the failure signal. The two pressures pull the same way every time,
/// so the error is not random: it always reads as green.
///
/// Measured 2026-08-28 over 89 151 recorded Bash commands: 65 734 carry a
/// pipe, 5 781 pipe a build or test runner into such a sink, and **830 of
/// those handed the agent unambiguous failure evidence under a SUCCESS
/// status** (601 `test result: FAILED`, the rest rustc `error[E…]`). Thirteen
/// were reported as errors.
///
/// This DISCLOSES rather than changing the code, deliberately. `set -o
/// pipefail` would turn the most ordinary idiom there is — `cmd | head`, where
/// `head` closes the pipe and the producer dies on SIGPIPE — into a failure,
/// and rewriting the caller's exit status is not tokenade's to do. Same
/// choice, and same shape, as the signal notice above: the number stays, the
/// silence goes.
pub(super) fn disclose_success_with_failure_evidence(out: &std::process::Output, shell_cmd: &str) {
    let (text, _) = tokenade_core::process_util::decode_child_output(&out.stdout);
    let (err_text, _) = tokenade_core::process_util::decode_child_output(&out.stderr);
    disclose_swallowed_failure(exit_code_of(out.status), &text, &err_text, shell_cmd);
}

/// The decision itself, on text rather than on a `Child`.
///
/// Shared with `tokenade exec`, which swallows a failure exactly the same way
/// and is the busiest command there is — a script whose last line is
/// `cargo test | tail -5` exits 0 from `exec` as surely as from `wrap`. Split
/// out rather than copied: a second marker list is a second list to keep
/// calibrated, and the `0 failed` trap is easy to fall back into (it was
/// re-entered once, an hour after being written down).
pub(crate) fn disclose_swallowed_failure(
    exit_code: i32,
    out_text: &str,
    err_text: &str,
    shell_cmd: &str,
) {
    if exit_code != 0 || !runs_a_build_or_test(shell_cmd) {
        return;
    }
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    // Colour codes go first: under `CARGO_TERM_COLOR=always` (CI, many
    // agents' shells) the line opens with `ESC[1m…error` and no marker matches.
    let out_plain = tokenade_core::ansi::strip_ansi(out_text);
    let err_plain = tokenade_core::ansi::strip_ansi(err_text);
    let Some(marker) = failure_evidence(&out_plain).or_else(|| failure_evidence(&err_plain)) else {
        return;
    };
    if SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    // The evidence is said every time — it is this command's; the lesson
    // once per context (`said_once`).
    if !crate::said_once::due("swallowed-failure") {
        eprintln!("[tokenade] exit 0, yet the output reports a failure ({marker:?}).");
        return;
    }
    let how = if shell_cmd.contains('|') {
        " A pipeline reports only its LAST stage's status, so the failing stage is invisible to the exit code."
    } else {
        ""
    };
    eprintln!(
        "[tokenade] this command exited 0, but its output reports a failure \
         ({marker:?}).{how} Read the output, not the status."
    );
}
