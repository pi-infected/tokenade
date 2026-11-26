#![allow(unused_imports)]
//! Mechanical module split of install.rs (no behaviour change). Wired
//! back into the `install` namespace by mod.rs via `pub(crate) use <name>::*;`.
use super::*;
use crate::hooks::*;
use crate::wrap::*;
use crate::*;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) enum HookOutcome {
    Installed(PathBuf),
    /// #562 — installer replaced a drifted tokenade-tagged entry
    /// in-place (matcher or command differed from the canonical
    /// current shape). Distinct from `Installed` so the call site
    /// can render "✓ refreshed …" instead of the misleading "✓
    /// installed …" message. Companion to the upgrade-in-place
    /// logic added in #561.
    Refreshed(PathBuf),
    AlreadyPresent,
    /// Emitted when `TOKENADE_NO_HOOK=1` (or legacy `STS_NO_HOOK=1`)
    /// is set in the environment. Returned by each hook installer
    /// directly so the opt-out applies uniformly — including to the
    /// LSP enforcement hook, which used to silently install (#556).
    OptedOut,
    Failed(String),
}

/// #1004 — single source of truth for the env-var pair that opts every
/// hook installer out (`TOKENADE_NO_HOOK` + legacy `STS_NO_HOOK`). The
/// 4 installers (PostToolUse Bash, PreToolUse Bash, UserPromptSubmit/
/// SessionStart/PreCompact group, auto-index) duplicated the literal
/// pair at the head of each fn after the #556 move-into-installer.
/// Drift between those 4 sites would silently re-introduce #556 for
/// whichever installer fell out of sync; this helper makes it
/// impossible.
pub(crate) fn is_hook_opted_out() -> bool {
    tokenade_core::env_compat::is_truthy("TOKENADE_NO_HOOK", "STS_NO_HOOK")
}

/// Merge a `PostToolUse` Bash hook into `<root>/.claude/settings.json` that
/// pipes each Bash tool result through `tokenade hookwrap` for compression.
/// Preserves any existing hooks (incl. rtk / codegraph) — purely additive.
/// Tagged with a `tokenade` marker so re-running install is idempotent.
/// Build the literal `command` string we put in `~/.claude/settings.json`
/// for a tokenade hook (`hookwrap` or `hookwrap-pretool`).
///
/// Claude Code's hook spec does **not** honour an `env` field on hook
/// entries (only on MCP server entries — validated 2026-05-28 by
/// patching `env` in and observing zero hookwrap records in the debug
/// log even though the hook itself fired). So when the caller wants
/// the hook process to see `TOKENADE_DEBUG_LOG`, we wrap the command
/// in a shell snippet that sets the env then exec's the binary.
///
/// **Cross-platform**: the install machine's OS dictates the wrapper —
/// Claude Code spawns the hook command via its host shell, so the
/// snippet we emit must be one the host shell understands. We emit:
///
/// | host OS  | wrapper                                                        |
/// |----------|----------------------------------------------------------------|
/// | Linux/macOS | `sh -c 'TOKENADE_DEBUG_LOG=<p> exec <exe> <sub>'`           |
/// | Windows  | `<exe-fwd-slash> <sub>` (direct; env via settings.json `env`)  |
///
/// Windows used to wrap the call in `cmd /C "set TOKENADE_DEBUG_LOG=<p>&&
/// <exe> <sub>"`, but that nested `cmd.exe` invocation corrupted the hook
/// process's stdout — the JSON Claude Code parses to apply the compaction
/// *substitution* — so the rewrite was silently dropped and output stayed
/// RAW (Windows beta-tester, 2026-06-23). The fix is to spawn the binary
/// directly; the debug-log env var rides the settings.json global `env`
/// block (`ensure_debug_log_env`), which Claude Code applies to every
/// hook process it launches.
/// Ensure `settings["env"]["TOKENADE_DEBUG_LOG"]` carries the debug-log
/// path on Windows. Since the Windows hook commands are now direct exe
/// calls (no inline `cmd /C "set …&&"` — that polluted the hook's JSON
/// stdout, breaking the compaction substitution), the debug-log path
/// rides Claude Code's settings-level `env` block instead, which it
/// applies to every spawned hook process. No-op on Unix (the `sh -c`
/// wrapper injects the env inline) and when no debug-log was requested.
/// Idempotent: overwrites only our own key, leaves any other `env`
/// entries untouched.
pub(crate) fn ensure_debug_log_env(settings: &mut serde_json::Value, debug_log_path: Option<&str>) {
    if !cfg!(windows) {
        return;
    }
    let Some(p) = debug_log_path else { return };
    let Some(obj) = settings.as_object_mut() else {
        return;
    };
    let env = obj.entry("env").or_insert_with(|| serde_json::json!({}));
    if !env.is_object() {
        *env = serde_json::json!({});
    }
    env.as_object_mut()
        .unwrap()
        .insert("TOKENADE_DEBUG_LOG".into(), serde_json::json!(p));
}

