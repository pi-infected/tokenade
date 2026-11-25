//! `tokenade` — the Tokenade CLI.
//!
//! Subcommands:
//!   tokenade proxy <cmd...>   run a command, print compressed output, log savings
//!   tokenade filter <cmd...>  filter output piped on stdin for the given command
//!   tokenade index [--force]  build & cache the codebase symbol index
//!   tokenade query <name>     find a symbol
//!   tokenade impact <file>    show refactor impact radius
//!   tokenade map              print the codebase structure map
//!   tokenade skeleton <file>  print a file's skeleton
//!   tokenade gain [--history] show cumulative token savings
//!   tokenade statusline       one-line summary for ~/.claude/settings.json statusLine
//!   tokenade measure          scan project, report startup-token cost of agent-loaded files
//!   tokenade detect           print the detected agent
//!   tokenade install          register tokenade with the detected agent
//!                             flags: --with-hook · --with-claude-md · --with-lsp-enforcement-hook · --with-brevity-skill · --only
//!   tokenade mcp              launch the MCP server on stdio
//!   tokenade --version

mod bounded_input;
mod edit_hint;
mod oss_migration;
mod semantic_cli;
mod star_invite;
use edit_hint::{
    edited_file_path, emit_edit_diff_hint_from_stdin, path_within_root, touch_cross_project_index,
};
mod atomic_io;
mod commands;
mod hook_health;
// Rank 4, the LLM proxy.
//
// WIRED on the operator's decision of 2026-08-16, after being written and
// tested with no route reaching it. The `dead_code` allowance that stood while
// the routing was absent is gone, as its own comment promised: an attribute
// that outlives its reason becomes a place for real dead code to hide.
//
// What is wired is `tokenade llm-proxy` and nothing else. `tokenade install`
// still does not look for a proxy, start one, or write a base URL anywhere:
// the rungs above install for everybody because they cannot break anything,
// and this one can — a base URL pointing at a proxy that is not running fails
// every turn — so it is asked for by name, per agent.
mod llm_proxy;
pub(crate) use atomic_io::*;
pub(crate) use commands::*;
pub(crate) use hook_health::*;
use std::env;

mod command_table;
mod help_routing;
mod install;
mod mcp_proxy;
use crate::install::*;
mod hooks;
mod said_once;
use crate::hooks::*;
mod compactors;
use crate::compactors::*;
mod dashboard_cli;
use crate::dashboard_cli::*;
mod lifecycle;
use crate::lifecycle::*;
mod wrap;
use crate::wrap::*;
mod healthcheck;
use crate::healthcheck::*;
mod code_intel;
use crate::code_intel::*;
mod install_cli;
use crate::install_cli::*;
mod codex_trust;
mod hookwrap;
use crate::hookwrap::*;
mod diagnostics_cli;
use crate::diagnostics_cli::*;
mod feature_cli;
use crate::feature_cli::*;
mod bench_cli;
use crate::bench_cli::cmd_bench;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tokenade_core::{
    auto, command_rewrite, competitors, detect, filter, framework, gain as gain_log, index, judge,
    redact, sandbox, semantic, tokenizer, web, Savings, VERSION,
};

// `main.rs` is the DISPATCHER. Whatever does not dispatch goes down into
// `commands/` — except `root()`, which every index key depends on and which a
// guard reads HERE, and `log_gain`, which writes to the ledger.
pub(crate) use commands::{argv_helpers::*, filter_cmd::*, formatting::*, index_cmds::*};

