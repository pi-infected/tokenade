//! The JSON payload the offline web dashboard renders.
//!
//! Aggregation only: one payload powers every view, because the page slices
//! it client-side over per-day buckets.

use super::*;

/// Aggregate `thrash:<trigger>:<gap>` rows from `gain_estimated.jsonl` into
/// `(trigger:gap, events, tokens)`, newest `window_days` only.
///
/// These rows are accounting-neutral counters (before == after, saved 0), so
/// they never touch a savings figure — `before` is the size of the context
/// that was re-sent.
pub(super) fn cache_rewrites_in_window(
    home: &std::path::Path,
    window_days: u64,
) -> Vec<(String, u64, u64)> {
    use std::collections::HashMap;
    let path = home.join(".tokenade").join("gain_estimated.jsonl");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let now = tokenade_core::platform::now_unix_secs();
    let cutoff = now.saturating_sub(window_days.saturating_mul(86_400));
    let mut agg: HashMap<String, (u64, u64)> = HashMap::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(op) = v.get("op").and_then(|o| o.as_str()) else {
            continue;
        };
        let Some(rest) = op.strip_prefix("thrash:") else {
            continue;
        };
        if v.get("ts").and_then(|t| t.as_u64()).unwrap_or(0) < cutoff {
            continue;
        }
        let e = agg.entry(rest.to_string()).or_insert((0, 0));
        e.0 += 1;
        e.1 += v.get("before").and_then(|b| b.as_u64()).unwrap_or(0);
    }
    let mut out: Vec<(String, u64, u64)> = agg.into_iter().map(|(k, (n, t))| (k, n, t)).collect();
    out.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    out
}

/// Build the JSON payload for the interactive web dashboard
/// (`tokenade_core::webdash`). One pre-aggregated bucket per UTC day —
/// the page does all day/week/month slicing and per-period drill-down
/// client-side, so a single payload powers every view. Mirrors the
/// #735/#544 aggregation rules used by the text/JSON surfaces: code-
/// intel rows get the read-time heuristic expansion when before==after
/// and stay OUT of the measured day totals (summed into `est`
/// instead); agent/project/layer breakdowns cover measured ops only,
/// so a project that only ran semantic_search doesn't show a
/// misleading "saved" bar (same rationale as cmd_dashboard_text).
/// The model with the most tokens (input+output) across all agents — picks
/// which live price table to value token savings against (#3).
pub(super) fn dominant_model_of(
    usage: &tokenade_core::agent_transcripts::ModelUsageByDay,
) -> Option<String> {
    let mut totals: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for models in usage.values() {
        for (m, triple) in models {
            *totals.entry(m.as_str()).or_default() += triple.0.saturating_add(triple.1);
        }
    }
    tokenade_core::agent_transcripts::dominant_key(totals.into_iter()).map(|m| m.to_string())
}

/// One UTC day of the ledger, as the page slices it.
type Triple = (u64, u64, u64);
#[derive(Default)]
struct Day {
    b: u64,
    a: u64,
    est: u64,
    n: u64,
    // op → (before, after, calls, is_code_intel). Ops keep BOTH
    // kinds — the page tags code-intel rows "est." instead of
    // hiding them.
    ops: std::collections::BTreeMap<String, (u64, u64, u64, bool)>,
    agents: std::collections::BTreeMap<String, Triple>,
    projects: std::collections::BTreeMap<String, Triple>,
    layers: std::collections::BTreeMap<String, Triple>,
    // P2b — tokens lost to hook bypasses (separate ledger,
    // bypass_lost.jsonl). NEVER added to b/a/n: lost tokens are
    // not savings and must not inflate the measured totals.
    lost: u64,
    lost_n: u64,
}

/// Everything the page needs out of the MEASURED ledger, in one pass.
///
/// Bucketing per UTC day is this page's own concern; row decoding is not —
/// `gain::decode_row` owns rollup cardinality, the `filter` fallback for `op`
/// and the code-intel baseline, and every surface reads rows through it.
struct MeasuredScan {
    days: std::collections::BTreeMap<String, Day>,
    by_tool: std::collections::BTreeMap<String, u64>,
    by_day_by_tool: std::collections::BTreeMap<String, std::collections::BTreeMap<String, u64>>,
}