/// Quote the tokenade exe path for a Windows hook command — a single form
/// that runs under BOTH shells Claude Code uses to spawn hooks on Windows,
/// because real users hit each one:
///
///   * **Git Bash** (a user report, 0.6.4): an UNQUOTED backslash path had every
///     separator eaten (`\U`→`U`, `\A`→`A`) → `C:Usersuser…: command not
///     found`. The first fix emitted FORWARD slashes to dodge this.
///   * **cmd.exe** (a user report, 0.7.0): that forward-slash path then broke the
///     OTHER way. `cmd /c C:/Users/…/@tokenade/…/tokenade.exe` does not run
///     reliably — cmd treats `/…` as switches, drops to an INTERACTIVE prompt,
///     and echoes the hook's stdin JSON onto the hook's STDOUT. That poisoned
///     everything: PreToolUse decision JSON was prefixed with a cmd banner so
///     Claude Code couldn't parse the rewrite (commands ran RAW, zero
///     compaction), and PostToolUse/SessionStart additionalContext became a
///     1.4 MB cmd-banner-plus-echoed-payload blob injected into the model.
///
/// The fix that satisfies both: **native backslashes, ALWAYS double-quoted.**
/// Backslashes are cmd.exe-native (no switch confusion). Double quotes stop
/// Git Bash from eating them (verified: `"C:\a\b.exe"` survives) and MSYS
/// happily execs a quoted backslash Windows path. Spaces are covered by the
/// same quotes. (Single quotes are NOT used — cmd.exe treats them as literal
/// characters, not quoting.)
pub(crate) fn win_hook_exe(exe: &str) -> String {
    format!("\"{}\"", exe.replace('/', "\\"))
}

/// The tokenade exe path as it must appear INSIDE a rewritten `updatedInput`
/// shell command (the Bash / PowerShell auto-wrap). DISTINCT from the hook
/// command in settings.json: this string is re-parsed by the AGENT's shell
/// when Claude Code runs the rewritten command.
///
/// Windows prod bug (user report, 0.7.0, 2026-06-27 — "the Bash hook shim is
/// broken, mangled path"): the rewrite embedded the raw `current_exe()`
/// backslash path (`C:\Users\…\@tokenade\…\tokenade.exe`) UNQUOTED. Claude
/// Code's Bash tool is **Git Bash**, which eats backslashes in an unquoted
/// token (`\U`→`U`, `\A`→`A`) → `C:Users…tokenade.exe: command not found`.
/// Because the rewrite REPLACES the real command, the original never ran
/// either → the agent saw a broken path, fled to PowerShell, and command
/// compaction produced ZERO savings (gain.jsonl had a single Read-dedup row).
/// `win_hook_exe` already forward-slashed the *hook* command (so the hooks
/// themselves fired) but the rewrite still used the raw path.
///
/// Fix: emit forward slashes here too. Git Bash, cmd.exe and PowerShell all
/// accept `/` in a full exe path; spaces get double-quoted (safe in all
/// three). POSIX is unchanged — the raw path already works there and the
/// existing chain-wrap tests assert it.
pub(crate) fn wrap_exe_token(exe: &str) -> String {
    if cfg!(windows) {
        win_hook_exe(exe)
    } else if exe
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+'))
    {
        exe.to_string()
    } else {
        // A home dir with a space (`/home/user name/.local/bin/tokenade`)
        // emitted an unquoted exe token — and since the rewrite REPLACES the
        // command, every Bash call broke. Single-quote defensively.
        format!("'{}'", exe.replace('\'', "'\\''"))
    }
}

