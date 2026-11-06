//! One function per health check.
//!
//! Each answers a single question about this machine and returns the same
//! `HealthCheckResult` shape, so `cmd_healthcheck` only orders and prints
//! them.

use super::*;

mod coverage;
mod resources;
pub(crate) use coverage::*;
pub(crate) use resources::*;

/// #921 — Walk a Claude-Code `settings.json` text and return the
/// list of `/…/tokenade` binary paths that hook commands reference
/// but which don't exist on disk. Returns deduplicated paths.
///
/// Hook commands embed the binary path inside single-quoted strings
/// inside `sh -c '…'` wrappers. We scan for occurrences of
/// `tokenade`/`tokenade-mcp` mentioned next to an absolute path
/// (must start with `/` to skip bare `tokenade` references that the
/// shell would resolve via PATH). Anything matching that we can't
/// find on disk gets reported.
/// A check whose subject set is EMPTY has no verdict to give.
///
/// `Ok` prints, so answering an empty set with one puts a green line under a
/// question nobody answered. Measured 2026-09-03 on a HOME with no agent
/// config: `OK all agent-config hook paths resolve` and `OK every hook runs
/// the same tokenade binary`, both true of zero hooks, on a machine where
/// tokenade was compacting nothing at all. And on the far more common state —
/// a settings.json that exists and carries no tokenade hook — two more:
/// the paths check and the shape check, each green over nothing.
///
/// The emptiness that matters is the set the check EXAMINED, not the list of
/// places it looked: `hook_agent_configs` names where each agent would keep a
/// config, so that list is never empty and says nothing about this machine.
pub(super) fn nothing_to_check(id: &'static str, message: &str) -> HealthCheckResult {
    hc(id, CheckStatus::Info, message.into(), vec![])
}

pub(crate) fn stale_hook_binary_paths(json_text: &str) -> Vec<String> {
    hook_binary_paths(json_text)
        .into_iter()
        .filter(|p| !std::path::Path::new(p).exists())
        .collect()
}

/// Every tokenade binary an agent config points a hook at, existing or not.
///
/// Split out from [`stale_hook_binary_paths`] because "does each path resolve?"
/// and "do they all resolve to the SAME binary?" are two questions, and only
/// the first was ever asked. Measured 2026-09-01 on the development machine:
/// `~/.claude/settings.json` pointed eight hooks at `~/.cargo/bin/tokenade`
/// (0.5.1, June) and three at `~/.local/bin/tokenade` (1.1.7) — every path
/// resolved, every command matched the installed shape, and `healthcheck`
/// printed OK for both while two thirds of the session's hooks ran a binary
/// fifteen months old. A stale binary does not fail loudly; it quietly does
/// less, and 0.5.1 predates hooks-first and `mcp-wrap`.
///
/// Note the backslash is a separator: the installer writes shell-escaped JSON
/// (`'\''…'\''`), so a Windows path is split at every component. That is why
/// only forward-slash paths are collected — a Windows agent config is out of
/// this extractor's reach, and pretending otherwise would report drive letters
/// as binaries.
pub(crate) fn hook_binary_paths(json_text: &str) -> Vec<String> {
    use std::collections::BTreeSet;
    // Greedy scan: split on whitespace + shell quote chars, look at
    // every token, keep ones that look like an absolute path ending
    // in /tokenade or /tokenade-mcp (no further suffix).
    let mut out: BTreeSet<String> = BTreeSet::new();
    let seps = [' ', '\t', '\n', '\'', '"', '\\'];
    for tok in json_text.split(|c| seps.contains(&c)) {
        if !tok.starts_with('/') {
            continue;
        }
        // Strip a trailing colon that often appears in paths inside
        // JSON arrays. Also strip trailing ')' from shell exec
        // syntax. The split above already handled the common cases.
        let path = tok.trim_end_matches([':', ')']);
        let basename = path.rsplit('/').next().unwrap_or("");
        if basename != "tokenade" && basename != "tokenade-mcp" {
            continue;
        }
        out.insert(path.to_string());
    }
    out.into_iter().collect()
}

/// Build one [`HealthCheckResult`]. Small constructor shared by every
/// `check_*` helper below so each block reads as a sequence of values
/// rather than struct literals.
pub(super) fn hc(
    id: &'static str,
    status: CheckStatus,
    message: String,
    detail: Vec<String>,
) -> HealthCheckResult {
    HealthCheckResult {
        id,
        status,
        message,
        detail,
    }
}

/// 1a. Can we fetch the web, and at full strength?
///
/// INFO rather than FAIL: the in-process fallback still fetches, it just cannot
/// accept cookies, so cookie-gated sites (tensorflow.org is the measured case)
/// degrade. A degraded web is worth surfacing; it is not a broken install, and
/// it must not make `healthcheck` exit non-zero on a machine that works.
pub(super) fn check_web_transport() -> HealthCheckResult {
    match tokenade_core::web_fetch::transport_warning() {
        None => hc(
            "web_transport",
            CheckStatus::Ok,
            "curl found — web fetching at full strength".into(),
            vec![],
        ),
        Some(w) => hc(
            "web_transport",
            CheckStatus::Info,
            "curl not found on PATH — web fetching is degraded".into(),
            vec![
                w.to_string(),
                "install: apt install curl · apk add curl · brew install curl".into(),
            ],
        ),
    }
}

