// The tests of `crate::gain`, moved out of the production file.
//
// Mounted by `include!`, not by `mod`: the tokens are spliced INTO `gain`,
// so every `super::` keeps the meaning it had. Split by
// `srcsweep::production_lines`.

#[cfg(test)]
mod mcp_report_tests {
    use super::*;

    #[test]
    fn aggregates_per_tool_with_dollars() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-mcp-report-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let ledger = dir.join("gain.jsonl");
        // Two calls to search_code (compacted) + one uncompacted call
        // (saved 0) + a foreign non-mcp row that must be ignored.
        let rows = [
            r#"{"source":"wrap-mcp","op":"mcp:search_code","sess":"s1","ts":100,"before":1000,"after":200,"saved":800,"bytes_before":4000,"bytes_after":800}"#,
            r#"{"source":"wrap-mcp","op":"mcp:search_code","sess":"s1","ts":200,"before":500,"after":100,"saved":400,"bytes_before":2000,"bytes_after":400}"#,
            r#"{"source":"wrap-mcp","op":"mcp:read_file","sess":"s1","ts":300,"before":50,"after":50,"saved":0,"bytes_before":200,"bytes_after":200}"#,
            r#"{"source":"wrap","op":"read","sess":"s1","ts":400,"before":9999,"after":1,"saved":9998}"#,
        ];
        std::fs::write(&ledger, rows.join("\n")).unwrap();
        // Stub attributor: everything ran on Opus (input $15/Mtok).
        let model_of = |_s: Option<&str>, _t: u64| "claude-opus-4-8".to_string();
        let stats = mcp_tool_report_from(std::slice::from_ref(&ledger), &model_of);

