// The tests of `crate::dashboard`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `dashboard`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {
    use super::*;

    /// The redaction line must name the rule that did most of the masking.
    /// A bare count reads the same whether a rule is working or misfiring —
    /// the entropy sweep masked Rust test names for weeks behind one.
    #[test]
    fn the_redaction_line_names_the_busiest_rule() {
        let out = render(&sample(), &Theme::default());
        // The count itself sits between colour codes; assert on the prose.
        assert!(
            out.contains("secrets redacted before reaching the model"),
            "{out}"
        );
        assert!(
            out.contains("most by `entropy` (2)"),
            "rule not named: {out}"
        );

        let mut d = sample();
        d.secrets_redacted_top_rule = None;
        let old = render(&d, &Theme::default());
        assert!(
            !old.contains("most by"),
            "claims a rule it does not have: {old}"
        );
    }

    fn sample() -> DashboardData {
        DashboardData {
            cache_rewrites: Vec::new(),
            project_path: "/tmp/demo".into(),
            ledger_path: "~/.tokenade/gain.jsonl".into(),
            operations: 128,
            tokens_before: 425_000,
            tokens_after: 76_200,
            last_7d_saved: 92_000,
            window_days: 30,
            lifetime_saved: 980_000,
            estimated_tokens_saved: 33_500,
            secrets_redacted: 3,
            secrets_redacted_top_rule: Some(("entropy".into(), 2)),
            top_compactors: vec![
                ("auto:json".into(), 120_000, 8_400, 12),
                ("auto:diff".into(), 65_000, 14_300, 7),
                ("auto:html".into(), 40_000, 11_200, 4),
            ],
            by_layer: vec![
                ("content-auto".into(), 225_000, 33_900, 23),
                ("web".into(), 40_000, 11_200, 4),
            ],
            by_project: vec![("/tmp/demo".into(), 300_000, 50_000, 80)],
            code_intel: vec![
                ("semantic_search".into(), 6, 12_500),
                ("symbol_find".into(), 14, 21_000),
            ],
            code_intel_disclaimer: Some(
                "Code-search savings can't be measured directly — estimate based on avg Read/Grep cost.".into(),
            ),
            claude_usage: vec![
                ClaudeUsageRow {
                    agent_label: "Claude".into(),
                    display_cwd: "/home/me/proj-a".into(),
                    cwd: "/home/me/proj-a".into(),
                    sessions: 4,
                    total_tokens: 250_000,
                    tool_result_tokens: 12_000,
                    tool_result_tokens_by_tool: {
                        let mut m = std::collections::HashMap::new();
                        m.insert("Read".to_string(), 6_500u64);
                        m.insert("Bash".to_string(), 4_200u64);
                        m.insert("Edit".to_string(), 1_300u64);
                        m
                    },
                    last_ts: "2026-05-29T01:00:00Z".into(),
                    tokenade_calls: 3,
                    read_calls: 42,
                    grep_calls: 5,
                    glob_calls: 1,
                    bash_calls: 18,
                    web_calls: 2,
                },
                ClaudeUsageRow {
                    agent_label: "Codex".into(),
                    display_cwd: "/home/me/proj-b".into(),
                    cwd: "/home/me/proj-b".into(),
                    sessions: 2,
                    total_tokens: 80_000,
                    tool_result_tokens: 4_500,
                    tool_result_tokens_by_tool: {
                        let mut m = std::collections::HashMap::new();
                        m.insert("Read".to_string(), 3_000u64);
                        m.insert("Bash".to_string(), 1_500u64);
                        m
                    },
                    last_ts: "2026-05-28T22:00:00Z".into(),
                    tokenade_calls: 0,
                    read_calls: 12,
                    grep_calls: 0,
                    glob_calls: 0,
                    bash_calls: 8,
                    web_calls: 0,
                },
            ],
            bypass_by_tool: vec![
                ("Read".into(), 9_500),
                ("Bash".into(), 5_700),
                ("Edit".into(), 1_300),
            ],
            tokenade_after_by_tool: {
                let mut m = std::collections::HashMap::new();
                // #684 sample data: Bash and Read have ~80% of their
                // tool_result tokens attributed to tokenade hook traffic,
                // so the rendered Functions line should show small bypass.
                m.insert("Read".to_string(), 7_500u64);
                m.insert("Bash".to_string(), 4_400u64);
                m
            },
            claude_usage_since: Some("2026-05-29".into()),
            home_dir: Some("/home/me".into()),
            lang_fr: false,
            cache_hits: 412,
            cache_misses: 28,
            cache_bytes_served: 7_700_000,
            semantic_docs: 482,
            semantic_terms: 9_120,
            model_used: Some("claude-sonnet-4-6".into()),
            dollars_saved: 1.23,
            wrapped_mcps: vec![],
        }
    }

    #[test]
    fn render_color_does_not_panic_and_contains_brand() {
        let out = render(&sample(), &Theme::default());
        // brand visible in coloured render
        assert!(out.contains("TOKENADE") || out.contains("tokenade"));
        // hero hit
        assert!(out.contains("348.8k") || out.contains("348.8"));
        // bars present
        assert!(out.contains('█'));
    }

    #[test]
    fn render_mono_strips_escape_codes() {
        let out = render(
            &sample(),
            &Theme {
                color: false,
                width: 100,
            },
        );
        assert!(
            !out.contains('\x1b'),
            "mono dashboard must not emit ESC codes"
        );
        // The brand reads "TOKENADE" via the block-letter logo, but
        // that word never appears as a literal string in the output —
        // only the lowercase "tokenade" in the subtitle and footer do.
        assert!(out.contains("tokenade"));
        assert!(out.contains("tokens saved"));
    }

    #[test]
    fn op_layer_classifies_known_families() {
        // wrapped MCP traffic + code-intel defer to their dedicated buckets.
        assert_eq!(op_layer("mcp-proxy:github"), "wrapped-mcp");
        assert_eq!(op_layer("semantic_search"), "code-intel");
        assert_eq!(op_layer("symbol_find:file"), "code-intel");
        // file reads.
        assert_eq!(op_layer("read_auto_compact"), "file-read");
        assert_eq!(op_layer("read_content_dedup"), "file-read");
        assert_eq!(op_layer("cat"), "file-read");
        // web family.
        assert_eq!(op_layer("serp_compact"), "web");
        assert_eq!(op_layer("webfetch_compact"), "web");
        // content-routed auto:* picks.
        assert_eq!(op_layer("auto:json"), "content-auto");
        assert_eq!(op_layer("auto:stacktrace"), "content-auto");
        // shell-boundary wrappers.
        assert_eq!(op_layer("cargo-test"), "shell");
        assert_eq!(op_layer("git-diff"), "shell");
        assert_eq!(op_layer("exec:bash"), "shell");
        assert_eq!(op_layer("grep"), "shell");
        assert_eq!(op_layer("generic"), "shell");
        // genuinely unknown ops fall through to `other`.
        assert_eq!(op_layer("brand_new_thing"), "other");
    }

    #[test]
    fn per_layer_rollup_groups_sums_and_sorts() {
        let ops = vec![
            ("auto:json".to_string(), 100_000, 10_000, 5),
            ("auto:yaml".to_string(), 50_000, 5_000, 3),
            ("cargo-test".to_string(), 20_000, 18_000, 2),
            ("serp_compact".to_string(), 30_000, 3_000, 1),
        ];
        let out = per_layer_rollup(&ops);
        // content-auto = json + yaml merged: before 150k, after 15k, 8 calls.
        let ca = out.iter().find(|(l, ..)| l == "content-auto").unwrap();
        assert_eq!((ca.1, ca.2, ca.3), (150_000, 15_000, 8));
        // Sorted by saved desc: content-auto (135k) > web (27k) > shell (2k).
        let order: Vec<&str> = out.iter().map(|(l, ..)| l.as_str()).collect();
        assert_eq!(order, vec!["content-auto", "web", "shell"]);
    }

    #[test]
    fn by_layer_section_renders_when_populated() {
        let out = render(
            &sample(),
            &Theme {
                color: false,
                width: 100,
            },
        );
        assert!(out.contains("BY LAYER"));
        assert!(out.contains("content-auto"));
    }

    #[test]
    fn human_tokens_thresholds() {
        assert_eq!(human_tokens(0), "0");
        assert_eq!(human_tokens(999), "999");
        assert_eq!(human_tokens(1_000), "1.0k");
        assert_eq!(human_tokens(12_500), "12.5k");
        assert_eq!(human_tokens(2_500_000), "2.5M");
        assert_eq!(human_tokens(1_200_000_000), "1.2B");
    }

    #[test]
    fn claude_usage_section_appears_when_data_present() {
        let out = render(
            &sample(),
            &Theme {
                color: false,
                width: 100,
            },
        );
        assert!(out.contains("AGENT TOKENS (outside of tokenade)"));
        assert!(out.contains("/home/me/proj-a"));
        assert!(out.contains("250.0k") || out.contains("250"));
    }

    #[test]
    fn claude_usage_breakdown_hidden_on_narrow_terminal() {
        let out = render(
            &sample(),
            &Theme {
                color: false,
                width: 80,
            },
        );
        assert!(out.contains("AGENT TOKENS (outside of tokenade)"));
        // On 80-col terminals the main row still shows…
        assert!(out.contains("/home/me/proj-a"));
        // …but the breakdown sub-line MUST be suppressed (it would wrap).
        assert!(
            !out.contains("Read 42"),
            "narrow terminal must hide the breakdown line"
        );
        assert!(
            !out.contains("→ exec_compact"),
            "narrow terminal must hide the suggestion line"
        );
    }

    #[test]
    fn claude_usage_breakdown_visible_on_wide_terminal() {
        let out = render(
            &sample(),
            &Theme {
                color: false,
                width: 120,
            },
        );
        assert!(out.contains("Read 42"), "wide terminal shows the breakdown");
    }

    #[test]
    fn claude_usage_section_hidden_when_no_data() {
        let mut d = sample();
        d.claude_usage.clear();
        // #683 — Functions section reads from bypass_by_tool, which is
        // built from the same transcript walk as claude_usage. In
        // production both go empty together; mirror that invariant
        // here so the section truly hides.
        d.bypass_by_tool.clear();
        let out = render(
            &d,
            &Theme {
                color: false,
                width: 100,
            },
        );
        assert!(!out.contains("AGENT TOKENS (outside of tokenade)"));
    }

    // #498: WRAPPED MCPs section must appear when populated and stay
    // hidden when empty (user explicitly asked for first-class category
    // visibility instead of burial inside TOP COMPACTORS).
    #[test]
    fn wrapped_mcps_section_hidden_when_empty() {
        let out = render(
            &sample(),
            &Theme {
                color: false,
                width: 100,
            },
        );
        assert!(
            !out.contains("WRAPPED MCPs"),
            "empty wrapped_mcps must not render the section"
        );
    }

    #[test]
    fn wrapped_mcps_section_appears_when_populated() {
        let mut d = sample();
        d.wrapped_mcps = vec![
            ("mcp-proxy:github".into(), 80_000, 1_500, 4),
            ("mcp-proxy:sqlite".into(), 20_000, 6_000, 2),
        ];
        let out = render(
            &d,
            &Theme {
                color: false,
                width: 100,
            },
        );
        assert!(out.contains("WRAPPED MCPs"), "section heading missing");
        assert!(out.contains("mcp-proxy:github"), "github row missing");
        assert!(out.contains("mcp-proxy:sqlite"), "sqlite row missing");
        // Saved-tokens column uses human_tokens(); 78.5k saved on github.
        assert!(out.contains("78.5k") || out.contains("78.4k"));
    }

    #[test]
    fn functions_section_excludes_all_tokenade_namespaces() {
        // #685 — every namespace that ToolBreakdown::bump treats as
        // tokenade must also be filtered out of the Functions section.
        // Pre-fix: only `mcp__tokenade__*` was filtered; plugin and
        // legacy installs leaked into the "outside" view.
        let mut d = sample();
        d.bypass_by_tool = vec![
            ("mcp__tokenade__semantic_search".into(), 5_000),
            ("mcp__plugin_tokenade_tokenade__symbol_find".into(), 4_000),
            ("mcp__supertokensaver__exec_compact".into(), 3_000),
            ("mcp__sts__auto_compact".into(), 2_000),
            ("Edit".into(), 999), // single legitimate bypass entry
        ];
        d.tokenade_after_by_tool = Default::default();
        let out = render(
            &d,
            &Theme {
                color: false,
                width: 100,
            },
        );
        let funcs_idx = out
            .find("AGENT TOKENS (outside of tokenade) - Functions")
            .expect("Functions section missing");
        let section_tail = &out[funcs_idx..];
        // Cut to the next blank-line-bounded block so unrelated rows
        // elsewhere in the dashboard don't accidentally satisfy the
        // assertion.
        let section = section_tail.split("\n\n").next().unwrap_or(section_tail);
        assert!(
            !section.contains("mcp__tokenade__"),
            "direct-install tokenade leaked into Functions"
        );
        assert!(
            !section.contains("mcp__plugin_tokenade_tokenade__"),
            "plugin-install tokenade leaked into Functions"
        );
        assert!(
            !section.contains("mcp__supertokensaver__"),
            "legacy supertokensaver leaked into Functions"
        );
        assert!(
            !section.contains("mcp__sts__"),
            "legacy sts leaked into Functions"
        );
        assert!(
            section.contains("Edit"),
            "Edit (true bypass) should still appear"
        );
    }

    #[test]
    fn agent_usage_shows_agent_label_per_row() {
        // Multi-agent fix (D10): each row carries an `agent_label`
        // ("Claude", "Codex", "Cline"). Wide-terminal render must
        // surface them so the user can tell apart same-cwd activity
        // from different IDEs.
        let out = render(
            &sample(),
            &Theme {
                color: false,
                width: 120,
            },
        );
        assert!(out.contains("[Claude]"), "Claude tag missing");
        assert!(out.contains("[Codex]"), "Codex tag missing");
    }

    /// #739 — regression test: the 2nd hero bar's `total_bypass` must
    /// subtract `tokenade_after_for_cwd` ONCE per cwd, not once per
    /// agent row. Pre-#739 implementation summed per row, so a cwd
    /// shared by 2 agents would cancel tokenade_after twice → bypass
    /// under-counted by (N-1)×tokenade_after.
    #[test]
    fn second_hero_bar_dedups_tokenade_after_by_cwd() {
        // Construct: same cwd /shared, two agents each with 1000 tool
        // result tokens. by_project[/shared].after = 300. Expected
        // total_bypass = (1000+1000) - 300 = 1700.
        // Pre-#739 buggy result would be: (1000-300) + (1000-300) = 1400.
        let d = DashboardData {
            tokens_before: 1000, // headline first bar denom — ensures total_potential >0
            tokens_after: 500,
            operations: 1,
            by_project: vec![(
                "/shared".to_string(),
                10_000, // before — irrelevant to test
                300,    // after — the value that must be counted ONCE
                5,
            )],
            claude_usage: vec![
                ClaudeUsageRow {
                    agent_label: "Claude".into(),
                    display_cwd: "/shared".into(),
                    cwd: "/shared".into(),
                    sessions: 1,
                    total_tokens: 5_000,
                    tool_result_tokens: 1_000,
                    ..Default::default()
                },
                ClaudeUsageRow {
                    agent_label: "Codex".into(),
                    display_cwd: "/shared".into(),
                    cwd: "/shared".into(),
                    sessions: 1,
                    total_tokens: 5_000,
                    tool_result_tokens: 1_000,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let out = render(
            &d,
            &Theme {
                color: false,
                width: 120,
            },
        );
        // The 2nd-bar copy includes "incl. {human_tokens(total_bypass)} bypassed".
        // 1700 → "1.7k" via human_tokens. 1400 (the buggy result) → "1.4k".
        // Assert the correct text appears and the buggy text doesn't.
        assert!(
            out.contains("incl. 1.7k bypassed") || out.contains("incl. 1700 bypassed"),
            "expected bypass to be 1700 (single-subtract per cwd), got render:\n{out}"
        );
        assert!(
            !out.contains("incl. 1.4k bypassed") && !out.contains("incl. 1400 bypassed"),
            "got the pre-#739 buggy 1400 result:\n{out}"
        );
    }

    #[test]
    fn empty_dashboard_renders_clean() {
        let out = render(
            &DashboardData::default(),
            &Theme {
                color: false,
                width: 80,
            },
        );
        // No crashes, brand subtitle present, hero copy present even
        // when there are zero recorded operations.
        assert!(out.contains("tokenade"));
        assert!(out.contains("tokens saved"));
    }

    #[test]
    fn bar_widths_clamp() {
        let p = Pal {
            t: &Theme {
                color: false,
                width: 80,
            },
        };
        assert!(!render_bar(10, -5.0, &p).is_empty());
        assert!(!render_bar(10, 105.0, &p).is_empty());
    }
}
#[cfg(test)]
mod cache_rewrite_tests {
    use super::*;

    fn data(rewrites: Vec<(String, u64, u64)>) -> DashboardData {
        DashboardData {
            cache_rewrites: rewrites,
            window_days: 30,
            ..Default::default()
        }
    }

    /// The section is a cost the USER controls, so it has to name the window it
    /// counted over and the trigger to move. MEASURED on a real install when it
    /// was added: 1 068 rewrites, 341,8M tokens, 547 of them one scheduled task
    /// firing again inside two minutes.
    #[test]
    fn the_section_names_its_window_and_the_trigger_to_move() {
        let d = data(vec![
            ("scheduled:lt2m".to_string(), 547, 181_500_000),
            ("toolloop:lt2m".to_string(), 119, 41_300_000),
        ]);
        let mut s = String::new();
        render_cache_rewrites(
            &mut s,
            &d,
            &Pal {
                t: &Theme::default(),
            },
        );
        assert!(s.contains("in the last 30 day(s)"), "unscoped count: {s}");
        assert!(
            s.contains("scheduled"),
            "did not name the dominant trigger: {s}"
        );
        assert!(
            s.contains("under 2 min apart"),
            "gap bucket not spelled out: {s}"
        );
        assert!(
            s.contains("Spacing it out"),
            "no actionable line for a scheduled-dominated profile: {s}"
        );
    }

    /// Silent when there is nothing to report — a heading with a zero under it
    /// is noise on every dashboard of every user who never thrashes.
    #[test]
    fn the_section_is_silent_when_there_is_nothing_to_say() {
        let mut s = String::new();
        render_cache_rewrites(
            &mut s,
            &data(Vec::new()),
            &Pal {
                t: &Theme::default(),
            },
        );
        assert!(s.is_empty(), "printed a heading with no data: {s}");
    }
}
