#![allow(unused_imports)]
//! Split out of the monolithic `hooks.rs` (mechanical module split, no
//! behaviour change). Wired back into the `hooks` namespace by mod.rs via
//! `pub(crate) use <name>::*;`.
use super::*;
use crate::*;

/// Cursor `preToolUse` hook: rewrite a Shell command to `tokenade wrap <cmd>`
/// so its output is compacted, emitting Cursor's response shape
/// (`{permission, updated_input.command}`). No-op (exit 0, no rewrite) for an
/// empty command or one already routed through tokenade — `tokenade wrap`
/// itself passes interactive/TTY commands through untouched.
pub(crate) fn handle_cursor_pretool(
    parsed: &serde_json::Value,
    started: std::time::Instant,
) -> i32 {
    // Same gates as the Bash handler — these were MISSING: the Cursor path
    // rewrote vim/ssh/tail -f into `tokenade wrap …` (cmd_wrap always pipes,
    // so interactive commands hung), and ignored TOKENADE_HOOK_DISABLED.
    if hook_disabled_by_env() {
        return 0;
    }
    // A Cursor Read, redirected to a compacted temp.
    //
    // Measured 2026-08-19 against a real `cursor-agent` (2026.08.11-e8db854),
    // because the contract could not be read off anything: `preToolUse` fires
    // for `tool_name: "Read"` with `tool_input.file_path`, and answering
    // `updated_input.file_path` REDIRECTS the read — asked to quote the first
    // line of a 300-line log, the agent quoted the redirect target's line.
    // Two other shapes were tried first and measured NOT to work, which is
    // why only this one ships: `beforeReadFile` receives the whole `content`
    // but returning a replacement `content` changed nothing the model saw, and
    // the installed matcher was `"Shell"`, so the Read never reached us at all.
    //
    // `compact_read_to_temp` is Gemini's, unchanged: same redirect, same
    // bounding, same repeat-fold. Cursor hands us `session_id`, so the fold can
    // prove what it claims.
    if crate::hooks::tool_is(
        parsed
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "Read",
    ) {
        if let Some(fp) = parsed
            .pointer("/tool_input/file_path")
            .and_then(|v| v.as_str())
        {
            // The workspace root, so the temp lands inside the project the way
            // the Gemini path needs; `cwd` is the fallback.
            let root = parsed
                .pointer("/workspace_roots/0")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    std::env::current_dir()
                        .ok()
                        .map(|p| p.display().to_string())
                });
            let session = crate::hooks::session_id_of(parsed);
            if let Some(temp) = crate::install::compact_read_to_temp(fp, root.as_deref(), session) {
                let out = serde_json::json!({
                    "permission": "allow",
                    "updated_input": { "file_path": temp }
                });
                println!("{out}");
                tokenade_core::debug_log::log(
                    "hookwrap-pretool",
                    "cursor_read_redirect",
                    serde_json::json!({
                        "file": fp,
                        "elapsed_ms": started.elapsed().as_millis() as u64,
                    }),
                );
                return 0;
            }
        }
        return 0; // nothing to gain — let Cursor read the original
    }
    let cmd = parsed
        .pointer("/tool_input/command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if cmd.is_empty() || cmd.starts_with("tokenade ") || cmd.starts_with("tokenade\t") {
        return 0; // nothing to rewrite — let the original command run
    }
    if cmdline_requests_hook_bypass(cmd) {
        return 0;
    }
    // THE shared gate, not a fourth copy of it. The three conditions were
    // written out here, in `antigravity_pretool` and in
    // `bash_segment_excluded_from_wrap`, each resolving the program its own
    // way — so the launcher peel (a launcher prefix must not hide `vim`/`less`/`ssh`)
    // reached only the Claude-format path and left Cursor and Antigravity
    // hanging on `timeout 60 vim f`.
    if crate::hooks::bash_pretool::bash_segment_excluded_from_wrap(cmd) {
        return 0;
    }
    // REPLAY parity with the Claude-format path: an unchanged read-only
    // re-run answers from cache instead of re-executing (same 4-guard
    // stack — see hooks/replay.rs).
    let rewritten = {
        let session = crate::hooks::session_id_of(parsed).unwrap_or("nosess");
        let skey: String = session
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let exe = std::env::current_exe()
            .ok()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "tokenade".into());
        // The SAME token the Bash rewrite uses, and it was not the same until
        // 2026-08-20: this branch emitted the bare name `tokenade` while every
        // other agent emitted the absolute path of the running binary. On a
        // machine where tokenade is on PATH the two are indistinguishable,
        // which is why it lived. Measured on Windows: the rewritten command
        // came back `'tokenade' n'est pas reconnu`, 33 738 → 0 bytes, exit 1 —
        // the compaction cell red and the user's command DESTROYED, on the one
        // agent whose rewrite replaces the command outright.
        // On Windows Cursor runs the result in PowerShell, which needs the
        // `&` call operator in front of a quoted path (#14) — the replay
        // command included, which until then got the raw unquoted path.
        let exe_token = crate::install::powershell_agent_exe_token(&exe);
        crate::hooks::replay::try_replay_rewrite(&skey, &cwd, cmd, &exe_token)
            .unwrap_or_else(|| cursor_wrap_rewrite_with_session(cmd, &skey, &exe_token))
    };
    let out = serde_json::json!({
        "permission": "allow",
        "updated_input": { "command": rewritten }
    });
    println!("{out}");
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "cursor_rewrite",
        serde_json::json!({"elapsed_ms": started.elapsed().as_millis() as u64}),
    );
    0
}

