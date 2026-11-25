//! **Re-reading the ledger: count, deduplicate, summarise.**
//!
//! Reading is not the inverse of writing. A row may have been written twice by
//! two processes, a time window is not the whole file, and a raw sum is not
//! what can be announced — hence the credit key, the server-side dedup key,
//! and the distinction between the total and its REALISTIC share.

use super::*;

/// One row of `tokenade mcp-proxy report` — the per-tool savings a wrapped
/// MCP server produced. `calls` counts every recorded result for the tool
/// (compacted or not); `*_saved` are the deltas; `usd_saved` prices the
/// saved tokens as INPUT at each row's attributed model (the same rate table
/// the dashboard uses — [`crate::pricing::rates_for_model`]).
#[derive(Debug, Clone, Default)]
pub struct McpToolStat {
    pub tool: String,
    pub calls: u64,
    pub bytes_saved: u64,
    pub tokens_saved: u64,
    pub usd_saved: f64,
}

/// Aggregate the measured ledger into a per-MCP-tool savings report.
/// Walks `gain.jsonl` (+ its rollup) for `source == "wrap-mcp"` rows whose
/// `op` is `mcp:<tool>`, sums calls / saved bytes / saved tokens, and prices
/// the saved tokens per row at its attributed model's INPUT rate (MCP tool
/// results land in the model's context as input tokens). Sorted by $ desc.
pub fn mcp_tool_report() -> Vec<McpToolStat> {
    let home = crate::platform::home_dir();
    let attributor = home
        .as_deref()
        .map(crate::agent_transcripts::ModelAttributor::build);
    let model_of = |sess: Option<&str>, ts_ms: u64| -> String {
        attributor
            .as_ref()
            .and_then(|a| a.model_for(sess, ts_ms))
            .unwrap_or_default()
    };
    let ledger = current_ledger_path();
    mcp_tool_report_from(&[rollup_path_for(&ledger), ledger], &model_of)
}

/// The ledger-walk core of [`mcp_tool_report`], with the ledger paths and
/// the per-row model attributor injected so it can be unit-tested without
/// touching the process-global ledger root or real transcripts.
pub(super) fn mcp_tool_report_from(
    paths: &[PathBuf],
    model_of: &dyn Fn(Option<&str>, u64) -> String,
) -> Vec<McpToolStat> {
    use std::collections::HashMap;
    // (calls, bytes_saved, tokens_saved, usd_saved)
    let mut agg: HashMap<String, (u64, u64, u64, f64)> = HashMap::new();
    for path in paths {
        let Ok(body) = fs::read_to_string(path) else {
            continue;
        };
        for line in body.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if v["source"].as_str() != Some("wrap-mcp") {
                continue;
            }
            let Some(tool) = v["op"].as_str().and_then(|op| op.strip_prefix("mcp:")) else {
                continue;
            };
            let saved = v["saved"].as_u64().unwrap_or_else(|| {
                v["before"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_sub(v["after"].as_u64().unwrap_or(0))
            });
            let bytes_saved = v["bytes_before"]
                .as_u64()
                .unwrap_or(0)
                .saturating_sub(v["bytes_after"].as_u64().unwrap_or(0));
            let ts_ms = v["ts"].as_u64().unwrap_or(0).saturating_mul(1000);
            let model = model_of(v["sess"].as_str(), ts_ms);
            let usd = crate::pricing::rates_for_model(&model).usd(saved, 0, 0);
            let e = agg.entry(tool.to_string()).or_insert((0, 0, 0, 0.0));
            e.0 += 1;
            e.1 += bytes_saved;
            e.2 += saved;
            e.3 += usd;
        }
    }
    let mut out: Vec<McpToolStat> = agg
        .into_iter()
        .map(
            |(tool, (calls, bytes_saved, tokens_saved, usd_saved))| McpToolStat {
                tool,
                calls,
                bytes_saved,
                tokens_saved,
                usd_saved,
            },
        )
        .collect();
    out.sort_by(|a, b| {
        b.usd_saved
            .partial_cmp(&a.usd_saved)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.tokens_saved.cmp(&a.tokens_saved))
    });
    out
}

