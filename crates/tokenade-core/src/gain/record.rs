//! **Write ONE row, and refuse to write a false one.**
//!
//! Every ledger entry goes through `record_inner`, and that is where the
//! safeguards live — because a single careless caller makes every honest
//! figure next to it implausible. A "saving" that made the output GROW is not
//! one; a row claiming more than `MAX_PLAUSIBLE_ROW_SAVING` is always an
//! estimation error (136 rows once carried 3.1 BILLION tokens, 99.8% of the
//! ledger, for an 800 MB video); a development build never counts
//! (440 424 613 tokens fabricated by an evaluation loop in 2026-07); and a
//! label rarely exceeds 200 bytes, except that **791 rows out of 875 exceed
//! it** and beyond `PIPE_BUF` two processes interleave inside the row every
//! published figure derives from.

use super::*;

/// Append a single record. `source` is who called it (`cli` / `mcp` / `lib`),
/// `op` is the canonical operation name (the same one tools advertise).
/// Failures (no .tokenade dir, full disk, …) are silent — the caller's hot path
/// must never block on telemetry.
///
/// Side effect: if the ledger has grown past [`ROTATION_THRESHOLD_BYTES`]
/// since the last write, [`rotate_if_oversized`] is called inline to keep
/// only the most recent [`ROTATION_WINDOW_SECS`] of records.
pub fn record(source: &str, op: &str, label: &str, s: Savings) {
    // Anti-inflation backstop moved INTO record_inner (writers audit: only
    // this wrapper had it; record_classed/estimated/cmd could
    // all write after>before unguarded).
    record_inner("gain.jsonl", source, op, label, s, None, None, None, None);
}

pub fn record_classed(source: &str, op: &str, label: &str, class: &str, s: Savings) {
    record_inner(
        "gain.jsonl",
        source,
        op,
        label,
        s,
        None,
        None,
        Some(class),
        None,
    );
}

/// Append an APPROXIMATE saving to the separate `gain_estimated.jsonl`
/// ledger (output-lean, batching, compounding) or a negative-sense
/// `overhead` row (scaffold/system-prompt injection cost). Kept out of
/// `gain.jsonl` so every existing reader of the exact ledger stays correct
/// by construction — the dashboard opts in to summing the estimated file
/// under its own "~estimated" line. `class` is required (the bucket);
/// rows carry `"estimated": true`. For `overhead`, `s.tokens_before` is the
/// injected size and `s.tokens_after` 0, so `saved` = the cost the
/// dashboard SUBTRACTS.
/// The cache-read tokens a batch of N results SAVES versus fetching/searching
/// them one at a time — the exact figure, using tokenade's own batching model.
///
/// Done sequentially, each result would be re-sent as cached context on every
/// LATER turn: result at 0-indexed emission position `i` (size `sᵢ`) rides along
/// in the `N-1-i` turns that follow it, so the avoided re-send is
/// `Σ sᵢ·(N-1-i)` cache_read tokens. This is the SAME formula
/// `claudecode_transcript::batching_saved_tokens` credits for NATIVE parallel
/// tool calls — but that estimator sees a tokenade batch as ONE Bash tool call
/// (K=1 → 0), so this level is where the tokenade-batch saving must be booked.
/// No double-count: the two credit disjoint things (native tool-call
/// parallelism vs the targets — URLs, queries, questions — fused inside one
/// tokenade call). Every such fusion books through
/// [`record_batch_cache_saving`], so they share one definition.
pub fn batching_cache_saved(sizes: &[usize]) -> usize {
    let n = sizes.len();
    sizes
        .iter()
        .enumerate()
        .map(|(i, &s)| s * (n - 1 - i))
        .sum()
}

/// Book the avoided re-sends of `sizes` — the results ONE call returned that
/// sequential calls would each have carried into the later turns — in the
/// estimated `batching` ledger, priced as cache_read (`io=cache`). Only the
/// results the agent can use belong in `sizes`: an empty answer bought no
/// turn. Nothing is booked below two.
pub fn record_batch_cache_saving(source: &str, label: &str, sizes: &[usize]) {
    if sizes.len() < 2 {
        return;
    }
    let saved = batch_credit(source, label, sizes);
    if saved > 0 {
        record_estimated_io(
            source,
            "batching",
            label,
            "batching",
            "cache",
            Savings::new(saved, 0),
        );
    }
    // Each turn saved would also have re-read the whole conversation: its
    // context at fire, in BILLED tokens (`turns_context` is not converted by
    // `gain::weigh`). Booked apart so the two terms stay distinct.
    if let Some(ctx) = super::fire_context::context_at_fire() {
        let turns_context = turns_context_credit(source, label, sizes.len(), ctx);
        if turns_context > 0 {
            record_estimated_io(
                source,
                "turns_context",
                label,
                "batching",
                "cache",
                Savings::new(turns_context, 0),
            );
        }
    }
}