fn main() {
    // FIRST thing on Windows, before anything can paint to screen: hide the
    // console window Windows allocates when a GUI agent (Claude Code / a
    // Node/Electron harness with no console of its own) launches this
    // console-subsystem binary for a hook. Left unhidden it flashes on every
    // per-prompt / per-tool-call invocation — the beta "windows flashing in a
    // loop" report. Guarded to never touch an interactive terminal. No-op off
    // Windows. Must precede any spawn/stdout work below.
    tokenade_core::process_util::hide_spurious_console();

    // FIRST thing, before any native component can be touched: install the
    // fatal-signal guard. A SIGSEGV/SIGBUS/SIGILL/SIGABRT in a bundled C
    // extension (sqlite-vec / tree-sitter) or the embedding runtime would
    // otherwise dump core on EVERY hook fire — the Fedora report where
    // systemd-coredump writing a huge tokenade core per invocation slowed the
    // whole machine. The guard logs a breadcrumb and `_exit(0)` (fail-open, no
    // core). Opt out with TOKENADE_NO_CRASH_GUARD=1. No-op on non-Unix.
    tokenade_core::crash_guard::install();

    // Restore SIG_DFL for SIGPIPE. Rust ignores it before `main`, which turns
    // every `tokenade … | head` (or an early-quit `less`) into a Rust panic on
    // stdout instead of the silent exit every other CLI performs.
    //
    // NOT for the subcommands that WRITE to a child's pipe and handle `EPIPE`
    // themselves: `mcp-proxy` breaks its forwarding loop on a failed
    // `write_all` (a killed process never reaches that branch, so the wrapped
    // server's exit stops shutting the proxy down cleanly), and `exec` feeds a
    // script to an interpreter's stdin best-effort so that `wait` can surface
    // the interpreter's real error. Dying on SIGPIPE there replaces a
    // diagnostic with a silent 141.
    {
        let sub = std::env::args().nth(1).unwrap_or_default();
        let handles_own_pipes = matches!(
            sub.as_str(),
            "mcp-wrap" | "mcp_wrap" | "mcp-proxy" | "mcp" | "exec" | "wrap" | "wrap-mcp"
        );
        if !handles_own_pipes {
            tokenade_core::crash_guard::restore_sigpipe_default();
        }
    }

    // Defense-in-depth (#454): tighten ~/.tokenade and stash perms
    // to 0700/0600 on every CLI start. Best-effort, silent on
    // permission errors or non-Unix targets.
    {
        let _p = tokenade_core::profile::phase("startup:storage_perms");
        tokenade_core::platform::ensure_storage_perms_secure();
    }

    // D4 — fire-and-forget background updater. Cheap fast-path: if the
    // 24h TTL hasn't elapsed we skip the thread spawn entirely. Never
    // blocks the user; never writes to stdout/stderr (would corrupt MCP
    // JSON-RPC when the MCP server transitively links this binary).
    // Opt-out: TOKENADE_NO_AUTO_UPDATE=1 OR --no-auto-update arg.
    // Session key for gain rows booked by THIS process: wrap children get
    // it via the TOKENADE_SESSION env the pretool rewrite injects (hook
    // processes set it from their payload instead).
    if let Some(skey) = tokenade_core::session::ambient_session_key() {
        tokenade_core::session::set_session_key(&skey);
    }
    let args: Vec<String> = env::args().skip(1).collect();
    let is_dry_run = args.iter().any(|a| a == "--dry-run");
    // Compile the redaction rules OFF the critical path,
    // for every content subcommand at once.
    //
    // `redact::warm` has existed since 2026-08-04 and was wired into exactly
    // two entry points: `cmd_wrap` and `cmd_hookwrap_pretool`. Measured
    // 2026-08-08 with `TOKENADE_PROFILE=1`, release build, on the two channels
    // that carry the most traffic in the product:
    //
    //     wrap 'true'   (warm wired)      rd:providers    643 µs    8.6 ms/call
    //     hookwrap      (not wired)       rd:providers  8 697 µs   19.1 ms/call
    //     read -        (not wired)       rd:providers  8 535 µs   18.5 ms/call
    //
    // The payload is not what costs: 33 kB through `read -` costs 0.9 ms more
    // than 36 bytes. The 8.7 ms is 102 `Regex::new` calls, paid once per
    // PROCESS — and a tokenade process serves exactly one call. `read -` is
    // what the six plugin bridges invoke, and `hookwrap` is the Bash
    // PostToolUse hook, so the fix reached neither of the paths that matter.
    //
    // Hoisted here rather than added to each entry point, because "one fix,
    // one entry point, seven still paying" is precisely the bug being fixed.
    // A racing main thread blocks exactly as long as it would have anyway
    // (`OnceLock`), and a process that exits first loses nothing.
    if !is_dry_run && subcommand_touches_content(args.first().map(String::as_str)) {
        std::thread::spawn(|| {
            tokenade_core::redact::warm();
        });
    }
    // `uninstall` wipes ~/.tokenade as its last step; a background updater
    // would RE-CREATE the dir after the wipe (update-check.json / update.lock
    // reappear — "uninstall didn't work"). Suppress startup side-effects on
    // uninstall, exactly as --dry-run does.
    let is_uninstall = args.first().map(String::as_str) == Some("uninstall");
    let suppress_bg = is_dry_run || is_uninstall;
    // 1.x → 2.x (open source): drop the licence/reporter state once, and
    // tell a human (stderr, TTY only) that there is no account any more.
    if !suppress_bg {
        let _p = tokenade_core::profile::phase("startup:oss_migration");
        oss_migration::run();
    }
    if !args.iter().any(|a| a == "--no-auto-update") && !suppress_bg {
        let _p = tokenade_core::profile::phase("startup:updater_gate");
        tokenade_core::updater::spawn_background_check();
    }
    // The out-of-repo CLI lesson is refreshed on EVERY invocation (wrap, hooks,
    // --version). Not an opt-in: neither `tokenade install`, nor the version
    // gate, nor TOKENADE_NO_AUTO_UPDATE. Silent. A file already up to date is
    // not rewritten.
    //
    // #1036 — silent post-upgrade refresh of hooks / MCP / fence court.
    // Gated on a version-mismatch check so it fires at most once per
    // upgrade. Skipped when TOKENADE_NO_AUTO_UPDATE is set. The lecture
    // itself is NOT in this gate.
    if !suppress_bg {
        {
            let _p = tokenade_core::profile::phase("startup:heal_global_lectures");
            maybe_heal_global_lectures();
        }
        let _p = tokenade_core::profile::phase("startup:post_upgrade_refresh");
        maybe_run_post_upgrade_refresh();
    }
    // The release profile unwinds (see Cargo.toml): a panic that nothing
    // caught would otherwise leave with 101 and a backtrace on stderr, where
    // the abort profile used to fail open through the fatal-signal handler.
    // Hooks must keep failing open — a steering hook that reports an error
    // interrupts the agent over a job that was only ever advisory — so the
    // top of `main` reproduces that contract, and hands back any output the
    // run had already produced and armed.
    let code = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&args))) {
        Ok(c) => c,
        Err(_) => {
            // Fail open, exactly as the fatal-signal handler did under the
            // abort profile. No attempt to hand back armed output here: the
            // salvage buffer is BORROWED, and unwinding frees the frame that
            // owns it on the way up, so reading it would be a use-after-free.
            // The window that actually matters — a panic inside a compactor —
            // is caught in place by `filter_command_fail_open`, which still has
            // the bytes.
            tokenade_core::debug_log::log(
                "main",
                "panicked_failed_open",
                serde_json::json!({ "argv1": args.get(1).cloned().unwrap_or_default() }),
            );
            0
        }
    };
    std::process::exit(code);
}