fn scan_measured(content: &str, rollup_content: &str) -> MeasuredScan {
    use tokenade_core::dashboard::op_layer;
    use tokenade_core::platform::utc_date_from_ts;
    let mut days: std::collections::BTreeMap<String, Day> = Default::default();
    let mut tokenade_after_by_tool: std::collections::BTreeMap<String, u64> = Default::default();
    // Per-day variant of the above so the windowed "outside tokenade"
    // panel can subtract tokenade's OWN per-tool output for the SAME
    // period it slices the bypass totals over (otherwise an all-time
    // `inside` term would over-subtract a short window).
    let mut tokenade_after_by_day_by_tool: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, u64>,
    > = Default::default();

    for line in content.lines().chain(rollup_content.lines()) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // Row decoding — rollup cardinality, the `filter` fallback for `op`,
        // and the #544 code-intel baseline expansion — is shared with every
        // other surface that reads this ledger. Only the bucketing below is
        // this page's own (per UTC day, which the flat scan doesn't model).
        let row = tokenade_core::gain::decode_row(&v);
        let (b, a, ts, calls, is_code_intel) =
            (row.before, row.after, row.ts, row.calls, row.is_code_intel);
        let op = row.op.clone();
        let agent = row.agent.clone();
        let project = row.project.clone();
        // #684/#731 — same per-tool attribution as the text dashboard,
        // so the page's "outside tokenade" section can subtract genuine
        // tokenade activity from the transcript-scraped totals.
        {
            let source = v["source"].as_str().unwrap_or("");
            let label_raw = v["label"].as_str().unwrap_or("");
            let op_raw = v["op"].as_str().unwrap_or("");
            if let Some(tool) = attribute_agent_tool(source, label_raw, op_raw) {
                *tokenade_after_by_tool.entry(tool.to_string()).or_insert(0) += a;
                *tokenade_after_by_day_by_tool
                    .entry(utc_date_from_ts(ts))
                    .or_default()
                    .entry(tool.to_string())
                    .or_insert(0) += a;
            }
        }
        let day = days.entry(utc_date_from_ts(ts)).or_default();
        if is_code_intel {
            day.est += b.saturating_sub(a);
        } else {
            day.b += b;
            day.a += a;
            day.n += calls;
            let e = day.agents.entry(agent).or_default();
            e.0 += b;
            e.1 += a;
            e.2 += calls;
            let e = day.projects.entry(project).or_default();
            e.0 += b;
            e.1 += a;
            e.2 += calls;
            let e = day.layers.entry(op_layer(&op).to_string()).or_default();
            e.0 += b;
            e.1 += a;
            e.2 += calls;
        }
        let e = day.ops.entry(op).or_insert((0, 0, 0, is_code_intel));
        e.0 += b;
        e.1 += a;
        e.2 += calls;
    }
    MeasuredScan {
        days,
        by_tool: tokenade_after_by_tool,
        by_day_by_tool: tokenade_after_by_day_by_tool,
    }
}

/// The LOST ledger: output that flowed past tokenade uncompacted. Folded into
/// the same per-day buckets so the page can show both sides of one day.
fn fold_lost_ledger(days: &mut std::collections::BTreeMap<String, Day>) {
    use tokenade_core::platform::utc_date_from_ts;
    let lost_path = tokenade_core::gain::lost_ledger_path();
    let lost_content = fs::read_to_string(&lost_path).unwrap_or_default();
    let lost_rollup =
        fs::read_to_string(tokenade_core::gain::rollup_path_for(&lost_path)).unwrap_or_default();
    for line in lost_content.lines().chain(lost_rollup.lines()) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let b = v["before"].as_u64().unwrap_or(0);
        let a = v["after"].as_u64().unwrap_or(0);
        let ts = v["ts"].as_u64().unwrap_or(0);
        let calls = v["rollup_calls"].as_u64().unwrap_or(1);
        let day = days.entry(utc_date_from_ts(ts)).or_default();
        day.lost += b.saturating_sub(a);
        day.lost_n += calls;
    }
}

/// A `day → (tool → tokens)` rollup as a JSON object of objects.
///
/// Two sites built this identical two-level shape by hand — the bypass series
/// and the tokenade-after series. Same nesting, same `json!(n)` leaf, different
/// source expression.
fn by_day_by_tool_json<M>(rows: impl IntoIterator<Item = (String, M)>) -> serde_json::Value
where
    M: IntoIterator<Item = (String, u64)>,
{
    serde_json::Value::Object(
        rows.into_iter()
            .map(|(day, tools)| {
                (
                    day,
                    serde_json::Value::Object(
                        tools
                            .into_iter()
                            .map(|(t, n)| (t, serde_json::json!(n)))
                            .collect(),
                    ),
                )
            })
            .collect(),
    )
}