/// Is this measurement worth a row in the ledger?
///
/// #542/#540 — the same gate the pruner applies to history, so what gets
/// written and what survives a prune are one decision, not four. It was
/// copied at every write site and they had already drifted: `cmd_wrap`'s copy
/// had no code-intel exemption, so a code-intel op routed through `wrap`
/// (`Savings::new(N, N)`, saved 0 by construction) was dropped at write time
/// while the same op through any other path was kept and expanded at read
/// time by the #544 heuristic.
///
/// A tiny saving is statistical noise that drags the per-op averages on the
/// dashboard (measured: 113 of 119 `grep` events saved 1-2 tokens each and
/// pulled its average from 76.0 % down to 50.5 %). Code-intel is exempt
/// because its saving CANNOT be measured directly — a small number there is
/// signal, not noise.
pub fn worth_recording(before: u64, after: u64, op: &str) -> bool {
    if crate::code_intel_heuristic::is_code_intel(op) {
        return true;
    }
    let saved = before.saturating_sub(after);
    let pct = if before > 0 {
        (saved as f64) * 100.0 / (before as f64)
    } else {
        0.0
    };
    saved >= 10 && pct >= 5.0
}

/// Convenience for callers that don't carry a label.
/// Sum the `saved` field across every row of one JSONL file. Returns `None`
/// when the file is missing/unreadable (distinct from `Some(0)`, an
/// existing-but-empty file). Best-effort: garbled rows are skipped.
pub(super) fn sum_saved_in_file(path: &std::path::Path) -> Option<u64> {
    let body = fs::read_to_string(path).ok()?;
    let mut total: u64 = 0;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            // Defensive: the MEASURED ledger must never carry estimated rows
            // (those live in gain_estimated.jsonl and are summed separately by
            // estimated_summary). If one ever leaked in, counting it here would
            // double-bill it into the weighted total. Skip it.
            if v.get("estimated")
                .and_then(|x| x.as_bool())
                .unwrap_or(false)
                || v.get("est").and_then(|x| x.as_bool()).unwrap_or(false)
            {
                continue;
            }
            total = total.saturating_add(v.get("saved").and_then(|x| x.as_u64()).unwrap_or(0));
        }
    }
    Some(total)
}

/// Lifetime gross tokens tokenade has folded — the live ledger PLUS the
/// durable rollup of history that has rotated out of it. Counting both is
/// what makes the figure monotonic across rotation: #1042 fixed the
/// recurring "all-time savings reset" by folding aged-out rows into the
/// rollup instead of deleting them, and summing the rollup here. Returns
/// `None` only when the live ledger is missing/unreadable (distinct from
/// `Some(0)`); a missing rollup contributes 0, never `None`.
pub fn lifetime_saved() -> Option<u64> {
    let ledger = current_ledger_path();
    let mut total = sum_saved_in_file(&ledger)?;
    if let Some(rolled) = sum_saved_in_file(&rollup_path_for(&ledger)) {
        total = total.saturating_add(rolled);
    }
    Some(total)
}

/// REALISTIC savings for ONE compaction event. The agent never ingests a huge
/// file raw — the harness truncates at `cap` — so count the directly-delivered
/// part (`min(before, cap) − after`) in full, then credit `overflow_ratio` of
/// the truncated tail (`before − cap`), the grep/Read dance the no-tokenade
/// world pays to recover it. When the harness would have delivered the whole
/// thing (`before ≤ cap`) this is exactly the naive `before − after`. See
/// [`crate::coefficients::delivery_cap_tokens`] / `overflow_credit_ratio`.
pub fn realistic_saved(before: u64, after: u64, cap: u64, overflow_ratio: f64) -> u64 {
    let direct = before.min(cap).saturating_sub(after);
    let overflow = before.saturating_sub(cap);
    // The overflow credit models the grep/Read dance the no-tokenade world
    // pays to recover the truncated tail — and that dance is BOUNDED: each
    // recovery command delivers at most `cap` again, and an agent runs a
    // couple of them before moving on, no matter how huge the raw blob was.
    // A proportional-only credit invented savings linear in blob size
    // (field over-count: `auto:diff` at 9.77M tokens over 43 events — a
    // ~3 MB `git diff` credited ~220k/event; no agent ever re-reads 27% of
    // a 3 MB diff). Ceiling = 2 recovery commands' worth (same ≤2× posture
    // as the 0.8.11 compounding bound).
    let credit = (overflow as f64 * overflow_ratio).round() as u64;
    direct + credit.min(cap.saturating_mul(2))
}

