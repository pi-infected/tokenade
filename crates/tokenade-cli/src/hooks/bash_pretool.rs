#![allow(unused_imports)]
//! Split out of the monolithic `hooks.rs` (mechanical module split, no
//! behaviour change). Wired back into the `hooks` namespace by mod.rs via
//! `pub(crate) use <name>::*;`.
use super::*;
use crate::*;

// PreToolUse handler for Bash. Rewrites the Bash invocation to run
// through `tokenade wrap '<original>'` so the SAME process executes
// the command AND applies the per-command compactor before stdout
// reaches Claude Code. The model sees the compact output as the
// tool result, no `permissionDecision: deny`, no "Error:" UI prefix.
//
// This is the RTK pattern (RTK's `src/hooks/hook_cmd.rs` —
// `permissionDecision: "allow" + updatedInput.command`). An
// earlier version of this function used deny+reason which Claude
// Code surfaces as "Error: …" on every intercept, alarming users
// even when the data was fine. The rewrite avoids that entirely.
//
// Trade-offs vs the deny approach:
//   - **No double-execution risk**: the command runs ONCE inside
//     tokenade wrap. Side effects fire exactly once.
//   - **No additive-PostToolUse trap**: PostToolUse is no longer
//     needed for substitution; we keep it log-only for analytics.
//   - **UI shows the rewrite** (`Bash(tokenade wrap 'git status')`)
//     instead of the bare command. Cosmetic; the alternative was
//     "Error: ..." on every line.
//
// Exclude list: editors (vim/nano), TUI (top/htop/less), continuous
// streams (`tail -f`, `journalctl -f`), `sudo` (interactive prompt).
// For those we emit nothing — Bash runs normally without a wrapper.

/// Split a leading `cd <simple-dir> && ` off a command. Only a PLAIN dir
/// token (no quotes, vars, globs, metachars) qualifies — anything fancier
/// keeps the conservative passthrough.
/// What the caller is prepared to hand to ONE `tokenade wrap`.
///
/// Two answers because the two callers differ, not two copies of the rule.
/// `Single` is the historical contract: a multi-segment payload belongs to the
/// split path, which wraps each segment on its own. `Chain` is for the caller
/// the split path has already REFUSED — a cross-segment shell variable — where
/// one unit is the only wrapping left and is safe exactly when no segment needs
/// the outer shell.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeelPayload {
    Single,
    Chain,
}

pub(crate) fn peel_leading_cd_for(cmd: &str, payload_kind: PeelPayload) -> Option<(String, &str)> {
    let rest = cmd.strip_prefix("cd ")?;
    // Every separator the SPLITTER knows, not just `&&`. This copy knew
    // one where `split_chain_segments` knows four, so `cd /x; VAR=…; use $VAR`
    // and its newline form fell between two paths: the split path refuses a
    // cross-segment variable, and this one refused the separator. Measured on
    // 2 500 real Bash calls replayed through the gate: 61 excluded as
    // `cross-segment-shell-var`, 38 of them a leading `cd` and nothing else
    // needing the outer shell. Same shape as the newline fix below, one layer up.
    let (at, sep) = ["&&", "||", ";", "\n"]
        .iter()
        .filter_map(|sep| rest.find(sep).map(|i| (i, *sep)))
        .min_by_key(|(i, _)| *i)?;
    let dir = rest[..at].trim();
    if dir.is_empty()
        || dir.chars().any(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    '\'' | '"' | '$' | '`' | ';' | '|' | '&' | '(' | ')' | '*' | '?'
                )
        })
    {
        return None;
    }
    let payload = rest[at + sep.len()..].trim();
    if payload.is_empty() {
        return None;
    }
    // Asked of the declarer instead of spelled a third time. This copy
    // knew three separators, so a payload split across two LINES was lifted as
    // if it were one command. `Unparsed` is accepted on purpose: quotes and
    // `$()` are exactly why this path exists.
    if command_rewrite::chain_shape(payload) == command_rewrite::Chain::Many {
        if payload_kind == PeelPayload::Single {
            return None;
        }
        // One unit is safe only if no segment needs the OUTER shell. Asked of
        // `segment_must_not_be_wrapped`, the declarer.
        let all_wrappable = command_rewrite::split_chain_segments(payload).is_some_and(|segs| {
            segs.iter()
                .all(|s| !command_rewrite::segment_must_not_be_wrapped(s.cmd))
        });
        if !all_wrappable {
            return None;
        }
    }
    // A newline separator cannot survive into `format!("{prefix} …")` — it is
    // trimmed and the result reads `cd /x tokenade wrap …`, which runs `cd`
    // with two arguments. `cd /x\nfoo` runs `foo` whatever `cd` did, so `;`
    // is the faithful one-line spelling. Every other separator is kept as the
    // caller wrote it.
    let sep = if sep == "\n" { ";" } else { sep };
    Some((format!("cd {dir} {sep}"), payload))
}
mod exclusions;
mod shell_chain;
pub(crate) mod worktree_git;
pub(crate) use exclusions::*;
pub(crate) use shell_chain::*;