/// The API call a recovery took (`expand-ref` after a fold) re-read the
/// whole conversation: a cost the fold caused, the mirror of the turn a batch
/// spares. Its context at fire, in BILLED tokens like `turns_context`
/// (`gain::weigh` does not convert it), booked as overhead read from cache.
/// Outside Claude Code the context is not readable and nothing is booked.
///
/// Only a call that did nothing else: MEASURED 2026-10-05 over 30 days of
/// local transcripts, 677 of 925 messages running `expand-ref` made other
/// tool calls too — no call was added for them, and charging their context
/// would have counted 329 M tokens instead of 83 M.
pub fn record_recovery_turn(source: &str, label: &str) {
    if let Some(f) = super::fire_context::fired().filter(|f| f.tool_calls == 1) {
        record_estimated_io(
            source,
            "recovery_turn",
            label,
            "overhead",
            "cache",
            Savings::new(f.context as usize, 0),
        );
    }
}

/// The context a fused call of `n` targets spared: `(n − 1)` turns, less the
/// share the agent would have fused natively, each re-reading `ctx`.
///
/// Error margin: `native_parallel_share` is a LOWER bound (a sequential run
/// also counts calls that depended on each other), so this is an upper bound
/// — by ±2 points of share on files and questions (±0.02 × the formula's
/// `(n−1)·ctx`), ±5 on pages, ±6 on symbols. The context itself is read, not
/// estimated.
pub fn turns_context_credit(source: &str, label: &str, n: usize, ctx: u64) -> usize {
    if n < 2 {
        return 0;
    }
    let left = 1.0 - native_parallel_share(source, label);
    ((n - 1) as f64 * left * ctx as f64).round() as usize
}

/// What [`record_batch_cache_saving`] books: the avoided re-sends, less the
/// share the agent would have avoided anyway by calling in parallel.
pub fn batch_credit(source: &str, label: &str, sizes: &[usize]) -> usize {
    if sizes.len() < 2 {
        return 0;
    }
    let left = 1.0 - native_parallel_share(source, label);
    (batching_cache_saved(sizes) as f64 * left).round() as usize
}

/// How often the agent, needing several independent targets of the kind this
/// fused call served, would have asked for them in ONE turn anyway — native
/// parallel tool calls. That share of the batch saved no turn.
///
/// `batching_cache_saved` assumes the alternative was N SEQUENTIAL calls. It
/// mostly is not. MEASURED 2026-10-05 on 30 days of local Claude Code
/// transcripts (1 632 files), counting, per tool, the occasions it was called
/// for several targets: as one parallel group (one message), or as a run of
/// consecutive single-call messages.
///
/// | stands in for | parallel | sequential | share | ±95 % |
/// |---|---|---|---|---|
/// | `WebFetch` (`web`) | 96 | 6 | 0.94 | 0.05 |
/// | `WebSearch` (`search`) | 87 | 1 | 0.99 | 0.02 |
/// | `Read` (questions, files: `skeleton`) | 1 832 | 577 | 0.76 | 0.02 |
/// | `Grep` (symbols: `query`) | 133 | 153 | 0.47 | 0.06 |
///
/// A sequential run also counts calls that DEPENDED on each other (following
/// a link the previous page gave), so each share is a lower bound and the
/// credit left is an upper bound. What it does not answer: whether the agent
/// would batch natively WITHOUT tokenade's own batching rule in its context.
pub fn native_parallel_share(source: &str, label: &str) -> f64 {
    match (source, label) {
        (_, "questions") => 0.76,
        (_, "symbols") => 0.47,
        ("web", _) => 0.94,
        ("search", _) => 0.99,
        _ => 0.76,
    }
}

/// `record_estimated` with an explicit io direction (e.g. "cache" for the
/// overhead re-billing rows — the server's weighted SQL cases on io).
pub fn record_estimated_io(
    source: &str,
    op: &str,
    label: &str,
    class: &str,
    io: &str,
    sv: Savings,
) {
    record_inner(
        "gain_estimated.jsonl",
        source,
        op,
        label,
        sv,
        None,
        None,
        Some(class),
        Some(serde_json::json!({ "io": io })),
    );
}