/// 1. Binary path + version — and whether it is the one the shell would run.
///
/// The path this reported used to be the running process's own, full stop,
/// which can only ever describe the copy that already won. Measured 2026-09-01
/// on the development machine: `~/.local/bin/tokenade` (1.1.7) and
/// `~/.cargo/bin/tokenade` (0.5.1) were both on PATH, and the second answers to
/// a bare `tokenade` the moment anything puts `~/.cargo/bin` first — a shell
/// rc, an editor's integrated terminal, a systemd unit, a Makefile. A fifteen
/// month old tokenade predates hooks-first and `mcp-wrap`; it
/// does not fail loudly, it just quietly does less. Nothing in the product said
/// the second copy was there.
///
/// INFO, never FAIL: two copies is a common and often deliberate state (a
/// developer running a build out of `dist/`, a distro package beside a user
/// install). The check's job is to name what the shell resolves, not to have an
/// opinion about it.
pub(super) fn check_binary() -> HealthCheckResult {
    binary_check_from(
        env::current_exe().ok().as_deref(),
        &tokenade_core::platform::tokenade_on_path(),
    )
}

/// The verdict, separated from where its two inputs come from. The real ones
/// are the process's own path and the process's own PATH, neither of which a
/// test can set without reaching outside itself.
pub(super) fn binary_check_from(
    me: Option<&std::path::Path>,
    on_path: &[std::path::PathBuf],
) -> HealthCheckResult {
    let bin = me
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unknown>".into());
    let headline = format!("binary at {bin}  ({})", env!("CARGO_PKG_VERSION"));

    let winner = on_path.first();
    let running_is_winner = match (me, winner) {
        (Some(me), Some(w)) => tokenade_core::platform::same_file(me, w),
        _ => false,
    };
    if on_path.len() <= 1 && running_is_winner {
        return hc("binary", CheckStatus::Ok, headline, vec![]);
    }

    let mut details: Vec<String> = Vec::new();
    match winner {
        Some(w) if !running_is_winner => details.push(format!(
            "a bare `tokenade` on this PATH runs {} — not the binary answering here",
            w.display()
        )),
        Some(w) => details.push(format!("a bare `tokenade` on this PATH runs {}", w.display())),
        None => details.push(
            "no `tokenade` is reachable on this PATH — agents that call it by name will not find it"
                .into(),
        ),
    }
    for extra in on_path.iter().skip(1) {
        details.push(format!("also on PATH, later: {}", extra.display()));
    }
    if on_path.len() > 1 {
        details.push(
            "PATH order alone decides which one runs; remove the copy you do not want, or put the one you do want first"
                .into(),
        );
    }
    hc("binary", CheckStatus::Info, headline, details)
}