/// **The way back must not need a permission the way out did not.**
///
/// Every rewritten command leaves this hook with `permissionDecision: allow`;
/// `tokenade expand-ref <hash>` — the recovery every fold banner names —
/// left with no decision, so the host asked the user. Read in a real session
/// (2026-10-02): "I could not read the output of `identity show`: it came
/// back folded by tokenade, and `expand-ref` asks for an approval" — the
/// agent gave up on the bytes it had been told to fetch.
///
/// Only the BARE form: one `tokenade expand-ref …` and nothing the shell could
/// chain, pipe, redirect or substitute — an allow must not ride along with
/// `; rm -rf ~`. It reads tokenade's own store and writes nothing.
pub(crate) fn is_a_bare_recovery_read(cmd: &str) -> bool {
    if cmd.contains(|c: char| {
        matches!(
            c,
            ';' | '|' | '&' | '>' | '<' | '`' | '$' | '\n' | '(' | ')'
        )
    }) {
        return false;
    }
    let mut words = cmd.split_whitespace();
    let prog = words
        .next()
        .map(tokenade_core::argv::basename)
        .unwrap_or_default();
    prog == "tokenade" && words.next() == Some("expand-ref")
}

/// The tool input with only its `command` replaced. `updatedInput` REPLACES
/// the whole input (hooks reference): sent as `{command}` alone, it would
/// drop the call's `timeout`, `run_in_background` and `description`.
pub(crate) fn updated_input(parsed: &serde_json::Value, command: &str) -> serde_json::Value {
    let mut input = parsed
        .get("tool_input")
        .cloned()
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    input["command"] = serde_json::Value::String(command.to_string());
    input
}