// ── The command SURFACE, as seen from the dispatcher ───────────────────
//
// These three blocks described `wrap`, `mcp-wrap` and `activate`. They were
// DOCUMENTATION comments hanging in the void — three `///` in a row with no
// item below them, for long enough that the function they described had moved
// elsewhere. A `///` without an item documents nothing and only compiles by
// tolerance; turned into `//`, they say what they always meant to say, at the
// place that dispatches.
//
// `tokenade wrap <shell-cmd>` — run an arbitrary shell command via
// `sh -c`, apply the right per-command compactor to the combined
// stdout/stderr, write the COMPACT result to stdout, the banner to
// stderr, and exit with the underlying command's status code.
//
// The hook installer rewrites Bash tool invocations to call this:
// `Bash(git log -5)` becomes `Bash(tokenade wrap 'git log -5')`.
// Claude Code's UI shows the rewritten command and prints the
// compact output as the tool result — no `permissionDecision: deny`,
// no "Error:" prefix.
//
// This is the SAME architectural pattern RTK uses (its
// `src/hooks/hook_cmd.rs`); the earlier
// deny+reason pattern caused every intercept to surface as "Error:"
// in the Claude Code UI even when the data was fine.
// `tokenade mcp-wrap -- <target-binary> [<target-args>…]` — spawn
// the target MCP server as a child process and stream JSON-RPC
// between it and the agent that launched this proxy. On every
// inbound response (`tools/call` result) we walk the
// `result.content[].text` payloads and run them through
// [`tokenade_core::disclose::maybe_stash`]: anything above the 5 k
// token threshold gets stashed to disk and replaced with a
// structural summary. Errors, notifications, and non-text content
// items are forwarded verbatim.
//
// The proxy is what `tokenade install` rewrites every third-party
// MCP entry to spawn. Each agent boot spawns one proxy per MCP
// server; the proxy exits when its stdin closes.