pub(crate) fn build_hook_command(
    exe: &str,
    subcommand: &str,
    debug_log_path: Option<&str>,
) -> String {
    if cfg!(windows) {
        // DIRECT exe call — deliberately NO `cmd /C "set …&& <exe> …"`
        // wrapper. That nesting was the Windows root-cause (beta-tester,
        // 2026-06-23): cmd.exe's `set X=Y&& …` chaining polluted the hook
        // process's STDOUT (the hook emits JSON that Claude Code parses to
        // drive the compaction *substitution*; the cmd wrapper's quoting /
        // echo corrupted it), so Claude Code couldn't honour the rewrite →
        // command output stayed RAW, zero savings. The fix is to invoke the
        // binary directly so its stdout is exactly the hook's JSON. The
        // debug-log path (if any) rides the settings.json global `env`
        // block instead (see `ensure_debug_log_env`), not the command.
        return format!("{} {subcommand}", win_hook_exe(exe));
    }
    // POSIX (macOS / Linux). FAIL-OPEN wrapper. A hook must NEVER surface a
    // non-zero status to the agent for an INTERNAL tokenade problem — Claude
    // Code renders that as a per-prompt "UserPromptSubmit hook error: … failed
    // with non-blocking status code N" nag (macOS field report, 0.8.9). The
    // classic trigger is a baked exe path the shell can no longer run: an
    // auto-update / npm reinstall relocated the binary, a wrong-arch download,
    // or a Gatekeeper-killed unsigned binary → the wrapper exits 126/127 (or
    // the process is signalled) on EVERY prompt.
    //
    // Two rules make it fail-open:
    //   1. NO `exec`. A failed `exec` is FATAL in a non-interactive `sh` — it
    //      exits 126/127 immediately and NOTHING after it runs, so the obvious
    //      `exec … || exit 0` does NOT fail-open (verified on macOS). Running
    //      the binary as a normal child instead means a missing / killed /
    //      crashing binary just leaves a non-zero `$?` that the trailing…
    //   2. …`; exit 0` overrides. The hook then always exits 0.
    // The binary's STDOUT (the hook's JSON decision) still reaches the agent;
    // its STDERR is dropped — internal errors already land in
    // ~/.tokenade/debug.log and MUST NOT pollute the agent context (Claude
    // Code ingests a UserPromptSubmit hook's exit-0 stderr as context). No
    // tokenade hook uses a non-zero exit (or exit 2) to signal — decisions
    // ride stdout JSON `permissionDecision` — so discarding the child's exit
    // code loses nothing.
    //
    // Both `exe` and the debug-log path are POSIX-quoted for spaces /
    // apostrophes / `$` in HOME (`/Users/Bob's iMac/…`).
    let inner = match debug_log_path {
        Some(p) => format!(
            "TOKENADE_DEBUG_LOG={} {} {subcommand} 2>/dev/null; exit 0",
            posix_quote(p),
            posix_quote(exe),
        ),
        None => format!("{} {subcommand} 2>/dev/null; exit 0", posix_quote(exe)),
    };
    format!("sh -c {}", posix_quote(&inner))
}