/// Recovery telemetry — accounting-neutral like [`record_nudge`] (before ==
/// after, saved 0). `op` becomes `recovery:<event>`, and the metric is TOKENS,
/// not a count.
///
/// It exists because a question this product is built around had no denominator
/// on 2026-08-04: when a fold stashes content and offers `expand-ref`, does
/// anyone ever come back for it? Counting `expand-ref` runs alone answers
/// nothing — 46 of them could be a 1 % recovery rate or a 90 % one. `debug.log`
/// cannot supply the other half either: not every banner passes through it, and
/// it rotates.
///
/// Two events, and tokens rather than counts, so the answer is about VOLUME:
///   - `offered`  — a fold withheld this many tokens behind a pointer
///   - `expanded` — a pointer was followed and returned this many
///
/// A large `offered` with a near-zero `expanded` means either the folds are
/// good enough to make recovery unnecessary, or agents do not take the offer —
/// which reading is right needs a look at WHAT was withheld, but until this
/// exists neither reading can even be tested.
pub fn record_recovery(event: &str, tokens: usize) {
    record_inner(
        "gain_estimated.jsonl",
        "hook",
        &format!("recovery:{event}"),
        event,
        Savings::new(tokens, tokens),
        None,
        None,
        Some("recovery"),
        None,
    );
}

/// Cache-thrash telemetry counter — accounting-neutral like [`record_nudge`]
/// (before == after, saved 0): a full-context cache rewrite inside the live
/// cache window was DETECTED, not avoided. `op` becomes
/// `thrash:<trigger>:<gapbucket>` (trigger ∈ userturn|meta|scheduled|
/// toolloop; gapbucket ∈ lt2m|lt5m|lt15m|lt1h) — the third segment encodes
/// the idle gap (same 3-segment trick as `nudge:<kind>:<event>`).
/// `metric` = re-written tokens (rides before/after); `extra` keeps the exact
/// prev_ctx/gap. Local ledger only — read by the local web dashboard. See `tokenade_core::cache_thrash` for the detector
/// and the 2026-07-12 field audit that sized this class at ~24% of spend.
pub fn record_thrash(trigger: &str, rewritten: usize, prev_ctx: usize, gap_secs: u64) {
    let bucket = match gap_secs {
        0..=119 => "lt2m",
        120..=299 => "lt5m",
        300..=899 => "lt15m",
        _ => "lt1h",
    };
    record_inner(
        "gain_estimated.jsonl",
        "hook",
        &format!("thrash:{trigger}:{bucket}"),
        trigger,
        Savings::new(rewritten, rewritten),
        None,
        None,
        Some("thrash"),
        Some(serde_json::json!({ "prev_ctx": prev_ctx, "gap": gap_secs })),
    );
}

/// `n` tokens tokenade put into this session's context: they are re-read on
/// every later call until the conversation compacts, and that re-read is
/// charged (`overhead_rebill`, from these two logs). Injected scaffolding
/// and what `expand-ref` hands back both land here.
///
/// Injected-overhead accumulator for the per-turn re-bill (latency: spares it
/// a full estimated-ledger scan). Appends are tiny and the per-session file is
/// GC'd with the rest of sessions/.
pub fn note_injected_into_context(n: u64) {
    note_injected(n, false)
}

/// `persistent`: in EVERY call, through compactions (the rules file) — logged `p`.
fn note_injected(n: u64, persistent: bool) {
    if let Some(skey) = session_key() {
        // Added while a subagent runs: it sits in the subagent's context, not
        // the parent's, and is re-read on the subagent's calls only.
        if crate::session::sidechain_generation(&skey) > 0 {
            record_estimated_io(
                "hook",
                "overhead_rebill",
                super::PROXY_LABEL,
                "overhead",
                "cache",
                Savings::new(
                    (n.saturating_mul(super::SUBAGENT_EVENT_REREADS)) as usize,
                    0,
                ),
            );
            return;
        }
        let p = session_overhead_counter_path(&skey);
        // Single-write append (`platform::append_line`): concurrent hook processes
        // interleaving two `write()`s make a line that parses and is wrong.
        crate::platform::append_line(&p, &n.to_string());
        crate::platform::append_line(
            &crate::session::session_injected_log_path(&skey),
            &format!(
                "{} {n}{}",
                crate::platform::now_unix_secs(),
                if persistent { " p" } else { "" }
            ),
        );
    }
}