fn run(args: &[String]) -> i32 {
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest = &args[args.len().min(1)..];

    // A5 fleet catch — TOTAL silent failure: a plugin overwrote a client's
    // settings.json, every tokenade hook vanished, ~3M tokens flowed
    // uncompacted with ZERO signal (self-heal was gated on a SURVIVING
    // hook). When hooks are gone, hook-side code never runs — the only
    // remaining trigger is the user invoking the CLI. So: on user-facing
    // commands, if we previously installed but Claude settings now carry
    // no tokenade hooks, warn LOUDLY (throttled to once per day).
    warn_if_hooks_vanished(cmd);

    // L85 — universal --help/-h interception. Without this, typing
    // `tokenade <cmd> --help` runs `<cmd>` with --help treated as
    // an unknown arg (silently ignored) — destructive for commands
    // like `index` (rebuilds 4192-file index), `query` (searches
    // for literal "--help"), `impact` (treats --help as file path).
    // For dedicated subcommand help, see the per-cmd functions
    // (e.g. install_help_text); this fallback shows the top-level
    // command list so a confused user always gets a useful response.
    //
    // Exceptions: subcommands that genuinely accept `--help` as
    // shell-passthrough syntax (proxy/raw/wrap/filter — they forward
    // to the underlying tool which has its own --help). For these
    // we let the call proceed normally.
    //
    // #659 — scan the WHOLE rest, not just position 0. Originally
    // `.take(1)` was used because some handlers (install, scrub-*)
    // already scan rest for --help themselves. But the validators
    // added in #655/#656 (uninstall, unwrap-mcps, evict-stale)
    // accept --help only as a whitelisted flag and never route to
    // the help branch — so `uninstall --dry-run --help` ran the
    // dry-run and silently swallowed --help. Scan-anywhere makes
    // --help work in any position for every non-passthrough cmd.
    let helpish = crate::help_routing::asks_for_help(rest);
    let passthrough_cmd = cmd_forwards_help(cmd);
    if helpish && !passthrough_cmd {
        return help_routing::respond(cmd, rest);
    }
    // One-shot migration of pre-rename `.sts/` → `.tokenade/` in the cwd.
    // Cheap when there's nothing to do (one `metadata` call). Surfaces a
    // single-line stderr note when a rename actually happens so the user
    // (or the agent invoking us) sees what changed.
    //
    // #997 — also surface the `AlreadyMigrated` case (both dirs coexist).
    // `MigrationOutcome` documents it as "we log a one-liner suggesting
    // they merge by hand", but pre-fix only `Renamed` was matched here,
    // so users with a leftover `.sts/` next to `.tokenade/` had no idea
    // their legacy per-project ledger was being silently ignored. The
    // hint is still TTY-only (hooks/MCP stay silent).
    match tokenade_core::paths::migrate_legacy_dir(&root()) {
        tokenade_core::paths::MigrationOutcome::Renamed => {
            tokenade_core::progress!(
                "[tokenade] migrated legacy `.sts/` → `.tokenade/` (one-time, post-rename)"
            );
        }
        tokenade_core::paths::MigrationOutcome::AlreadyMigrated => {
            tokenade_core::progress!(
                "[tokenade] both `.sts/` (legacy) and `.tokenade/` exist next to each \
                 other — the legacy dir is IGNORED. Merge by hand (e.g. `cat \
                 .sts/gain.jsonl >> .tokenade/gain.jsonl && rm -rf .sts/`) then re-run."
            );
        }
        tokenade_core::paths::MigrationOutcome::NotPresent => {}
        tokenade_core::paths::MigrationOutcome::Failed(e) => {
            tokenade_core::progress!(
                "[tokenade] could not migrate legacy `.sts/`: {e} — \
                 your gain ledger may be split. Merge by hand."
            );
        }
    }
    match cmd {
        "--version" | "-V" | "version" => {
            println!("tokenade {VERSION}");
            0
        }
        // REMOVED 2026-08-10: `proxy` was an argv-only variant of `wrap`, and
        // its main consumer was the PATH shim, which is gone. One command per
        // behaviour; the name is reserved for the LLM proxy.
        "proxy" => {
            eprintln!(
                "tokenade proxy: removed. Use `tokenade wrap '<command>'` — it does the \
                 same compaction and also handles pipes, redirections and globs.\n  \
                 The name `proxy` is reserved for the LLM proxy."
            );
            2
        }
        "shim-exec" => cmd_shim_exec(rest),
        "raw" | "bypass" | "noproxy" => cmd_raw(rest),
        "filter" => cmd_filter(rest),
        "index" => cmd_index(rest),
        "index-touch" => cmd_index_touch(),
        "query" | "find" => cmd_query(rest),
        "impact" => cmd_impact(rest),
        "map" => cmd_map(rest),
        "skeleton" => cmd_skeleton(rest),
        "web" => cmd_web(rest),
        "crawl" => cmd_crawl(rest),
        // ONE search surface: `serp` and its engine-name aliases were a
        // second command for the saved-page half of the same job.
        "search" | "serp" | "google" | "bing" | "ddg" => cmd_search(rest),
        "snapshot" => cmd_snapshot(rest),
        // Content entry point: `read` auto-detects the format and dispatches
        // to the right CORE compactor (tokenade_core::auto). The ~40 per-format
        // command entry points were removed 2026-07 (PO order) — the format
        // detection/compaction logic lives entirely in tokenade-core and is
        // reached via `read` (and the `wrap` command path for live commands).
        "read" => cmd_auto(rest),
        "ask" | "semantic" | "sem" => semantic_cli::cmd_semantic(rest),
        "subagent-brief" | "subagent_brief" => hooks::cmd_subagent_brief(rest),
        "session-tick" => hooks::cmd_session_tick(rest),
        "fold-payload" | "fold_payload" => llm_proxy::cli::cmd_fold_payload(rest),
        "expand-ref" | "expand_ref" | "xref" => cmd_expand_ref(rest),
        "expand-fold" | "expand_fold" | "xfold" => cmd_expand_fold(rest),
        "style" => cmd_style(rest),
        "watch" => cmd_watch(rest),
        "dashboard" | "dash" | "stats" => cmd_dashboard(rest),
        "gain" => cmd_gain(rest),
        "audit-gains" => cmd_audit_gains(rest),
        "statusline" => cmd_statusline(rest),
        // GitHub Copilot CLI postToolUse hook: folds the tool result and
        // returns Copilot's `modifiedResult` replace-output shape.
        "hookwrap-copilot" => cmd_hookwrap_copilot(),
        // Antigravity/Gemini AfterTool hook: folds tool_response.llmContent and
        // returns the deny+reason replace-output shape.
        "gemini-after-tool" => cmd_gemini_after_tool(),
        "gemini-lifecycle" => cmd_gemini_lifecycle(),
        // Detached, debounced recompute of the real per-model savings cache
        // (see dashboard_cli::spawn_real_savings_refresh). Runs the ~2s
        // transcript walk OFF the statusline hot path.
        "real-savings-refresh" => {
            let _ = tokenade_core::gain::real_savings_refresh();
            let r = root();
            let _ = std::fs::remove_file(r.join(".tokenade").join("real-savings-refresh.running"));
            0
        }
        // 1.x spawned a detached `reporter-run` child; a 1.x process still
        // alive after the swap to 2.x may spawn it against this binary.
        // Hidden no-op: there is no reporter any more.
        "reporter-run" => 0,
        "measure" => cmd_measure(rest),
        "judge" | "groundedness" => cmd_judge(rest),
        "read-mode" | "readmode" => cmd_read_mode(rest),
        "pack" => cmd_pack(rest),
        "review-context" | "review" => cmd_review_context(rest),
        "mcp-stats" | "mcpstats" => cmd_mcp_stats(rest),
        "mistake" | "mistakes" => cmd_mistake(rest),
        "disclose" => cmd_disclose(rest),
        "turn-summary" | "summarize-turn" | "turnsummary" => cmd_turn_summary(rest),
        "add-compactor" | "add-filter" => cmd_add_compactor(rest),
        "receipt" | "savings-receipt" => cmd_receipt(rest),
        "cache-note" | "cachenote" => cmd_cache_note(rest),
        "execute" | "exec" | "run-script" => cmd_execute(rest),
        "detect" => cmd_detect(rest),
        "incompatibilities" | "incompat" | "detect-competitors" | "competitors" | "scan" => {
            cmd_detect_competitors(rest)
        }
        "discover" => cmd_discover(rest),
        "healthcheck" | "health" | "doctor" => cmd_healthcheck(rest),
        "audit-claude-md" | "audit-claudemd" | "claude-md-audit" | "claudemd" => {
            cmd_audit_claude_md(rest)
        }
        "hooks" => cmd_hooks(rest),
        "install" => cmd_install(rest),
        "hookwrap" => cmd_hookwrap(),
        "gemini-hook" | "geminihook" => cmd_gemini_hook(),
        // Gemini CLI AfterTool web-result compactor: replaces
        // google_web_search / web_fetch results via the documented
        // deny+reason replace channel (capability-gated at install time).
        "gemini-afterhook" => crate::hooks::cmd_gemini_afterhook(),
        "shell-init" | "shell_init" => cmd_shell_init(rest),
        "shellwrap" => cmd_shellwrap(rest),
        "hookwrap-pretool" => cmd_hookwrap_pretool(),
        "hookwrap-postread" => cmd_hookwrap_postread(),
        "hookwrap-postwebfetch" => crate::hooks::cmd_hookwrap_postwebfetch(),
        // PostToolUse WebSearch compactor: replaces tool_response via
        // hookSpecificOutput.updatedToolOutput (Claude Code >= 2.1.121;
        // capability-gated at install time).
        "hookwrap-websearch-post" => crate::hooks::cmd_hookwrap_websearch_post(),
        "hookwrap-mcp-post" => crate::hooks::mcp_posttool::cmd_hookwrap_mcp_post(),
        "hookwrap-cursor-mcp-post" => crate::hooks::mcp_posttool::cmd_hookwrap_cursor_mcp_post(),
        "hookwrap-userprompt" => cmd_hookwrap_userprompt(),
        "hookwrap-sessionstart" => cmd_hookwrap_sessionstart(),
        "hookwrap-subagentstart" => hooks::cmd_hookwrap_subagent_start(),
        "hookwrap-precompact" => cmd_hookwrap_precompact(),
        "hookwrap-subagentstop" => crate::hooks::cmd_hookwrap_subagentstop(),
        "pre-edit-snapshot" | "hookwrap-pre-edit" => cmd_pre_edit_snapshot(),
        // `exec-compact` is not a second implementation: it is the name three
        // of our OWN guidance sites tell agents to run — the subagent nudge and
        // the rules we write into every project. The binary answered "unknown
        // command", so an agent that followed our instructions was punished for
        // it, on the first noisy command it tried. Making the name real fixes
        // every rules file already on disk; renaming the docs would not.
        "wrap" | "exec-compact" => cmd_wrap(rest),
        "upgrade" | "self-update" | "selfupdate" => cmd_upgrade(rest),
        "semantic-status" | "semantic_status" => semantic_cli::cmd_semantic_status(rest),
        "evict-stale" | "evict_stale" => cmd_evict_stale(rest),
        "compact-sessions" | "compact_sessions" => cmd_compact_sessions(rest),
        "scan-savings" | "scan_savings" => cmd_scan_savings(rest),
        "scrub-secret" | "scrub_secret" => cmd_scrub_secret(rest),
        "scrub-cache" | "scrub_cache" => cmd_scrub_cache(rest),
        "redact" => cmd_redact(rest),
        "calibrate" => cmd_calibrate(rest),
        "bench" | "benchmark" => cmd_bench(rest),
        "replay" => cmd_replay(rest),
        "trust-filters" | "trust_filters" => cmd_trust_filters(rest),
        "list-commands" | "list_commands" => cmd_list_commands(rest),
        "ensure-hooks" | "ensure_hooks" => cmd_ensure_hooks(rest),
        // Repair/verify: (re)write Codex hook trust into ~/.codex/config.toml
        // so the hooks we installed actually run (Codex refuses untrusted
        // hooks — an untrusted tokenade hook is a silent no-op). `tokenade
        // install` does this automatically; this is the standalone re-apply.
        "codex-trust" | "codex_trust" => match crate::codex_trust::ensure_trust_default() {
            Ok(n) => {
                println!("trusted {n} Codex hook(s) in ~/.codex/config.toml");
                0
            }
            Err(e) => {
                eprintln!("codex-trust: {e}");
                1
            }
        },
        // `mcp-wrap` is the name: this wraps an MCP server exactly as `wrap`
        // wraps a command. `mcp-proxy` stays as a DEPRECATED alias because the
        // old name is already written into the `CLAUDE.md` block shipped to
        // every user — renaming without it would break their file silently.
        // `proxy` now belongs to the LLM proxy.
        "mcp-wrap" | "mcp_wrap" | "mcp-proxy" | "mcp_proxy" => mcp_proxy::cmd_mcp_proxy(rest),
        "llm-proxy" | "llm_proxy" => llm_proxy::cli::cmd_llm_proxy(rest),
        "unwrap-mcps" | "unwrap_mcps" => cmd_unwrap_mcps(rest),
        "uninstall" => cmd_uninstall(rest),
        "help" | "--help" | "-h" => {
            // #505 — `tokenade help <subcmd>` previously ignored the
            // subcmd arg and dumped the generic help dump, even for
            // subcommands that ship dedicated help text (hooks per #461,
            // scrub-secret per #483, install per #412, …). Forward to
            // `tokenade <subcmd> --help` so the user lands on the right
            // page. Falls through to generic help when no arg or when
            // the subcmd is itself a help alias (avoids infinite recursion).
            let helpless = ["help", "--help", "-h", ""];
            if let Some(sub) = rest.first() {
                if !helpless.contains(&sub.as_str()) {
                    let forwarded: Vec<String> = std::iter::once(sub.clone())
                        .chain(std::iter::once("--help".to_string()))
                        .collect();
                    return run(&forwarded);
                }
            }
            print_help();
            0
        }
        other => {
            // #507 — flag-shaped unknown args don't deserve the
            // command-name fuzzy match. `tokenade -v` previously
            // suggested `csv` (Levenshtein 2 from `-v`) which is
            // gibberish. Anything starting with `-` is the user
            // reaching for a flag, not a command — point them at the
            // closest real flag instead.
            if other.starts_with('-') {
                let suggestion = match other {
                    "-v" => Some("--version (or -V)".to_string()),
                    "-h" | "--h" => Some("--help".to_string()),
                    _ => None,
                };
                // #666 — Levenshtein fallback for partial / transposed
                // root-level flag typos like `--he` → `--help`,
                // `--versoin` → `--version`. The hard-coded cases above
                // catch single-letter shortcuts (`-v`, `--h`) where
                // distance >3 from any canonical flag but the user
                // intent is unambiguous.
                let suggestion = suggestion.or_else(|| {
                    const ROOT_FLAGS: &[&str] = &["--help", "-h", "--version", "-V"];
                    ROOT_FLAGS
                        .iter()
                        .map(|k| (*k, levenshtein(other, k)))
                        .min_by_key(|(_, d)| *d)
                        .filter(|(_, d)| *d <= 3)
                        .map(|(k, _)| k.to_string())
                });
                match suggestion {
                    Some(flag) => eprintln!(
                        "tokenade: unknown flag '{other}'. Did you mean `{flag}`?"
                    ),
                    None => eprintln!(
                        "tokenade: unknown flag '{other}'. Try `tokenade --help` for available flags."
                    ),
                }
                return 2;
            }
            // L86 — fuzzy-suggest the nearest known command for typos.
            // `tokenade insall` → "did you mean 'install'?".
            // Bound 2 catches single-character typos and small
            // transpositions; beyond that the suggestion is likely
            // wrong and silence is better.
            // A FORMAT name is not a typo, and the fuzzy matcher makes it
            // worse: `csv` came back "did you mean 'ask'?" and `logs` came
            // back "did you mean 'login'?" — a user holding a log file was
            // pointed at authentication. These names were subcommands once;
            // formats are detected from content now, so there is exactly one
            // right answer and the product already knows it.
            if warn_format_not_a_command(other) {
                return 2;
            }
            let suggestion = nearest_command(other, 2);
            match suggestion {
                Some(near) => eprintln!(
                    "tokenade: unknown command '{other}'. Did you mean '{near}'? \
                     Try `tokenade help` for the full list."
                ),
                None => {
                    eprintln!("tokenade: unknown command '{other}'. Try `tokenade help`.")
                }
            }
            2
        }
    }
}

