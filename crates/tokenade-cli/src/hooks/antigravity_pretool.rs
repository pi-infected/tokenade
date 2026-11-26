#![allow(unused_imports)]
//! The Antigravity CLI (`agy`) hook dialect.
//!
//! Antigravity ships two unrelated products. The IDE is the one this matrix
//! called "appel manuel"; the `agy` CLI keeps its own configuration under
//! `~/.gemini/config/` and has a **native hook system** that nothing here was
//! wired into. Its contract is not inferred from the Claude family it
//! resembles — it is READ OUT OF THE SHIPPED BINARY, which carries its own
//! reference text (`strings agy`, 2026-08-25, against the `agy` on this
//! machine):
//!
//! * Events: `PreToolUse`, `PostToolUse`, `PreInvocation`, `PostInvocation`,
//!   `Stop`. The two tool-scoped ones are GROUPED (`{matcher, hooks:[…]}`);
//!   the other three are flat handler lists. Getting that wrong is silent.
//! * A handler is `{type:"command", command, timeout}`, run through `sh -c`
//!   (`cmd /c` on Windows), `~` expanded, cwd = the directory holding
//!   `hooks.json`.
//! * **Every key in the payload is camelCase** (protojson): `conversationId`,
//!   `workspacePaths`, `transcriptPath`, `stepIdx`, `toolCall.name`,
//!   `toolCall.args`. A handler written for the Claude envelope
//!   (`tool_name`, `tool_input`) reads nothing at all and answers nothing.
//! * `PreToolUse` answers `{"decision": "allow"|"deny"|"ask"|"force_ask",
//!   "reason", "overwrite": {…}}`, and `overwrite` is what makes this worth
//!   wiring: "Key-value pairs merged into the tool call's arguments before it
//!   runs … The modified tool call is what actually executes and is
//!   recorded." That is a command-REWRITE seam, the same rung Claude Code's
//!   `updatedInput` and Cursor's `updated_input` occupy.
//! * `PostToolUse` "expects an empty JSON object `{}`" — it can observe and it
//!   cannot substitute. So the RESULT never comes back through a hook here,
//!   and folding has to happen by rewriting the command, exactly as on Cursor.
//!
//! The shell tool is `run_command` and its argument is `CommandLine` (the
//! binary's own example: `"overwrite": {"CommandLine": "ls -la"}`). Tool names
//! are "derived by lowercasing the step type and removing the
//! `CORTEX_STEP_TYPE_` prefix", so they are stable lowercase identifiers.

use super::*;
use crate::*;

/// True when this payload is Antigravity CLI's, not another agent's.
///
/// The discriminator is `toolCall.name`: no other dialect this binary handles
/// carries a camelCase `toolCall` object. Checked BEFORE the Claude-shaped
/// `tool_name` read, which would otherwise find nothing and fall through to
/// "not a shell command" — a silent no-op, which is how an unwired agent looks
/// from the outside and why it is worth being explicit here.
pub(crate) fn is_antigravity_pretool(parsed: &serde_json::Value) -> bool {
    parsed
        .pointer("/toolCall/name")
        .and_then(|v| v.as_str())
        .is_some()
}

/// Antigravity CLI's shell tool, and the argument holding the command line.
const SHELL_TOOL: &str = "run_command";
const COMMAND_ARG: &str = "CommandLine";

/// Rewrite a `run_command` step to run through `tokenade wrap`, so its output
/// is compacted before the model ever sees it.
///
/// Returns `Some(json)` to print, `None` to stay silent (which Antigravity
/// treats as no opinion, leaving the original tool call alone).
pub(crate) fn antigravity_pretool_response(
    parsed: &serde_json::Value,
    exe_token: &str,
    skey: &str,
) -> Option<serde_json::Value> {
    let name = parsed.pointer("/toolCall/name").and_then(|v| v.as_str())?;
    if !name.eq_ignore_ascii_case(SHELL_TOOL) {
        return None;
    }
    let cmd = parsed
        .pointer(&format!("/toolCall/args/{COMMAND_ARG}"))
        .and_then(|v| v.as_str())?
        .trim();
    if cmd.is_empty() || cmd.starts_with("tokenade ") || cmd.starts_with("tokenade\t") {
        return None;
    }
    if cmdline_requests_hook_bypass(cmd) {
        return None;
    }
    // The same gates every other rewrite path applies, through the SAME
    // declarer rather than a third copy of the three conditions. Without them
    // this hook turns `vim`, `ssh` and `tail -f` into a piped `wrap` and hangs
    // the agent loop — and Antigravity's own limitations note says "Hooks run
    // synchronously and block the agent loop", so a hang here is the whole
    // session, not one call. That is also why the copies mattered: a fix taught
    // the Claude-format path to peel a launcher prefix off the program, and a
    // hand-written copy here would have kept `timeout 60 vim f` hanging on the
    // agent whose hang costs the most.
    if crate::hooks::bash_pretool::bash_segment_excluded_from_wrap(cmd) {
        return None;
    }
    // Byte-identical quoting/shell rules to the Cursor path: the outer shell
    // re-parses the rewritten string, so the command travels as ONE quoted
    // slot or its inner quotes are lost.
    let rewritten = crate::hooks::cursor_wrap_rewrite_with_session(cmd, skey, exe_token);
    Some(serde_json::json!({
        // `"ask"`, not `"allow"`, and this is a safety decision rather than a
        // stylistic one.
        //
        // `decision` is required on this event, and the four values are not
        // interchangeable: `"allow"` means "Automatically allow the tool
        // execution", so answering it would AUTO-APPROVE every shell command
        // the agent proposes — tokenade would be silently switching off the
        // user's permission prompts as a side effect of compacting output.
        // Nothing about folding a result justifies that.
        //
        // `"ask"` is the host's own default path: "Prompt the user for
        // permission (respects 'Always Allow')" — as against `"force_ask"`,
        // which ignores the cache. So a user who has already allowed this
        // command is not re-prompted and one who has not is asked, which is
        // exactly what happens today without us.
        //
        // OPEN, and deliberately not guessed: the permission cache is keyed on
        // the command TEXT, and we hand back a rewritten one. Whether an
        // existing `command(npm test)` grant still covers
        // `tokenade wrap 'npm test'` has not been measured on a live session,
        // and `permissionOverrides` (documented as "temporary permission
        // grants") is the field that would answer it. Until that is measured,
        // the failure mode here is one extra prompt — never one fewer.
        "decision": "ask",
        "overwrite": { COMMAND_ARG: rewritten }
    }))
}