pub(crate) fn handle_bash_pretool(parsed: &serde_json::Value, started: std::time::Instant) -> i32 {
    if hook_disabled_by_env() {
        return 0;
    }
    // #4 (0.4.4 findings) — FAIL-OPEN when not activated. See the matching
    // guard in `handle_read_pretool`: with the MCP recovery tools gated off,
    // rewriting the command to route through `tokenade wrap` would compact
    // its output with no way to recover the folded bytes. Let Bash run raw.
    let cmd = parsed
        .get("tool_input")
        .and_then(|i| i.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim();
    crate::hooks::session_lifecycle::bump_write_generation_if_bash_writes(parsed, cmd);
    // A user `deny`/`ask` rule is matched against what this hook returns: a
    // wrapped command would slip past it. Leave such a command untouched.
    let cwd = parsed
        .get("cwd")
        .and_then(|c| c.as_str())
        .map(std::path::Path::new);
    if crate::hooks::permission_rules::a_user_rule_may_guard("Bash", cmd, cwd) {
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "user_rule_guards",
            serde_json::json!({ "head": cmd.chars().take(120).collect::<String>() }),
        );
        return 0;
    }
    if is_a_bare_recovery_read(cmd) {
        println!(
            "{}",
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "allow",
                    "permissionDecisionReason":
                        "tokenade: reading back the bytes tokenade itself folded",
                }
            })
        );
        return 0;
    }
    if let Some(fixed) = exec_script_lift(cmd) {
        println!(
            "{}",
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "allow",
                    "permissionDecisionReason":
                        "tokenade: inline --script contains apostrophes sh would break on — lifted to --script-file",
                    "updatedInput": updated_input(parsed, &fixed)
                }
            })
        );
        return 0;
    }
    if cmd.is_empty() {
        return 0;
    }
    // Mirror the PostToolUse cmdline-prefix bypass. Without
    // this the PreToolUse path would still rewrite / redirect the
    // command even when the user explicitly wrote
    // `TOKENADE_HOOK_DISABLED=1 …`. Same nudge tick as the env path.
    if cmdline_requests_hook_bypass(cmd) {
        // The tick belongs to the PostToolUse handler, which sees the command
        // that actually RAN — this one fires for a command the user may still
        // deny. Both ticked, so one bypass counted as two: measured live on
        // the installed binary, `TOKENADE_HOOK_DISABLED=1 echo x` moved the
        // 24h counter by 2. The nudge quotes that number back to the agent.
        //
        // This return is BEFORE the exclude-list gate, so it is the
        // only place a leading `TOKENADE_HOOK_DISABLED=1 sudo …` can be told
        // that the variable changed nothing. Putting the notice on the
        // exclude path alone left it unreachable in production; caught by
        // driving the hook rather than the function.
        emit_redundant_bypass_notice(parsed, cmd);
        return 0;
    }
    // First-word extraction for the exclude check. Same logic as the
    // old deny path — split on shell metachars to find the leading
    // program (`cargo test 2>&1 | tee` → `cargo`).
    //
    // Skip:
    //   * leading comment lines (`# header\ngrep …`) — the shell
    //     ignores them but argv[0]=`#` would mis-route compactors,
    //     so we advance to the first non-comment line first.
    //   * env-prefix tokens (`LANG=C cmd`) — `=` in the first token
    //     means it's an env assignment, the real prog is the next.
    // `first_program_token` already strips path prefix and leading `!`,
    // so the result is the bare program name (e.g. `cargo`, `python3`).
    let prog = extract_first_meaningful_word(cmd).to_string();

    // Track which gate fired so telemetry shows the right reason.
    // The previous form `if is_continuous_stream(cmd) { … } else { … }`
    // re-checked is_continuous_stream after the || short-circuit, so any
    // command that hit BOTH gates (e.g. an old code path where `cargo`
    // matched both exclude-list AND the streamer heuristic) was logged as
    // "continuous-stream" — masking the real exclude-list cause. 122
    // historical events for cd/cargo/echo/grep/printf/sshpass were
    // mis-labeled this way. Compute each gate once and prefer the
    // exclude-list label when it fires (it's the cheaper, more common
    // path; continuous-stream is the narrow exception).
    let excluded_by_list = bash_command_is_excluded_by_list(cmd);
    let excluded_by_stream = !excluded_by_list && is_continuous_stream(cmd);
    // P2 — flag-gated interactive sessions (`docker exec -it`,
    // `kubectl exec -it`, `attach`). Same family as the two gates
    // above: wrapping would hang waiting for a TTY that the wrap side
    // can't provide.
    let excluded_by_tty =
        !excluded_by_list && !excluded_by_stream && is_interactive_tty_invocation(cmd);
    // Remembered for the chain-split fallthrough below: when the early-exit
    // is DEFERRED for a chain but split_chain_segments then fails (quotes,
    // heredoc, control flow) or yields one segment, the `_` arm used to
    // whole-wrap anyway — wrapping `tail -f "a;b.log"` or `docker exec -it
    // api sh -c "x && y"` hangs the agent's shell tool.
    let gates_deferred = excluded_by_list || excluded_by_stream || excluded_by_tty;
    if excluded_by_list || excluded_by_stream || excluded_by_tty {
        // Defer the exclude-list / continuous-stream early-exit
        // when the command is a chain (`;` / `&&` / `||`). The original
        // gate checked only the FIRST program token (`cd` for
        // `cd /tmp; ls -la`, `set` for `set -e; cargo test ...`), so
        // chains whose leading segment was a shell-state modifier
        // bailed before split_chain_segments could rescue the heavy
        // commands later in the chain. User-reported pattern: 6 events
        // in recent ledger where `cd X; <cmd>` returned passthrough
        // because the first-prog gate matched `cd`.
        //
        // The chain-split path below (split_chain_segments + the
        // segment_must_not_be_wrapped check) already evaluates EACH
        // segment independently and wraps only the wrappable ones —
        // so deferring the early-exit to that path gives the same
        // safety guarantees without losing coverage on chains.
        //
        // For non-chain commands the gate behaves exactly as before.
        // A NEWLINE is a chain separator too, and this predicate did not
        // know it while `split_chain_segments` did (it records `separator_after:
        // "\n"` and reassembly restores it). So an agent writing
        //
        //     cd /some/project
        //     grep -rn pattern crates
        //
        // hit `segment_must_not_be_wrapped` on the leading `cd`, found no `;`,
        // `&&` or `||` anywhere, and the WHOLE command was excluded — the grep's
        // output never folded. Measured over 37 892 real Bash calls from this
        // machine's transcripts: **5 223 excluded here, and every single one of
        // them is multi-line**, not one a lone `cd`. The comment below is right
        // that a solo builtin has nothing to fold; the population reaching it was
        // not solo builtins.
        // Defer to the split path unless this is plainly ONE command: a scanner
        // refusal still reaches the last-chance peel below.
        let has_chain = command_rewrite::chain_shape(cmd) != command_rewrite::Chain::One;
        if !has_chain {
            let reason = if excluded_by_list {
                "exclude-list"
            } else if excluded_by_stream {
                "continuous-stream"
            } else {
                "interactive-tty"
            };
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "bash_excluded",
                serde_json::json!({
                    "prog": prog,
                    "reason": reason,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
            // The one thing worth SAYING on this otherwise silent path: the
            // caller paid a bypass for a command that was already passing
            // through. Reached when the assignment sits mid-chain
            // (`cd x; TOKENADE_HOOK_DISABLED=1 sudo …`), which the leading-
            // prefix gate above does not see.
            emit_redundant_bypass_notice(parsed, cmd);
            return 0;
        }
    }

    // Background execution: `python server.py &` (or `a & b`). wrap pipes
    // the child's stdout and only returns when the pipe CLOSES — a daemon
    // holding it open turns an instant-return command into a hang. `&` is
    // not in the chain-split set, so these fell through to the whole-wrap
    // arm. A standalone `&` token is the discriminator (`&&`, `2>&1`, `&>`
    // never tokenize to a lone `&`).
    if cmd.split_whitespace().any(|t| t == "&")
        || cmd.trim_end().ends_with('&') && !cmd.trim_end().ends_with("&&")
    {
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "bash_excluded",
            serde_json::json!({
                "prog": prog,
                "reason": "background-amp",
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return 0;
    }

    // Skip the rewrite if the command IS ALREADY going through
    // tokenade — otherwise we'd recurse infinitely
    // (`tokenade wrap 'tokenade wrap …'`).
    if prog == "tokenade" || cmd.starts_with("tokenade ") || cmd.contains("/tokenade wrap ") {
        return 0;
    }

    // Solo shell-state builtin (`cd`, `pushd`, `export`, `source`, …):
    // wrapping into `bash -c '…'` puts the builtin in a subshell that
    // exits before its effect (cwd/env mutation) can reach the caller.
    // For lone commands the effect is moot — Claude Code's Bash tool
    // already runs each call in its own bash — but the wrap round-trip
    // is pure overhead and clutters telemetry (42 cd events / 24h).
    // The chain-split path below ALREADY consults this list for each
    // segment of a multi-segment cmd; this mirrors the same check for
    // the single-segment path.
    // Same defer pattern as the exclude-list gate above.
    // For a single-command input, segment_must_not_be_wrapped on the
    // whole cmd correctly identifies `cd /tmp` or `set -e` as a
    // shell-state modifier and bails. But for a CHAIN like
    // `cd /tmp; ls -la`, `first_program_token` returns `cd` (the
    // first token), so this gate fires even though `ls -la` later in
    // the chain IS wrappable. Defer to the split-wrap path which
    // evaluates each segment independently.
    // A NEWLINE is a chain separator too, and this predicate did not
    // know it while `split_chain_segments` did (it records `separator_after:
    // "\n"` and reassembly restores it). So an agent writing
    //
    //     cd /some/project
    //     grep -rn pattern crates
    //
    // hit `segment_must_not_be_wrapped` on the leading `cd`, found no `;`,
    // `&&` or `||` anywhere, and the WHOLE command was excluded — the grep's
    // output never folded. Measured over 37 892 real Bash calls from this
    // machine's transcripts: **5 223 excluded here, and every single one of
    // them is multi-line**, not one a lone `cd`. The comment below is right
    // that a solo builtin has nothing to fold; the population reaching it was
    // not solo builtins.
    // Defer to the split path unless this is plainly ONE command: a scanner
    // refusal still reaches the last-chance peel below.
    let has_chain = command_rewrite::chain_shape(cmd) != command_rewrite::Chain::One;
    if command_rewrite::segment_must_not_be_wrapped(cmd) && !has_chain {
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "bash_excluded",
            serde_json::json!({
                "prog": prog,
                "reason": "shell-state-builtin",
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return 0;
    }

    // (Removed: the Bash `grep -r 'identifier'` → symbol-index
    // deny-redirect. It shared the Grep/Glob deny channel — Claude Code
    // wraps a PreToolUse deny reason in <error>…</error>, so the redirect
    // read as a failed grep and risked a wasted retry. Bash here only
    // takes the safe allow+updatedInput wrap path below.)

    // Use the absolute path of the currently-running binary so the
    // rewritten command finds the same tokenade — survives PATH games,
    // multiple installs, and brew shadowing.
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "tokenade".to_string());
    // Windows: forward-slash the path so Git Bash (the agent's Bash tool)
    // doesn't strip the backslashes — see `wrap_exe_token`. No-op on Unix.
    let exe = wrap_exe_token(&exe);
    // Chain-aware rewrite (Axe 2): when the command is composed of
    // multiple top-level segments joined by `&&` / `||` / `;`, wrap
    // #8: inside an isolated worktree, git must not be a `tokenade` operand.
    if worktree_git::in_isolated_worktree(parsed) && worktree_git::mentions_git(cmd) {
        let Some(piped) = worktree_git::git_pipe_form(cmd, &exe, parsed) else {
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "bash_excluded",
                serde_json::json!({
                    "prog": prog,
                    "reason": "isolated-worktree-git",
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
            return 0;
        };
        println!(
            "{}",
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "allow",
                    "permissionDecisionReason": "tokenade auto-wrap for compaction",
                    "updatedInput": updated_input(parsed, &piped)
                }
            })
        );
        return 0;
    }
    let rewritten = match rewrite_for_wrap(cmd, &exe, parsed, &prog, started, gates_deferred) {
        Ok(r) => r,
        Err(rc) => return rc,
    };

    let mut hso = serde_json::json!({
        "hookEventName": "PreToolUse",
        "permissionDecision": "allow",
        "permissionDecisionReason": "tokenade auto-wrap for compaction",
        "updatedInput": updated_input(parsed, &rewritten)
    });
    // Loop guard: same command run 3×+ this session → steer to a different
    // tack. Then the cat-as-batch-read forensics catch (2 bench storms, ~16
    // wasted turns each: cat sources → every Edit refused "File has not been
    // read yet" → each file re-Read, content billed twice). Then the batching
    // steer for one-command-per-turn chains (largest avoidable re-send pool
    // in trace mining). All piggyback on the SAME JSON as the rewrite — no
    // extra hook output, no cache risk.
    let nudge = if let Some(n) = loop_guard_nudge(parsed, cmd) {
        Some(n) // fire/ignored/ok recorded inside loop_guard_nudge
    } else if let Some(n) = cat_edit_precondition_nudge(parsed, cmd) {
        tokenade_core::gain::record_nudge(
            "cat-edit",
            "fire",
            tokenade_core::gain::NudgeMetric::None,
        );
        Some(n)
    } else {
        crate::hooks::batching_bash_nudge(parsed) // recorded inside
    };
    if let Some(nudge) = nudge {
        // Book the injected-context cost — these three used to bypass
        // log_emitted_additional_context (unbooked overhead, unlike every
        // other nudge emission).
        crate::log_emitted_additional_context("hookwrap-pretool-bashnudge", "PreToolUse", &nudge);
        hso["additionalContext"] = serde_json::Value::String(nudge);
    }
    let decision = serde_json::json!({ "hookSpecificOutput": hso });
    println!("{decision}");
    // Redacted command (secrets stripped) for observability — the
    // L122 audit found 65 bash_rewritten events with empty `command`
    // field, blocking post-hoc analysis. We MUST use the full
    // argv-aware redaction chain (redact_argv handles paired flags
    // like `sshpass -p PASSWORD` that a plain regex pass misses).
    // Mirror gain.jsonl's `safe_label` recipe exactly so any value
    // hidden from the dashboard is also hidden from debug.log.
    // Truncated to 200 chars to keep debug.log small for long
    // pipelines.
    let redacted_cmd = {
        let argv: Vec<String> = cmd.split_whitespace().map(String::from).collect();
        let r = redact::redact(&redact::redact_argv(&argv).join(" "));
        // Bash commands can contain multi-byte UTF-8 (unicode
        // filenames, non-ASCII args); `&r[..200]` would panic when the
        // byte 200 lands inside a codepoint. Use the char-boundary-safe
        // helper.
        let cut = tokenade_core::caps::truncate_at_char_boundary(&r, 200);
        if cut.len() < r.len() {
            format!("{cut}…[+{} bytes]", r.len() - cut.len())
        } else {
            r
        }
    };
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "bash_rewritten",
        serde_json::json!({
            "prog": prog,
            "original_len": cmd.len(),
            "command": redacted_cmd,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

/// PreToolUse handler for Claude Code's `PowerShell` tool. Rewrites the
/// command to `tokenade wrap --shell powershell '<cmd>'` so its output is
/// compacted (the agent sees only the compact result via allow+updatedInput,
/// same substitution model as Bash). Deliberately SIMPLER than the Bash
/// handler — no chain-splitting — because the win is compacting noisy cmdlet /
/// tool output, and PowerShell handles `;`/`|` chaining inside one `-Command`.
pub(crate) fn handle_powershell_pretool(
    parsed: &serde_json::Value,
    started: std::time::Instant,
) -> i32 {
    if hook_disabled_by_env() {
        return 0;
    }
    // Fail-open when not activated (mirror the Bash/Read guards): with the
    // recovery surface gated off, compacting now would hand a folded view with
    // no way back. Let PowerShell run raw.
    let cmd = parsed
        .get("tool_input")
        .and_then(|i| i.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim();
    if cmd.is_empty() {
        return 0;
    }
    // Same reason as the Bash hook: `PowerShell(…)` deny/ask rules are matched
    // against the command this hook returns.
    let cwd = parsed
        .get("cwd")
        .and_then(|c| c.as_str())
        .map(std::path::Path::new);
    if crate::hooks::permission_rules::a_user_rule_may_guard("PowerShell", cmd, cwd) {
        return 0;
    }
    if cmdline_requests_hook_bypass(cmd) {
        // The `return 0` below says a tokenade command needs no wrapping.
        // That was already true here — the tick just ran before reaching it.
        if !crate::wrap::invokes_tokenade_itself(cmd) {
            // The message is composed and goes to stderr, which this hook
            // does not surface: it is NOT delivered, so no firing is counted
            // here. The bypass itself is still observed by the counter. This
            // site is the last one that speaks without a channel;
            // `hookwrap::dispatch` shows the form that gets through
            // (`additionalContext`).
            maybe_emit_bypass_nudge();
        }
        return 0;
    }
    // Don't double-wrap.
    if cmd.starts_with("tokenade ") || cmd.contains("tokenade wrap ") {
        return 0;
    }
    // Skip commands where wrapping is wrong or pointless:
    //   * state changes (cd / Set-Location / $env: / $var = …) — running them
    //     in a child PowerShell loses the effect, same as bash `cd`/`export`.
    //   * interactive cmdlets (Read-Host, Get-Credential) — would hang.
    // First non-space token, lowercased, for a cheap prefix check.
    let first = cmd.split(|c: char| c.is_whitespace()).next().unwrap_or("");
    let low = first.to_ascii_lowercase();
    let is_state_or_interactive = matches!(
        low.as_str(),
        "cd" | "sl"
            | "set-location"
            | "push-location"
            | "pop-location"
            | "pushd"
            | "popd"
            | "set-variable"
            | "read-host"
            | "get-credential"
            | "clear"
            | "cls"
            | "exit"
    ) || cmd.starts_with("$env:")
        || (first.starts_with('$') && cmd.contains('='));
    if is_state_or_interactive {
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "powershell_excluded",
            serde_json::json!({
                "first": first,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            }),
        );
        return 0;
    }
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "tokenade".to_string());
    // The rewritten command is run BY PowerShell, so the outer wrapping uses
    // PowerShell single-quote rules (not POSIX). `--shell powershell` tells
    // the inner `tokenade wrap` to execute under powershell.exe.
    //
    // ALWAYS single-quote the exe (not just when it has a space). The npm
    // install path contains `@tokenade` and an unquoted `& C:\…\@tokenade\…`
    // is fragile in PowerShell (`@` begins splatting / array / hashtable /
    // here-string syntax). Forward-slash it too so the literal path is
    // uniform with the Bash rewrite and the hook command. PowerShell's `&`
    // call operator runs a single-quoted path verbatim. (A 0.7.0 user report:
    // zero PowerShell-rewrite gains alongside the mangled Bash path.)
    let exe_q = powershell_quote(&exe.replace('\\', "/"));
    let rewritten = format!(
        "& {exe_q} wrap --shell powershell {}",
        powershell_quote(cmd)
    );
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "tokenade auto-wrap for compaction",
            "updatedInput": updated_input(parsed, &rewritten)
        }
    });
    println!("{decision}");
    tokenade_core::debug_log::log(
        "hookwrap-pretool",
        "powershell_rewritten",
        serde_json::json!({
            "first": first,
            "original_len": cmd.len(),
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

#[cfg(test)]
mod one_unit_is_the_fallback_not_an_exclusion {
    use super::*;

    fn segs(cmd: &str) -> Vec<command_rewrite::ChainSegment<'_>> {
        command_rewrite::split_chain_segments(cmd).expect("splits")
    }

    /// The shape the exclusion exists for: a bare assignment consumed by a
    /// later segment. Wrapped per segment the child never sees `$S`; wrapped
    /// as one unit it does, because assignment and use share a shell.
    #[test]
    fn a_bare_assignment_consumed_later_belongs_in_one_unit() {
        for c in [
            "S=/tmp/x; printf hi > $S/f; cat $S/f",
            "A=1\ngrep -rn foo crates/ | head -3\necho $A",
            // `$(...)` in the assignment is fine — the splitter is quote and
            // paren aware, so the segment boundary is still found.
            "CUTOFF=abc; tail -n 100 app.log | grep $CUTOFF",
        ] {
            assert!(
                one_unit_keeps_the_variable(&segs(c)),
                "would be excluded from compaction entirely: {c}"
            );
        }
    }

    /// One-unit wrapping cannot let a segment reach the OUTER shell. A `cd`
    /// whose directory must persist is the case, and it stays excluded.
    #[test]
    fn a_chain_that_needs_the_outer_shell_is_not_rescued() {
        for c in [
            "cd /tmp; A=1; ls | head -3; echo $A",
            "A=1; cd /tmp; echo $A",
            "source ./env.sh; echo $A; A=2; echo $A",
        ] {
            assert!(
                !one_unit_keeps_the_variable(&segs(c)),
                "one unit would silently drop an outer-shell effect: {c}"
            );
        }
    }

    /// It answers ONLY for the cross-segment case. A plain chain still goes
    /// through per-segment wrapping, which compacts each part separately.
    #[test]
    fn a_chain_with_no_cross_segment_variable_is_left_to_the_segment_path() {
        for c in [
            "grep -rn foo crates/ | head -3; ls -la",
            "cargo build; cargo test",
        ] {
            assert!(
                !one_unit_keeps_the_variable(&segs(c)),
                "per-segment wrapping is strictly better when it is available: {c}"
            );
        }
    }

    /// The list of "needs the outer shell" is not copied here.
    #[test]
    fn it_asks_the_existing_list_rather_than_keeping_a_second_one() {
        // The FAMILY: the exclusions moved down into
        // `bash_pretool/exclusions.rs`.
        let src = tokenade_core::source_guard::module_family(
            &tokenade_core::source_guard::crates_dir()
                .join("tokenade-cli")
                .join("src")
                .join("hooks"),
            "bash_pretool",
        );
        let src = src.as_str();
        // The needles are SPLIT: the family contains this test, and a needle
        // written in one piece would find itself before finding the
        // definition.
        let at = src
            .find(concat!("fn ", "one_unit_keeps_the_variable("))
            .expect("gone");
        let end = src[at..]
            .find(concat!("fn ", "rewrite_for_wrap("))
            .expect("gone")
            + at;
        let body = &src[at..end];
        assert!(
            body.contains("segment_must_not_be_wrapped"),
            "it must ask the existing list; a second copy drifts and this one \
             decides whether a command is compacted at all"
        );
        for spelled_out in ["\"cd\"", "\"export\"", "\"source\""] {
            assert!(
                !body.contains(spelled_out),
                "the list is spelled out again here: {spelled_out}"
            );
        }
    }
}