// All top-level commands recognised by [`run`]. Used by
// [`nearest_command`] for typo suggestions. Kept as a free function
// (not part of the dispatch) so the list lives in one place; if a
// new command lands without being added here, the worst that
// happens is no typo suggestion fires for it — never a crash.

/// `include_str!` is resolved at COMPILE time, so a path it names is a BUILD
/// dependency. Pointing one at a gitignored file makes the crate unbuildable
/// from a clean checkout while still building perfectly on the machine where
/// the file happens to exist — which is how `.claude/CLAUDE.md` broke
/// `cargo test --workspace` in CI without breaking it for anyone locally.
///
/// The failure is invisible exactly where it matters, so the guard reads the
/// repository's own `.gitignore` rather than a hardcoded list: a directory
/// added to it tomorrow is covered without anyone remembering this test.
/// Every command keys its index on `root()`, and `root()` must answer the
/// PROJECT root — not the directory the command happened to run in. Asserted
/// on the source because the alternative is mutating the process cwd, which is
/// global and would race every other test in the binary.///
/// Format names an agent naturally types as if they were commands. Shared by
/// BOTH unknown-command sites: the dispatcher, and the `--help` path. They had
/// drifted — `tokenade csv` explained that csv is a format, while
/// `tokenade csv --help` answered "did you mean `ask`?", on a branch whose own
/// comment cites `tokenade csv --help` as the case it exists to serve.
const FORMAT_NAMES: &[&str] = &[
    "ansible",
    "archive",
    "curl",
    "csv",
    "diff",
    "dns",
    "ghactions",
    "graphql",
    "html",
    "ini",
    "json",
    "jsonl",
    "log",
    "logs",
    "markdown",
    "md",
    "metrics",
    "ndjson",
    "netstat",
    "notebook",
    "openapi",
    "patch",
    "pkginstall",
    "prometheus",
    "serp",
    "sourcemap",
    "stack",
    "stacktrace",
    "systemd",
    "toml",
    "tsv",
    "xml",
    "yaml",
    "yml",
];