/// The session key Antigravity gives us, reduced the way every other hook
/// reduces one, so the rows this agent books reach the session ticker.
pub(crate) fn antigravity_session_key(parsed: &serde_json::Value) -> String {
    parsed
        .get("conversationId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect()
}

pub(crate) fn handle_antigravity_pretool(
    parsed: &serde_json::Value,
    started: std::time::Instant,
) -> i32 {
    if hook_disabled_by_env() {
        return 0;
    }
    let skey = antigravity_session_key(parsed);
    if !skey.is_empty() {
        tokenade_core::session::set_session_key_authoritative(&skey);
    }
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "tokenade".into());
    // `run_command` runs in PowerShell on Windows: a bare quoted path there
    // is a string, not a call (#14).
    let exe_token = crate::install::powershell_agent_exe_token(&exe);
    let Some(out) = antigravity_pretool_response(parsed, &exe_token, &skey) else {
        return 0;
    };
    println!("{out}");
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "antigravity_rewrite",
        serde_json::json!({"elapsed_ms": started.elapsed().as_millis() as u64}),
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The payload shape, copied from the reference text inside `agy` itself.
    fn payload(tool: &str, cmd: &str) -> serde_json::Value {
        serde_json::json!({
            "conversationId": "ec33ebf9-0cba-4100-8142-c61503f6c587",
            "workspacePaths": ["/path/to/workspace"],
            "transcriptPath": "/path/to/workspace/.gemini/antigravity-cli/transcript.jsonl",
            "modelName": "auto",
            "stepIdx": 19,
            "toolCall": { "name": tool, "args": { "CommandLine": cmd } }
        })
    }

    #[test]
    fn a_shell_step_is_rewritten_through_wrap_in_antigravitys_own_answer_shape() {
        let out =
            antigravity_pretool_response(&payload("run_command", "git status"), "/opt/tk", "abc")
                .expect("a shell step must be rewritten");
        // Never `"allow"`: that would auto-approve every shell command as a
        // side effect of compacting its output.
        assert_eq!(out["decision"], "ask");
        let got = out["overwrite"]["CommandLine"].as_str().unwrap();
        assert!(got.contains("/opt/tk wrap"), "{got}");
        assert!(got.contains("git status"), "{got}");
        // `overwrite` is a SHALLOW top-level merge into `args`, so the key has
        // to be spelled exactly as the tool call spells it.
        assert!(out["overwrite"].get("CommandLine").is_some());
        assert!(out["overwrite"].get("command").is_none());
    }

    #[test]
    fn the_envelope_is_recognised_and_a_claude_shaped_one_is_not() {
        assert!(is_antigravity_pretool(&payload("run_command", "ls")));
        assert!(!is_antigravity_pretool(&serde_json::json!({
            "tool_name": "Bash", "tool_input": { "command": "ls" }
        })));
    }

    #[test]
    fn quotes_survive_the_outer_shells_reparse() {
        let out =
            antigravity_pretool_response(&payload("run_command", r#"grep "foo bar" ."#), "tk", "")
                .unwrap();
        let got = out["overwrite"]["CommandLine"].as_str().unwrap();
        assert!(got.contains(r#"grep "foo bar" ."#), "{got}");
    }

    #[test]
    fn non_shell_steps_and_our_own_commands_are_left_alone() {
        // A file-read step: nothing to wrap. Silence, not an empty decision —
        // Antigravity treats no output as no opinion.
        assert!(antigravity_pretool_response(&payload("view_file", "x"), "tk", "").is_none());
        // Already ours: wrapping twice would double-count and re-fold.
        assert!(
            antigravity_pretool_response(&payload("run_command", "tokenade map"), "tk", "")
                .is_none()
        );
        // No command at all.
        assert!(antigravity_pretool_response(&payload("run_command", "   "), "tk", "").is_none());
    }

    #[test]
    fn an_interactive_step_is_never_piped_through_wrap() {
        // Antigravity's own note: "Hooks run synchronously and block the agent
        // loop". A rewritten `vim` would hang the whole session, not one call.
        for interactive in ["vim Cargo.toml", "ssh box", "tail -f /var/log/syslog"] {
            assert!(
                antigravity_pretool_response(&payload("run_command", interactive), "tk", "")
                    .is_none(),
                "{interactive}"
            );
        }
    }

    #[test]
    fn the_session_key_comes_from_the_camel_case_conversation_id() {
        assert_eq!(
            antigravity_session_key(&payload("run_command", "ls")),
            "ec33ebf90cba41008142c61503f6c587"
        );
        // A Claude-shaped payload has no such field, and an empty key is what
        // "unsessioned" has to look like — never a panic, never a guess.
        assert_eq!(
            antigravity_session_key(&serde_json::json!({"session_id": "x"})),
            ""
        );
    }
}