/// "Outside tokenade" per-tool totals from the local agent transcripts — same
/// source + install-date cutoff as the text dashboard's AGENT TOKEN FOOTPRINT
/// section.
fn bypass_tool_totals(
    projects: &[(
        tokenade_core::agent_transcripts::Agent,
        tokenade_core::agent_transcripts::ProjectUsage,
    )],
) -> Vec<(String, u64)> {
    let mut agg: std::collections::BTreeMap<String, u64> = Default::default();
    for (_, u) in projects {
        for (tool, n) in &u.tool_result_tokens_by_tool {
            *agg.entry(tool.clone()).or_insert(0) += *n;
        }
    }
    let mut v: Vec<(String, u64)> = agg.into_iter().collect();
    v.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    v
}

fn compounding_json() -> serde_json::Value {
    use tokenade_core::platform::utc_date_from_ts;
    let mut by_day: std::collections::BTreeMap<String, u64> = Default::default();
    let est_path = tokenade_core::gain::estimated_ledger_path();
    let est_content = fs::read_to_string(&est_path).unwrap_or_default();
    let est_rollup =
        fs::read_to_string(tokenade_core::gain::rollup_path_for(&est_path)).unwrap_or_default();
    for line in est_content.lines().chain(est_rollup.lines()) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["class"].as_str() != Some("compounding") {
            continue;
        }
        let ts = v["ts"].as_u64().unwrap_or(0);
        let saved = v["saved"].as_u64().unwrap_or_else(|| {
            v["before"]
                .as_u64()
                .unwrap_or(0)
                .saturating_sub(v["after"].as_u64().unwrap_or(0))
        });
        *by_day.entry(utc_date_from_ts(ts)).or_insert(0) += saved;
    }
    serde_json::Value::Object(
        by_day
            .into_iter()
            .map(|(d, n)| (d, serde_json::json!(n)))
            .collect(),
    )
}

fn models_by_day_json(
    model_usage: std::collections::BTreeMap<String, std::collections::BTreeMap<String, Triple>>,
) -> serde_json::Value {
    serde_json::Value::Object(
        model_usage
            .into_iter()
            .map(|(d, m)| {
                (
                    d,
                    serde_json::Value::Object(
                        m.into_iter()
                            .map(|(k, (i, o, n))| (k, serde_json::json!([i, o, n])))
                            .collect(),
                    ),
                )
            })
            .collect(),
    )
}

fn triple_map_json(m: std::collections::BTreeMap<String, Triple>) -> serde_json::Value {
    serde_json::Value::Object(
        m.into_iter()
            .map(|(k, (b, a, n))| (k, serde_json::json!([b, a, n])))
            .collect(),
    )
}

fn day_rows_json(days: std::collections::BTreeMap<String, Day>) -> Vec<serde_json::Value> {
    days.into_iter()
        .map(|(d, day)| {
            serde_json::json!({
                "d": d,
                "b": day.b,
                "a": day.a,
                "est": day.est,
                "n": day.n,
                "ops": serde_json::Value::Object(
                    day.ops
                        .into_iter()
                        .map(|(k, (b, a, n, ci))| (k, serde_json::json!([b, a, n, u8::from(ci)])))
                        .collect(),
                ),
                "agents": triple_map_json(day.agents),
                "projects": triple_map_json(day.projects),
                "layers": triple_map_json(day.layers),
                "lost": day.lost,
                "lost_n": day.lost_n,
            })
        })
        .collect()
}

/// A path under `$HOME` shown as `~/…`, or unchanged when it is elsewhere.
fn home_relative(path: &std::path::Path, home_str: &str) -> String {
    let s = path.display().to_string();
    s.strip_prefix(home_str)
        .map(|p| format!("~{p}"))
        .unwrap_or(s)
}

/// The lifetime 3-way savings split, priced. SAME math as the terminal block
/// (`print_estimated_accounting`, lifetime scope) so the two local surfaces can
/// never disagree: [`tokenade_core::gain::EstimatedSummary::by_io`].
struct IoSplit {
    input: u64,
    cache: i64,
    output: u64,
    overhead: u64,
    cross: u64,
    usd: f64,
    p_in: f64,
    p_cache: f64,
    p_out: f64,
}