fn root() -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // The PROJECT root, not the directory the command happened to run in.
    // Keying the index on the raw cwd gave an agent that `cd`s into a
    // subdirectory its own index of that subtree, and `query` then answered
    // `no symbol matching` about a symbol declared one crate over — a
    // confident negative from a narrower scope. See
    // `platform::project_root_of`.
    tokenade_core::platform::project_root_of(&cwd)
}

/// Append one savings record to the **global** `~/.tokenade/gain.jsonl`
/// via the shared `tokenade_core::gain` ledger so MCP, CLI, and library
/// callers all write to the same source of truth that `tokenade gain`
/// (and `tokenade dashboard`) read. The ledger is global, not per-project:
/// the `project` field in each record carries the cwd so any consumer can
/// still group by project at read time.
///
/// #541 — drops noise-tier savings (< 10 tokens absolute OR < 5 % of
/// the input). Mirrors the gate the PostToolUse hook and cmd_wrap
/// already apply; this single line covers every CLI subcommand
/// compactor (csv / openapi / graphql / web / skeleton / serp /
/// docs / sourcemap / …) without each one having to remember the
/// threshold. The compactor's stdout output is unaffected; this
/// only filters telemetry.
///
/// #545 — code-intel ops (symbol_find / semantic_search / structure_map
/// / skeleton / impact_of / call_hierarchy / expand_fold and their
/// `:variant` suffixes) are exempt from the threshold. Mirrors the
/// exemption already present in `log_mcp` (#543) and the read-time
/// expansion in the dashboard (#544). Without it, any caller that
/// passes raw `Savings::new(N, N)` for a code-intel op (current
/// `log_code_intel` happens to expand at write time, but future
/// paths might not) would be silently dropped instead of preserved
/// for the dashboard's read-time baseline expansion.
fn log_gain(label: &str, op: &str, s: Savings) {
    if !tokenade_core::gain::worth_recording(s.tokens_before as u64, s.tokens_after as u64, op) {
        return;
    }
    gain_log::record("cli", op, label, s);
}

#[cfg(test)]
include!("main_tests.rs");