pub fn record_estimated(source: &str, op: &str, label: &str, class: &str, s: Savings) {
    if class == "overhead" {
        note_injected(
            s.tokens_before.saturating_sub(s.tokens_after) as u64,
            op == "scaffold_overhead",
        );
    }
    record_inner(
        "gain_estimated.jsonl",
        source,
        op,
        label,
        s,
        None,
        None,
        Some(class),
        None,
    );
}

/// Like [`record`], but also tags the row with the bare command name
/// (`cmd`, e.g. `"git"` — basename of argv[0], never arguments/paths) and
/// whether a *command-specific* compactor handled it (`covered`, see
/// [`crate::filter::FilterResult::is_command_specific`]). The central
/// server aggregates these to surface which commands agents run most and
/// still lack a dedicated compactor. `cmd` is a program name only —
/// anonymous and free of secrets — so it is not redacted.
pub fn record_cmd(source: &str, op: &str, label: &str, s: Savings, cmd: &str, covered: bool) {
    // Fleet-data fix: Windows self-invocations (`tokenade.exe …`) showed up
    // as a "command" in the fleet analytics — pure noise, never record self.
    if cmd == "tokenade" || cmd == "tokenade.exe" {
        return;
    }
    record_inner(
        "gain.jsonl",
        source,
        op,
        label,
        s,
        Some(cmd),
        Some(covered),
        None,
        None,
    );
}

/// Append a record to the bypass-lost ledger (see [`lost_ledger_path`]).
/// Shape is identical to `gain.jsonl` rows so existing parsers can be
/// reused; here `before` = the raw tokens the agent actually paid and
/// `after` = what the right compactor WOULD have returned — so `saved`
/// reads as "tokens lost to this bypass". Same guards (opt-out, test
/// isolation, redaction) and the same rotation policy as `record`.
pub fn record_lost(source: &str, op: &str, label: &str, s: Savings) {
    record_inner(
        "bypass_lost.jsonl",
        source,
        op,
        label,
        s,
        None,
        None,
        None,
        None,
    );
}

/// Record an MCP tool result the proxy SAW but did NOT compact (already terse,
/// image-only, or a content shape we don't fold yet). `before` is the raw
/// token volume; `after` == `before` (zero saving by design). Lands in
/// `gain.jsonl` tagged `class="mcp-uncompacted"` so the dashboard + central
/// server can report WHICH MCP tools still lack effective compaction (and how
/// much volume they carry) — the inverse of the `mcp:<tool>` saving rows.
pub fn record_mcp_uncompacted(op: &str, before: usize) {
    record_inner(
        "gain.jsonl",
        "wrap-mcp",
        op,
        op,
        Savings::new(before, before),
        None,
        None,
        Some("mcp-uncompacted"),
        None,
    );
}

/// Record a compacted MCP tool result with BOTH the token saving and the
/// raw byte volume (before/after). Lands in `gain.jsonl` under the same
/// `wrap-mcp` / `mcp:<tool>` shape as [`record`] did, but carries
/// `bytes_before`/`bytes_after` in the row so `tokenade mcp-proxy report`
/// can show a bytes column alongside tokens without re-deriving it from a
/// token estimate. `op` is `mcp:<tool>` (per-tool) or `mcp-proxy:<label>`.
pub fn record_mcp_tool(op: &str, tokens: Savings, bytes_before: usize, bytes_after: usize) {
    record_inner(
        "gain.jsonl",
        "wrap-mcp",
        op,
        op,
        tokens,
        None,
        None,
        None,
        Some(serde_json::json!({
            "bytes_before": bytes_before,
            "bytes_after": bytes_after,
        })),
    );
}

/// Ceiling on what ONE ledger row may claim to have saved.
///
/// Calibrated against the real ledger: excluding the media bug this guard was
/// written for, the largest row ever recorded was 936k tokens, with a single
/// 6.6M outlier — so 2M is comfortably above honest traffic while still three
/// orders of magnitude below the 209M a single careless `bytes/4` produced.
/// This is a backstop, not a budget: callers are expected to compute a bounded
/// credit themselves (see `media_credit`, `delivery_cap_tokens`), and anything
/// reaching this clamp is a bug worth finding — which is why it is logged.
pub const MAX_PLAUSIBLE_ROW_SAVING: usize = 2_000_000;

