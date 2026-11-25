//! **Gathering what the dashboard displays.**
//!
//! The numbers come from several ledgers that do not count the same thing —
//! the measured fold, the estimate, the bypassed, the usage reported by the
//! agent. Adding them naively yields a total nothing supports; this is where
//! we decide what adds up with what.

use super::*;

/// Path of the global gain ledger (`~/.tokenade/gain.jsonl`). Same
/// resolution rule as the writer in `tokenade_core::gain::jsonl_path`
/// so the dashboard / `tokenade gain` / `tokenade statusline` always
/// read from the same file the hook + MCP write to.
pub(crate) fn global_gain_path() -> std::path::PathBuf {
    let dir = tokenade_core::platform::home_dir().unwrap_or_else(root);
    dir.join(".tokenade").join("gain.jsonl")
}

/// Best-effort user language for the terminal dashboard's localized
/// strings: French when the POSIX locale says so, English otherwise.
/// Checked in the standard precedence order (LC_ALL > LC_MESSAGES >
/// LANG). Resolved here — not in the renderer — because dashboard.rs
/// is pure by contract (no env reads).
pub(crate) fn user_lang_is_fr() -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty())
        .is_some_and(|v| v.to_ascii_lowercase().starts_with("fr"))
}

/// Flatten a `(name, (before, after, calls))` rollup into dashboard rows,
/// dropping the ones that saved nothing.
///
/// Compactors, wrapped MCPs and projects each did this by hand, with the same
/// rationale written out three times: a row logged at `before == after` is an
/// ACTIVITY record, not a saving (historic pre-#449 memory entries, code-intel
/// handlers logged at b == a, …). Showing it as `saved 0` adds noise without
/// information.
pub(super) fn rows_that_saved(
    rollup: impl IntoIterator<Item = (String, (u64, u64, u64))>,
) -> Vec<(String, u64, u64, u64)> {
    rollup
        .into_iter()
        .map(|(name, (b, a, n))| (name, b, a, n))
        .filter(|(_, b, a, _)| b.saturating_sub(*a) > 0)
        .collect()
}

/// Rank rows by what they actually saved, biggest first. Three sites spelled
/// the same `Reverse(b.saturating_sub(a))` key out.
pub(super) fn sort_by_saved(rows: &mut [(String, u64, u64, u64)]) {
    rows.sort_by_key(|(_, b, a, _)| std::cmp::Reverse(b.saturating_sub(*a)));
}

/// Per-session rows for the CLAUDE USAGE block: what the agent itself spent,
/// read straight from its transcripts.
pub(super) fn claude_usage_rows(
    global_dir: &Path,
    home_str: &str,
    since_iso: Option<&str>,
) -> Vec<tokenade_core::dashboard::ClaudeUsageRow> {
    use tokenade_core::dashboard::ClaudeUsageRow;
    // Walk every registered agent parser (Claude Code / Codex /
    // Cline) and merge the rows. Same cwd from two agents becomes
    // two rows, distinguished by the `agent_label` column.
    let mut rows: Vec<ClaudeUsageRow> =
        tokenade_core::agent_transcripts::aggregate_all_agents(global_dir, since_iso)
            .into_iter()
            .filter(|(_, u)| u.total_tokens() > 0)
            .map(|(agent, u)| {
                let total = u.total_tokens();
                let display_cwd = u
                    .cwd
                    .strip_prefix(home_str)
                    .map(|p| format!("~{p}"))
                    .unwrap_or_else(|| u.cwd.clone());
                ClaudeUsageRow {
                    agent_label: agent.label().to_string(),
                    display_cwd,
                    cwd: u.cwd,
                    sessions: u.sessions,
                    total_tokens: total,
                    tool_result_tokens: u.tool_result_tokens,
                    tool_result_tokens_by_tool: u.tool_result_tokens_by_tool,
                    last_ts: u.last_ts,
                    tokenade_calls: u.tools.tokenade,
                    read_calls: u.tools.read,
                    grep_calls: u.tools.grep,
                    glob_calls: u.tools.glob,
                    bash_calls: u.tools.bash,
                    web_calls: u.tools.web,
                }
            })
            .collect();
    // Sort heaviest project first regardless of agent — matches
    // "where is my budget going?" more directly than recency.
    rows.sort_by_key(|r| std::cmp::Reverse(r.total_tokens));
    rows
}