        assert_eq!(stats.len(), 2, "search_code + read_file, foreign ignored");
        // Sorted by $ desc → search_code first.
        let sc = &stats[0];
        assert_eq!(sc.tool, "search_code");
        assert_eq!(sc.calls, 2);
        assert_eq!(sc.tokens_saved, 1200);
        assert_eq!(sc.bytes_saved, 4000 - 800 + 2000 - 400);
        // 1200 input tokens at whatever Opus 4.8 costs — read from the SAME
        // chain the server prices with, never a literal. This assertion said
        // `* 15.0` until 2026-09-10, and that literal was the bug: Opus has
        // been $5/Mtok since 4.5, so the test was pinning a 3x overstatement.
        let rate = crate::pricing::rates_for_model("claude-opus-4-8").input;
        assert!((sc.usd_saved - 1200.0 / 1e6 * rate).abs() < 1e-12);
        let rf = &stats[1];
        assert_eq!(rf.tool, "read_file");
        assert_eq!(rf.calls, 1);
        assert_eq!(rf.tokens_saved, 0);
        assert_eq!(rf.usd_saved, 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
#[cfg(test)]
mod realistic_tests {
    use super::*;
    #[test]
    fn small_output_equals_naive_huge_output_is_capped() {
        // before <= cap: realistic == naive (harness would deliver it whole).
        assert_eq!(realistic_saved(2000, 500, 7500, 0.27), 1500);
        // huge file (before >> cap): direct = cap-after, plus 27% of overflow
        // BOUNDED at 2×cap. before=91520 after=1808 cap=7500: direct=5692,
        // overflow=84020, credit=min(0.27*84020=22685, 15000) -> 20692
        // (vs naive 89712, vs 28377 pre-bound).
        assert_eq!(realistic_saved(91_520, 1_808, 7_500, 0.27), 20_692);
        // ratio 0 => only the direct (capped) part counts.
        assert_eq!(realistic_saved(100_000, 1_000, 7_500, 0.0), 6_500);
        // Field over-count regression (`auto:diff 9.77M/43 events`): a ~3.3 MB
        // diff (820k tokens) must credit at most direct + 2×cap — the credit
        // no longer grows with blob size.
        assert_eq!(
            realistic_saved(820_000, 500, 7_500, 0.27),
            (7_500 - 500) + 15_000
        );
        assert_eq!(
            realistic_saved(10_000_000, 500, 7_500, 0.27),
            (7_500 - 500) + 15_000,
            "credit is size-independent past the ceiling"
        );
    }
    #[test]
    fn realistic_of_row_uses_precomputed_then_legacy_then_live() {
        // precomputed `realistic` (new rollup row) wins verbatim.
        let r =
            serde_json::json!({"before": 99999, "after": 0, "realistic": 42, "source": "rollup"});
        assert_eq!(realistic_of_row(&r), 42);
        // legacy rollup row (no realistic) keeps naive before-after.
        let r = serde_json::json!({"before": 1000, "after": 100, "source": "rollup"});
        assert_eq!(realistic_of_row(&r), 900);
        // live Bash row beyond the cap gets discounted.
        let r = serde_json::json!({"before": 100_000, "after": 1_000, "source": "wrap", "agent": "claude-code"});
        let cap = crate::coefficients::DEFAULT_CAP_BASH_TOKENS;
        let expect = realistic_saved(
            100_000,
            1_000,
            cap,
            crate::coefficients::overflow_credit_ratio(),
        );
        assert_eq!(realistic_of_row(&r), expect);
    }

    /// The point of the whole caps module, reduced to one assertion: a row that
    /// carries what the agent REALLY would have delivered is credited on that,
    /// not on the per-agent bound. The two answers must differ here, or the
    /// wiring proves nothing.
    #[test]
    fn a_measured_delivery_beats_the_scalar_bound() {
        let scalar = crate::coefficients::delivery_cap_tokens("gemini-cli", "wrap", "cat");
        // gemini-cli's shell rule is head+tail on a computed threshold, so for
        // a payload of many short lines the real delivery is far under the
        // bound the scalar has to assume.
        let payload = "ab\n".repeat(400_000); // 1.2 M chars, 400 k lines
        let delivered = crate::agent_caps::delivered_chars(
            "gemini-cli",
            crate::agent_caps::Action::Command,
            &payload,
            crate::agent_caps::Ctx::UNKNOWN,
        )
        .expect("gemini-cli command cap")
            / 4;
        assert_ne!(
            delivered, scalar,
            "anti-vacuity: the measurement must differ from the bound"
        );
        let with = serde_json::json!({
            "before": 500_000u64, "after": 1_000, "source": "wrap", "op": "cat",
            "agent": "gemini-cli", "deliverable": delivered
        });
        let without = serde_json::json!({
            "before": 500_000u64, "after": 1_000, "source": "wrap", "op": "cat",
            "agent": "gemini-cli"
        });
        assert_ne!(
            realistic_of_row(&with),
            realistic_of_row(&without),
            "the recorded delivery must change the credited figure"
        );
        // And a row WITHOUT the field must still be credited exactly as before,
        // so nothing already on disk moves.
        let bounded = realistic_saved(
            500_000,
            1_000,
            scalar,
            crate::coefficients::overflow_ratio_for_channel("gemini-cli", "wrap", "cat"),
        );
        assert_eq!(realistic_of_row(&without), bounded);
    }

    #[test]
    fn only_rows_the_cap_actually_bit_count_as_approximate() {
        let row = |agent: &str, before: u64| {
            serde_json::json!({
                "before": before, "after": 500, "source": "hook", "op": "read",
                "agent": agent, "ts": 1_700_000_000u64
            })
        };
        let cap = crate::coefficients::delivery_cap_tokens("claude-code", "hook", "read");
        let mut scan = LedgerScan::default();
        // Under the cap: delivered whole, the arithmetic is exact.
        scan_row(&mut scan, &row("claude-code", cap - 1), 0);
        assert_eq!(
            scan.approximated_saved, 0,
            "a payload the cap never touched is measured, not approximated"
        );
        // Over the cap on a REFUSING channel: the credit is a stand-in.
        scan_row(&mut scan, &row("claude-code", cap + 10_000), 0);
        assert_eq!(scan.approximated_saved, cap + 10_000 - 500);
        // Same shape on a TRUNCATING channel stays measured.
        let before = scan.approximated_saved;
        scan_row(&mut scan, &row("gemini-cli", cap + 10_000), 0);
        assert_eq!(
            scan.approximated_saved, before,
            "gemini-cli truncates, so its credit is exact arithmetic"
        );
        // And the label never exceeds the total it qualifies.
        assert!(scan.approximated_saved <= scan.measured_saved());
    }

    #[test]
    fn binary_dump_on_nontruncating_agent_is_bounded_by_bash_cap() {
        // The reported over-count: `strings` on a binary spewing 50 M tokens on
        // a NON-truncating shell channel (aider) was credited ~50 M "saved"
        // (u64::MAX cap => realistic == naive). Now the per-agent bash cap
        // bounds the direct term AND the overflow credit is suppressed (nothing
        // was truncated -> no recoverable tail), so realistic ~= cap - after.
        let cap = crate::coefficients::delivery_cap_tokens("aider", "wrap", "strings");
        assert_eq!(cap, 12_800); // finite per-agent bash cap, not u64::MAX
        let r = serde_json::json!({
            "before": 50_000_000u64, "after": 1_000,
            "source": "wrap", "agent": "aider", "op": "strings"
        });
        assert_eq!(realistic_of_row(&r), cap - 1_000); // 11_800, no overflow blow-up
                                                       // gemini-cli USED to be bounded the same way, on the belief that its
                                                       // shell channel truncates nothing. Measured 2026-08-10, it does
                                                       // (`saveTruncatedContent`, on by default), so a truncated tail really
                                                       // is there to recover and the overflow credit is earned. Still nowhere
                                                       // near the naive 50 M: that bound is what this test exists to hold.
        let g = serde_json::json!({
            "before": 50_000_000u64, "after": 1_000,
            "source": "wrap", "agent": "gemini-cli", "op": "xxd"
        });
        let gem = realistic_of_row(&g);
        assert!(gem > 12_800 - 1_000, "the recoverable tail is credited");
        assert!(gem < 100_000, "and it stays bounded, far from 50 M: {gem}");
        // A TRUNCATING agent (claude-code) keeps the recoverable-tail credit:
        // direct(cap) + 0.27*overflow > cap.
        let c = serde_json::json!({
            "before": 50_000_000u64, "after": 1_000,
            "source": "wrap", "agent": "claude-code", "op": "strings"
        });
        assert!(realistic_of_row(&c) > 7_500);
    }
}
#[cfg(test)]
mod weighting_contract {
    use super::*;

    fn only(f: impl Fn(&mut EstimatedSummary)) -> EstimatedSummary {
        let mut e = EstimatedSummary::default();
        f(&mut e);
        e
    }

    /// THE price table, pinned one bucket at a time.
    ///
    /// This formula is money. It decides the headline number on the terminal,
    /// on the HTML dashboard and on tokenade.net, and the SERVER MIRRORS IT in
    /// `drizzle/queries.ts` (`WEIGHTED_SQL`) under the standing "one formula,
    /// one place" directive. Until this test existed, the whole thing was
    /// unguarded on both sides — which is how `batching` spent its life
    /// credited at ten times its own price class.
    ///
    /// If you change a number here, the SQL has to move in the SAME release.
    /// A client and a server that disagree about what a token is worth is a
    /// worse bug than either value being wrong.
    #[test]
    fn every_bucket_is_weighted_at_its_own_price_class() {
        const N: u64 = 1_000;
        let none = EstimatedSummary::default();

        // Input class, x1 — measured compaction, and recovery, which brings
        // back content the model would otherwise pay fresh input for.
        assert_eq!(weighted_from_parts(N, 0, &none), N, "measured is x1");
        assert_eq!(
            weighted_from_parts(0, 0, &only(|e| e.recovery = N)),
            N,
            "recovery is x1"
        );

        // Output class, x5 — an output token costs five input tokens.
        assert_eq!(
            weighted_from_parts(0, 0, &only(|e| e.output = N)),
            N * 5,
            "output is x5"
        );
        assert_eq!(
            weighted_from_parts(0, N, &none),
            N * 5,
            "cross-output is x5"
        );

        // Cache-read class, x0.1 — re-reads of a prefix that is already
        // cached. `batching` counts avoided RE-sends, so it belongs here and
        // nowhere else.
        assert_eq!(
            weighted_from_parts(0, 0, &only(|e| e.compounding = N)),
            N / 10,
            "compounding is x0.1"
        );
        assert_eq!(
            weighted_from_parts(0, 0, &only(|e| e.batching = N)),
            N / 10,
            "batching is x0.1 — it buys re-sends that never happen"
        );

        // Debits carry the sign, at the class they were billed on.
        assert_eq!(
            weighted_from_parts(N, 0, &only(|e| e.overhead = N)),
            0,
            "overhead is a x1 debit"
        );
        assert_eq!(
            weighted_from_parts(N, 0, &only(|e| e.overhead_cache = N)),
            N - N / 10,
            "injected-context re-billing is a x0.1 debit"
        );
    }

    /// A row counts at ITS model's ratios, not Opus's. Pinned on hand-set
    /// weighted twins (the lookup itself reads the machine's LiteLLM cache).
    #[test]
    fn weighted_twins_replace_the_default_ratios() {
        const N: u64 = 1_000;
        // fable-5-1-like: cache x0.025; gpt-5.5-like: output x6.
        let est = EstimatedSummary {
            output: N,
            compounding: N,
            overhead_cache: N,
            weighted: Some(WeightedBuckets {
                output: 6.0 * N as f64,
                cache_credit: 0.025 * N as f64,
                overhead_cache: 0.025 * N as f64,
            }),
            ..Default::default()
        };
        assert_eq!(weighted_from_parts(N, 0, &est), N + 6 * N);
        // Cross-agent output carries no model: default ratio.
        assert_eq!(weighted_from_parts(0, N, &est), 6 * N + 5 * N);
        // Never below zero, like the integer path.
        let debit = EstimatedSummary {
            weighted: Some(WeightedBuckets {
                overhead_cache: 1e9,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(weighted_from_parts(N, 0, &debit), 0);
    }

    /// Rows without a model (every row before 1.1.12) book their twins at the
    /// default ratios: the total is exactly the old integer formula's.
    #[test]
    fn rows_without_a_model_weigh_exactly_as_before() {
        let rows = [
            ("output", None, 1_000u64),
            ("compounding", None, 30_000),
            ("batching", None, 2_000),
            ("overhead", Some("cache"), 10_000),
            ("overhead", None, 500),
            ("recovery", None, 700),
        ];
        let mut est = EstimatedSummary::default();
        for (class, io, saved) in rows {
            let mut v = serde_json::json!({ "class": class, "saved": saved });
            if let Some(io) = io {
                v["io"] = serde_json::json!(io);
            }
            est.add_row(&v, saved);
        }
        let legacy = EstimatedSummary {
            weighted: None,
            ..est
        };
        assert!(est.weighted.is_some());
        assert_eq!(
            weighted_from_parts(5_000, 300, &est),
            weighted_from_parts(5_000, 300, &legacy)
        );
    }

    #[test]
    fn io_weights_are_the_model_price_ratios_within_bounds() {
        let r = |input, cache, output| crate::pricing::ModelRates {
            input,
            cache,
            output,
        };
        assert_eq!(io_weights_from_rates(&r(5.0, 0.5, 25.0)), Some((5.0, 0.1)));
        assert_eq!(
            io_weights_from_rates(&r(10.0, 0.25, 50.0)),
            Some((5.0, 0.025))
        );
        assert_eq!(
            io_weights_from_rates(&r(0.0, 0.1, 1.0)),
            None,
            "no input price"
        );
        assert_eq!(
            io_weights_from_rates(&r(1.0, 5.0, 5.0)),
            None,
            "cache dearer than input"
        );
        assert_eq!(
            io_weights_from_rates(&r(1.0, 0.1, 500.0)),
            None,
            "output ratio x500"
        );
        assert_eq!(
            io_weights_for_model(""),
            (DEFAULT_OUTPUT_WEIGHT, DEFAULT_CACHE_WEIGHT)
        );
    }

    /// The legend every surface prints has to describe the table above. A
    /// number that drifts out of the prose is a claim we no longer honour.
    #[test]
    fn the_published_legend_matches_the_table() {
        for legend in [WEIGHTED_LEGEND_EN, WEIGHTED_LEGEND_FR] {
            assert!(legend.contains('5'), "legend must state the output ratio");
            assert!(
                legend.contains("50"),
                "legend must state the cache-read ratio"
            );
        }
    }
}
#[cfg(test)]
mod real_savings_tests {
    use super::{row_io, Io};
    use serde_json::json;

    #[test]
    fn row_io_prefers_explicit_io_over_class() {
        // io column wins over class (mirrors the server's COALESCE(io,...))
        let (io, ov) = row_io(&json!({"class": "output", "io": "cache"}));
        assert!(matches!(io, Io::Cache) && !ov);
        let (io, _) = row_io(&json!({"class": "compounding", "io": "output"}));
        assert!(matches!(io, Io::Output));
    }

    #[test]
    fn row_io_derives_from_class_when_io_absent() {
        assert!(matches!(
            row_io(&json!({"class": "compounding"})).0,
            Io::Cache
        ));
        assert!(matches!(row_io(&json!({"class": "output"})).0, Io::Output));
        assert!(matches!(
            row_io(&json!({"class": "output_lean"})).0,
            Io::Output
        ));
        // op → class fallback
        assert!(matches!(
            row_io(&json!({"op": "output_lean"})).0,
            Io::Output
        ));
        assert!(row_io(&json!({"op": "overhead_rebill"})).1);
    }

    #[test]
    fn row_io_overhead_flag_and_default_input() {
        let (io, ov) = row_io(&json!({"class": "overhead", "io": "cache"}));
        assert!(matches!(io, Io::Cache) && ov);
        let (io, ov) = row_io(&json!({"class": "overhead"}));
        assert!(matches!(io, Io::Input) && ov); // scaffold overhead defaults input
        let (io, ov) = row_io(&json!({}));
        assert!(matches!(io, Io::Input) && !ov); // measured row → input, not a cost
    }
}
#[cfg(test)]
mod estimated_bound_tests {
    use super::*;
    use serde_json::json;

    fn recovery_row(before: u64) -> serde_json::Value {
        json!({"agent":"claude-code","source":"cli","op":"digest_recovery",
               "class":"recovery","before":before,"after":0,"saved":before})
    }

    /// The field row that named this defect: one `expand-ref` credited
    /// 393 740 tokens, many times what its own channel can deliver.
    #[test]
    fn a_recovery_row_is_bounded_by_its_channel() {
        let cap = crate::coefficients::delivery_cap_tokens("claude-code", "cli", "digest_recovery");
        assert!(
            cap < u64::MAX,
            "the recovery channel carries no cap at all — nothing below can bound it"
        );
        let bounded = estimated_saved_of_row(&recovery_row(393_740));
        assert!(
            bounded < 393_740,
            "recovery is still claiming the whole blob: {bounded}"
        );
        // Exactly ONE truncated re-run, and no overflow credit on top: this
        // row IS the dance that credit pays for.
        assert_eq!(
            bounded, cap,
            "a recovery must be worth one truncated re-run, no more"
        );
        assert_eq!(
            crate::coefficients::overflow_ratio_for_channel(
                "claude-code",
                "cli",
                "digest_recovery"
            ),
            0.0,
            "crediting an overflow dance on the row that IS the dance counts it twice"
        );
    }

    /// What the round delivered is a cost this fold caused, so it comes off
    /// the credit rather than sitting at zero on the other side.
    #[test]
    fn what_the_round_delivered_comes_off_the_credit() {
        let cap = crate::coefficients::delivery_cap_tokens("claude-code", "cli", "digest_recovery");
        let mut v = recovery_row(393_740);
        v["after"] = json!(1_200);
        v["saved"] = json!(393_740 - 1_200);
        assert_eq!(estimated_saved_of_row(&v), cap - 1_200);
    }

    /// Same size, exempt class. A per-turn accounting delta is routinely far
    /// beyond any per-call cap by construction, so bounding it there would
    /// under-count on purpose.
    #[test]
    fn an_exempt_class_of_the_same_size_keeps_its_full_figure() {
        let mut v = recovery_row(393_740);
        v["class"] = json!("compounding");
        v["op"] = json!("compounding");
        assert_eq!(estimated_saved_of_row(&v), 393_740);
    }

    /// The bound only ever removes credit — a row the harness would have
    /// delivered whole reads exactly as it did before.
    #[test]
    fn the_bound_never_adds_credit() {
        assert_eq!(estimated_saved_of_row(&recovery_row(900)), 900);
        // `realistic_of_row` short-circuits on a stamped `realistic` and on
        // rollup rows, and neither figure is derived from this row's own
        // before/after. A bound is allowed to take credit away and never to
        // hand any back, however that stamp got there.
        let mut stamped = recovery_row(900);
        stamped["realistic"] = json!(50_000);
        assert_eq!(estimated_saved_of_row(&stamped), 900);
        let mut rolled = recovery_row(900);
        rolled["source"] = json!("rollup");
        rolled["before"] = json!(50_000);
        assert_eq!(
            estimated_saved_of_row(&rolled),
            900,
            "a rollup row must not out-credit the `saved` it carries"
        );
    }

    /// Exempt BY LIST. A lever nobody thought about must inherit the bound,
    /// not the exemption: the reassuring direction is the one that ships
    /// unbounded claims in silence.
    #[test]
    fn the_exempt_list_is_closed() {
        for c in [
            "compounding",
            "batching",
            "overhead",
            "output",
            "output_lean",
        ] {
            assert!(estimated_class_is_cap_exempt(c), "{c} must stay exempt");
        }
        for c in ["recovery", "digest_recovery", "some-future-lever", ""] {
            assert!(!estimated_class_is_cap_exempt(c), "{c} must be bounded");
        }
    }

    /// Rotation drops `class`. The bound and the bucket must then read the
    /// SAME fallback, or a row is credited as one class and bounded as
    /// another.
    #[test]
    fn a_rollup_row_that_lost_its_class_is_still_bounded_by_its_op() {
        let mut v = recovery_row(393_740);
        v.as_object_mut().unwrap().remove("class");
        assert_eq!(
            canonical_estimated_class(None, "digest_recovery"),
            "recovery"
        );
        assert!(estimated_saved_of_row(&v) < 393_740);
    }

    /// One resolver, used by the summary and by the upload. Both spellings a
    /// rotated ledger can carry must land on the same bucket.
    #[test]
    fn the_class_resolver_covers_every_estimated_op() {
        for (op, class) in [
            ("output_lean", "output"),
            ("write_to_edit", "output"),
            ("batching", "batching"),
            ("compounding", "compounding"),
            ("scaffold_overhead", "overhead"),
            ("injected_context", "overhead"),
            ("overhead_rebill", "overhead"),
            ("digest_recovery", "recovery"),
        ] {
            assert_eq!(canonical_estimated_class(None, op), class, "op {op}");
            assert_eq!(
                canonical_estimated_class(Some(""), op),
                class,
                "an empty class must fall back like a missing one ({op})"
            );
        }
    }
}
#[cfg(test)]
mod tests {

    /// The ledger's lock waits a bounded time and then gets on with it.
    ///
    /// `lock_exclusive()` blocks forever, and every append site discarded its
    /// result anyway, so the wait bought nothing. Measured cost: with the
    /// suite's 56 test binaries running wrapped commands at once, one
    /// `tokenade wrap` had not started after 30 SECONDS, four times in one
    /// session, against 0.25 s for the same test alone.
    ///
    /// The call runs on its own thread and is read back through a channel, so
    /// a version that blocks forever FAILS this test instead of hanging it.
    #[test]
    fn a_held_ledger_lock_is_waited_for_briefly_and_then_ignored() {
        use fs2::FileExt;
        use std::sync::mpsc;

        let dir = std::env::temp_dir().join(format!("tkn-gain-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gain.jsonl");
        fs::write(&path, "{}\n").unwrap();

        let holder = fs::OpenOptions::new()
            .read(true)
            .append(true)
            .open(&path)
            .unwrap();
        holder.lock_exclusive().unwrap();

        let p2 = path.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let f = fs::OpenOptions::new()
                .read(true)
                .append(true)
                .open(&p2)
                .unwrap();
            let t0 = std::time::Instant::now();
            let got = crate::platform::lock_briefly(&f);
            let _ = tx.send((got, t0.elapsed()));
        });

        let (got, waited) = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("lock_briefly never returned: it is still blocking on the lock");
        assert!(!got, "the lock is held, so it cannot have been taken");
        assert!(
            waited >= crate::platform::LEDGER_LOCK_WAIT,
            "it must actually wait first ({waited:?})"
        );
        assert!(
            waited < crate::platform::LEDGER_LOCK_WAIT * 8,
            "and give up close to the ceiling, not long after it ({waited:?})"
        );

        FileExt::unlock(&holder).unwrap();
        let f = fs::OpenOptions::new()
            .read(true)
            .append(true)
            .open(&path)
            .unwrap();
        assert!(
            crate::platform::lock_briefly(&f),
            "anti-vacuity: a free lock must be taken"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// A rotation that cannot take the lock does NOTHING, rather than rewrite
    /// the file underneath the process holding it — the read-filter-write-
    /// rename race those locks exist to close.
    #[test]
    fn a_rotation_that_cannot_lock_leaves_the_ledger_alone() {
        use fs2::FileExt;

        let dir = std::env::temp_dir().join(format!("tkn-gain-rot-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gain.jsonl");
        // Old enough that a rotation WOULD drop every line.
        let old = crate::platform::now_unix_secs().saturating_sub(90 * 86_400);
        let body: String = (0..20)
            .map(|i| format!("{{\"ts\":{old},\"op\":\"x{i}\"}}\n"))
            .collect();
        fs::write(&path, &body).unwrap();

        // The SIDECAR, because that is the object the rotation takes — see
        // `lock_path_for`. Holding the ledger itself would prove nothing now,
        // and on Windows it would lock the rotation out of READING the file
        // rather than out of the lock, which is a different refusal.
        let holder = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(super::lock_path_for(&path))
            .unwrap();
        holder.lock_exclusive().unwrap();

        // On its own thread and read back through a channel, so a version
        // that blocks on the lock FAILS this test instead of hanging it —
        // the first draft did hang, and a mutation that has to be killed by
        // hand is not a verdict.
        let p2 = path.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            let r = super::rotate_if_oversized(&p2, 30 * 86_400);
            let _ = tx.send((r.is_ok(), t0.elapsed()));
        });
        let (ok, waited) = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("rotate_if_oversized never returned: it is still blocking on the lock");
        assert!(ok, "giving up on the lock is not an error");
        assert!(
            waited < std::time::Duration::from_secs(5),
            "it must give up, not block ({waited:?})"
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            body,
            "the ledger must be untouched while another process holds the lock"
        );

        FileExt::unlock(&holder).unwrap();
        super::rotate_if_oversized(&path, 30 * 86_400).unwrap();
        assert_ne!(
            fs::read_to_string(&path).unwrap(),
            body,
            "anti-vacuity: with the lock free, the rotation must actually run"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    use super::*;
    use std::fmt::Write as _;

    #[test]
    fn estimated_summary_buckets_by_class_and_recovers_from_op_after_rollup() {
        // Fresh rows carry `class`; rotated/rollup rows lose it (class absent)
        // but keep `op` — both must land in the right bucket.
        let content = "\
{\"op\":\"output_lean\",\"class\":\"output\",\"saved\":100}
{\"op\":\"batching\",\"class\":\"batching\",\"saved\":40}
{\"op\":\"compounding\",\"class\":\"compounding\",\"saved\":7}
{\"op\":\"scaffold_overhead\",\"class\":\"overhead\",\"saved\":25}
{\"op\":\"output_lean\",\"before\":50,\"after\":0}
{\"op\":\"compounding\",\"saved\":3}
garbage line that should be skipped
{\"op\":\"mystery\",\"saved\":999}";
        let s = estimated_summary_from(content);
        assert_eq!(
            s.output, 150,
            "100 (class) + 50 (op-recovered before/after)"
        );
        assert_eq!(s.batching, 40);
        assert_eq!(s.compounding, 10, "7 (class) + 3 (op-recovered)");
        assert_eq!(s.overhead, 25);
        assert_eq!(
            s.saved(),
            200,
            "output+batching+compounding, excludes overhead"
        );
        // an unknown op/class contributes to nothing (no silent misbucket).
    }

    #[test]
    fn is_test_cwd_catches_known_helper_prefixes() {
        // #734 — must drop on these
        assert!(crate::stores::is_test_cwd(
            "/tmp/claude-1000/tokenade-cli-cwd-12345-67890"
        ));
        assert!(crate::stores::is_test_cwd(
            "/tmp/tokenade-core-gain-noop-1-2"
        ));
        assert!(crate::stores::is_test_cwd(
            "/var/folders/x/tokenade-mcp-fixture-1"
        ));
        assert!(crate::stores::is_test_cwd("/tmp/cargo-test-1234"));

        // Must NOT drop on real project paths even when they contain
        // "tokenade" elsewhere.
        assert!(!crate::stores::is_test_cwd("/home/me/projects/tokenade"));
        assert!(!crate::stores::is_test_cwd("/home/me/work/tokenade-clone"));
        assert!(!crate::stores::is_test_cwd(
            "/home/username/.devworkspace/projects/tokenade"
        ));
        assert!(!crate::stores::is_test_cwd(""));
    }

    /// Serialize gain tests that mutate the environment `record()` reads on
    /// every call (`TOKENADE_FORCE_GAIN`), so one test's toggle cannot make a
    /// sibling's `record()` return early mid-flight.
    /// One lock, because the ENVIRONMENT is one. This used to be a private
    /// `Mutex` of its own, which serialises the tests that take IT and
    /// nothing else: measured across `tokenade-core`, twelve lock groups
    /// existed and four shared a variable with the crate-wide guard —
    /// **`HOME` alone was set under three mutually unaware locks**. That is
    /// the shape of the flakes once observed: one run in
    /// eight, never the same test name. A lock is only sound when no OTHER
    /// lock touches its variables.
    fn gain_env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    /// The one ledger root every test passes to `set_root`: it is a
    /// OnceLock, so the first caller wins for the whole test binary and a
    /// test that redirected elsewhere would read a file nobody wrote.
    fn gain_test_root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("tokenade-gain-{}", std::process::id()))
    }

    #[test]
    fn the_former_opt_out_no_longer_stops_the_ledger() {
        // `TOKENADE_NO_GAIN=1` used to skip every ledger write, and the
        // reporter only sends what the ledger holds: it was a way to run
        // Tokenade unmetered. The variable must now change nothing.
        let _g = gain_env_lock();
        // `test` rows are refused while the ledger root is not redirected.
        set_root(gain_test_root());
        let prev = std::env::var("TOKENADE_NO_GAIN").ok();
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::set_var("TOKENADE_NO_GAIN", "1");
            std::env::set_var("TOKENADE_FORCE_GAIN", "1");
        }
        record("test", "no-gain-probe", "lab", Savings::new(100, 25));
        let ledger = fs::read_to_string(current_ledger_path()).unwrap_or_default();
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::remove_var("TOKENADE_FORCE_GAIN");
            std::env::remove_var("TOKENADE_NO_GAIN");
            if let Some(v) = prev {
                std::env::set_var("TOKENADE_NO_GAIN", v);
            }
        }
        assert!(
            ledger.contains("no-gain-probe"),
            "TOKENADE_NO_GAIN skipped the ledger"
        );
    }

    #[test]
    fn a_dev_build_books_nothing_without_the_opt_in() {
        // 2026-07-21: a web-reader evaluation loop ran the distiller over a
        // 175-page corpus dozens of times from `target/debug` and booked
        // 440 424 613 tokens of fabricated savings against a real licence —
        // 297 M in one day — which consumed the 100 M monthly cap and paused
        // tokenade on the machine. A guard that must be remembered is not a guard.
        //
        // Deliberately does NOT touch HOME or `set_root`: both are
        // process-global and shared with every sibling test in this binary.
        let _g = gain_env_lock();
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe { std::env::remove_var("TOKENADE_FORCE_GAIN") };
        let ledger = current_ledger_path();
        record("test", "devguard-probe", "lab", Savings::new(1_000, 10));
        let after = fs::read_to_string(&ledger).unwrap_or_default();
        // Assert only that OUR record is absent.
        //
        // The first version compared the whole ledger before and after, which
        // made this test fail whenever a sibling appended between the two
        // reads — `gain_env_lock()` serialises the tests that take it, and the
        // rotation tests write through a different path. It failed roughly one
        // run in three and was recorded as "unreproduced" before a later
        // run caught it in the act. A shared mutable file is not a
        // fixture; the only claim this test needs is that the dev build did
        // not book THIS saving.
        assert!(
            !after.contains("devguard-probe"),
            "a dev build booked a saving without TOKENADE_FORCE_GAIN"
        );
    }

    #[test]
    fn writes_jsonl_record() {
        let _g = gain_env_lock();
        let dir = gain_test_root();
        let _ = fs::create_dir_all(&dir);
        set_root(dir.clone());
        // Dev builds book nothing (see `record_inner`); this test exercises the
        // write path itself, so it opts back in explicitly.
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe { std::env::set_var("TOKENADE_FORCE_GAIN", "1") };
        record("test", "demo", "lab", Savings::new(100, 25));
        let content = fs::read_to_string(dir.join(".tokenade").join("gain.jsonl")).unwrap();
        assert!(content.contains("\"op\":\"demo\""));
        assert!(content.contains("\"saved\":75"));
        // record_lost lands in its OWN ledger — never in gain.jsonl,
        // where its before>after rows would inflate every savings sum.
        record_lost("test", "bypass_lost", "lab", Savings::new(200, 40));
        let lost = fs::read_to_string(dir.join(".tokenade").join("bypass_lost.jsonl")).unwrap();
        assert!(lost.contains("\"op\":\"bypass_lost\""));
        assert!(lost.contains("\"saved\":160"));
        let gain_again = fs::read_to_string(dir.join(".tokenade").join("gain.jsonl")).unwrap();
        assert!(
            !gain_again.contains("bypass_lost"),
            "lost row leaked into gain.jsonl"
        );
        // A baseline COMPUTED from the delivered output is stamped at write
        // time, without any of the thirteen callers of `coefficients` having
        // to think about it.
        record(
            "test",
            "cf_demo",
            "lab",
            crate::coefficients::web_search_credit_savings(1_000),
        );
        let content = fs::read_to_string(dir.join(".tokenade").join("gain.jsonl")).unwrap();
        let cf = content
            .lines()
            .find(|l| l.contains("\"op\":\"cf_demo\""))
            .expect("the counterfactual row is written");
        assert!(
            cf.contains("\"class\":\"counterfactual\""),
            "the fabricated baseline is not stamped: {cf}"
        );
        // And the stamp SURVIVES the anti-inflation safeguard: zeroing the
        // claim does not change where the baseline came from, and a row that
        // loses its class here becomes indistinguishable from a measurement.
        record(
            "test",
            "cf_inflated",
            "lab",
            Savings::new(10, 500).counterfactual(),
        );
        let content = fs::read_to_string(dir.join(".tokenade").join("gain.jsonl")).unwrap();
        let inflated = content
            .lines()
            .find(|l| l.contains("\"op\":\"cf_inflated\""))
            .expect("the inflated row is written");
        assert!(
            inflated.contains("\"class\":\"counterfactual\""),
            "the safeguard ate the stamp: {inflated}"
        );
        assert!(inflated.contains("\"saved\":0"), "{inflated}");
        // An ordinary measurement still carries no class.
        assert!(
            !content
                .lines()
                .find(|l| l.contains("\"op\":\"demo\""))
                .expect("the ordinary measurement")
                .contains("\"class\""),
            "a measured reading must not declare itself counterfactual"
        );
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe { std::env::remove_var("TOKENADE_FORCE_GAIN") };
        let _ = fs::remove_dir_all(&dir);
    }

    // Regression test for #480 lives in its own integration-test
    // binary (`tests/gain_record_redacts_label.rs`) because the
    // ROOT_OVERRIDE OnceLock is contended with `writes_jsonl_record`
    // above when both run in the same test binary — a separate
    // binary = fresh process = empty OnceLock + no parallel writer.

    #[test]
    fn rotation_keeps_only_in_window_records() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-gain-rot-{}-{}",
            std::process::id(),
            crate::platform::now_unix_secs()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("gain.jsonl");
        let now = crate::platform::now_unix_secs();
        let stale = now - 60 * 86_400; // 60 days ago
        let fresh = now - 86_400; // 1 day ago
        let content =
            format!("{{\"ts\":{stale},\"op\":\"old\"}}\n{{\"ts\":{fresh},\"op\":\"new\"}}\n");
        fs::write(&path, &content).unwrap();
        rotate_if_oversized(&path, ROTATION_WINDOW_SECS).unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.contains("\"op\":\"new\""));
        assert!(
            !after.contains("\"op\":\"old\""),
            "stale record not dropped"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A rotation that cannot fold its aged-out rows into the rollup must
    /// keep them: it used to delete them anyway, and they left every
    /// lifetime total for good.
    #[test]
    fn rotation_keeps_rows_it_could_not_fold_into_the_rollup() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-gain-rot-held-{}-{}",
            std::process::id(),
            crate::platform::now_unix_secs()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("gain.jsonl");
        let now = crate::platform::now_unix_secs();
        let stale = now - 60 * 86_400;
        fs::write(
            &path,
            format!("{{\"ts\":{stale},\"op\":\"old\",\"before\":900,\"after\":100}}\n{{\"ts\":{now},\"op\":\"new\"}}\n"),
        )
        .unwrap();
        let rollup = rollup_path_for(&path);
        let held = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(lock_path_for(&rollup))
            .unwrap();
        fs2::FileExt::lock_exclusive(&held).unwrap();
        rotate_if_oversized(&path, ROTATION_WINDOW_SECS).unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("\"op\":\"old\""),
            "a row the rollup did not take was deleted"
        );
        fs2::FileExt::unlock(&held).unwrap();
        rotate_if_oversized(&path, ROTATION_WINDOW_SECS).unwrap();
        assert!(!fs::read_to_string(&path)
            .unwrap()
            .contains("\"op\":\"old\""));
        assert!(fs::read_to_string(&rollup)
            .unwrap()
            .contains("\"saved\":800"));
        let _ = fs::remove_dir_all(&dir);
    }

    // #500 — `rotate_if_oversized` used to silently drop unparseable
    // lines (truncated by an interrupted write, or pre-schema records).
    // The fix treats parse failures as "in window" so we don't destroy
    // content we can't judge. This regression test pins the behaviour.
    #[test]
    fn rotation_preserves_unparseable_lines() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-gain-rot-malformed-{}-{}",
            std::process::id(),
            crate::platform::now_unix_secs()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("gain.jsonl");
        let now = crate::platform::now_unix_secs();
        let stale = now - 60 * 86_400; // 60 days ago — will get rotated
        let fresh = now - 86_400; // 1 day ago — kept
        let content = format!(
            "{{\"ts\":{stale},\"op\":\"stale-but-valid\"}}\n\
             {{partial write here — no closing brace\n\
             {{\"ts\":{fresh},\"op\":\"fresh\"}}\n"
        );
        fs::write(&path, &content).unwrap();
        rotate_if_oversized(&path, ROTATION_WINDOW_SECS).unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.contains("\"op\":\"fresh\""), "fresh row dropped");
        assert!(
            !after.contains("stale-but-valid"),
            "stale parseable row should be dropped by window check"
        );
        assert!(
            after.contains("partial write here"),
            "unparseable line silently destroyed — data loss regression"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_falls_back_when_all_records_are_fresh() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-gain-rot2-{}-{}",
            std::process::id(),
            crate::platform::now_unix_secs()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("gain.jsonl");
        let now = crate::platform::now_unix_secs();
        let mut content = String::new();
        for i in 0..100 {
            let _ = writeln!(content, "{{\"ts\":{},\"op\":\"x{i}\"}}", now - i as u64);
        }
        fs::write(&path, &content).unwrap();
        // Everything is in window → fall back to dropping oldest 25 %.
        rotate_if_oversized(&path, ROTATION_WINDOW_SECS).unwrap();
        let after = fs::read_to_string(&path).unwrap();
        let kept = after.lines().count();
        assert!(
            (70..=80).contains(&kept),
            "expected ~75 records after fallback drop, got {kept}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// #1035 — concurrency lock-in: spawn N threads each appending to
    /// the file, then call rotate_if_oversized in parallel from another
    /// thread that races with the writers. With the pre-#1035 read-
    /// then-write-without-lock, records appended during the rotation's
    /// read→rename window were silently lost. With the cross-process
    /// flock now held during read→write→rename, every appended record
    /// must survive.
    ///
    /// Direct flock on the file under test (not record() — which uses
    /// jsonl_path() backed by ROOT_OVERRIDE, single-set so we can't
    /// retarget per-test). The lock contract is the same as the one
    /// `record()` uses (open + lock_exclusive on the gain.jsonl path),
    /// so the test exercises the same lock the production write path
    /// respects.
    #[cfg(unix)]
    #[test]
    fn rotate_under_concurrent_writes_loses_no_records() {
        use fs2::FileExt;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let dir = std::env::temp_dir().join(format!(
            "tokenade-gain-race-{}-{}",
            std::process::id(),
            crate::platform::now_unix_secs()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("gain.jsonl");
        // Seed the file with enough content that rotation actually
        // has work (rotate_if_oversized's fallback drops 25 % of >4
        // lines; we want it to NOT early-exit).
        let now = crate::platform::now_unix_secs();
        let mut seed = String::new();
        for i in 0..50 {
            let _ = writeln!(
                seed,
                "{{\"ts\":{},\"op\":\"seed{i}\",\"saved\":1}}",
                now - i as u64
            );
        }
        fs::write(&path, &seed).unwrap();
        let baseline_lines = 50usize;

        let path = Arc::new(path);
        let done = Arc::new(AtomicBool::new(false));
        let writer_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // 4 writer threads, each appending records protected by the
        // same lock that record() takes — the SIDECAR, not the ledger
        // (`lock_path_for`). Emulating record() against the wrong object is
        // emulating nothing: the writers and the rotator would stop excluding
        // each other and this test would report the torn writes it exists to
        // forbid. They keep appending until the rotator signals done.
        let mut writers = Vec::new();
        for tid in 0..4 {
            let p = Arc::clone(&path);
            let d = Arc::clone(&done);
            let wc = Arc::clone(&writer_count);
            writers.push(std::thread::spawn(move || {
                while !d.load(Ordering::Relaxed) {
                    let lock = fs::OpenOptions::new()
                        .create(true)
                        .read(true)
                        .append(true)
                        .open(super::lock_path_for(&p));
                    if let (Ok(lock), Ok(f)) = (
                        lock,
                        fs::OpenOptions::new()
                            .create(true)
                            .read(true)
                            .append(true)
                            .open(&*p),
                    ) {
                        let _ = lock.lock_exclusive();
                        let mut f = f;
                        let n = wc.fetch_add(1, Ordering::Relaxed);
                        let _ = writeln!(
                            f,
                            "{{\"ts\":{},\"op\":\"w{tid}-{n}\",\"saved\":1}}",
                            crate::platform::now_unix_secs(),
                        );
                        let _ = FileExt::unlock(&lock);
                    }
                    std::thread::yield_now();
                }
            }));
        }

        // Rotator thread: triggers rotation 50× while writers race.
        // With the fix, rotate_if_oversized holds the flock for its
        // entire read-write-rename cycle, so writers serialise behind
        // it and no record vanishes.
        let p = Arc::clone(&path);
        let rotator = std::thread::spawn(move || {
            for _ in 0..50 {
                // Use a very wide window so EVERY record is "in window"
                // — this exercises the 25 %-drop fallback path which
                // actively rewrites the file (the most race-prone arm).
                let _ = rotate_if_oversized(&p, u64::MAX);
                std::thread::yield_now();
            }
        });

        rotator.join().unwrap();
        done.store(true, Ordering::Relaxed);
        for w in writers {
            w.join().unwrap();
        }

        // Count what survived. Each rotator iteration drops 25 % of
        // the lines present at read time, so the absolute count is
        // not predictable — but the test isn't asserting a count.
        // It asserts an INVARIANT: every w{tid}-{n} that was emitted
        // either:
        //   (a) survived in the file (we see it now), OR
        //   (b) was correctly dropped by the 25%-fallback rotation
        //       (so its `n` is among the lowest 25% emitted at some
        //       rotation point).
        //
        // The strict invariant we CAN check: the file is still
        // parseable line-per-line JSON (no torn writes from
        // race-induced interleaving) and contains at least the
        // baseline_lines/4 newest records (the rotation can't have
        // dropped more than 25% per pass, and only 50 passes ran).
        // The pre-#1035 bug manifests as JSON parse failures from
        // interleaved partial writes — the post-fix file is always
        // line-clean.
        let final_content = fs::read_to_string(&*path).unwrap();
        let lines: Vec<&str> = final_content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        assert!(!lines.is_empty(), "rotation must not have emptied the file");
        for (i, line) in lines.iter().enumerate() {
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(line);
            assert!(
                parsed.is_ok(),
                "race-induced torn write at line {i}: {line}"
            );
        }
        // Sanity: total writes happened (at least one per writer).
        let total_writes = writer_count.load(Ordering::Relaxed);
        assert!(
            total_writes >= 4,
            "writers should have produced ≥4 records (got {total_writes})"
        );
        // baseline_lines reference: ensure we exercised a non-trivial
        // amount of churn (file isn't just the seed).
        assert!(
            total_writes >= baseline_lines / 10,
            "too few writes to meaningfully exercise the race \
             (writes={total_writes}, baseline={baseline_lines})"
        );
        let _ = fs::remove_dir_all(&*path);
    }

    // #1042 — THE core invariant of the rollup fix: the lifetime gross
    // saved total must NOT change when stale rows rotate out of the live
    // ledger. Pre-fix, rotation deleted aged-out rows and `lifetime_saved`
    // read only the live ledger, so every rotation (and every historical
    // `.bak` reset) silently dropped the all-time figure — the recurring
    // "my saved tokens reset to ~0" the user reported. Post-fix the dropped
    // rows are folded into the durable rollup and `ledger + rollup` is
    // conserved across rotation.
    /// A rotation must not make what it folds ANONYMOUS.
    ///
    /// `label` becomes `(rolled-up)` and `cmd` disappears, so nothing said any
    /// more WHICH compactor of a family had delivered those bytes.
    /// MEASURED 2026-09-07: 44 411 043 of the 52 690 500 bytes still delivered
    /// — 84% — were in anonymous buckets, and the value ranking that decides
    /// where to work was blind exactly where there is the most to gain. The
    /// name enters the bucket key and comes back out under `compactor`.
    #[test]
    fn a_rotation_keeps_the_name_of_the_compactor_it_folds() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-gain-rollup-name-{}-{}",
            std::process::id(),
            crate::platform::now_unix_secs()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("gain.jsonl");
        let now = crate::platform::now_unix_secs();
        let stale = now - 60 * 86_400;
        // Two compactors, SAME op and SAME day: exactly the case the nameless
        // key crushed into a single bucket.
        let content = format!(
            "{{\"ts\":{stale},\"op\":\"auto-compact\",\"label\":\"auto:ini\",\"before\":100,\"after\":60,\"saved\":40}}\n\
             {{\"ts\":{stale},\"op\":\"auto-compact\",\"label\":\"auto:json\",\"before\":900,\"after\":90,\"saved\":810}}\n\
             {{\"ts\":{stale},\"op\":\"npm\",\"cmd\":\"pnpm\",\"label\":\"Bash: pnpm run ci\",\"before\":500,\"after\":100,\"saved\":400}}\n"
        );
        fs::write(&path, &content).unwrap();
        rotate_if_oversized(&path, ROTATION_WINDOW_SECS).unwrap();
        let rollup = fs::read_to_string(rollup_path_for(&path)).unwrap();
        let rows: Vec<serde_json::Value> = rollup
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let named: Vec<&str> = rows
            .iter()
            .filter_map(|r| r["compactor"].as_str())
            .collect();
        assert!(
            named.contains(&"auto:ini") && named.contains(&"auto:json"),
            "two compactors of one family merged into one bucket: {named:?}"
        );
        // A row born from a COMMAND carries its program under `cmd`: that is
        // the name, not the label repeating the command line.
        assert!(
            named.contains(&"pnpm"),
            "the program of a command row must survive: {named:?}"
        );
        // The sentinel stays where it was: no reader of `label`
        // changes behaviour.
        assert!(
            rows.iter()
                .all(|r| r["label"].as_str() == Some("(rolled-up)")),
            "`label` must stay the sentinel"
        );
        // Each bucket keeps ITS bytes, separately.
        let ini = rows
            .iter()
            .find(|r| r["compactor"].as_str() == Some("auto:ini"))
            .expect("auto:ini");
        assert_eq!(ini["after"].as_u64(), Some(60));
        assert_eq!(ini["before"].as_u64(), Some(100));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollup_conserves_lifetime_across_rotation() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-gain-rollup-inv-{}-{}",
            std::process::id(),
            crate::platform::now_unix_secs()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("gain.jsonl");
        let now = crate::platform::now_unix_secs();
        let stale1 = now - 60 * 86_400; // aged out
        let stale2 = now - 59 * 86_400; // aged out, different UTC day
        let fresh = now - 86_400; // kept in ledger
        let content = format!(
            "{{\"ts\":{stale1},\"op\":\"a\",\"before\":100,\"after\":25,\"saved\":75}}\n\
             {{\"ts\":{stale2},\"op\":\"a\",\"before\":200,\"after\":50,\"saved\":150}}\n\
             {{\"ts\":{fresh},\"op\":\"b\",\"before\":10,\"after\":4,\"saved\":6}}\n"
        );
        fs::write(&path, &content).unwrap();
        let before_total = sum_saved_in_file(&path).unwrap();
        assert_eq!(before_total, 231);

        rotate_if_oversized(&path, ROTATION_WINDOW_SECS).unwrap();

        let ledger_after = sum_saved_in_file(&path).unwrap();
        let rollup_after = sum_saved_in_file(&rollup_path_for(&path)).unwrap_or(0);
        assert_eq!(
            ledger_after + rollup_after,
            before_total,
            "lifetime saved changed across rotation — the reset bug is back"
        );
        // The fresh row stays measured per-row; the stale ones live in the rollup.
        assert_eq!(
            ledger_after, 6,
            "fresh row should remain in the live ledger"
        );
        assert_eq!(rollup_after, 225, "stale savings should land in the rollup");
        let rollup = fs::read_to_string(rollup_path_for(&path)).unwrap();
        assert!(
            rollup.contains("\"rollup\":true"),
            "rollup rows must be tagged"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // Fold must collapse rows that share a (UTC-day, op) bucket, summing
    // before/after and counting cardinality in `rollup_calls`; distinct ops
    // stay separate. This is what lets the dashboard's per-day/op slicing
    // and call tallies stay correct for rotated-out history.
    #[test]
    fn fold_collapses_same_day_op_and_counts_calls() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-gain-fold-{}-{}",
            std::process::id(),
            crate::platform::now_unix_secs()
        ));
        let _ = fs::create_dir_all(&dir);
        let rollup = dir.join("gain.rollup.jsonl");
        let day = (crate::platform::now_unix_secs() / 86_400) * 86_400;
        let lines = vec![
            format!(
                "{{\"ts\":{},\"op\":\"x\",\"before\":100,\"after\":40}}",
                day + 10
            ),
            format!(
                "{{\"ts\":{},\"op\":\"x\",\"before\":50,\"after\":20}}",
                day + 20
            ),
            format!(
                "{{\"ts\":{},\"op\":\"y\",\"before\":30,\"after\":10}}",
                day + 30
            ),
        ];
        fold_lines_into_rollup(&lines, &rollup);
        let body = fs::read_to_string(&rollup).unwrap();
        let rows: Vec<serde_json::Value> = body
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(rows.len(), 2, "two ops → two buckets");
        let x = rows.iter().find(|r| r["op"] == "x").unwrap();
        assert_eq!(x["before"], 150);
        assert_eq!(x["after"], 60);
        assert_eq!(x["saved"], 90);
        assert_eq!(x["rollup_calls"], 2, "two x rows collapsed");
        let y = rows.iter().find(|r| r["op"] == "y").unwrap();
        assert_eq!(y["saved"], 20);
        assert_eq!(y["rollup_calls"], 1);

        // Folding again must be ADDITIVE (merges with the existing rollup),
        // honouring the pre-aggregated rollup_calls rather than resetting it.
        fold_lines_into_rollup(
            &[format!(
                "{{\"ts\":{},\"op\":\"x\",\"before\":10,\"after\":3}}",
                day + 40
            )],
            &rollup,
        );
        let body2 = fs::read_to_string(&rollup).unwrap();
        let x2: serde_json::Value = body2
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .find(|r| r["op"] == "x")
            .unwrap();
        assert_eq!(x2["before"], 160);
        assert_eq!(x2["rollup_calls"], 3, "additive across fold calls");
        let _ = fs::remove_dir_all(&dir);
    }

    // Migration ingests pre-existing `.bak`/`.pre*` backups into the rollup,
    // dedups byte-identical rows across overlapping snapshots (never double-
    // counting), renames consumed backups `.counted`, and is idempotent.
    #[test]
    fn migration_folds_backups_dedups_and_is_idempotent() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-gain-migrate-{}-{}",
            std::process::id(),
            crate::platform::now_unix_secs()
        ));
        let _ = fs::create_dir_all(&dir);
        let ledger = dir.join("gain.jsonl");
        let day = (crate::platform::now_unix_secs() / 86_400) * 86_400;
        let row_x =
            format!("{{\"ts\":{day},\"op\":\"x\",\"before\":100,\"after\":25,\"saved\":75}}");
        let row_y =
            format!("{{\"ts\":{day},\"op\":\"y\",\"before\":200,\"after\":50,\"saved\":150}}");
        fs::write(dir.join("gain.jsonl.bak-100"), format!("{row_x}\n")).unwrap();
        fs::write(dir.join("gain.jsonl.pre740reset"), format!("{row_y}\n")).unwrap();
        // Overlapping snapshot: byte-identical to bak-100 → must be deduped.
        fs::write(dir.join("gain.jsonl.bak-200"), format!("{row_x}\n")).unwrap();

        let ingested = migrate_backups_for(&ledger);
        assert_eq!(ingested, 2, "3 rows but one is a duplicate → 2 unique");
        assert_eq!(
            sum_saved_in_file(&rollup_path_for(&ledger)).unwrap(),
            225,
            "75 + 150, duplicate not counted twice"
        );
        assert!(dir.join("gain.jsonl.bak-100.counted").exists());
        assert!(!dir.join("gain.jsonl.bak-100").exists());

        // Second run: nothing left to ingest, total unchanged.
        let again = migrate_backups_for(&ledger);
        assert_eq!(again, 0, "migration must be idempotent");
        assert_eq!(
            sum_saved_in_file(&rollup_path_for(&ledger)).unwrap(),
            225,
            "re-running migration must not double-count"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The audit must NAME the double count, because the per-row invariants
    /// cannot see it: both copies of a duplicated row are individually
    /// well-formed. Reported, not repaired — rewriting accounting history is a
    /// human decision.
    #[test]
    fn the_audit_reports_rows_present_in_both_ledger_and_folded_backup() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-gain-overlap-{}-{}",
            std::process::id(),
            crate::platform::now_unix_secs()
        ));
        let _ = fs::create_dir_all(&dir);
        let ledger = dir.join("gain.jsonl");
        let day = (crate::platform::now_unix_secs() / 86_400) * 86_400;
        let dup =
            format!("{{\"ts\":{day},\"op\":\"dup\",\"before\":100,\"after\":25,\"saved\":75}}");
        let only_live =
            format!("{{\"ts\":{day},\"op\":\"solo\",\"before\":10,\"after\":4,\"saved\":6}}");
        let only_bak =
            format!("{{\"ts\":{day},\"op\":\"gone\",\"before\":20,\"after\":8,\"saved\":12}}");
        fs::write(&ledger, format!("{dup}\n{only_live}\n")).unwrap();
        fs::write(
            dir.join("gain.jsonl.prepurge-1.counted"),
            format!("{dup}\n{only_bak}\n"),
        )
        .unwrap();

        let out = ingested_backup_overlap(&ledger);
        assert_eq!(out.len(), 1, "one line naming the overlap");
        assert!(out[0].contains("1 rows"), "only `dup` overlaps: {}", out[0]);
        assert!(out[0].contains("75 tokens"), "its saving: {}", out[0]);

        // A backup that has NOT been folded yet (no `.counted`) is not a
        // double count — migration will skip its live rows now.
        fs::rename(
            dir.join("gain.jsonl.prepurge-1.counted"),
            dir.join("gain.jsonl.prepurge-1"),
        )
        .unwrap();
        assert!(
            ingested_backup_overlap(&ledger).is_empty(),
            "an un-folded backup is not yet counted anywhere"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A backup is a SNAPSHOT of the live ledger, not an archive of rows that
    /// left it. Rows still sitting in `gain.jsonl` must not also be folded into
    /// the rollup, because the dashboard reads `ledger + rollup`.
    ///
    /// The test above never wrote a live ledger at all, which is why this went
    /// unnoticed. Measured on a dogfood install: 29 216 rows in both a
    /// `.counted` backup and the live file, 199 194 952 tokens counted twice —
    /// +93 % on the lifetime figure.
    #[test]
    fn migration_ignores_rows_the_live_ledger_still_holds() {
        let dir = std::env::temp_dir().join(format!(
            "tokenade-gain-migrate-live-{}-{}",
            std::process::id(),
            crate::platform::now_unix_secs()
        ));
        let _ = fs::create_dir_all(&dir);
        let ledger = dir.join("gain.jsonl");
        let day = (crate::platform::now_unix_secs() / 86_400) * 86_400;
        let still_live =
            format!("{{\"ts\":{day},\"op\":\"live\",\"before\":100,\"after\":25,\"saved\":75}}");
        let aged_out =
            format!("{{\"ts\":{day},\"op\":\"gone\",\"before\":200,\"after\":50,\"saved\":150}}");
        fs::write(&ledger, format!("{still_live}\n")).unwrap();
        // The snapshot holds both: one row that is still in the ledger and one
        // that a rotation has since removed from it.
        fs::write(
            dir.join("gain.jsonl.prepurge-1"),
            format!("{still_live}\n{aged_out}\n"),
        )
        .unwrap();

        let ingested = migrate_backups_for(&ledger);
        assert_eq!(ingested, 1, "only the row the ledger no longer holds");
        assert_eq!(
            sum_saved_in_file(&rollup_path_for(&ledger)).unwrap(),
            150,
            "folding the still-live row would make ledger+rollup report 75 twice"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
#[cfg(test)]
mod spill_tests {
    use super::*;

    /// A cost the human view subtracts must be a cost the machine view
    /// subtracts. MEASURED 2026-08-04 before the formula was shared: 923 844 942
    /// on the terminal against 975 053 822 in `--json`, same install, same
    /// window.
    #[test]
    fn the_net_figure_subtracts_every_overhead_it_names() {
        let est = EstimatedSummary {
            output: 1_000,
            batching: 100,
            compounding: 500,
            overhead: 40,
            overhead_cache: 5_000,
            recovery: 0,
            weighted: None,
        };
        let net = net_tokens(10_000, 200, &est);
        // 10 000 + (1 000 + 100 + 500) + 200 − 40 − 5 000
        assert_eq!(
            net, 6_760,
            "an overhead the dashboard prints is missing from net"
        );
        // Dropping the cache re-bill is the exact regression this pins.
        let without = EstimatedSummary {
            overhead_cache: 0,
            ..est
        };
        assert!(
            net_tokens(10_000, 200, &without) > net,
            "the cache re-bill is not affecting net at all"
        );
    }

    /// The aggregator exists because nine copies of these rules drifted. The
    /// rules themselves are what must not drift, so they are pinned here rather
    /// than only through whichever surface happens to call it.
    /// The write gate and the pruner must answer the same question the same
    /// way: a row worth writing is a row worth keeping. When these were four
    /// copies, the `wrap` copy dropped code-intel rows that every other path
    /// kept.
    #[test]
    fn one_gate_decides_both_what_is_written_and_what_survives_a_prune() {
        // Noise: under both the absolute and the percentage floor.
        assert!(!worth_recording(100, 95, "git_status"), "5 tokens is noise");
        assert!(!worth_recording(1000, 985, "git_status"), "1.5% is noise");
        // Worth it: clears both floors.
        assert!(worth_recording(1000, 500, "git_status"));
        // Code-intel is exempt: its saving cannot be measured directly, so a
        // row that recorded no delta is still the only trace that it ran.
        assert!(worth_recording(500, 500, "semantic_search"));
        assert!(worth_recording(500, 500, "semantic_search:file"));
        // A degenerate row must not divide by zero into a pass.
        assert!(!worth_recording(0, 0, "git_status"));
    }

    #[test]
    fn the_local_credit_key_is_the_servers_key_plus_two_fields() {
        // The safety argument is a DIRECTION: the local key may split groups
        // the server keeps together, never the reverse, so the local figure
        // can only sit at or above the metered one. That used to be asserted
        // in prose, in three places, and all three went stale the day the
        // local key gained the label and the session. Here it is structural.
        let row = serde_json::json!({
            "ts": 1788500000u64, "op": "kubectl", "before": 9198u64, "after": 6674u64,
            "label": "kubectl get pods", "sess": "s1",
        });
        let (ts, op, before, after) = server_dedup_key(&row);
        let full = credit_key(&row);
        assert_eq!(
            (full.0, full.1.clone(), full.2, full.3),
            (ts, op, before, after),
            "the local key no longer contains the server's key, so it can no longer \
             be shown to be finer"
        );

        // Two rows the server cannot tell apart, and the local key can.
        let mut other = row.clone();
        other["label"] = serde_json::Value::from("kubectl get po -A");
        assert_eq!(
            server_dedup_key(&row),
            server_dedup_key(&other),
            "the fixture does not actually collide on the server's key"
        );
        assert_ne!(
            credit_key(&row),
            credit_key(&other),
            "the local key collapses two distinct commands"
        );

        // And the legacy spelling resolves to the same op on both.
        let legacy = serde_json::json!({
            "ts": 1788500000u64, "filter": "kubectl", "before": 9198u64, "after": 6674u64,
            "label": "kubectl get pods", "sess": "s1",
        });
        assert_eq!(
            server_dedup_key(&row),
            server_dedup_key(&legacy),
            "a legacy row keys differently from its modern twin"
        );
    }

    #[test]
    fn the_scan_applies_every_rule_the_surfaces_used_to_re_implement() {
        let rows = [
            // A plain measured row: one call unless it says otherwise.
            r#"{"op":"git-status","before":1000,"after":100,"ts":900,"project":"/p"}"#,
            // A rollup row speaks for many calls.
            r#"{"op":"git-status","before":500,"after":50,"rollup_calls":7,"ts":900,"project":"/p"}"#,
            // Code-intel NEVER enters the measured totals (#735) …
            r#"{"op":"symbol_find","before":900,"after":100,"ts":900}"#,
            // … and a flat one is expanded by the heuristic (#544).
            r#"{"op":"symbol_find","before":80,"after":80,"ts":900}"#,
            // An old row labels its compactor `filter`, not `op`.
            r#"{"filter":"legacy-op","before":300,"after":30,"ts":900}"#,
            // Out of window.
            r#"{"op":"git-status","before":9999,"after":1,"ts":10}"#,
        ]
        .join("\n");

        let scan = scan_ledger(&rows, 500);
        assert_eq!(
            (scan.measured_before, scan.measured_after),
            (1800, 180),
            "measured totals took a code-intel row, or kept an out-of-window one"
        );
        assert_eq!(
            scan.calls, 9,
            "rollup_calls not honoured (1 + 7 + 1 legacy)"
        );
        assert_eq!(
            scan.per_op.get("legacy-op").map(|t| t.0),
            Some(300),
            "a row labelled `filter` lost its op"
        );
        assert_eq!(
            scan.per_op.get("git-status"),
            Some(&(1500, 150, 8)),
            "per-op merged or dropped a measured row"
        );
        // Both code-intel rows are credited, the flat one via the heuristic.
        let ci = scan
            .code_intel
            .get("symbol_find")
            .expect("no code-intel bucket");
        assert_eq!(ci.0, 2, "code-intel calls lost");
        assert!(
            ci.1 > 800,
            "the flat code-intel row was not expanded by the heuristic: {}",
            ci.1
        );
        assert_eq!(
            scan.code_intel_saved, ci.1,
            "the code-intel total disagrees with its bucket"
        );

        // Window 0 means everything.
        let all = scan_ledger(&rows, 0);
        assert!(
            all.measured_before > scan.measured_before,
            "a window of 0 must keep the row the cutoff excluded"
        );
    }

    /// `lifetime_saved` is cumulative BY DEFINITION: the dashboard prints it
    /// beside the windowed total, and a window must not narrow it. `recent_saved`
    /// is the "last 7 days" line, measured rows only.
    #[test]
    fn the_scan_keeps_a_cumulative_figure_a_window_cannot_narrow() {
        let rows = [
            r#"{"op":"git-status","before":1000,"after":100,"ts":900}"#,
            r#"{"op":"git-status","before":600,"after":200,"ts":10}"#,
            r#"{"op":"symbol_find","before":900,"after":100,"ts":900}"#,
        ]
        .join("\n");
        let scan = scan_ledger_with(&rows, 500, 800, |_, _| {});
        assert_eq!(
            scan.measured_saved(),
            900,
            "the window failed to exclude the old row from the measured total"
        );
        assert_eq!(
            scan.lifetime_saved, 2100,
            "the cumulative figure was narrowed by the window (900 + 400 + 800)"
        );
        assert_eq!(
            scan.recent_saved, 900,
            "`recent` took a code-intel row or an out-of-window one"
        );
        // The hook sees every row, and is told which side of the window it fell.
        let mut seen = (0u32, 0u32);
        let _ = scan_ledger_with(&rows, 500, 0, |_, in_window| {
            if in_window {
                seen.0 += 1
            } else {
                seen.1 += 1
            }
        });
        assert_eq!(
            seen,
            (2, 1),
            "the row hook did not see every row with its window flag"
        );
    }
}
#[cfg(test)]
mod the_rows_model_wins {
    /// **The local figure and the server-side figure attribute the same row
    /// to the same model.** The server never re-infers the model of an event
    /// that carries one; local valuation did the opposite — it asked the
    /// transcript attributor, which at best answers a session's dominant
    /// model. Two models for the same tokens, and a gap between $5 and $15
    /// per Mtok left unexplained.
    #[test]
    fn the_rows_own_model_beats_the_transcript_guess() {
        // `ScratchHome` moves HOME *and* USERPROFILE under the env lock, and
        // restores both. Setting `HOME` alone redirects NOTHING on Windows —
        // the lesson has been written in its own doc since 1.1.7, and this
        // test paid for it again on the 1.1.12 Windows build: it read the
        // build machine's real profile.
        let home = crate::platform::ScratchHome::new("row-model");
        std::fs::create_dir_all(home.path().join(".tokenade")).unwrap();
        std::fs::write(
            super::current_ledger_path(),
            [
                r#"{"ts":1789000000,"source":"wrap","op":"read","sess":"s1","before":100000,"after":10,"saved":99990,"model":"claude-haiku-4-5"}"#,
                r#"{"ts":1789000001,"source":"wrap","op":"read","sess":"s1","before":100000,"after":10,"saved":99990}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let rs = super::real_savings_compute_with(None);
        let named: Vec<_> = rs
            .by_model
            .iter()
            .map(|(m, _, _, _, _)| m.as_str())
            .collect();
        assert!(
            named.contains(&"claude-haiku-4-5"),
            "the row named its model and valuation ignored it: {named:?}"
        );
        // The silent row stays open to inference; without a transcript it
        // attributes nothing, and an empty model is valued like Sonnet.
        assert!(named.contains(&""), "{named:?}");
    }
}

#[cfg(test)]
mod delivered_only_once {
    use super::*;

    fn row(
        op: &str,
        sess: &str,
        label: &str,
        ts: u64,
        before: u64,
        after: u64,
    ) -> serde_json::Value {
        serde_json::json!({"op": op, "sess": sess, "label": label, "ts": ts,
            "source": "wrap", "before": before, "after": after, "saved": before - after})
    }

    /// A call the stash (or a dedup) served writes TWO rows: the compactor's
    /// output was never delivered. Measured 2026-10-02: 218 013 phantom
    /// tokens, 168 378 of them for `auto:json`.
    #[test]
    fn the_row_replaced_by_the_next_stage_delivers_nothing() {
        let mut rows = vec![
            row("auto:json", "s1", "api read", 100, 118_532, 108_078),
            row("disclose:stash", "s1", "api read", 101, 86_995, 627),
            // same label, other session: another call
            row("auto:json", "s2", "api read", 100, 5_000, 4_000),
            // same session and label, too far apart: another call
            row("auto:json", "s1", "api read", 110, 5_000, 4_000),
            // exec_dedup and cmd_dedup replace too
            row("generic", "s1", "poll", 200, 900, 800),
            row("exec_dedup", "s1", "poll", 200, 800, 30),
        ];
        assert_eq!(mark_superseded(&mut rows), 2);
        let delivered: Vec<u64> = rows.iter().map(delivered_of).collect();
        assert_eq!(delivered, vec![0, 627, 4_000, 4_000, 0, 30]);
        // what goes to the server does not move
        assert_eq!(rows[0]["after"], 108_078);
        assert_eq!(rows[0]["saved"], 10_454);
    }

    #[test]
    fn the_rollup_keeps_delivered_without_touching_after_or_saved() {
        let dir = std::env::temp_dir().join(format!("tk-delivered-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rollup = dir.join("gain.rollup.jsonl");
        let lines: Vec<String> = [
            row("auto:json", "s1", "x", 100, 1_000, 900),
            row("disclose:stash", "s1", "x", 100, 900, 50),
            row("auto:json", "s1", "y", 300, 1_000, 400),
        ]
        .iter()
        .map(|v| v.to_string())
        .collect();
        fold_lines_into_rollup(&lines, &rollup);
        let text = std::fs::read_to_string(&rollup).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let buckets: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .filter(|v| v["op"] == "auto:json")
            .collect();
        let sum = |f: &dyn Fn(&serde_json::Value) -> u64| buckets.iter().map(f).sum::<u64>();
        assert_eq!(sum(&|v| v["after"].as_u64().unwrap()), 1_300, "{text}");
        assert_eq!(sum(&|v| v["saved"].as_u64().unwrap()), 700, "{text}");
        assert_eq!(sum(&delivered_of), 400, "{text}");
    }
}

#[cfg(test)]
mod in_billed_tokens {
    /// **A ledger token is an ESTIMATED token (cl100k); a dollar is paid in
    /// the model's tokens.** Valuation converts — except `output_lean`, read
    /// from the transcript's billed `output_tokens`, already in the right
    /// unit.
    #[test]
    fn a_ledger_token_is_priced_in_the_model_s_own_units() {
        let home = crate::platform::ScratchHome::new("billed-units");
        std::fs::create_dir_all(home.path().join(".tokenade")).unwrap();
        std::fs::write(
            super::current_ledger_path(),
            r#"{"ts":1789000000,"source":"wrap","op":"read","before":1010,"after":10,"saved":1000,"model":"claude-opus-5-5"}"#,
        )
        .unwrap();
        std::fs::write(
            super::estimated_ledger_path(),
            r#"{"ts":1789000001,"source":"hookwrap-userprompt","op":"output_lean","class":"output","estimated":true,"before":1000,"after":0,"saved":1000,"model":"claude-opus-5-5"}"#,
        )
        .unwrap();
        let rs = super::real_savings_compute_with(None);
        assert_eq!(rs.input, 1_600, "the estimated token was not converted");
        assert_eq!(rs.output, 1_000, "output_lean is already in billed tokens");
    }

    /// The context at fire time is read from the transcript's `usage`:
    /// billed, not converted; the results term, estimated, is.
    #[test]
    fn the_context_of_a_saved_turn_is_already_billed() {
        let home = crate::platform::ScratchHome::new("billed-context");
        std::fs::create_dir_all(home.path().join(".tokenade")).unwrap();
        std::fs::write(
            super::estimated_ledger_path(),
            concat!(
                r#"{"ts":1789000001,"source":"web","op":"turns_context","label":"batch","class":"batching","io":"cache","estimated":true,"before":1000,"after":0,"saved":1000,"model":"claude-opus-5-5"}"#,
                "\n",
                r#"{"ts":1789000002,"source":"web","op":"batching","label":"batch","class":"batching","io":"cache","estimated":true,"before":100,"after":0,"saved":100,"model":"claude-opus-5-5"}"#,
            ),
        )
        .unwrap();
        let rs = super::real_savings_compute_with(None);
        assert_eq!(
            rs.cache, 1_160,
            "turns_context 1 000 as is + batching 100 × 1.6"
        ); // A recovery turn, read the same way: a billed cost, not converted.
        let turn = r#"{"ts":1789000003,"source":"cli","op":"recovery_turn","label":"expand-ref","class":"overhead","io":"cache","estimated":true,"before":500,"after":0,"saved":500,"model":"claude-opus-5-5"}"#;
        let ledger = std::fs::read_to_string(super::estimated_ledger_path()).unwrap() + "\n" + turn;
        std::fs::write(super::estimated_ledger_path(), ledger).unwrap();
        assert_eq!(
            super::real_savings_compute_with(None).cache,
            1_160 - 500,
            "recovery_turn 500 as is"
        );
    }
}

#[cfg(test)]
mod rebill_bound {
    /// A session cannot be re-billed more than 12× what it injected: the
    /// pre-1.1.21 rows that reached 429× are read down to the bound; a session
    /// within it keeps every row.
    #[test]
    fn a_session_s_rebill_is_bounded_by_what_it_injected() {
        let home = crate::platform::ScratchHome::new("rebill-bound");
        std::fs::create_dir_all(home.path().join(".tokenade")).unwrap();
        let row = |ts: u64, sess: &str, op: &str, class: &str, io: &str, n: u64| {
            format!(
                r#"{{"ts":{ts},"source":"hookwrap-userprompt","op":"{op}","class":"{class}","io":"{io}","estimated":true,"sess":"{sess}","before":{n},"after":0,"saved":{n},"model":"claude-opus-5-5"}}"#
            )
        };
        let mut rows = vec![
            row(
                1789000000,
                "a",
                "injected_context",
                "overhead",
                "input",
                1_000,
            ),
            row(
                1789000000,
                "b",
                "injected_context",
                "overhead",
                "input",
                1_000,
            ),
            row(1789000000, "c", "batching", "batching", "cache", 100_000)
                .replace("hookwrap-userprompt", "web"),
        ];
        // `a`: 40 turns re-billed at 1 000 — 40× its injection.
        rows.extend((1..=40).map(|i| {
            row(
                1789000000 + i,
                "a",
                "overhead_rebill",
                "overhead",
                "cache",
                1_000,
            )
        }));
        // `b`: 5 turns — within the bound.
        rows.extend((1..=5).map(|i| {
            row(
                1789000000 + i,
                "b",
                "overhead_rebill",
                "overhead",
                "cache",
                1_000,
            )
        }));
        std::fs::write(super::estimated_ledger_path(), rows.join("\n")).unwrap();
        let rs = super::real_savings_compute_with(None);
        // Estimated units, priced at 1.6 billed tokens each.
        assert_eq!(rs.cache, (100_000 - 12_000 - 5_000) * 16 / 10);
        // Every surface reading the ledger applies the same bound: the
        // dashboard summaries too, windowed or not.
        assert_eq!(super::estimated_summary().overhead_cache, 12_000 + 5_000);
        assert_eq!(
            super::estimated_summary_windowed(1789000005).overhead_cache,
            8_000 + 1_000
        );
        let by_day: u64 = super::estimated_by_day()
            .values()
            .map(|d| d.overhead_cache)
            .sum();
        assert_eq!(by_day, 17_000);
        // Rolled-up rows (no `sess`) are bounded together by the injections
        // that carry none either.
        let rolled = |op: &str, n: u64| {
            format!(
                r#"{{"ts":1788000000,"source":"rollup","op":"{op}","class":"overhead","io":"cache","estimated":true,"rollup":true,"before":{n},"after":0,"saved":{n}}}"#
            )
        };
        std::fs::write(
            super::estimated_ledger_path(),
            [
                rolled("injected_context", 1_000).replace(r#""io":"cache""#, r#""io":"input""#),
                rolled("overhead_rebill", 50_000),
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(super::estimated_summary().overhead_cache, 12_000);
        // A row counted call by call up to the real compaction is already
        // bounded: kept whole, before and after the rollup.
        let exact = row(
            1789000000,
            "e",
            "overhead_rebill",
            "overhead",
            "cache",
            300_000,
        )
        .replace(r#""op""#, r#""label":"exact","op""#);
        std::fs::write(
            super::estimated_ledger_path(),
            [
                row(
                    1789000000,
                    "e",
                    "injected_context",
                    "overhead",
                    "input",
                    1_000,
                ),
                exact,
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(super::estimated_summary().overhead_cache, 300_000);
        // Likewise a row priced by the user-turn proxy, bounded by its horizon.
        let proxied = row(
            1789000000,
            "e",
            "overhead_rebill",
            "overhead",
            "cache",
            300_000,
        )
        .replace(r#""op""#, r#""label":"proxy","op""#);
        std::fs::write(
            super::estimated_ledger_path(),
            [
                row(
                    1789000000,
                    "e",
                    "injected_context",
                    "overhead",
                    "input",
                    1_000,
                ),
                proxied,
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(super::estimated_summary().overhead_cache, 300_000);
        let rolled_exact =
            rolled("overhead_rebill", 300_000).replace(r#""op""#, r#""compactor":"exact","op""#);
        std::fs::write(
            super::estimated_ledger_path(),
            [
                rolled("injected_context", 1_000).replace(r#""io":"cache""#, r#""io":"input""#),
                rolled_exact,
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(super::estimated_summary().overhead_cache, 300_000);
    }
}

#[cfg(test)]
mod priced_as_billed {
    /// One row in dollars, by the lifetime figure's rules: an estimated row
    /// converted to the model's tokens, a billed one not, a cost negative.
    #[test]
    fn a_row_is_priced_in_the_model_s_own_tokens() {
        let m = "claude-opus-5-5";
        let r = crate::pricing::rates_for_model(m);
        let row = |op: &str, class: &str, io: &str| serde_json::json!({"op": op, "class": class, "io": io, "estimated": true, "before": 1_000_000, "after": 0, "saved": 1_000_000});
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(close(
            super::row_usd(&row("compounding", "compounding", "cache"), m),
            1.6 * r.cache
        ));
        assert!(close(
            super::row_usd(&row("output_lean", "output", "output"), m),
            r.output
        ));
        assert!(close(
            super::row_usd(&row("overhead_rebill", "overhead", "cache"), m),
            -1.6 * r.cache
        ));
        assert!(close(
            super::row_usd(&row("recovery_turn", "overhead", "cache"), m),
            -r.cache
        ));
        let mut image = row("image-downscale", "", "");
        assert!(
            close(super::row_usd(&image, m), 1.6 * r.input),
            "an older row keeps its value"
        );
        image["billed"] = serde_json::Value::Bool(true);
        assert!(
            close(super::row_usd(&image, m), r.input),
            "vision tokens are billed tokens"
        );
    }

    /// An avoided turn re-sends a cached prefix: batching is a CACHE saving,
    /// and the overhead re-bill comes out of the same bucket.
    #[test]
    fn batching_is_a_cache_read_saving() {
        let est = super::EstimatedSummary {
            output: 5,
            batching: 40,
            compounding: 60,
            overhead: 3,
            overhead_cache: 10,
            recovery: 7,
            ..Default::default()
        };
        assert_eq!(est.by_io(100, 2), (100 + 7 - 3, 60 + 40 - 10, 7));
        // A re-bill larger than the re-reads it offsets is a COST, not zero.
        let cost = super::EstimatedSummary {
            compounding: 10,
            batching: 5,
            overhead_cache: 40,
            ..Default::default()
        };
        assert_eq!(cost.by_io(0, 0).1, -25);
    }
}