pub(crate) fn realistic_of_row(v: &serde_json::Value) -> u64 {
    if let Some(r) = v.get("realistic").and_then(|x| x.as_u64()) {
        return r;
    }
    let before = v.get("before").and_then(|x| x.as_u64()).unwrap_or(0);
    let after = v.get("after").and_then(|x| x.as_u64()).unwrap_or(0);
    if v.get("source").and_then(|x| x.as_str()) == Some("rollup") {
        return before.saturating_sub(after); // legacy rollup: naive
    }
    let agent = v.get("agent").and_then(|x| x.as_str()).unwrap_or("");
    let source = v.get("source").and_then(|x| x.as_str()).unwrap_or("");
    let op = v.get("op").and_then(|x| x.as_str()).unwrap_or("");
    // A row that recorded what the agent would really have delivered beats any
    // scalar: the scalar is a per-agent bound, `deliverable` was measured
    // against THIS payload with `crate::agent_caps`. Older rows have no such
    // field and keep the bound, so nothing already published moves.
    let cap = v
        .get("deliverable")
        .and_then(|x| x.as_u64())
        .unwrap_or_else(|| crate::coefficients::delivery_cap_tokens(agent, source, op));
    let ratio = crate::coefficients::overflow_ratio_for_channel(agent, source, op);
    realistic_saved(before, after, cap, ratio)
}

pub(super) fn sum_realistic_in_file(path: &std::path::Path) -> Option<u64> {
    let body = fs::read_to_string(path).ok()?;
    let mut total: u64 = 0;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            total = total.saturating_add(realistic_of_row(&v));
        }
    }
    Some(total)
}

/// Lifetime REALISTIC tokens saved — the [`lifetime_saved`] counterpart that
/// bounds each huge-file compaction by what the harness would actually have
/// put in front of the model. Same live-ledger + rollup composition, so it is
/// monotonic across rotation for the same reason.
pub fn lifetime_saved_realistic() -> Option<u64> {
    let ledger = current_ledger_path();
    let mut total = sum_realistic_in_file(&ledger)?;
    if let Some(rolled) = sum_realistic_in_file(&rollup_path_for(&ledger)) {
        total = total.saturating_add(rolled);
    }
    Some(total)
}

/// 0.6.4 — token totals from the ESTIMATED ledger (`gain_estimated.jsonl`),
/// bucketed by `class`. `output` / `batching` / `compounding` are approximate
/// SAVINGS; `overhead` is a COST the net subtracts. Kept separate from
/// [`lifetime_saved`] (the exact measured hero) so an estimate never inflates
/// the measured figure — the dashboard sums them under its own line.
#[derive(Default, Debug, Clone, Copy)]
pub struct EstimatedSummary {
    pub output: u64,
    pub batching: u64,
    pub compounding: u64,
    pub overhead: u64,
    /// Re-billing of OUR injected context on later turns (cache_read class,
    /// weighted x0.1) — the symmetric DEBIT of the compounding CREDIT.
    pub overhead_cache: u64,
    /// Re-runs avoided by recovering folded bytes (expand-ref after a
    /// post-compaction digest, …). Input-side: what comes back is content the
    /// model would have paid FRESH input for, not a re-send of a cached
    /// prefix — which is what separates it from `batching`.
    pub recovery: u64,
    /// The price-weighted twins of the buckets above, each row counted at the
    /// ratios of ITS model ([`super::io_weights_for_model`]). `None` on a
    /// hand-built summary: the default ratios apply to the raw buckets.
    pub weighted: Option<WeightedBuckets>,
}

impl EstimatedSummary {
    /// Tokens saved as `(input, cache, output)` — what each bucket would have
    /// been BILLED as, the mapping the reporter uploads
    /// (`license::reporter`: compounding and batching are cache reads).
    ///
    /// The terminal block and the local web dashboard each did this sum, both
    /// with batching in INPUT: an avoided turn re-sends a cached prefix, priced
    /// at a tenth of fresh input, so both surfaces priced it ~10× too high —
    /// and only the web one netted the overhead re-bill out of the cache bucket.
    /// `measured` is the folded-output saving; `cross_output` the output-lean
    /// credited to other agents.
    ///
    /// The cache bucket is SIGNED. Floored at zero, a re-bill larger than the
    /// re-reads it offsets vanished from the dollar figure: measured
    /// 2026-10-05 on this machine's last 30 days, 3.4 M credited against
    /// 13.1 M re-billed, shown as "0" and priced at $0 instead of −$2.9.
    pub fn by_io(&self, measured: u64, cross_output: u64) -> (u64, i64, u64) {
        let input = measured
            .saturating_add(self.recovery)
            .saturating_sub(self.overhead);
        let cache =
            self.compounding.saturating_add(self.batching) as i64 - self.overhead_cache as i64;
        (input, cache, self.output.saturating_add(cross_output))
    }
}