/// Tokens that reached the agent WITHOUT passing a compactor, per tool.
pub(super) fn bypass_rows(
    claude_usage: &[tokenade_core::dashboard::ClaudeUsageRow],
) -> Vec<(String, u64)> {
    let mut agg: std::collections::BTreeMap<String, u64> = Default::default();
    for r in claude_usage {
        for (tool, n) in &r.tool_result_tokens_by_tool {
            *agg.entry(tool.clone()).or_insert(0) += *n;
        }
    }
    let mut v: Vec<(String, u64)> = agg.into_iter().collect();
    v.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    v
}

/// Everything the dashboard shows, gathered: the ledger scan, the rollups, the
/// agent-side usage, and the cache/index counters.
///
/// Split from `cmd_dashboard_text`, where 305 lines of data-gathering sat above
/// 70 lines of presentation. Returns the window cutoff too — the accounting
/// footer must use the same one as the hero number.
pub(super) fn collect_dashboard_data(
    window_days: u32,
) -> (tokenade_core::dashboard::DashboardData, u64) {
    use tokenade_core::cache::{Cache, Stats};
    use tokenade_core::dashboard::DashboardData;
    let r = root();
    // GLOBAL gain ledger — `~/.tokenade/gain.jsonl`. The user can `tokenade
    // dashboard` from any cwd and see the union of every project's
    // savings. Records carry a `project` field so we can break down by
    // project below.
    let global_dir = tokenade_core::platform::home_dir().unwrap_or_else(|| r.clone());
    let path = global_dir.join(".tokenade").join("gain.jsonl");
    // #1042 — fold pre-rollup `.bak` files in once, then read ledger UNION
    // its durable rollup so the lifetime hero survives rotation (rollup rows
    // carry `rollup_calls` for collapsed cardinality, `ts`=UTC-day-start for
    // the week filter, `project`="(rolled-up)" so old aggregate activity
    // lands in one honest bucket rather than being silently dropped).
    tokenade_core::gain::migrate_legacy_backups_into_rollup();
    let content = tokenade_core::gain::ledger_with_rollup(&path);

    // Hero number split: code-intelligence ops report HEURISTIC saved-
    // token estimates and must NOT contaminate the "measured" figure.
    // We classify each record once at the ledger level so totals,
    // per-project, and per-op all stay coherent. Single source of
    // truth lives in `code_intel_heuristic::is_code_intel` — call it
    // (not `CODE_INTEL_TOOLS.contains`) so colon-suffixed variants
    // like `semantic_search:file` partition correctly. Bug fixed in
    // #501: `.contains(&op.as_str())` was an exact match that left
    // every `semantic_search:file` / `semantic_search:global` record
    // in the "measured" bucket.
    use tokenade_core::code_intel_heuristic::is_code_intel as is_ci;

    // #684 — per-tool sum of post-compaction tokens, used by the
    // Functions dashboard section to subtract genuine tokenade
    // activity from `tool_result_tokens`. This is the ONE thing this
    // surface needs that the shared scan doesn't compute, so it rides
    // along on the scan's per-row hook instead of opening a second pass.
    let mut tokenade_after_by_tool: std::collections::HashMap<String, u64> = Default::default();
    let now_secs = tokenade_core::platform::now_unix_secs();
    let week_ago = now_secs.saturating_sub(7 * 86_400);
    // #window — only entries newer than this feed the windowed totals
    // (window_days=0 → lifetime, the historical behaviour). Entries without a
    // `ts` (legacy, pre-timestamp) can't be placed in time, so they count
    // toward `lifetime_saved` only — never the windowed figure.
    let window_cutoff: u64 = if window_days > 0 {
        now_secs.saturating_sub(window_days as u64 * 86_400)
    } else {
        0
    };

    // Every ledger rule (rollup cardinality, the `filter` fallback for `op`,
    // code-intel kept out of the measured figure, the #544 baseline
    // expansion, lifetime accumulating outside the window) lives in
    // `gain::scan_ledger_with` — shared with the JSON and web surfaces, so a
    // fix to one is a fix to all.
    let scan =
        tokenade_core::gain::scan_ledger_with(&content, window_cutoff, week_ago, |v, in_window| {
            if !in_window {
                return;
            }
            // #731 — attribution is label-shaped and best-effort: the Read
            // pretool writes an absolute path rather than "Read:…", so the
            // op-prefix fallback is what carries those rows.
            let source = v["source"].as_str().unwrap_or("");
            let label_raw = v["label"].as_str().unwrap_or("");
            let op_raw = v["op"].as_str().unwrap_or("");
            if let Some(tool) = attribute_agent_tool(source, label_raw, op_raw) {
                *tokenade_after_by_tool.entry(tool.to_string()).or_insert(0) +=
                    v["after"].as_u64().unwrap_or(0);
            }
        });
    let total_before = scan.measured_before;
    let total_after = scan.measured_after;
    let estimated_saved = scan.code_intel_saved;
    let count = scan.calls;
    let lifetime_saved = scan.lifetime_saved;
    let last_week_saved = scan.recent_saved;
    let by_project = scan.per_project;
    let per_op = scan.per_op;

    // Type alias keeps the partition signature readable + silences
    // clippy's `type_complexity` lint.
    type OpStats = Vec<(String, (u64, u64, u64))>;
    let (code_intel_raw, compactor_raw): (OpStats, OpStats) =
        per_op.into_iter().partition(|(op, _)| is_ci(op));

    // Top compactors — sorted by absolute tokens saved. Drop rows where
    // before == after: these are non-compaction ops the ledger still
    // records for activity counts but that have nothing to say in a
    // "tokens saved" view (historic memory_set / exploration_log
    // entries from before #449, code-intel handlers logged at b==a, …).
    let all_compactors = rows_that_saved(compactor_raw);
    // #498: split wrapped-MCP ops (mcp-proxy:<name>) into their own
    // category. The user explicitly asked for a separate section so
    // third-party MCP traffic is visible as a first-class metric
    // rather than buried in TOP COMPACTORS.
    let (mut wrapped_mcps, mut top_compactors): (Vec<_>, Vec<_>) = all_compactors
        .into_iter()
        .partition(|(op, _, _, _)| op.starts_with("mcp-proxy:"));
    sort_by_saved(&mut top_compactors);
    sort_by_saved(&mut wrapped_mcps);

    // Per-project rollup. `global_dir` already IS the user's home directory
    // (`/home/username` on Linux, `/Users/foo` on macOS, etc.).
    //
    // #740 — CRITICAL: keep the RAW absolute path here, NOT the
    // ~/-rewritten display string. The dashboard's AGENT TOKENS -
    // Projects section looks up `by_project[cwd]` where cwd is
    // the raw absolute path from the transcript JSONL. Pre-#740
    // we rewrote `/home/me/foo` to `~/foo` here for display
    // convenience, which silently broke the lookup → tokenade_after
    // resolved to 0 for every project → bypass figure = full
    // row.tool_result_tokens (no subtraction applied), heavily
    // overstating bypass for any project with real compaction
    // activity. Production impact: one project's bypass shown 67.5k vs
    // actual ~18.3k (73% overstated). The ~/ rewrite is now done
    // at render time in dashboard.rs where each consumer can
    // choose what to display without breaking lookups.
    let home_str = global_dir.display().to_string();
    let mut projects = rows_that_saved(by_project);
    sort_by_saved(&mut projects);

    // Code-intel rollup. The estimates come straight from gain.jsonl
    // (the MCP and CLI handlers apply the CodeGraph-derived heuristic at
    // log time — see `tokenade_core::code_intel_heuristic`). We surface
    // total saved per tool exactly as recorded, so a future benchmark
    // can change the constant in `code_intel_heuristic.rs` and every
    // historic + future record updates uniformly.
    // Same filter rationale as compactors/projects: a code-intel op
    // whose response was below MIN_RESPONSE_TOKENS_FOR_HEURISTIC
    // is logged with before==after by design — surfacing it as
    // `est. saved 0` adds noise without information.
    let mut code_intel: Vec<(String, u64, u64)> = code_intel_raw
        .into_iter()
        .map(|(op, (b, a, n))| (op, n, b.saturating_sub(a)))
        .filter(|(_, _, est)| *est > 0)
        .collect();
    code_intel.sort_by_key(|(_, _, est)| std::cmp::Reverse(*est));

    // Cache stats.
    let cache = Cache::open(&r);
    let cs: Stats = cache.stats();

    // Semantic snapshot.
    let (semantic_docs, semantic_terms) = semantic::SemanticIndex::load(&r)
        .map(|s| (s.docs.len() as u64, s.inv.len() as u64))
        .unwrap_or((0, 0));

    // Out-of-Tokenade Claude API usage scraped from
    // ~/.claude/projects/*/*.jsonl — only the global ledger sees
    // compactor activity, but Claude Code writes a full transcript
    // even when nothing routes through us. Surfacing this honestly
    // tells the user "here's the shortfall: tokens we never touched".
    // Cutoff = when Tokenade was first installed. Counting Claude
    // tokens BEFORE install would inflate the bypass figure with
    // history we never had a chance to intercept — that's not "tokens
    // that got past Tokenade", just "tokens that existed before it".
    // Reuse `global_dir` (the home dir computed at the top of this
    // function for the gain ledger path) instead of recomputing —
    // platform::home_dir() does an env::var lookup each call.
    let install_iso = tokenade_core::claudecode_transcript::install_timestamp_iso(&global_dir);
    // #window — agent usage (consumed / bypass) must respect the same window.
    // The transcript `since` filter is lexicographic ISO-8601, so cap it to the
    // LATER of install and the window cutoff. Lifetime mode (cutoff 0) keeps
    // the install date. This makes the "AGENT TOKENS" + bypass figures move
    // with --days, the same as the compaction totals above.
    let since_iso: Option<String> = if window_cutoff > 0 {
        let cutoff_iso = tokenade_core::platform::epoch_to_iso8601_z(window_cutoff);
        Some(match install_iso.as_deref() {
            Some(inst) if inst > cutoff_iso.as_str() => inst.to_string(),
            _ => cutoff_iso,
        })
    } else {
        install_iso.clone()
    };
    let claude_usage = claude_usage_rows(&global_dir, &home_str, since_iso.as_deref());
    // #683 — aggregate tool_result tokens by tool name across all
    // rows so the renderer can paint the "AGENT TOKENS (outside of
    // tokenade) - Functions" section. The Projects section answers
    // "which CWD?", this one answers "which tool?".
    let bypass_by_tool = bypass_rows(&claude_usage);
    // Trim the install ISO to just the date part — the time of day
    // adds noise without information at dashboard granularity.
    let claude_usage_since = since_iso
        .as_deref()
        .and_then(|s| s.get(..10).map(str::to_string));

    // Conflux-inspired BY LAYER rollup (added 2026-06-09): bucket every
    // MEASURED op (per-op compactors ∪ wrapped MCPs) into a logical layer
    // so the dashboard answers "where do my savings come from" at a glance.
    // Heuristic code-intel is deliberately excluded (own section, estimated
    // tokens). Pure function lives in tokenade-core so it unit-tests cheaply.
    let by_layer = {
        let mut measured: Vec<(String, u64, u64, u64)> = top_compactors.clone();
        measured.extend(wrapped_mcps.iter().cloned());
        tokenade_core::dashboard::per_layer_rollup(&measured)
    };

    // Real-dollar value: EVERY saved token priced at the real rate of the
    // exact model that ran it (attributed per session/day), summed across
    // all models and directions — the same figure the statusline, the HTML
    // dashboard and the server report. `model_used` (day-dominant) is kept
    // only as a label for the "Models" panel.
    let model_usage = tokenade_core::agent_transcripts::model_usage_all_agents(&global_dir, None);
    let model_used = dominant_model_of(&model_usage);
    // Compute AND refresh the statusline's cache — opening a dashboard keeps
    // the hot-path figure fresh without the statusline ever walking transcripts.
    let real = tokenade_core::gain::real_savings_refresh();
    let dollars_saved = real.usd;

    let data = DashboardData {
        project_path: r.display().to_string(),
        ledger_path: path.display().to_string(),
        operations: count,
        tokens_before: total_before,
        tokens_after: total_after,
        last_7d_saved: last_week_saved,
        window_days,
        lifetime_saved,
        estimated_tokens_saved: estimated_saved,
        cache_rewrites: cache_rewrites_in_window(&global_dir, window_days as u64),
        top_compactors,
        by_layer,
        by_project: projects,
        code_intel,
        wrapped_mcps,
        // #490: user asked to drop the "Estimate · CodeGraph 0.9.4 …"
        // attribution from the dashboard — it was noise vs signal in
        // the rendered view.
        code_intel_disclaimer: None,
        claude_usage,
        bypass_by_tool,
        tokenade_after_by_tool,
        claude_usage_since,
        // #740 — populate home_dir so dashboard.rs can do the ~/ rewrite
        // at render time. We removed the rewrite at by_project insertion
        // (it broke the AGENT TOKENS bypass lookup against row.cwd).
        home_dir: Some(home_str.clone()),
        lang_fr: user_lang_is_fr(),
        cache_hits: cs.hits,
        cache_misses: cs.misses,
        cache_bytes_served: cs.bytes_served,
        semantic_docs,
        semantic_terms,
        model_used,
        dollars_saved,
        // Secrets masked before any byte reached a model — windowed like
        // every other figure (redact.jsonl rows carry a ts).
        secrets_redacted: tokenade_core::redact::redactions_since(window_cutoff),
        secrets_redacted_top_rule: tokenade_core::redact::redactions_by_rule(window_cutoff)
            .into_iter()
            .max_by_key(|(_, n)| *n),
    };
    (data, window_cutoff)
}