/// Build the `tokenade wrap …` rewrite for a Cursor Shell command.
///
/// Two correctness concerns the old flat `tokenade wrap {cmd}` got wrong:
///
/// 1. **Quote preservation.** Cursor's OUTER shell re-parses the rewritten
///    string, splitting it into argv, then `tokenade wrap` re-joins its rest
///    args with spaces — so `grep "foo bar" .` arrived as `grep foo bar .`,
///    losing the quotes. Fix: pass the command as ONE single-quoted argv slot
///    so it reaches `wrap` verbatim (one rest element → `join` is a no-op).
///
/// 2. **Shell match on Windows.** Cursor's default Windows shell is PowerShell
///    — both the outer executor that parses this string AND the shell the
///    command was authored for. The old code let `wrap` fall back to Git Bash
///    (POSIX) inner-shell, so a PowerShell-native cmdlet failed. Fix: on
///    Windows, escape for PowerShell single-quoting (embedded `'` → `''`) and
///    pass `--shell powershell` so the inner shell matches. Elsewhere, POSIX
///    single-quoting (embedded `'` → `'\''`) and the default POSIX inner shell.
/// Test-only convenience wrapper: production always has a session key and
/// calls `cursor_wrap_rewrite_with_session` directly.
#[cfg(test)]
pub(crate) fn cursor_wrap_rewrite(cmd: &str) -> String {
    cursor_wrap_rewrite_with_session(cmd, "", "tokenade")
}

/// Same rewrite, with the session key injected as a `TOKENADE_SESSION=`
/// prefix on POSIX — without it every row a Cursor session books has
/// sess=null and the whole agent is invisible to the session ticker
/// (writers audit). PowerShell has no VAR=val prefix syntax; Windows
/// Cursor sessions stay unsessioned rather than risk a broken command.
pub(crate) fn cursor_wrap_rewrite_with_session(cmd: &str, skey: &str, exe: &str) -> String {
    if cfg!(windows) {
        let q = cmd.replace('\'', "''");
        format!("{exe} wrap --shell powershell '{q}'")
    } else {
        let q = cmd.replace('\'', "'\\''");
        let prefix = if skey.is_empty() {
            String::new()
        } else {
            format!("TOKENADE_SESSION={skey} ")
        };
        format!("{prefix}{exe} wrap '{q}'")
    }
}

#[cfg(test)]
mod cursor_rewrite_tests {
    use super::cursor_wrap_rewrite;

    #[test]
    fn passes_command_as_one_quoted_slot_preserving_inner_quotes() {
        let got = cursor_wrap_rewrite(r#"grep "foo bar" ."#);
        // Whichever OS: the original command must survive verbatim inside the
        // single-quoted slot (the old flat form dropped the inner quotes).
        assert!(got.contains(r#"grep "foo bar" ."#), "got: {got}");
        assert!(got.starts_with("tokenade wrap "), "got: {got}");
    }

    /// The rewritten command must name the binary the way every other agent's
    /// rewrite does: by the path of the binary that is running.
    ///
    /// Cursor is the agent where this matters most and it was the one agent
    /// that got it wrong. Its hook REPLACES the user's command, so a name the
    /// outer shell cannot resolve does not fall back to the original — it runs
    /// nothing. Measured on Windows 2026-08-20: 33 738 bytes of output became
    /// 0, exit 1, `'tokenade' n'est pas reconnu`.
    #[test]
    fn the_rewrite_names_the_binary_that_is_running_not_a_bare_name() {
        let exe = if cfg!(windows) {
            r"C:\Program Files\tokenade\tokenade.exe"
        } else {
            "/opt/some where/tokenade"
        };
        let token = crate::install::powershell_agent_exe_token(exe);
        let got = super::cursor_wrap_rewrite_with_session("git status", "", &token);
        assert!(got.contains(&token), "got: {got}");
        // PowerShell reads a line that opens on a quoted string as a value,
        // not a call (#14).
        if cfg!(windows) {
            assert!(got.starts_with("& '"), "got: {got}");
        }
        assert!(
            !got.starts_with("tokenade "),
            "a bare name is what broke Windows: {got}"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn posix_single_quotes_and_escapes_embedded_quote() {
        assert_eq!(
            cursor_wrap_rewrite("git status"),
            "tokenade wrap 'git status'"
        );
        // embedded ' → '\'' so the POSIX outer shell rebuilds it as one arg.
        assert_eq!(
            cursor_wrap_rewrite("echo 'hi'"),
            r#"tokenade wrap 'echo '\''hi'\'''"#
        );
        // no powershell inner shell on POSIX.
        assert!(!cursor_wrap_rewrite("ls").contains("--shell"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_uses_powershell_inner_shell_and_doubles_quote() {
        assert_eq!(
            cursor_wrap_rewrite("git status"),
            "tokenade wrap --shell powershell 'git status'"
        );
        // embedded ' → '' (PowerShell single-quote escaping).
        assert_eq!(
            cursor_wrap_rewrite("Write-Output 'hi'"),
            "tokenade wrap --shell powershell 'Write-Output ''hi'''"
        );
    }
}