/// One pass over the gain ledger, shared by every surface that reports savings.
///
/// WHY THIS EXISTS. Nine separate loops in `dashboard_cli.rs` and five more
/// here each re-implemented "read the ledger and add it up", and they drifted:
/// on 2026-08-04 the text dashboard and `--json` reported nets 51 million
/// tokens apart, and a comment dated `#735` records a PREVIOUS hand-alignment
/// of the same two surfaces. Aligning copies by hand is how they diverge again;
/// there has to be one place that decides what a row means.
///
/// The rules live here, once:
///   - `rollup_calls` defaults to 1 (a plain row is one call);
///   - code-intelligence rows are HEURISTIC, so they never enter the measured
///     totals (`#735`) and a row stuck at `before == after` is expanded through
///     [`crate::code_intel_heuristic`] (`#544`);
///   - a window of 0 means everything; otherwise a row must carry a `ts` at or
///     after the cutoff (`#window`).
#[derive(Default, Debug, Clone)]
pub struct LedgerScan {
    pub measured_before: u64,
    pub measured_after: u64,
    pub calls: u64,
    /// op → (before, after, calls), measured rows only.
    pub per_op: std::collections::BTreeMap<String, (u64, u64, u64)>,
    /// project path → (before, after, calls), measured rows only.
    pub per_project: std::collections::BTreeMap<String, (u64, u64, u64)>,
    /// op → (calls, estimated tokens saved) for code-intelligence rows.
    pub code_intel: std::collections::BTreeMap<String, (u64, u64)>,
    /// Sum of the code-intel column above.
    pub code_intel_saved: u64,
    /// Savings across EVERY row, window ignored — the cumulative figure sits
    /// beside the windowed one and must not be narrowed by it.
    pub lifetime_saved: u64,
    /// Measured savings from rows at/after `recent_cutoff` (0 disables).
    pub recent_saved: u64,
    /// The slice of `measured_saved()` that rests on an APPROXIMATION rather
    /// than exact arithmetic — rows on a channel where the agent would have
    /// REFUSED the payload instead of truncating it, and where crediting the
    /// cap is a deliberate stand-in (Paul, 2026-08-10). Only rows the cap
    /// actually bit count: a payload under the cap is delivered whole and its
    /// saving is exact. Surfaces that print a total use this to say so.
    pub approximated_saved: u64,
}

impl LedgerScan {
    pub fn measured_saved(&self) -> u64 {
        self.measured_before.saturating_sub(self.measured_after)
    }
}

/// Fold one already-parsed ledger row into a scan. `window_cutoff == 0` keeps
/// every row.
/// **DECLARED RESERVE.** The ledger scan now reads its rows through its own
/// path; this function is the PURE half, kept because it is the only form
/// testable without writing a file.
#[allow(dead_code)]
pub(crate) fn scan_row(scan: &mut LedgerScan, v: &serde_json::Value, window_cutoff: u64) {
    scan_row_windowed(scan, v, window_cutoff, 0);
}

/// Two cutoffs: `window_cutoff` gates everything except `lifetime_saved`, which
/// is cumulative BY DEFINITION and must survive any window; `recent_cutoff`
/// feeds `recent_saved`, the "last 7 days" line that sits beside the total.
/// One ledger record, read the way every surface has to read it.
///
/// The rules encoded here were re-implemented at each call site before, which
/// is how they drifted: a surface that forgot `rollup_calls` under-counted
/// rotated history, one that forgot the `filter` fallback merged old rows into
/// `?`, and one that forgot the #544 expansion showed code-intel as free.
/// Decode once, accumulate however the surface needs to.
pub struct Row {
    pub op: String,
    pub label: String,
    pub project: String,
    pub agent: String,
    pub ts: u64,
    /// Rollup rows collapse N records into one; a live row counts as 1.
    pub calls: u64,
    pub before: u64,
    pub after: u64,
    pub is_code_intel: bool,
}

impl Row {
    pub fn saved(&self) -> u64 {
        self.before.saturating_sub(self.after)
    }
    /// Lifetime mode (cutoff 0) keeps everything. A row with no `ts` cannot be
    /// placed in time, so it never counts toward a bounded window.
    pub fn in_window(&self, cutoff: u64) -> bool {
        cutoff == 0 || (self.ts > 0 && self.ts >= cutoff)
    }
}