/// Wrap `s` in single quotes, replacing every embedded quote with `escape`.
///
/// One rule, two dialects: POSIX shells close-escape-reopen (`'\''`),
/// PowerShell doubles the quote (`''`). Both spellings keep their own name and
/// doc above; only the loop is shared.
fn single_quoted(s: &str, escape: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str(escape);
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// POSIX shell single-quote escaping. Wraps `s` in single quotes,
/// and substitutes any internal `'` with the canonical `'\''`
/// (close-quote + escaped-quote + reopen-quote) sequence. Result is
/// always safe to embed in a `sh -c '…'` block.
pub(crate) fn posix_quote(s: &str) -> String {
    single_quoted(s, "'\\''")
}

/// PowerShell single-quote escaping. A PowerShell single-quoted string is
/// literal (no `$` expansion, no backtick escapes) — exactly what we want for
/// wrapping the agent's command verbatim. The ONLY escape inside is a literal
/// single quote, written by DOUBLING it (`''`). Used to wrap the rewritten
/// `tokenade wrap --shell powershell '<cmd>'` so PowerShell (which runs the
/// rewritten command) re-tokenises `<cmd>` as one literal argument.
pub(crate) fn powershell_quote(s: &str) -> String {
    single_quoted(s, "''")
}

/// The tokenade exe as the head of a command PowerShell will run: the `&`
/// call operator, then the path single-quoted with forward slashes.
///
/// A line that STARTS with a quoted string is an expression to PowerShell, not
/// a call: `"C:\…\tokenade.exe" wrap …` fails with `Unexpected token 'wrap'`,
/// and since the rewrite replaces the agent's command, nothing runs (#14,
/// Antigravity on Windows 11). `&` is what turns the string into a call.
/// Single quotes keep it literal (`$`, and the `@` in the npm install path,
/// mean nothing inside them); forward slashes keep it identical to the
/// Claude Code PowerShell rewrite, which has used this form since 0.7.0.
pub(crate) fn powershell_call_exe(exe: &str) -> String {
    format!("& {}", powershell_quote(&exe.replace('\\', "/")))
}

/// The exe token for an agent whose shell tool runs through PowerShell on
/// Windows (Cursor, Antigravity) and a POSIX shell elsewhere.
pub(crate) fn powershell_agent_exe_token(exe: &str) -> String {
    if cfg!(windows) {
        powershell_call_exe(exe)
    } else {
        wrap_exe_token(exe)
    }
}

#[cfg(test)]
mod hook_quote_tests {
    use super::*;

    #[test]
    fn install_qwen_hook_writes_pretooluse_into_qwen_settings() {
        // Deterministic: pass an explicit home so we don't touch the real
        // ~/.qwen.
        //
        // **This test used to pin `BeforeTool`**, i.e. Gemini's vocabulary,
        // because Qwen Code was a fork of it. Its shipped documentation says
        // otherwise
        // (<https://qwenlm.github.io/qwen-code-docs/en/users/features/hooks/>):
        // its events are Claude's, and an entry placed under a name the host
        // does not know fires nothing — "file read, dead entries". The
        // HANDLER, on the other hand, stays shared: Qwen sends the same payload
        // (`tool_name`, `tool_input`) and the same tool ids.
        let base = std::env::temp_dir().join(format!("tokenade-qwenhook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let outcome = install_qwen_cli_hook("tokenade", Some(&base), false);
        assert!(
            matches!(outcome, HookOutcome::Installed(_)),
            "expected Installed, got {outcome:?}"
        );

        let settings_path = base.join(".qwen").join("settings.json");
        assert!(
            settings_path.exists(),
            "settings.json not written under .qwen"
        );
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert!(
            v["hooks"].get("BeforeTool").is_none(),
            "Gemini's event fires nothing in Qwen: do not write it"
        );
        // `enabled` IS required in Qwen too. This assertion used to demand its
        // absence, on the reasoning that the flag belongs to Gemini.
        // MEASURED on 2026-09-12 on `qwen 0.7.x`, twice in each direction:
        // without it, no hook fires — the model answers "no error" on an
        // output truncated to 102 lines; with it, it names the `ERROR` line
        // number 401. A test that forbade the flag held the stage inert in
        // place.
        assert_eq!(
            v["hooks"]["enabled"],
            serde_json::json!(true),
            "without `hooks.enabled`, NO hook fires in Qwen Code"
        );
        let before = &v["hooks"]["PreToolUse"];
        assert!(before.is_array(), "PreToolUse array missing");
        let entry = &before[0];
        assert_eq!(
            entry["matcher"].as_str(),
            Some(GEMINI_HOOK_MATCHER),
            "matcher should be the shared Gemini-family matcher"
        );
        // #288 — the attribution, not the whole literal: the command now
        // carries the fail-open wrapper `build_hook_command` owns.
        let qwen_cmd = entry["hooks"][0]["command"].as_str().unwrap_or_default();
        assert!(
            qwen_cmd.contains("gemini-hook --agent qwen-code"),
            "Qwen reuses the gemini-hook HANDLER but must not reuse its \
             ATTRIBUTION: the two agents install the same command, so without \
             `--agent` every Qwen saving lands on gemini-cli: {qwen_cmd}"
        );
        // Second install is idempotent.
        assert!(matches!(
            install_qwen_cli_hook("tokenade", Some(&base), false),
            HookOutcome::AlreadyPresent
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn install_grok_hooks_writes_into_the_directory_grok_actually_reads() {
        // This test used to assert the file was written to
        // `~/.grok/user-settings.json`, and it passed for as long as the
        // install was inert. Grok discovers hooks from `~/.grok/hooks/*.json`;
        // measured 2026-09-11 on grok 1.0.0, the same document at the old path
        // gave `grok inspect` → "Hooks (0)" and at the new one → "Hooks (6)".
        // A test that pins the address we chose proves nothing about the
        // address the host reads.
        let base = std::env::temp_dir().join(format!("tokenade-grokhook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let outcome = install_grok_hooks("tokenade", None, Some(&base), false);
        assert!(
            matches!(outcome, HookOutcome::Installed(_)),
            "expected Installed, got {outcome:?}"
        );
        let path = base.join(".grok").join("hooks").join("tokenade.json");
        assert!(path.exists(), "tokenade.json not written under .grok/hooks");
        assert!(
            !base.join(".grok").join("user-settings.json").exists(),
            "user-settings.json is not a hook source — nothing should be written there"
        );
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let hooks = &v["hooks"];
        // The two events grok can honour, and ONLY those. Everything else it
        // documents as passive-with-stdout-ignored, and its binary carries no
        // `updatedToolOutput` / `updatedInput` to deliver a substitution with.
        assert!(hooks["SessionStart"].is_array());
        assert!(hooks["PreCompact"].is_array());
        for dead in [
            "PreToolUse",
            "PostToolUse",
            "UserPromptSubmit",
            "SubagentStart",
        ] {
            assert!(
                hooks.get(dead).is_none(),
                "{dead} cannot deliver anything on grok 1.0.0 — installing it \
                 spends a process per event and books a saving nobody receives"
            );
        }
        // `PreCompact`'s matcher tests the compaction TRIGGER; empty is the
        // documented "matches everything", where `"*"` is not a regex.
        assert_eq!(hooks["PreCompact"][0]["matcher"].as_str(), Some(""));
        assert!(hooks["SessionStart"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("hookwrap-sessionstart"));
        assert!(matches!(
            install_grok_hooks("tokenade", None, Some(&base), false),
            HookOutcome::AlreadyPresent
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The migration off the dead address, which is the half a fresh-home test
    /// never sees: every machine installed before that fix carries six inert
    /// entries in a file that also holds the user's own settings.
    #[test]
    fn installing_grok_hooks_clears_the_dead_entries_from_user_settings() {
        let base = std::env::temp_dir().join(format!("tokenade-grokmig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join(".grok")).unwrap();
        let stale = serde_json::json!({
            "theme": "dracula",
            "hooks": {
                "PostToolUse": [{
                    "matcher": "bash",
                    "hooks": [{
                        "type": "command",
                        "command": "sh -c '/usr/local/bin/tokenade hookwrap'",
                        "_tag": "tokenade-posttool",
                    }],
                }],
            },
        });
        let usp = base.join(".grok").join("user-settings.json");
        std::fs::write(&usp, serde_json::to_string_pretty(&stale).unwrap()).unwrap();

        let _ = install_grok_hooks("tokenade", None, Some(&base), false);

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&usp).unwrap()).unwrap();
        assert_eq!(
            after["theme"].as_str(),
            Some("dracula"),
            "the user's own settings must survive the migration untouched"
        );
        assert!(
            after["hooks"].as_object().is_none_or(|h| h
                .values()
                .all(|v| v.as_array().is_none_or(|a| a.is_empty()))),
            "the inert tokenade entries must be gone: {after}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn posix_quote_basics() {
        assert_eq!(posix_quote("simple"), "'simple'");
        assert_eq!(posix_quote("with space"), "'with space'");
        assert_eq!(posix_quote("with$dollar"), "'with$dollar'");
        // Single quote in the input gets the close-escape-reopen dance.
        assert_eq!(posix_quote("o'brien"), "'o'\\''brien'");
    }

    #[test]
    fn a_powershell_command_calls_the_exe_rather_than_naming_a_string() {
        assert_eq!(
            powershell_call_exe(
                r"C:\Users\a b\AppData\Roaming\npm\node_modules\@tokenade\cli\tokenade.exe"
            ),
            "& 'C:/Users/a b/AppData/Roaming/npm/node_modules/@tokenade/cli/tokenade.exe'"
        );
        assert_eq!(
            powershell_call_exe(r"C:\it's\tk.exe"),
            "& 'C:/it''s/tk.exe'"
        );
    }

    #[test]
    fn powershell_quote_doubles_single_quotes_keeps_backslashes() {
        assert_eq!(powershell_quote("Get-ChildItem"), "'Get-ChildItem'");
        // Backslashes are LITERAL in PowerShell single-quotes (unlike bash) —
        // a Windows path must survive intact.
        assert_eq!(
            powershell_quote(r"C:\tools\tokenade.exe"),
            r"'C:\tools\tokenade.exe'"
        );
        // The only escape: a literal single quote doubles.
        assert_eq!(powershell_quote("it's"), "'it''s'");
        // `$` must NOT expand → single-quotes already guarantee that.
        assert_eq!(powershell_quote("$env:PATH"), "'$env:PATH'");
    }

    #[test]
    fn powershell_rewrite_exe_is_always_single_quoted_at_token() {
        // The PowerShell auto-wrap emits `& <exe_q> wrap --shell powershell …`.
        // <exe_q> MUST be single-quoted even with no space: the npm path holds
        // `@tokenade`, and an UNQUOTED `& C:\…\@tokenade\…` is fragile in
        // PowerShell (`@` starts splatting / array / hashtable / here-string).
        // Mirror the handler expression exactly.
        let exe =
            r"C:\Users\Admin\AppData\Roaming\npm\node_modules\@tokenade\cli\vendor\tokenade.exe";
        let exe_q = powershell_quote(&exe.replace('\\', "/"));
        assert!(
            exe_q.starts_with('\'') && exe_q.ends_with('\''),
            "must be single-quoted: {exe_q}"
        );
        assert!(
            exe_q.contains("@tokenade"),
            "path preserved literally: {exe_q}"
        );
        // `&` call operator runs a single-quoted path verbatim.
        let rewritten = format!(
            "& {exe_q} wrap --shell powershell {}",
            powershell_quote("Get-ChildItem")
        );
        assert!(
            rewritten.starts_with("& '"),
            "call-operator + quoted path: {rewritten}"
        );
    }

    #[test]
    fn shell_kind_parses_aliases() {
        use ShellKind::*;
        for s in ["posix", "sh", "bash", "BASH"] {
            assert_eq!(ShellKind::parse(s), Some(Posix), "{s}");
        }
        for s in ["powershell", "pwsh", "ps", "PowerShell"] {
            assert_eq!(ShellKind::parse(s), Some(PowerShell), "{s}");
        }
        for s in ["cmd", "cmd.exe", "bat"] {
            assert_eq!(ShellKind::parse(s), Some(Cmd), "{s}");
        }
        assert_eq!(ShellKind::parse("fish"), None);
    }

    #[cfg(unix)]
    #[test]
    fn build_hook_command_handles_space_path() {
        // Real-world Mac scenario: `/Users/Jean Dupont/bin/tokenade`.
        let cmd = build_hook_command(
            "/Users/Jean Dupont/bin/tokenade",
            "hookwrap",
            Some("/Users/Jean Dupont/.tokenade/debug.log"),
        );
        // The whole thing should still be a single executable line.
        assert!(cmd.starts_with("sh -c "));
        // And the embedded values must NOT be word-broken.
        assert!(cmd.contains("Jean Dupont"));
    }

    #[cfg(unix)]
    #[test]
    fn build_hook_command_is_fail_open() {
        // A hook must exit 0 even when the baked binary can't be run (stale
        // path / wrong arch / killed) — Claude Code would otherwise nag with
        // "hook error … non-blocking status code" every prompt (macOS 0.8.9).
        // The wrapper must: run via `sh -c`, force a trailing `exit 0`, and
        // NOT use `exec` (a failed exec is fatal in sh, defeating fail-open).
        for dbg in [Some("/home/u/.tokenade/debug.log"), None] {
            let cmd = build_hook_command("/home/u/.local/bin/tokenade", "hookwrap-userprompt", dbg);
            assert!(cmd.starts_with("sh -c "), "must be sh-wrapped: {cmd}");
            assert!(cmd.contains("; exit 0"), "must force exit 0: {cmd}");
            assert!(
                !cmd.contains(" exec "),
                "must NOT use exec (failed exec is fatal in sh): {cmd}"
            );
            // The dispatch subcommand stays recognisable (space-anchored) so
            // idempotent re-install / drift-heal still matches the entry.
            assert!(
                cmd.contains(" hookwrap-userprompt "),
                "subcommand token lost: {cmd}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn build_hook_command_handles_apostrophe_path() {
        let cmd = build_hook_command(
            "/Users/Bob's iMac/tokenade",
            "hookwrap",
            Some("/tmp/dbg.log"),
        );
        // Apostrophe forces the close-escape-reopen sequence.
        assert!(cmd.contains("'\\''"), "expected POSIX escape — got {cmd}");
    }
}