fn io_split(comp_saved: u64, acc_cross: u64) -> IoSplit {
    let acc = tokenade_core::gain::estimated_summary();
    let (acc_in, acc_cache, acc_out) = acc.by_io(comp_saved, acc_cross);
    let (p_in, p_cache, p_out) = (
        tokenade_core::coefficients::price_input_per_mtok(),
        tokenade_core::coefficients::price_cache_per_mtok(),
        tokenade_core::coefficients::price_output_per_mtok(),
    );
    let acc_usd = acc_in as f64 / 1e6 * p_in
        + acc_cache as f64 / 1e6 * p_cache
        + acc_out as f64 / 1e6 * p_out;
    IoSplit {
        input: acc_in,
        cache: acc_cache,
        output: acc_out,
        overhead: acc.overhead,
        cross: acc_cross,
        usd: acc_usd,
        p_in,
        p_cache,
        p_out,
    }
}

pub(crate) fn build_webdash_payload() -> serde_json::Value {
    use tokenade_core::webdash::utc_datetime_from_ts;

    let r = root();
    let gain_path = global_gain_path();
    // #1042 — one-shot, idempotent recovery of historical ledger backups
    // into the durable rollup (no-op after the first run; renames consumed
    // backups to `.counted`). Ensures the dashboard surfaces savings that
    // pre-date the current ledger instead of silently resetting to ~0.
    tokenade_core::gain::migrate_legacy_backups_into_rollup();
    let content = fs::read_to_string(&gain_path).unwrap_or_default();
    // The durable per-day rollup holds aggregated rows for history that has
    // rotated out of the live ledger. We feed it through the SAME aggregation
    // loop (its rows carry `before`/`after`/`ts`/`op` + a `rollup_calls`
    // cardinality), so day/week/month slicing and lifetime totals include it.
    let rollup_content =
        fs::read_to_string(tokenade_core::gain::rollup_path_for(&gain_path)).unwrap_or_default();
    let global_dir = tokenade_core::platform::home_dir().unwrap_or_else(|| r.clone());
    let home_str = global_dir.display().to_string();

    let MeasuredScan {
        mut days,
        by_tool: tokenade_after_by_tool,
        by_day_by_tool: tokenade_after_by_day_by_tool,
    } = scan_measured(&content, &rollup_content);
    fold_lost_ledger(&mut days);

    // "Outside tokenade" totals from the local agent transcripts — same
    // source + install-date cutoff as the text dashboard's AGENT TOKEN
    // FOOTPRINT section.
    let since_iso = tokenade_core::claudecode_transcript::install_timestamp_iso(&global_dir);

    // ONE walk of every agent's transcripts. Everything below that used to
    // reach for `~/.claude/projects` — the bypass totals, the per-day tool
    // tokens, the cache-write series, the per-model panel, the model
    // attributor behind the $ figure, and the cross-agent output-lean
    // estimate — is folded out of this single pass. Six separate walks before
    // (measured 1.4 s of a 1.9 s render on a 1.3 GB corpus); see
    // `agent_transcripts::digest_all_agents`.
    let digest =
        tokenade_core::agent_transcripts::digest_all_agents(&global_dir, since_iso.as_deref());

    // "Outside tokenade" totals from the local agent transcripts — same
    // source + install-date cutoff as the text dashboard's AGENT TOKEN
    // FOOTPRINT section.
    let bypass_by_tool = bypass_tool_totals(&digest.projects);
    // Per-day variant so the panel honours the period selector. Same
    // source + cutoff; the JS slices it by the active range and subtracts
    // `tokenade_after_by_day_by_tool` for the same days. Emitted as a
    // {day: {tool: tokens}} object.
    let bypass_by_day_json = by_day_by_tool_json(digest.tools_by_day);
    let tokenade_after_by_day_json = by_day_by_tool_json(tokenade_after_by_day_by_tool);
    // Per-day cache-WRITE tokens (Anthropic-style agents). The page subtracts
    // these from total agent consumption for the bill-share denominator: cache
    // creation is dominated by re-caching the conversation/system-prompt/file
    // context — tokens tokenade structurally can't compact — so counting them
    // against tool-output savings makes the share misleadingly tiny.
    let cache_creation_by_day_json = serde_json::Value::Object(
        digest
            .cache_creation_by_day
            .into_iter()
            .map(|(d, n)| (d, serde_json::json!(n)))
            .collect(),
    );
    // Per-day estimated COMPOUNDING saving = the future cache_read tokenade
    // avoided by shrinking a tool result that then re-sends smaller on every
    // later turn. Shown as a clearly-labelled ESTIMATE next to (never blended
    // into) the measured figure — this is how we credit ourselves for the
    // cache we save, rather than penalising ourselves with a cache denominator.
    let compounding_by_day_json = compounding_json();

    let (semantic_docs, semantic_terms) = semantic::SemanticIndex::load(&r)
        .map(|s| (s.docs.len() as u64, s.inv.len() as u64))
        .unwrap_or((0, 0));

    // Per-model consumption, merged across every agent whose local
    // transcripts record the model name (Claude Code, Codex, Cline,
    // Copilot — agents without a model field contribute nothing
    // rather than guessing). Same install-date cutoff as the bypass
    // section.
    // Per-model usage → also pick the dominant model (most tokens) and its live
    // Helicone prices, so the dashboard can value token savings in real dollars.
    let dominant_model = dominant_model_of(&digest.models_by_day);
    let model_prices = dominant_model
        .as_deref()
        .and_then(tokenade_core::model_costs::cost_for_model);
    let models_json = models_by_day_json(digest.models_by_day);

    // 3-way at the SAME static rates as the terminal accounting block and
    // the former server-side split — one $ formula across every surface. (The
    // dominant model's live prices stay in the payload as a label.)
    // Input EXCLUDES the code-intel estimate (`est_saved`) — the terminal
    // block and the server split both leave it out of the $ math (it is
    // shown as a tokens-only "+ estimated" figure); output INCLUDES the
    // cross-agent output-lean, like both of them.
    let comp_saved: u64 = days.values().map(|d| d.b.saturating_sub(d.a)).sum();
    // Real per-model value: every saved token priced at the exact model
    // that ran it (see gain::real_savings_compute) — the SAME figure the
    // terminal dashboard, the statusline and the server report. Its
    // per-model breakdown rides into the payload below. The attributor is
    // handed in rather than rebuilt: building it re-walks every transcript,
    // and this render already has the two tables it is made of.
    let attributor = tokenade_core::agent_transcripts::ModelAttributor::from_sessions_and_usage(
        digest.session_models,
        &digest.lifetime_models_by_day,
    );
    let real_savings = tokenade_core::gain::real_savings_refresh_with(Some(attributor));
    let dollars_saved = real_savings.usd;
    let now_secs = tokenade_core::platform::now_unix_secs();
    let days_json = day_rows_json(days);
    let ledger_display = home_relative(&gain_path, &home_str);

    let split = io_split(
        comp_saved,
        digest.output_lean_by_agent.iter().map(|(_, n)| *n).sum(),
    );

    serde_json::json!({
        "days": days_json,
        // Per-day estimated buckets so the JS can window the accounting card
        // exactly like the terminal's `--days N` block. `cross` (other-agent
        // output-lean) is lifetime-only in the terminal too.
        "est_by_day": tokenade_core::gain::estimated_by_day()
            .into_iter()
            .map(|(d, e)| {
                // [5..7]: the same buckets weighted at each row's model ratios
                // (output, compounding+batching, overhead re-bill) — the JS
                // `weightedTok` uses them so the web total matches the terminal.
                let w = e.weighted_or_default();
                (d, serde_json::json!([e.output, e.batching, e.compounding, e.overhead, e.overhead_cache,
                    w.output, w.cache_credit, w.overhead_cache]))
            })
            .collect::<serde_json::Map<String, serde_json::Value>>(),
        "est_cross": split.cross,
        // Per-UTC-day secrets-redacted counts (key "" = undated legacy rows,
        // counted only in the all-time view) — the JS windows them exactly
        // like est_by_day and renders the security card.
        "redact_by_day": tokenade_core::redact::redactions_by_day()
            .into_iter()
            .map(|(d, n)| (d, serde_json::json!(n)))
            .collect::<serde_json::Map<String, serde_json::Value>>(),
        // Lifetime 3-way split (tokens + $) — mirrors the terminal
        // "Savings accounting" block (and the former online user dashboard).
        "io_split": {
            "input": split.input,
            "cache": split.cache,
            "output": split.output,
            "overhead": split.overhead,
            "usd": split.usd,
            "prices": {"input": split.p_in, "cache": split.p_cache, "output": split.p_out},
        },
        "semantic": {"docs": semantic_docs, "terms": semantic_terms},
        "models_by_day": models_json,
        // Dominant model + its live per-1M Helicone prices, so the dashboard
        // can value token savings in real dollars (#3).
        "model_used": dominant_model,
        "dollars_saved": dollars_saved,
        // Per-model breakdown behind `dollars_saved`: each saved token priced
        // at the real rate of the exact model that ran it. `model` is "" for
        // savings we can't attribute (no transcript), priced at the default.
        "savings_by_model": real_savings.by_model.iter().map(|(m, u, i, c, o)|
            serde_json::json!({"model": m, "usd": u, "input": i, "cache": c, "output": o})
        ).collect::<Vec<_>>(),
        "hook_installed": crate::install_cli::any_agent_has_tokenade(),
        "model_prices": model_prices.map(|c| serde_json::json!({
            "input_per_1m": c.input_per_1m,
            "output_per_1m": c.output_per_1m,
            "cache_write_per_1m": c.cache_write_per_1m,
            "cache_read_per_1m": c.cache_read_per_1m,
        })),
        "coverage_notes": tokenade_core::agent_transcripts::coverage_notes()
            .iter()
            .map(|(a, en, fr)| serde_json::json!([a, en, fr]))
            .collect::<Vec<_>>(),
        "bypass_by_tool": bypass_by_tool,
        "bypass_by_day": bypass_by_day_json,
        "tokenade_after_by_tool": tokenade_after_by_tool,
        "tokenade_after_by_day": tokenade_after_by_day_json,
        "cache_creation_by_day": cache_creation_by_day_json,
        "compounding_by_day": compounding_by_day_json,
        "usage_since": since_iso.as_deref().and_then(|s| s.get(..10)),
        "generated_ts": now_secs,
        "generated_at": utc_datetime_from_ts(now_secs),
        "ledger": ledger_display,
        "home": home_str,
    })
}