/// Identity of one ledger EVENT, for crediting it exactly once.
///
/// A ledger append that lands twice — a retried hook, a replayed rollup —
/// repeats every field, so keying on the event collapses it. What the key must
/// NOT collapse is two different commands that happen to produce the same
/// token counts in the same second, and until 2026-09-03 it did: the key was
/// `(ts, op, before, after)` and both sites carried a comment arguing such
/// events are "indistinguishable from a loop" — while the row carries the
/// `label` that distinguishes them.
///
/// Measured on a real ledger: 22 keys collapsed DIFFERENT commands, 47 rows
/// and 1 279 063 tokens of real savings never credited. They are command
/// VARIANTS, which is why they collide — `kubectl get pods`, `kubectl get po
/// -A` and `kubectl get pods --all-namespaces` fold the same data to the same
/// two numbers in the same second. Run 24 distinct commands in parallel and
/// every surface reported ONE operation; the signed receipt certified it.
///
/// `sess` is in the key for the same reason: two agents running the same
/// command in the same second are two events (17 more rows on that ledger),
/// and a doubled append cannot differ in it — it is written once, by one
/// process, and copied.
///
/// The label is hashed rather than kept: a ledger holds hundreds of thousands
/// of rows and the label is bounded at 512 bytes, so keeping it whole would
/// make the credit set tens of megabytes.
pub fn credit_key(v: &serde_json::Value) -> (u64, String, u64, u64, u64) {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v["label"].as_str().unwrap_or("").hash(&mut h);
    v["sess"].as_str().unwrap_or("").hash(&mut h);
    let (ts, op, before, after) = server_dedup_key(v);
    (ts, op, before, after, h.finish())
}

/// The key the SERVER's ingest dedups on — `tokenade_usage_event_dedup_idx`,
/// migration 0008. It is a function rather than a sentence because it was a
/// sentence, in three places, and every one of them went stale the day the
/// local key stopped matching it: "the key is the one the server's ingest
/// already dedups on, so the headline here now means the same thing as the
/// metered number" was true when written and false afterwards.
///
/// [`credit_key`] is built FROM this, plus the label and the session, so the
/// local key can only ever be FINER: it splits groups the server keeps
/// together, never the reverse. That direction is the whole safety argument
/// and it is now structural instead of asserted.
///
/// `parse_gain_line` sends nothing else that could key an event, so this is
/// the whole of it. Measured 2026-09-03 on a real ledger: the local figure
/// sits 62 events and 1 473 059 tokens above the metered one, 0.24 %.
pub fn server_dedup_key(v: &serde_json::Value) -> (u64, String, u64, u64) {
    (
        v["ts"].as_u64().unwrap_or(0),
        // `filter` is the legacy spelling of `op`; one site fell back to it
        // and the other keyed the empty string, so the same row had two
        // identities depending on which surface was asking.
        v["op"]
            .as_str()
            .or_else(|| v["filter"].as_str())
            .unwrap_or("?")
            .to_string(),
        v["before"].as_u64().unwrap_or(0),
        v["after"].as_u64().unwrap_or(0),
    )
}

pub fn decode_row(v: &serde_json::Value) -> Row {
    // Older rows carry the compactor under `filter`; `?` keeps an unlabelled
    // row visible rather than silently merging it into another op's bucket.
    let op = v["op"]
        .as_str()
        .or_else(|| v["filter"].as_str())
        .unwrap_or("?")
        .to_string();
    let raw_before = v["before"].as_u64().unwrap_or(0);
    let after = v["after"].as_u64().unwrap_or(0);
    let is_code_intel = crate::code_intel_heuristic::is_code_intel(&op);
    // #544 — a code-intel row that recorded no delta still avoided a read; the
    // heuristic supplies the baseline it would have cost.
    let before = if is_code_intel && raw_before == after {
        crate::code_intel_heuristic::estimated_baseline(after)
    } else {
        raw_before
    };
    Row {
        label: v["label"].as_str().unwrap_or(&op).to_string(),
        project: v["project"].as_str().unwrap_or("(unknown)").to_string(),
        agent: v["agent"].as_str().unwrap_or("unknown").to_string(),
        ts: v["ts"].as_u64().unwrap_or(0),
        calls: v["rollup_calls"].as_u64().unwrap_or(1),
        op,
        before,
        after,
        is_code_intel,
    }
}