// Longest `label` a row may carry, in bytes, AFTER redaction.
//
// The ledger's cross-process safety rests on the OS making an append atomic,
// which it promises only below `PIPE_BUF` (4 KiB on Linux and macOS) —
// [`write_lock`] is process-local and protects nothing between processes. The
// comment on that lock argued the point was moot because "every record we
// emit is ≤200 bytes"; measured on a real ledger 2026-09-03, **791 of 875
// rows are over 200 bytes, seven are over PIPE_BUF, and the largest is
// 8 696** — a `sha256sum` over many files whose label alone ran to 8 359. Two
// commands finishing together can interleave inside a row that long, and what
// they corrupt is the accounting every published figure derives from.
//
// 512 leaves the rest of a row (project, session, op, agent, the numbers) an
// order of magnitude of headroom under the cap. A label is a diagnostic —
// WHICH command produced this saving — and its head carries that; the tail of
// an oversized one is a file list. The cut is marked, so a reader never takes
// a truncated command for the whole of it.
// A per-booking id was added here on 2026-09-03 and REMOVED the same day,
// before release. Recording why, because the reasoning that led to it is
// sound and will recur.
//
// The composite key below cannot separate "this event happened twice" from
// "this row was appended twice", and 42 groups / 64 rows / 3 165 437 tokens on
// a real ledger are collapsed because of it. An id per booking fixes that
// exactly — and re-opens PO #37, which the coarse key exists for. MEASURED
// with the id in place: 30 identical `cat` runs booked 1 227 660 tokens where
// the coarse key credits 40 922.
//
// The arithmetic is what decides it. PO #37 saw ~37 identical events per
// second, 89 245 193 tokens in one day. Thirty-seven Bash tool calls per
// second is not a thing an agent does, and 89 million tokens is four hundred
// times a 200k context — so those events cannot each be output the agent
// received, and their counterfactual is fictional. The shell proxy makes every
// `cat` in a loop its own booking while the agent made one tool call.
//
// Two parallel Read calls on one file ARE two real turns, and they are
// indistinguishable from that loop in everything a row carries. So the trade
// stands: 3.17M under-credited against 89M over-credited, and this comment's
// own words for it — "under-crediting is the honest direction to err in".
// Do not add the id back without a way to tell a tool call from a subprocess.

pub(super) const MAX_LABEL_BYTES: usize = 512;

pub(super) fn bound_label(label: &str) -> String {
    if label.len() <= MAX_LABEL_BYTES {
        return label.to_string();
    }
    let mut end = MAX_LABEL_BYTES;
    while end > 0 && !label.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… (+{} bytes)", &label[..end], label.len() - end)
}