/// Write the interactive web dashboard and (optionally) open it in the
/// default browser. The page is a fully self-contained static HTML
/// file — inline payload, no server, no external requests. That's the
/// product stance, not a shortcut: these stats stay on the user's
/// machine by design (centralizing them on tokenade.net was explicitly
/// rejected on privacy grounds, 2026-06-10).
///
/// Default target is the GLOBAL `~/.tokenade/dashboard.html`: the data
/// is the global ledger, so one stable page location beats sprinkling
/// `.tokenade/` dirs into whatever cwd the command runs from. (This
/// also moves the `--html` default, which previously wrote under the
/// project root — documented in the help text.)
pub(crate) fn cmd_dashboard_web(explicit_path: Option<std::path::PathBuf>, open: bool) -> i32 {
    let payload = build_webdash_payload();
    let json = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
    let html = tokenade_core::webdash::render_html(&json, env!("CARGO_PKG_VERSION"));
    let out_path = explicit_path.unwrap_or_else(|| {
        tokenade_core::platform::home_dir()
            .unwrap_or_else(root)
            .join(".tokenade")
            .join("dashboard.html")
    });
    if let Some(parent) = out_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(e) = fs::write(&out_path, html) {
        eprintln!("tokenade dashboard: write {}: {e}", out_path.display());
        return 1;
    }
    let url = format!("file://{}", out_path.display());
    // One line, whether or not the launcher took it: the URL is the useful
    // half either way, and `cmd_dashboard` reads the flag to decide whether
    // the terminal report has to stand in for a page nobody can see.
    let opened = open && open_browser(&url);
    super::LAST_OPEN_SUCCEEDED.store(opened, std::sync::atomic::Ordering::Relaxed);
    // A browser already running takes the page as a new tab and may not come
    // forward: without this line the command seemed to do nothing.
    if opened {
        println!("  ✓ dashboard opened in your browser (new tab)");
    }
    println!("  ⤷ {url}");
    // The interactive run only (`--html` / `--no-open` are scripted exports),
    // and only once the page has saved tokens to show.
    if open && payload_saved_tokens(&payload) > 0 {
        crate::star_invite::maybe_show();
    }
    0
}