pub fn scan_row_windowed(
    scan: &mut LedgerScan,
    v: &serde_json::Value,
    window_cutoff: u64,
    recent_cutoff: u64,
) {
    let row = decode_row(v);
    let Row {
        op,
        ts,
        calls,
        before,
        after,
        is_code_intel: is_ci,
        project,
        agent,
        ..
    } = row;
    let in_window = window_cutoff == 0 || (ts > 0 && ts >= window_cutoff);

    // Cumulative first: it counts a row the window would have excluded.
    scan.lifetime_saved = scan
        .lifetime_saved
        .saturating_add(before.saturating_sub(after));
    if !in_window {
        return;
    }

    // The per-op breakdown lists EVERY op, heuristic ones included — it is a
    // "what ran" view. The measured totals below take only what was counted.
    let e = scan.per_op.entry(op.clone()).or_insert((0, 0, 0));
    e.0 += before;
    e.1 += after;
    e.2 += calls;

    if is_ci {
        let saved = before.saturating_sub(after);
        let e = scan.code_intel.entry(op).or_insert((0, 0));
        e.0 += calls;
        e.1 += saved;
        scan.code_intel_saved += saved;
        return;
    }

    scan.calls += calls;
    scan.measured_before += before;
    scan.measured_after += after;
    // Label, do not subtract: the figure stays in the total by decision, and
    // what is tracked is how much of it stands on a stand-in.
    let source = v["source"].as_str().unwrap_or("");
    scan.approximated_saved =
        scan.approximated_saved
            .saturating_add(crate::coefficients::approximated_slice(
                &agent, source, &op, before, after,
            ));
    // An unattributed row goes under `(unknown)` rather than nowhere: dropping
    // it would leave the per-project breakdown summing to less than the
    // measured total it sits beside.
    let e = scan.per_project.entry(project).or_insert((0, 0, 0));
    e.0 += before;
    e.1 += after;
    e.2 += calls;
    if recent_cutoff != 0 && ts >= recent_cutoff {
        scan.recent_saved = scan
            .recent_saved
            .saturating_add(before.saturating_sub(after));
    }
}

/// Scan a whole ledger body (one JSON object per line).
pub fn scan_ledger(content: &str, window_cutoff: u64) -> LedgerScan {
    // Delegate rather than loop again: this WAS a second loop over the same
    // content, and it silently skipped the credit-once rule its sibling
    // applies — which is how the signed receipt (this path) and `gain` (the
    // other) came to report 673 874 356 and 669 007 667 tokens for one
    // lifetime ledger. One loop, one set of rules.

    scan_ledger_with(content, window_cutoff, 0, |_, _| {})
}

/// [`scan_ledger`] with the second cutoff and a per-row hook, so a surface with
/// a bespoke accumulator (the dashboard attributes post-compaction tokens to
/// the agent tool that produced them) does it in the SAME pass instead of
/// opening a second loop — which is how the copies started.
pub fn scan_ledger_with(
    content: &str,
    window_cutoff: u64,
    recent_cutoff: u64,
    mut on_row: impl FnMut(&serde_json::Value, bool),
) -> LedgerScan {
    let mut scan = LedgerScan::default();
    // Credit each distinct event ONCE. `gain` has done this since PO #37 and
    // the other four surfaces did not, so one ledger produced two answers:
    // measured 2026-09-02 on a real install, `gain` reported 669 007 667
    // tokens before and the SIGNED receipt 673 874 356 over the same lifetime
    // scope — the receipt being the high one, which is the direction that
    // matters for a figure a user exports to say what tokenade did.
    //
    // The key is the event, not the line: a ledger append that lands twice
    // (a retried hook, a replayed rollup) carries the same timestamp, op and
    // both token counts. Two genuinely distinct events sharing all four are
    // collapsed, and that is the conservative trade `gain` already made.
    let mut credited: std::collections::HashSet<(u64, String, u64, u64, u64)> =
        std::collections::HashSet::new();
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if !credited.insert(credit_key(&v)) {
            continue;
        }
        let ts = v["ts"].as_u64().unwrap_or(0);
        let in_window = window_cutoff == 0 || (ts > 0 && ts >= window_cutoff);
        on_row(&v, in_window);
        scan_row_windowed(&mut scan, &v, window_cutoff, recent_cutoff);
    }
    scan
}

/// The net savings figure, from ONE place.
///
/// MEASURED 2026-08-04: `tokenade dashboard` reported 923 844 942 and
/// `tokenade dashboard --json` reported 975 053 822 for the same window — a
/// 51-million-token gap, because the human view subtracted `overhead_cache`
/// (our injected context re-billed on later turns) and the JSON's own copy of
/// the formula did not. A script and a person reading the same install
/// disagreed by 5,5 %.
///
/// Every surface calls this. A second copy is how the two drifted.
pub fn net_tokens(measured: u64, cross_agent: u64, est: &EstimatedSummary) -> u64 {
    measured
        .saturating_add(est.saved())
        .saturating_add(cross_agent)
        .saturating_sub(est.overhead)
        .saturating_sub(est.overhead_cache)
}