/// 1b. Windows: Bash-command compaction needs a POSIX shell (sh/bash).
/// Stock Windows ships none — Claude Code's own Bash tool relies on Git for
/// Windows. Without it, `tokenade wrap` can't run the agent's bash commands,
/// so Bash compaction silently does nothing (Windows beta-tester 2026-06-23).
#[cfg(windows)]
pub(super) fn check_posix_shell_windows() -> HealthCheckResult {
    let mut sh = tokenade_core::process_util::internal_command("sh");
    sh.arg("-c")
        .arg("exit 0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Windows-only probe with both streams nulled: it needs no console, and
    // `healthcheck` is user-facing enough that a flashing window is noticed.
    tokenade_core::process_util::configure_no_window(&mut sh);
    let sh_ok = sh.status().map(|s| s.success()).unwrap_or(false);
    let git_bash = tokenade_core::process_util::git_bash().map(|p| p.display().to_string());
    if sh_ok || git_bash.is_some() {
        hc(
            "posix-shell",
            CheckStatus::Ok,
            "POSIX shell available for Bash-command compaction".into(),
            git_bash
                .map(|p| vec![format!("Git Bash at {p}")])
                .unwrap_or_default(),
        )
    } else {
        hc(
            "posix-shell",
            CheckStatus::Fail,
            "no sh/bash found — Bash-command compaction is OFF".into(),
            vec![
                "install Git for Windows (git-scm.com/download/win) so `tokenade wrap` \
                 can run the agent's shell commands"
                    .into(),
            ],
        )
    }
}

/// Local log integrity (nothing leaves the machine): corrupted debug.log
/// lines (interleaved writes) and corrupt/absurd gain rows.
/// These are the signals that caught the 2026-07 field bugs — surfacing
/// them in `tokenade doctor` turns every install into a detector.
pub(crate) fn check_local_log_integrity(home: &std::path::Path) -> HealthCheckResult {
    let dir = home.join(".tokenade");
    let mut detail: Vec<String> = Vec::new();
    let mut worst = CheckStatus::Ok;
    let mut warn = |d: String, w: &mut CheckStatus| {
        detail.push(d);
        if *w == CheckStatus::Ok {
            *w = CheckStatus::Info;
        }
    };

    // gain.jsonl: unparseable / negative / absurd rows.
    if let Ok(raw) = fs::read_to_string(dir.join("gain.jsonl")) {
        let (mut bad, mut absurd, mut total) = (0usize, 0usize, 0usize);
        for line in raw.lines().filter(|l| !l.trim().is_empty()) {
            total += 1;
            match serde_json::from_str::<serde_json::Value>(line) {
                Err(_) => bad += 1,
                Ok(v) => {
                    // The ledger's fields are `before`/`after` (see
                    // `gain::record`). This read `before_tokens`/`after_tokens`
                    // — names that appear only in debug-log records — so both
                    // sides were always 0 and the implausibility check below
                    // could never fire on any ledger.
                    let b = v.get("before").and_then(|x| x.as_u64()).unwrap_or(0);
                    let a = v.get("after").and_then(|x| x.as_u64()).unwrap_or(0);
                    if a > b || b > 50_000_000 {
                        absurd += 1;
                    }
                }
            }
        }
        if bad > 0 {
            warn(
                format!("gain.jsonl: {bad}/{total} corrupt line(s) — savings silently lost"),
                &mut worst,
            );
        }
        if absurd > 0 {
            warn(
                format!("gain.jsonl: {absurd} implausible row(s) (after>before or >50M)"),
                &mut worst,
            );
        }
    }

    // debug.log: interleaved-write corruption ratio.
    if let Ok(raw) = fs::read_to_string(dir.join("debug.log")) {
        let mut bad = 0usize;
        let mut total = 0usize;
        for line in raw.lines().filter(|l| !l.trim().is_empty()) {
            total += 1;
            if serde_json::from_str::<serde_json::Value>(line).is_err() {
                bad += 1;
            }
        }
        if total > 0 && bad * 100 > total {
            warn(
                format!("debug.log: {bad}/{total} corrupt line(s) (>1%) — check for pre-0.7.5 binaries still running"),
                &mut worst,
            );
        }
    }

    if detail.is_empty() {
        hc(
            "local-logs",
            CheckStatus::Ok,
            "ledgers + debug.log healthy".into(),
            vec![],
        )
    } else {
        let msg = format!("{} local log anomaly(ies)", detail.len());
        hc("local-logs", worst, msg, detail)
    }
}

/// Stale absolute binary paths: every hook/plugin entry embeds the exe path
/// at install time; a package-manager move (brew → npm, npm prefix change)
/// leaves configs pointing at a dead path → TOTAL silent fail-open (field
/// case: hooks pinned to /opt/homebrew after a reinstall under ~/.local).
/// Installed-but-unused MCP servers: every configured server ships its full
/// tool manifest into EVERY session's context, whether the model ever calls
/// it or not (field data: one server with ~25 tools + system instructions,
/// ZERO calls across 15 sessions — pure manifest overhead). Scans
/// ~/.claude.json `mcpServers` against mcp__<name>__ tool_use occurrences in
/// the last 30 days of transcripts and flags dead weight.
/// Installed-hook integrity: a tokenade-tagged hook whose command was
/// EDITED (foreign shell appended, binary swapped) is an injection vector —
/// the hook runs on every tool call with the user's privileges (rtk ships
/// the same guard after their SA-2025-RTK-001). We re-derive what the
/// command SHOULD look like (build_hook_command shape) and flag drift.
/// C13 (dead-path class): a lever that compiles, passes lab tests, and
/// NEVER fires in production is invisible (17 PowerShell compactors were
/// unreachable for months; jest/vitest modules were orphaned; diff-on-
/// reread was dead for months on a unit bug). Compare the last 30 days of
/// ledger ops against the levers that any active install should exercise.
pub(crate) fn check_ledger_integrity(_home: &std::path::Path) -> HealthCheckResult {
    // Reconciliation surfaced in `doctor`: the accounting invariants that a
    // double-count or decoupled-`saved` regression would break.
    let (checked, bad) = tokenade_core::gain::audit_gains();
    if bad.is_empty() {
        HealthCheckResult {
            id: "ledger-integrity",
            status: CheckStatus::Ok,
            message: format!("{checked} ledger rows satisfy the accounting invariants"),
            detail: vec![],
        }
    } else {
        HealthCheckResult {
            id: "ledger-integrity",
            status: CheckStatus::Fail,
            message: format!(
                "{} accounting invariant violation(s) across {checked} rows                  (run `tokenade audit-gains` for detail)",
                bad.len()
            ),
            detail: bad,
        }
    }
}

mod ledger;
mod trust;
mod wiring;
pub(crate) use ledger::*;
pub(crate) use trust::*;
pub(crate) use wiring::*;

#[cfg(test)]
include!("checks_tests.rs");