pub(super) fn record_inner(
    file_name: &str,
    source: &str,
    op: &str,
    label: &str,
    s: Savings,
    cmd: Option<&str>,
    covered: Option<bool>,
    class: Option<&str>,
    extra: Option<serde_json::Value>,
) {
    // Central anti-inflation backstop (was only on `record()`): a "saving"
    // that grew the output is not a saving — keep the event for coverage
    // stats, zero the saving. Applies to EVERY writer/ledger uniformly.
    let s = if s.tokens_after > s.tokens_before {
        // The flag survives the safeguard: zeroing the CLAIM does not change
        // where the baseline came from, and a row that loses its stamp here
        // becomes indistinguishable from a measurement again.
        Savings {
            tokens_before: s.tokens_before,
            tokens_after: s.tokens_before,
            ..s
        }
    } else {
        s
    };

    // Central PLAUSIBILITY backstop, the twin of the anti-inflation one above.
    // A single row may not claim more than `MAX_PLAUSIBLE_ROW_SAVING` tokens:
    // no real interaction ever avoided that much in one call, so a bigger
    // number is always a bug in a caller's estimate — and a big enough one
    // poisons every aggregate downstream. Measured incident: a media serve
    // booked `bytes/4` and logged 209 MILLION tokens for ONE 800 MB video;
    // 136 such rows reached 3.1 BILLION, i.e. 99.8 % of the entire ledger, and
    // the dashboard reported "3.1B tokens saved (=46 688 $)". Clamping here
    // means one careless caller can no longer make every honest number next to
    // it unbelievable. Clamped rows are logged so the cause stays findable.
    let s = if s.tokens_before.saturating_sub(s.tokens_after) > MAX_PLAUSIBLE_ROW_SAVING {
        crate::debug_log::log(
            "gain",
            "row_saving_clamped",
            serde_json::json!({
                "op": op,
                "label": label,
                "before": s.tokens_before,
                "after": s.tokens_after,
                "clamped_to": MAX_PLAUSIBLE_ROW_SAVING,
            }),
        );
        // ZERO the claim, exactly like the anti-inflation twin above — do not
        // merely cap it. Capping still books 2M tokens for a row the code has
        // just concluded is a caller bug, and 2M is ~2x the largest honest row
        // ever recorded: the media incident would have booked 272M instead of
        // 3.1B, reduced by 91% but still nonsense. Keep the event for coverage
        // stats, drop the saving.
        Savings {
            tokens_before: s.tokens_after,
            tokens_after: s.tokens_after,
            ..s
        }
    } else {
        s
    };

    // No user opt-out: this ledger is what the reporter sends to meter the
    // plan, so a switch here (the former `TOKENADE_NO_GAIN=1`) was a way to
    // use Tokenade unmetered. Only the dev-build guard below skips writes.

    // A DEV build never books gains. Shipped binaries are release builds, so
    // `debug_assertions` is exactly "this is somebody's working copy" — a
    // harness, a benchmark, a `cargo run`. None of that is a user saving
    // tokens, and all of it is metered against a real licence.
    //
    // Measured incident (2026-07-21): a web-reader evaluation loop ran the
    // distiller over a 175-page corpus dozens of times from `target/debug`
    // and booked **440 424 613 tokens** of fabricated savings — 297 M in one
    // day — which consumed the account's 100 M monthly cap and paused
    // tokenade on the machine. A guard that has to be remembered is not a guard.
    //
    // `TOKENADE_FORCE_GAIN=1` re-enables writes for the gain tests
    // themselves, which must still exercise this path.
    if cfg!(debug_assertions) && !crate::env_compat::is_truthy_one("TOKENADE_FORCE_GAIN") {
        return;
    }
    // #464: defense-in-depth — refuse `test:*` ops when the ledger root
    // wasn't redirected. Test code uses `test:zero-drop-PID` etc. for
    // log_mcp assertions; production code never writes the `test:`
    // prefix. If we see one while ROOT_OVERRIDE is unset the calling
    // test forgot ensure_test_gain_isolated() and would otherwise
    // pollute ~/.tokenade/gain.jsonl. Production audit 2026-05-31
    // found 300 leaked `test:nonzero` records that arrived this way
    // before MCP tests were isolated in #422.
    if op.starts_with("test:") && ROOT_OVERRIDE.get().is_none() {
        return;
    }
    // #495: extend the defense to the `source` field too. Some tests
    // pass source="test" / "test-src" with an op that doesn't carry
    // the `test:` prefix — production audit found 2 such leaks
    // (op="leak_probe_op_480", source="test") from an early draft of
    // the #480 regression test. Block any source that starts with
    // "test" when ROOT_OVERRIDE is unset.
    if source.starts_with("test") && ROOT_OVERRIDE.get().is_none() {
        return;
    }
    let path = jsonl_path_for(file_name);
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    // `project` = the cwd at write time. We tag every record so a
    // global ledger can still be broken down per-project at read time
    // (`tokenade dashboard` does this). Best-effort: if cwd isn't
    // readable, the field is just absent (older records survive).
    // Booked for another agent (`as_agent`): this directory is the proxy's, not its project.
    let who = super::as_agent::row_agent();
    let project = who
        .is_none()
        .then(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.display().to_string())
        })
        .flatten();
    // #734 — defense-in-depth: drop writes whose cwd looks like a
    // test tempdir. Found 12 production ledger rows with project
    // paths like `/tmp/.../tokenade-cli-cwd-<pid>-<nanos>` —
    // they come from `with_tmp_cwd` (cli test helper) which sets
    // env::current_dir to a tempdir but does NOT redirect HOME, so
    // the writes go to the REAL `~/.tokenade/gain.jsonl` with the
    // tempdir as the project label. These contaminate the
    // dashboard's by_project list with bogus projects. This guard
    // catches any future test helper that forgets to also redirect
    // HOME; it's belt-and-suspenders on top of the existing
    // source="test:*" filter (#464/#495) — which only catches
    // explicitly-tagged test events, not real CLI subcommands
    // invoked from within a test (e.g. structure_map called from
    // a test that exercises the CLI surface).
    if let Some(p) = project.as_deref() {
        if crate::stores::is_test_cwd(p) {
            return;
        }
    }
    // #480 — SECURITY: redact the label before serialising. Several
    // call sites pass user-controlled strings as the label —
    // `grep_compact` writes the search pattern, `webfetch_compact`
    // writes the URL, `glob_compact` writes the glob. If the user
    // greps for an `AKIA…` key while debugging, the pattern lands in
    // ~/.tokenade/gain.jsonl in cleartext. Apply redact here so every
    // call site (present + future) is covered without needing to
    // remember at each emission point. The `source` and `op` fields
    // are enum-like strings hard-coded at call sites, so they don't
    // need redaction.
    let safe_label = bound_label(&crate::redact::redact(label));
    let agent = who
        .as_ref()
        .map(|w| w.0.clone())
        .unwrap_or_else(|| crate::detect::from_env().as_str().to_string());
    let mut line = serde_json::json!({
        "ts": crate::platform::now_unix_secs(),
        "sess": session_key(),
        "source": source,
        "op": op,
        "label": safe_label,
        "project": project,
        "agent": agent,
        "before": s.tokens_before,
        "after": s.tokens_after,
        "saved": s.tokens_before.saturating_sub(s.tokens_after),
    });
    // `cmd` / `covered` are only present for Bash command rows (set by
    // `record_cmd`); inserting them conditionally keeps Read/WebFetch rows
    // from carrying two `null` fields each.
    // `deliverable` is what the AGENT would really have put in front of its
    // model for this exact payload, computed at fold time when the bytes were
    // still in hand. Written only when it was computed: a missing field means
    // "nobody could tell", and readers fall back to the per-agent scalar. That
    // asymmetry is deliberate — a zero here would silently erase a row's whole
    // saving, and `Option` is the only encoding where "unknown" cannot be
    // mistaken for "nothing".
    // **The EXACT model, stamped here and not at send time.** The reporter
    // inferred the model from transcripts at POST time
    // (`license::stamp_event_models`): exact session, otherwise the session
    // covering the timestamp, otherwise the DAY'S DOMINANT model. An agent
    // switching models during the day thus valued its savings at another
    // model's price. Here, the session is known and the host declared its
    // model to the hook: it is the finest granularity available. The
    // inference stays as a fallback — `stamp_event_models` skips events that
    // are already stamped.
    if let Some(m) = who.and_then(|w| w.1).or_else(|| {
        session_key()
            .as_deref()
            .and_then(crate::gain::session_model_for)
    }) {
        // The day's rate is noted HERE, once per (model, day): ten months from
        // now this row will be valued at the price it was worth today, not at
        // the price current when someone looks at it.
        crate::rate_history::observe(&m, now_ms() / 1000);
        line["model"] = serde_json::Value::from(m);
    }
    let optional: [(&str, Option<serde_json::Value>); 4] = [
        (
            "billed",
            s.in_billed_tokens.then_some(serde_json::Value::Bool(true)),
        ),
        ("deliverable", s.deliverable_tokens.map(Into::into)),
        ("cmd", cmd.map(Into::into)),
        ("covered", covered.map(Into::into)),
    ];
    for (k, v) in optional.into_iter().filter_map(|(k, v)| Some((k, v?))) {
        line[k] = v;
    }
    // `class` buckets the saving by lever (compaction / dedup / output /
    // batching / compounding / overhead) so the dashboard can show each
    // separately. Absent on legacy rows — readers treat a missing class
    // as "compaction" (the only thing recorded before 0.6.4).
    // A FABRICATED baseline is stamped here, at write time, rather than at
    // each of the thirteen calls that produce one. An explicit class wins:
    // `ci_heuristic` already says the same thing more precisely.
    let class = class.or(if s.baseline_is_counterfactual {
        Some(crate::coefficients::CLASS_COUNTERFACTUAL)
    } else {
        None
    });
    if let Some(c) = class {
        line["class"] = serde_json::Value::from(c);
    }
    // `extra` merges caller-supplied fields onto the row.
    // Only object values are merged; anything else is ignored.
    if let Some(serde_json::Value::Object(map)) = extra {
        if let serde_json::Value::Object(dst) = &mut line {
            for (k, v) in map {
                // Canonical fields are NOT overridable via extra — a caller
                // slip would silently corrupt the row (writers audit).
                if matches!(
                    k.as_str(),
                    "before"
                        | "after"
                        | "saved"
                        | "ts"
                        | "op"
                        | "label"
                        | "source"
                        | "project"
                        | "agent"
                        | "sess"
                        | "class"
                        | "estimated"
                ) {
                    continue;
                }
                dst.insert(k, v);
            }
        }
    }
    // `estimated` marks rows whose saving is an approximation (output /
    // batching / compounding) rather than an exact raw→compacted delta.
    // It is set ONLY for the estimated ledger so even a parser that
    // unions both files can tell measured from estimated. Exact ledgers
    // (gain.jsonl / bypass_lost.jsonl) never carry it.
    if file_name == "gain_estimated.jsonl" {
        line["estimated"] = serde_json::Value::Bool(true);
    }
    // Serialise writes within the process so threads can't interleave
    // mid-line. We hold the lock for the file write AND the rotation
    // check; if the latter races on a multi-process setup, the worst-case
    // outcome is one stale snapshot of records — never corruption.
    let _guard = write_lock().lock().unwrap_or_else(|p| p.into_inner());
    // Up to one retry: a process blocked on the flock while ANOTHER process
    // rotates acquires the lock on the OLD inode after `rename(tmp, path)`
    // replaced it — its append would land in the orphaned file and vanish.
    // After locking, verify the fd still IS `path` (same dev+inode); if the
    // rotation won the race, reopen the new file and append there.
    // Cross-process lock: flock(LOCK_EX) on Unix, LockFileEx on Windows.
    // Prevents separate tokenade processes from interleaving records mid-line,
    // and holds off the rotation for the length of this append. It is taken on
    // the SIDECAR — see `lock_path_for` — because on Windows a lock on the
    // ledger would lock this process out of its own file.
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(lock_path_for(&path))
        .ok();
    if let Some(l) = lock_file.as_ref() {
        let _ = crate::platform::lock_briefly(l);
    }
    for attempt in 0..2 {
        // The counter drives the unix dev+inode recheck below and nothing else,
        // so it reads as dead when the file is compiled for Windows. Consuming
        // it explicitly there says "unused on purpose" instead of renaming it
        // `_attempt`, which would have made the unix branch stop compiling.
        #[cfg(not(unix))]
        let _ = attempt;
        let Ok(f) = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)
        else {
            break;
        };
        #[cfg(unix)]
        if attempt == 0 {
            use std::os::unix::fs::MetadataExt;
            let same = match (f.metadata(), fs::metadata(&path)) {
                (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
                _ => false,
            };
            if !same {
                continue; // rotated under us — reopen the new file
            }
        }
        let mut f = f;
        // Single write per record — cross-process appends interleave when a
        // record takes several write() calls (see debug_log.rs). This ledger is
        // the savings accounting; a torn line = silently dropped savings.
        let mut rec = line.to_string();
        rec.push('\n');
        let _ = f.write_all(rec.as_bytes());
        break;
    }
    if let Some(l) = lock_file.as_ref() {
        let _ = fs2::FileExt::unlock(l);
    }
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() >= ROTATION_THRESHOLD_BYTES {
            let _ = rotate_if_oversized(&path, ROTATION_WINDOW_SECS);
        }
    }
    drop(_guard);
    super::bank::bank_measured_row(file_name, source, op, class, &agent, &s);
}