impl EstimatedSummary {
    /// Approximate savings only (excludes the overhead cost).
    pub fn saved(&self) -> u64 {
        self.output
            .saturating_add(self.batching)
            .saturating_add(self.compounding)
            .saturating_add(self.recovery)
    }
}

/// Read + bucket the estimated ledger (UNION its rollup so lifetime totals
/// survive rotation). Best-effort: a missing file yields all-zero.
pub fn estimated_summary() -> EstimatedSummary {
    estimated_summary_from(&ledger_with_rollup(&estimated_ledger_path()))
}

/// Like [`estimated_summary`] but only counts entries with `ts >= cutoff_secs`
/// (0 = lifetime, identical to `estimated_summary`). Entries without a `ts`
/// (legacy / pre-timestamp) are excluded from the window — same rule the
/// dashboard ledger loop uses, so the accounting block matches the hero. Used
/// by the dashboard's `--days N` window.
pub fn estimated_summary_windowed(cutoff_secs: u64) -> EstimatedSummary {
    estimated_summary_since(&ledger_with_rollup(&estimated_ledger_path()), cutoff_secs)
}

/// Per-UTC-day estimated-ledger buckets `(date, summary)` — powers the
/// webdash range selector so its 3-way accounting card windows EXACTLY like
/// the terminal block (`estimated_summary_windowed`). Rows without a `ts`
/// (legacy / rollup) land under the empty-string date; the caller decides
/// whether to fold them into "all time" only.
pub fn estimated_by_day() -> std::collections::BTreeMap<String, EstimatedSummary> {
    let content = ledger_with_rollup(&estimated_ledger_path());
    let mut days: std::collections::BTreeMap<String, EstimatedSummary> = Default::default();
    let mut bound = RebillBound::over(&content);
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let date = v["ts"]
            .as_u64()
            .map(|ts| {
                // chrono-free UTC date (same as webdash::utc_date_from_ts).
                let days_since = ts / 86_400;
                let (y, m, d) = crate::platform::civil_from_days(days_since as i64);
                format!("{y:04}-{m:02}-{d:02}")
            })
            .unwrap_or_default();
        let saved = bound.take(&v, estimated_saved_of_row(&v));
        days.entry(date).or_default().add_row(&v, saved);
    }
    days
}

/// The class an estimated row belongs to, from `class` when the row carries
/// one and from `op` when rotation dropped it. ONE resolver: the bucket a row
/// lands in and the bound applied to it must be decided by the same rule, or a
/// row can be credited as one class and bounded as another.
pub fn canonical_estimated_class<'a>(class: Option<&'a str>, op: &'a str) -> &'a str {
    match class.filter(|c| !c.is_empty()) {
        Some(c) => c,
        None => match op {
            "output_lean" | "write_to_edit" => "output",
            "batching" => "batching",
            "compounding" => "compounding",
            "scaffold_overhead" | "injected_context" | "overhead_rebill" | "recovery_cost" => {
                "overhead"
            }
            "digest_recovery" => "recovery",
            _ => "",
        },
    }
}

/// Which estimated classes are EXEMPT from the per-tool-call delivery cap.
///
/// The cap models what one tool call can put in front of the model. It is the
/// right unit for a payload and the wrong unit for an accounting delta:
/// `compounding` and `batching` are per-TURN figures spanning many calls (a
/// single compounding row is routinely far beyond the 7.5k bash cap by
/// construction), `overhead` is a COST we subtract rather than a saving, and
/// `output` counts tokens the model would have GENERATED — no tool result was
/// ever truncated there.
///
/// Everything else IS one tool call's payload, and a payload is bounded
/// whether it was measured or estimated. `recovery` is the case this exists
/// for: `expand-ref` hands back a blob through a tool result exactly like a
/// `Read` does, and it was claiming the whole blob with no ceiling — one field
/// row credited 393 740 tokens, 17x the bound its own channel carries.
///
/// EXEMPT BY EXPLICIT LIST, bounded by default: a class nobody thought about
/// must inherit the bound, not the exemption.
pub fn estimated_class_is_cap_exempt(class: &str) -> bool {
    matches!(
        class,
        "compounding" | "batching" | "overhead" | "output" | "output_lean"
    )
}