/// Tokens saved across the payload's day rows (`b` before, `a` after).
fn payload_saved_tokens(payload: &serde_json::Value) -> u64 {
    payload["days"].as_array().map_or(0, |days| {
        days.iter()
            .map(|d| {
                d["b"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_sub(d["a"].as_u64().unwrap_or(0))
            })
            .sum()
    })
}

/// #684/#731 — given a gain.jsonl row's `source`, `label`, and `op`,
/// decide which agent tool the post-compaction bytes should be
/// attributed to (or `None` to skip — not agent-driven, e.g.
/// source=cli or test:*). Pulled out as a standalone fn so it can be
/// unit-tested cheaply; the dashboard aggregator just calls this
/// once per row.
///
/// Attribution rules (most specific first):
/// 1. source=hook AND label starts with "Bash:" / "Read:" — the
///    pre-#731 baseline, still authoritative when the writer set it.
/// 2. source=hook AND op starts with "read_" — covers all Read
///    pretool/posttool ops (read_disclose / read_auto_compact /
///    read_content_dedup / read_code_strip_lite). These label rows
///    with the absolute file path, not "Read:…", so rule 1 misses
///    them. Without rule 2 the entire Read after-token volume was
///    unattributed → dashboard showed 100% of Read traffic as
///    bypass even when tokenade actively compacted.
/// 3. source=hook AND op starts with "bash_" or op == "lsp_enforce_bash"
///    — Bash equivalent, future-proof against label format changes.
/// 4. source=wrap — always Bash-route (CLI `tokenade wrap` is
///    Bash-invoked).
/// 5. Anything else (source=cli, source=test:*, source=mcp, …) —
///    not agent traffic, skip.
pub(crate) fn attribute_agent_tool(source: &str, label: &str, op: &str) -> Option<&'static str> {
    // #731 added the read_*/bash_*/lsp_enforce_bash rules. #733 extends
    // with 3 more hook writers found in the dashboard audit:
    //   - `lsp_enforce` (Grep tool LSP-redirect, label="Grep:<pattern>")
    //   - `glob_compact` (Glob hook)
    //   - `webfetch_compact` (WebFetch hook)
    // Without these, the Grep/Glob/WebFetch entries in the Functions
    // section showed 100% of their tool_result_tokens as bypass, even
    // when tokenade actively redirected/compacted. Same bug class as
    // #731 — different writers, same root cause (label format ≠ the
    // hard-coded "Bash:" / "Read:" prefix).
    match source {
        "hook" if label.starts_with("Bash:") => Some("Bash"),
        "hook" if label.starts_with("Read:") => Some("Read"),
        "hook" if label.starts_with("Grep:") => Some("Grep"),
        // #951 — read_binary_extracted matches the same prefix as
        // existing read_dedup_hit / read_diff_hit / etc. Listed
        // explicitly so the intent is visible (the prefix rule would
        // catch it too, but reading like this surfaces the route).
        "hook" if op == "read_binary_extracted" => Some("Read"),
        "hook" if op.starts_with("read_") => Some("Read"),
        "hook" if op == "lsp_enforce" => Some("Grep"),
        "hook" if op == "glob_compact" => Some("Glob"),
        "hook" if op == "webfetch_compact" => Some("WebFetch"),
        // #950 — binary-extraction route (PDF/DOCX/EPUB/…) also
        // belongs under the WebFetch tool group in the dashboard.
        "hook" if op == "webfetch_binary_extracted" => Some("WebFetch"),
        "hook" if op.starts_with("bash_") || op == "lsp_enforce_bash" => Some("Bash"),
        // #737 — `auto:*` ops written by cmd_hookwrap (PostToolUse Bash
        // hook at main.rs:10349) when the auto-detector picks a filter
        // for the agent's Bash output (auto:filelist / auto:stacktrace
        // / auto:json / auto:diff / auto:dockerfile / …). The label is
        // the bash command, NOT prefixed with "Bash:", so the
        // label-prefix rule misses → these were 100% bypass before.
        // MCP auto_compact writes source="mcp", not "hook", so there's
        // no Bash/MCP ambiguity here: source=hook+op=auto:* is
        // unambiguously a PostToolUse Bash compaction.
        "hook" if op.starts_with("auto:") => Some("Bash"),
        "wrap" => Some("Bash"),
        _ => None,
    }
}
