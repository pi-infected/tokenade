//! **`tokenade filter`: compact an output we are GIVEN.**
//!
//! No index, no project: we receive a named program and bytes, we return
//! shorter bytes. It is the hook's entry door, and the only command whose
//! contract is "same exit code, same useful information, fewer bytes".
//!
//! `log_emitted_additional_context` is here because it records what THIS
//! route emitted on top — the only one that emits any.

use crate::*;

/// Which shell `tokenade wrap` runs the inner command under.
///
/// Claude Code's `Bash` tool emits POSIX/bash syntax; its `PowerShell` tool
/// (used on Windows when Git Bash is absent — they're mutually exclusive)
/// emits PowerShell. Non-Claude-Code agents may shell out via cmd.exe. The
/// wrapped command MUST run under the SAME shell it was written for, or the
/// syntax is mangled (`ls`→`dir`, `&&` chaining, `$VAR`→`$env:VAR`, …).
pub(crate) fn cmd_filter(rest: &[String]) -> i32 {
    // A leading `--help` before the inner argv used to consume
    // stdin and emit nothing. `filter --help` shows docs now.
    if crate::help_routing::asks_for_help_leading(rest) {
        println!(
            "usage: <cmd> 2>&1 | tokenade filter <argv...>\n\nFilter a command's stdout (piped via stdin) AS IF it had been produced\nby the argv list you supply. Useful when you've ALREADY captured the\noutput and just want to compact it with the right per-command rules.\n\nExample:\n  cargo build 2>&1 | tokenade filter cargo build\n  git log --oneline -100 > /tmp/log.txt\n  cat /tmp/log.txt | tokenade filter git log\n\nDifferent from `tokenade wrap '<cmd>'`, which RUNS the command for you AND\nfolds what no per-command rule shrank into a recoverable stash. `filter`\ndoes not stash, so on output the compactors leave alone it returns the\ninput unchanged where `wrap` would still fold it: measured on\n`cat /proc/interrupts` (81 very wide rows), `filter` saves 0% and `wrap`\n83%. What `filter` reports is the per-command compactor alone.\nDifferent again from `tokenade read -`, which auto-detects the format\nwithout an argv hint.\n\nFlags:\n  --deliver     also run the SECOND stage — stash what is still too big and\n                point back at what the fold dropped, exactly as `wrap` does.\n                Use it on an output you already captured when you want what\n                `wrap` would have delivered, not the per-command fold alone.\n  --help, -h    show this message and exit"
        );
        return 0;
    }
    // TTY-hint when stdin is a terminal (no pipe). Without
    // this, `tokenade filter` from a shell prompt blocks silently on
    // read_to_string forever. filter ALWAYS reads from stdin (no
    // file-path argument) so we can't use the existing
    // read_stdin_or_file_with_tty_hint helper — inline the check.
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        eprintln!(
            "usage: <cmd> 2>&1 | tokenade filter <argv...>\n  \
             filter a command's stdout (piped via stdin) AS IF produced by <argv...>.\n  \
             example:    cargo build 2>&1 | tokenade filter cargo build"
        );
        return 2;
    }
    let mut input = String::new();
    if crate::bounded_input::read_stdin_lossy(&mut input).is_err() {
        eprintln!("tokenade filter: failed to read stdin");
        return 1;
    }
    // `--deliver` asks for the SECOND stage on top of the first — what `wrap`
    // actually delivers. Without it, an already-captured output has no way to
    // get the fold `wrap` would have applied: this command's help measures it
    // itself on `cat /proc/interrupts`, 0 % vs 83 %.
    let deliver = rest.iter().any(|a| a == "--deliver");
    let mut argv: Vec<String> = rest.iter().filter(|a| *a != "--deliver").cloned().collect();
    // `--staged <key>`: the producing command was staged by the Bash hook
    // instead of spelled here (#8 — see `worktree_git`). A missing stage
    // filters with no argv hint rather than failing the pipe.
    if argv.first().map(String::as_str) == Some("--staged") {
        argv = argv
            .get(1)
            .and_then(|k| crate::hooks::bash_pretool::worktree_git::take_staged_argv(k))
            .unwrap_or_default();
    }
    let rest: &[String] = &argv;
    // Third and last bare call site, closed with the same boundary as `wrap`
    // and `hookwrap`: one way of reaching a compactor, not three.
    let r = {
        let _salvage = tokenade_core::crash_guard::arm_salvage(&input, 0);
        filter::filter_command_fail_open(rest, &input)
    };
    let safe_label = redact::redact(&redact::redact_argv(rest).join(" "));
    log_gain(&safe_label, &r.filter, r.savings);
    // Defense-in-depth: filter has a `user_wants_raw`
    // passthrough branch (cargo test --nocapture, `cmd | tail`,
    // `cmd | jq`, etc.) that returns the input verbatim. Most
    // per-compactor outputs already scrub via the redaction sweep,
    // but the passthrough branch and any future-added fallback
    // would leak. Scrub at the print site. Idempotent (Cow no-clone
    // on no-match). Same belt-and-suspenders pattern as for
    // cmd_auto and for MCP auto_compact.
    let output = if deliver {
        // The SAME block as `wrap`, not a second composition: the two stages
        // interact, and two spellings of their chaining would drift where
        // the delivered output does not forgive it.
        crate::wrap::second_stage(
            r.output.clone(),
            &input,
            &rest.join(" "),
            r.savings
                .tokens_before
                .saturating_sub(r.savings.tokens_after),
            r.savings.tokens_before,
            &r.filter,
        )
        .display
    } else {
        r.output.clone()
    };
    let safe_output = tokenade_core::redact::redact_for_model(&output);
    print!("{safe_output}");
    // `filter` sits in a PIPE, and the passthrough branch named just above
    // returns the input verbatim — so on that branch the terminator was
    // appending to the caller's own data. Measured 2026-09-03:
    // `printf 'no-newline' | tokenade filter cat` returned 11 bytes for 10.
    if crate::wrap::needs_terminator(&safe_output, safe_output == input) {
        println!();
    }
    0
}

/// Log the EXACT `additionalContext` text a hook is about to emit on stdout.
/// Claude Code injects this into the model's prompt (PreToolUse/PostToolUse/
/// UserPromptSubmit/SessionStart/PreCompact) where it counts toward the
/// prompt-cache prefix — but neither the transcript nor the bench layer
/// captures hook stdout, so without this the injected text and its
/// cache-invalidation effect are invisible. No-op cost unless
/// `TOKENADE_DEBUG_LOG` is set (one env read otherwise).
pub(crate) fn log_emitted_additional_context(component: &str, channel: &str, ctx: &str) {
    let est = tokenade_core::tokenizer::estimate(ctx);
    // Overhead audit (2026-07-04): this fn COMPUTED the injected cost and
    // threw it away — 43-95k tokens/day of our own additionalContext
    // (edit hints, digests, nudges, preambles) billed to the user and
    // debited nowhere. Every emission now books class=overhead, which the
    // net figure and the weighted total already subtract. The read-
    // compaction family is NOT routed through here (their reasons are
    // permissionDecision payloads, already netted in `after`).
    tokenade_core::gain::record_estimated(
        "hook",
        "injected_context",
        component,
        "overhead",
        tokenade_core::Savings::new(est, 0),
    );
    tokenade_core::debug_log::log(
        component,
        "additional_context_emitted",
        serde_json::json!({
            "channel": channel,
            "bytes": ctx.len(),
            "est_tokens": est,
            "content": ctx,
        }),
    );
}