/// `saved` for ONE estimated row, bounded the way a measured row is unless its
/// class is exempt. Never larger than the naive `before - after`: the bound
/// only ever removes credit.
pub fn estimated_saved_of_row(v: &serde_json::Value) -> u64 {
    let naive = v["saved"].as_u64().unwrap_or_else(|| {
        v["before"]
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(v["after"].as_u64().unwrap_or(0))
    });
    let class = canonical_estimated_class(v["class"].as_str(), v["op"].as_str().unwrap_or(""));
    if estimated_class_is_cap_exempt(class) {
        return naive;
    }
    realistic_of_row(v).min(naive)
}

/// Bucket estimated-ledger JSONL `content` by class. Split out from
/// [`estimated_summary`] so the rotation/rollup behaviour (which drops the
/// `class` field, forcing the op→class recovery below) is unit-testable
/// without touching the global ledger path.
pub(super) fn estimated_summary_from(content: &str) -> EstimatedSummary {
    estimated_summary_since(content, 0)
}

/// [`estimated_summary_from`] over the rows with `ts >= cutoff` (0 = all).
/// The re-bill bound runs over EVERY row, in order, before the window is
/// applied: a session's injections may predate the window its re-bills fall in.
fn estimated_summary_since(content: &str, cutoff: u64) -> EstimatedSummary {
    let mut s = EstimatedSummary::default();
    let mut bound = RebillBound::over(content);
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // Bounded per row before it enters any bucket — an estimated payload
        // is capped like a measured one unless its class is exempt.
        let saved = bound.take(&v, estimated_saved_of_row(&v));
        if cutoff > 0 && !v["ts"].as_u64().is_some_and(|ts| ts >= cutoff) {
            continue;
        }
        s.add_row(&v, saved);
    }
    s
}

/// **A session's overhead re-bill can never exceed `HORIZON_TURNS ×` what it
/// injected** — the rule the hook has enforced since 1.1.21, applied again at
/// EVERY site that reads the ledger (the dashboard summaries and the priced
/// figure in `weigh`).
///
/// Before 1.1.21 the bound reset at each compaction from a baseline that was
/// not updated, and every reset re-opened a full budget. MEASURED 2026-10-05 on
/// this machine's ledger: 62.8 M tokens of re-bill in 30 days, 95 % from two
/// long sessions charged **429×** and **197×** what they injected (82.7 k and
/// 122 k), 2–10 M a day until 2026-09-29, 0.1–0.25 M a day after. On the 146
/// sessions started after the fix, the highest ratio is exactly 12.0 — the
/// invariant holds on correct rows, so bounding at read time takes nothing
/// from them. It also covers rows still written by older clients.
///
/// Rows without a `sess` — the daily rollups of rotated ledgers (58.4 M of the
/// 62.8 M re-bill here) — are bounded TOGETHER by the injections that carry no
/// `sess` either: the invariant holds per session, so it holds on their sum.
pub(crate) struct RebillBound {
    room: std::collections::HashMap<String, u64>,
    used: std::collections::HashMap<String, u64>,
}

impl RebillBound {
    /// The budgets of every session in `content` (estimated-ledger JSONL).
    pub(crate) fn over(content: &str) -> Self {
        let mut room: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for line in content
            .lines()
            .filter(|l| l.contains("injected_context") || l.contains("scaffold_overhead"))
        {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if let Some("injected_context" | "scaffold_overhead") = v["op"].as_str() {
                let sess = v["sess"].as_str().unwrap_or("");
                *room.entry(sess.to_string()).or_insert(0) += v["saved"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_mul(super::HORIZON_TURNS);
            }
        }
        RebillBound {
            room,
            used: Default::default(),
        }
    }

    /// What may be counted of row `v`, whose saving is `saved`. Rows must be
    /// offered in ledger order.
    pub(crate) fn take(&mut self, v: &serde_json::Value, saved: u64) -> u64 {
        if v["op"].as_str() != Some("overhead_rebill")
            || matches!(
                super::files::compactor_of(v),
                Some(super::EXACT_LABEL | super::PROXY_LABEL)
            )
        {
            return saved;
        }
        let sess = v["sess"].as_str().unwrap_or("");
        let Some(&room) = self.room.get(sess) else {
            return saved;
        };
        let used = self.used.entry(sess.to_string()).or_insert(0);
        let take = saved.min(room.saturating_sub(*used));
        *used += take;
        take
    }
}