#[cfg(test)]
mod batch_credit_tests {
    use super::*;

    /// The sequential formula is only the share the agent would NOT have
    /// parallelised natively: a fetch batch keeps 6 %, a question batch 24 %.
    #[test]
    fn a_batch_is_credited_only_for_the_turns_native_parallelism_would_not_have_saved() {
        let sizes = [1_000, 1_000, 1_000];
        assert_eq!(batching_cache_saved(&sizes), 3_000);
        assert_eq!(batch_credit("web", "batch", &sizes), 180);
        assert_eq!(batch_credit("search", "batch", &sizes), 30);
        assert_eq!(batch_credit("web", "questions", &sizes), 720);
        assert_eq!(batch_credit("skeleton", "files", &sizes), 720);
        assert_eq!(batch_credit("query", "symbols", &sizes), 1_590);
        // The context term: 2 turns saved, 6 % of them not fused natively.
        assert_eq!(turns_context_credit("web", "batch", 3, 100_000), 12_000);
        assert_eq!(
            turns_context_credit("query", "symbols", 3, 100_000),
            106_000
        );
        assert_eq!(turns_context_credit("web", "batch", 1, 100_000), 0);
        assert_eq!(batch_credit("web", "batch", &[5_000]), 0);
    }

    /// What `expand-ref` hands back is a COST of the fold, subtracted — even
    /// on a rolled-up row that lost its class, which must never land in
    /// `recovery`, the class that is a saving.
    #[test]
    fn a_recovery_cost_stays_a_cost_once_rolled_up() {
        assert_eq!(
            super::super::canonical_estimated_class(None, "recovery_cost"),
            "overhead"
        );
        assert_eq!(
            super::super::canonical_estimated_class(Some("overhead"), "recovery_cost"),
            "overhead"
        );
    }
}
