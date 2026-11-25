// The tests of `auto`.
//
// Spliced by `include!` from `auto.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed. It is the shape this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

// When two compactors can both take the same table, the choice must survive
// one of them declining.
//
// `TabularCli` consults the per-command module first — it knows what the
// table MEANS — and fell back to the generic one only when no module
// existed. A module that answers with the table it was handed is the same
// situation and was not treated as one.
// A patch is a patch whatever language its payload is written in.
//
// Measured 2026-08-26 over the 256 patch files in the corpus: 30 never
// reached the diff compactor, claimed by the detector for their PAYLOAD's
// language — markdown for a README patch, HCL for a TypeScript one, SQL for
// `dist/dev.cjs`.
// Item-level retention: do the facts an agent would need survive the fold?
//
// The shape is borrowed from `alibaba/anolisa`'s L2 retention check, which
// scores a compressor fact-by-fact and NAMES what each one dropped rather
// than returning a single pass/fail. Checked here on the command shapes our
// own corpus shows agents actually run, so a fold that starts eating the
// answer fails on the case that matters rather than on a synthetic one.

#[cfg(test)]
/// Every `Kind` with a compactor, for the measurement tests that must not
/// silently skip one. Hoisted out of `misroute_audit` when a second
/// instrument needed the same list: two copies would drift, and a kind
/// missing from one of them is invisible exactly where it matters.
pub(crate) const ALL_KINDS: &[Kind] = &[
    Kind::Html,
    Kind::A11yTree,
    Kind::JupyterNotebook,
    Kind::Json,
    Kind::Yaml,
    Kind::UnifiedDiff,
    Kind::TerraformPlan,
    Kind::CurlVerbose,
    Kind::StackTrace,
    Kind::ArchiveListing,
    Kind::TabularCli,
    Kind::DashTable,
    Kind::Csv,
    Kind::Tsv,
    Kind::Logs,
    Kind::Challenge,
    Kind::EnvDump,
    Kind::Markdown,
    Kind::Lockfile,
    Kind::Minified,
    Kind::Base64Blob,
    Kind::SqlResult,
    Kind::PrometheusMetrics,
    Kind::AccessLog,
    Kind::Journal,
    Kind::SourceMap,
    Kind::OpenApi,
    Kind::Har,
    Kind::Email,
    Kind::Calendar,
    Kind::Contacts,
    Kind::GraphQl,
    Kind::Strace,
    Kind::JvmBuild,
    Kind::KubectlDescribe,
    Kind::Ansible,
    Kind::GhActions,
    Kind::PkgInstall,
    Kind::K8sManifest,
    Kind::GitLog,
    Kind::Dns,
    Kind::Ndjson,
    Kind::ClaudeTranscript,
    Kind::TomlConfig,
    Kind::Dockerfile,
    Kind::Ini,
    Kind::Makefile,
    Kind::CMake,
    Kind::Bazel,
    Kind::Hcl,
    Kind::Proto,
    Kind::Xml,
    Kind::JUnit,
    Kind::LaTeX,
    Kind::BibTeX,
    Kind::SqlSource,
    Kind::Systemd,
    Kind::FileList,
    Kind::Disk,
    Kind::SearchOut,
    Kind::Netstat,
    Kind::CargoBench,
    Kind::ContainerLogs,
    Kind::Vmstat,
    Kind::Tcpdump,
    Kind::Serp,
    Kind::Tsc,
    Kind::CommandChain,
    Kind::CargoBuild,
    Kind::BundleAssets,
    Kind::GTest,
    Kind::Tap,
    Kind::Unknown,
];

#[cfg(test)]
mod tests {

    /// A warning in front of a document must not cost the document.
    ///
    /// Every agent's shell tool merges stderr into stdout, so JSON arrives
    /// behind whatever the tool warned about. The detector reads the head,
    /// so one `warning:` line returned the whole document verbatim:
    /// measured on `cargo metadata --no-deps`, 8 741 tokens folded to
    /// 1 908 alone and all 8 757 came back with cargo's one-line warning
    /// in front.
    ///
    /// Four fixtures, opposing on every field the split reads: a prefixed
    /// document (folds, warning kept), a bare one (unchanged), a prefix
    /// that is most of the text (refused — that is a mixed document, not a
    /// preamble), and a prefix longer than the line bar (refused).
    #[test]
    fn a_warning_in_front_of_json_does_not_cost_the_json() {
        let doc = format!(
            "{{\"items\":[{}]}}",
            (0..400)
                .map(|i| format!(
                    "{{\"id\":{i},\"name\":\"item-{i}\",\"status\":\"ok\",\"region\":\"eu-west-1\"}}"
                ))
                .collect::<Vec<_>>()
                .join(",")
        );
        let bare = compact(&doc).1;
        let bare_tokens = tokenizer::estimate(&bare);

        let warned = format!("warning: please specify `--format-version` flag explicitly\n{doc}");
        let (_, out, _) = compact(&warned);
        assert!(
            out.contains("please specify `--format-version`"),
            "the warning was dropped to compact the thing it warns about:\n{out}"
        );
        // The property is EXACT: a prefix costs its own length and nothing
        // else. A loose bound is why the first version of this test passed
        // with the rescue removed — the unclaimed text still went through
        // the generic fold, landing at 8 927 tokens against 5 804 bare,
        // comfortably inside a "less than twice" bar that proved nothing.
        let warning_tokens =
            tokenizer::estimate("warning: please specify `--format-version` flag explicitly\n");
        assert!(
            tokenizer::estimate(&out) <= bare_tokens + warning_tokens + 4,
            "a warning in front cost the fold: {} tokens against {bare_tokens} bare plus {warning_tokens} of warning",
            tokenizer::estimate(&out)
        );

        // A "prefix" that is most of the output is the other half of a
        // mixed document. Peeling it would rebuild one section from its
        // parse and drop the other.
        let prose: String = (0..40)
            .map(|i| format!("line {i} of an ordinary report that happens to precede a document\n"))
            .collect();
        let small_doc = "{\"a\":1,\"b\":2}\n";
        assert!(
            split_leading_diagnostics(&format!("{prose}{small_doc}")).is_none(),
            "a prefix larger than the document was peeled off as a preamble"
        );

        // The share bar is not the line bar. Three lines can still be most
        // of the bytes, and then they are the document rather than a
        // preamble in front of one — this fixture is refused by the share
        // rule alone, with the line count well inside its limit.
        let three_long: String = (0..3)
            .map(|i| {
                format!(
                    "warning {i}: {}\n",
                    "a long diagnostic sentence ".repeat(40)
                )
            })
            .collect();
        assert!(
            split_leading_diagnostics(&format!("{three_long}{{\"a\":1}}\n")).is_none(),
            "three lines that outweigh the document were peeled off as a preamble"
        );

        // Half the bytes is already too much: the bar is a fifth, and a
        // fixture that only fails a generous bar cannot tell one from the
        // other.
        let half = "warning: a diagnostic line of roughly this length\n".repeat(3);
        let doc_same_size = format!("{{\"k\":\"{}\"}}\n", "v".repeat(half.len() - 10));
        assert!(
            split_leading_diagnostics(&format!("{half}{doc_same_size}")).is_none(),
            "a prefix worth half the output was peeled off as a preamble"
        );

        // A line that opens a structured value is the document. Without
        // that stop the scan walks INTO a TOML array of tables and hands
        // back its second entry as the "document", with the first as a
        // preamble — the same hijack the JSON detector carves out by name.
        // Long enough that its first entry is well inside the share bar,
        // so this fixture tests the stop and not another rule.
        let lockfile: String = (0..40)
            .map(|i| format!("[[package]]\nname = \"crate-{i}\"\nversion = \"1.0.{i}\"\n"))
            .collect();
        assert!(
            split_leading_diagnostics(&lockfile).is_none(),
            "the scan walked into a TOML array of tables"
        );

        // And six lines of preamble is not a preamble.
        let six: String = (0..6)
            .map(|i| format!("warning {i}: something\n"))
            .collect();
        assert!(
            split_leading_diagnostics(&format!("{six}{doc}")).is_none(),
            "six leading lines were accepted; the bar is five"
        );
        let five: String = (0..5)
            .map(|i| format!("warning {i}: something\n"))
            .collect();
        assert!(
            split_leading_diagnostics(&format!("{five}{doc}")).is_some(),
            "five leading lines were refused; the bar is five"
        );
    }
    use super::*;
    use std::fmt::Write;

    /// The empty floor is structurally blind to a well-formed summary
    /// OF NOTHING. Measured over 49 951 files: a 6 865-byte `.gitignore` was
    /// claimed by `PkgInstall` and served through `tokenade exec` as 36 bytes,
    /// `pnpm: 0 added, 0 warnings, 0 errors` — an install that never ran, with
    /// the file gone.
    #[test]
    fn a_summary_that_counted_nothing_is_not_served() {
        // The real shape: a hand-written file that MENTIONS the package
        // manager in prose. `pnpm install ` is one of the four markers
        // `pkginstall::detect` keys on, and a `.gitignore` explaining which
        // artefacts a `pnpm install ` leaves behind carries it.
        let mut gitignore =
            String::from("# Artefacts left behind by a `pnpm install ` run, and why\n");
        for i in 0..120 {
            let _ = writeln!(
                gitignore,
                "# rule {i}: keep the sources, drop what the build regenerates"
            );
        }
        let (_kind, out, sav) = compact(&gitignore);
        assert_eq!(out, gitignore, "the file was replaced by a summary:\n{out}");
        assert_eq!(sav.tokens_after, sav.tokens_before);
    }

    /// The three conditions, each refusing something the others allow.
    #[test]
    fn each_nothing_read_condition_refuses_something_real() {
        // ONE zero is not a count. The census flagged a 255-byte Python file
        // whose only digit is the `0` in a default argument; reading that as
        // "it counted nothing" would hand back source it should have stripped.
        assert!(!summarises_nothing_read("added 0 items to the set", 500));
        // A non-zero among the counts means it read something.
        assert!(!summarises_nothing_read(
            "pnpm: 12 added, 0 warnings, 0 errors",
            500
        ));
        // A FOLD is not a summary — length is what tells them apart.
        let long = format!("0 of 0 rows{}", " x".repeat(200));
        assert!(!summarises_nothing_read(&long, 500));
        // And on small input, "nothing" is a plausible truth.
        assert!(!summarises_nothing_read(
            "pnpm: 0 added, 0 warnings, 0 errors",
            10
        ));
        // The shape itself.
        assert!(summarises_nothing_read(
            "pnpm: 0 added, 0 warnings, 0 errors",
            500
        ));
    }

    #[test]
    fn the_error_guard_answers_the_same_three_ways_it_always_did() {
        let input = "error: could not compile `foo`\nnoise\nfatal: bad object\n";
        // Kept verbatim as lines → nothing to re-attach. This is the case the
        // line index answers in O(1) instead of one full scan of the output
        // per candidate.
        assert!(dropped_error_lines(input, input).is_none());
        // Dropped → re-attached, both of them.
        let appendix = dropped_error_lines(input, "noise\n").expect("errors re-attached");
        assert!(
            appendix.contains("error: could not compile `foo`"),
            "{appendix}"
        );
        assert!(appendix.contains("fatal: bad object"), "{appendix}");
        // Kept only INSIDE a line the compactor rebuilt: still present, so
        // still not re-attached. The line index cannot answer this one — the
        // substring scan behind it must stay.
        let rebuilt = "[x3] error: could not compile `foo` (and 2 more)\n";
        assert!(dropped_error_lines("error: could not compile `foo`\n", rebuilt).is_none());
    }

    #[test]
    fn running_out_of_scan_budget_re_attaches_rather_than_forgets() {
        // CONTAINS_SCAN_BUDGET bounds the substring fallback. Which way it
        // fails is the whole point: past the budget a candidate is
        // re-attached, never assumed present. A guard that exists so errors
        // are not lost must not lose one to its own cost ceiling.
        let mut input = String::new();
        for i in 0..CONTAINS_SCAN_BUDGET + 20 {
            let _ = writeln!(input, "ERROR unit {i} refused");
        }
        // Every line IS present, but only as a substring of a rebuilt line,
        // so the index misses all of them and the budget runs out at 64.
        let output: String = input
            .lines()
            .map(|l| format!("[x1] {l} (folded)\n"))
            .collect();
        let appendix = dropped_error_lines(&input, &output).expect("budget spent → re-attach");
        assert!(
            appendix.contains(&format!("ERROR unit {CONTAINS_SCAN_BUDGET} refused")),
            "{appendix}"
        );
    }

    #[test]
    fn a_document_whose_headings_are_h4_is_still_markdown() {
        // A PDF rendered to markdown puts its title at `##` and its sections at
        // `####`. Counting only h1-h3 saw ONE marker, so the doc fell through to
        // the last detector (`command_chain`, whose banner test matches `###`
        // and therefore `####` too) and a research paper was labelled
        // `api.rest.chain.txt` — MEASURED 2026-07-22 on arXiv:1706.03762.
        let doc = "Some front matter line.\n\n## Attention Is All You Need\n\n\
                   **Ashish Vaswani**\n\n#### Abstract\n\nThe dominant sequence \
                   transduction models are based on complex recurrent networks.\n\n\
                   #### References\n\n[1] Layer normalization.\n";
        assert_eq!(
            detect(doc),
            Kind::Markdown,
            "h4 headings not seen as markdown"
        );
        // h5/h6 too — the rule is ATX 1-6, not a hand-listed subset.
        let deep =
            "Intro paragraph.\n\n##### Deep one\n\ntext here\n\n###### Deeper\n\nmore text\n";
        assert_eq!(detect(deep), Kind::Markdown);
        // `#hashtag` (no space) is NOT a heading and must not qualify.
        let tags = "#hashtag one\n#hashtag two\nplain prose line here\n";
        assert_ne!(detect(tags), Kind::Markdown);
    }

    #[test]
    fn mixed_section_output_not_routed_to_rebuilding_compactor() {
        // Two sections: a dig answer + a curl section. Single-format Dns
        // compactor would drop the curl part — the veto must reroute.
        let mixed = "=== dns ===\n;; ANSWER SECTION:\nexample.com. 300 IN A 1.2.3.4\n=== http ===\nHTTP/2 200\ncontent-type: text/html\n";
        assert!(
            matches!(detect(mixed), Kind::Unknown),
            "got {:?}",
            detect(mixed)
        );
        // Homogeneous dig output keeps its dedicated route.
        let pure = ";; ANSWER SECTION:\nexample.com. 300 IN A 1.2.3.4\n";
        assert!(!matches!(detect(pure), Kind::Unknown));
    }

    #[test]
    fn pytest_banners_are_not_section_markers() {
        assert_eq!(
            section_marker_lines("========== test session starts ==========\ncollected 3 items\n"),
            0
        );
        assert_eq!(section_marker_lines("=== A ===\nx\n=== B ===\n"), 2);
        // one marker alone is not "mixed"
        assert_eq!(section_marker_lines("=== A ===\njust one section\n"), 1);
    }

    #[test]
    fn detects_html() {
        assert_eq!(
            detect("<!DOCTYPE html><html><body><p>hi</p></body></html>"),
            Kind::Html
        );
        assert_eq!(detect("<html><body>x</body></html>"), Kind::Html);
    }

    #[test]
    fn detects_notebook_over_json() {
        // Notebooks ARE JSON but should route to the notebook compactor.
        let nb = r#"{"cells":[{"cell_type":"code","source":["x=1\n"]}],"nbformat":4,"nbformat_minor":5,"metadata":{}}"#;
        assert_eq!(detect(nb), Kind::JupyterNotebook);
    }

    #[test]
    fn source_code_extensions_never_auto_compact() {
        // #570 — production showed Rust/Python/shell source files
        // being claimed by content detectors (SERP/INI/cmake/makefile)
        // and aggressively compacted, losing up to 90 % of the bytes
        // the agent asked for. The path-hint variants must hard-reject
        // any known source extension regardless of content.

        // Adversarial fixture: a Rust file slice that *would* match
        // multiple content detectors (INI-shaped attributes + cmake-
        // shaped header) if seen without a path hint.
        let rusty = "[derive(Debug)]\n\
                     pub struct Config {\n\
                         pub name: String,\n\
                         pub version: u32,\n\
                     }\n\
                     impl Config {\n\
                         pub fn new() -> Self { Self::default() }\n\
                     }\n";
        let (kind, out, sav) = compact_with_path_hint(rusty, "/tmp/x/src/config.rs");
        assert_eq!(kind, Kind::Unknown, ".rs must be passthrough");
        assert_eq!(out, rusty, ".rs must return raw bytes");
        assert_eq!(
            sav.tokens_before, sav.tokens_after,
            ".rs must have 0 savings"
        );

        // Same for .py, .sh, .ts, .go — the explicit production offenders.
        for path in &[
            "/tmp/x/foo.py",
            "/tmp/x/run.sh",
            "/tmp/x/app.ts",
            "/tmp/x/main.go",
            "/tmp/x/Lib.kt",
        ] {
            let (kind, out, _) =
                compact_with_path_hint("# header\nkey = value\n[section]\nfoo=bar\n", path);
            assert_eq!(kind, Kind::Unknown, "{path} must be passthrough");
            assert!(!out.is_empty(), "{path} must not be empty");
        }

        // The indexed variant must respect the same reject.
        let (kind, _, _) = compact_with_path_hint_indexed(rusty, "/tmp/x/config.rs", "abc123");
        assert_eq!(kind, Kind::Unknown);

        // Non-source path with the same INI-looking content still routes
        // by content (sanity: we didn't accidentally break the happy path).
        let cfg = "[server]\nport = 8080\nhost = localhost\n[client]\ntimeout = 30\n";
        let (kind, _, _) = compact_with_path_hint(cfg, "/tmp/x/app.cfg");
        assert_eq!(kind, Kind::Ini);
    }

    #[test]
    fn env_dump_does_not_claim_dockerfile_shape() {
        // #567 — looks_like_env_dump used to delegate to env::compact,
        // which accepts the `KEY VALUE` (space-separated) form. That
        // made Dockerfile directives like `FROM node:22-alpine` match
        // env_dump BEFORE the dockerfile detector could fire. With the
        // literal-`=`-required rule, the same input now routes to
        // Kind::Dockerfile.
        let dock = "# Build stage\n\
                    FROM node:22-alpine AS builder\n\
                    WORKDIR /app\n\
                    COPY package*.json ./\n\
                    RUN npm ci --ignore-scripts\n\
                    COPY src/ ./src/\n\
                    RUN npm run build\n\
                    # Cleanup\n\
                    RUN npm prune --omit=dev\n\
                    FROM node:22-alpine AS runtime\n\
                    EXPOSE 3000\n\
                    CMD [\"node\", \"dist/index.js\"]\n";
        assert_eq!(detect(dock), Kind::Dockerfile);
    }

    #[test]
    fn env_dump_still_claims_real_env_dump() {
        // Sanity: real env dumps with literal `KEY=value` still match.
        let env = "PATH=/usr/bin:/bin\n\
                   HOME=/home/me\n\
                   TERM=xterm-256color\n\
                   LANG=en_US.UTF-8\n";
        assert_eq!(detect(env), Kind::EnvDump);
    }

    #[test]
    fn detects_terraform_plan() {
        let t = "aws_instance.web: Refreshing state... [id=i-12345]\nPlan: 0 to add, 1 to change, 0 to destroy.\n";
        assert_eq!(detect(t), Kind::TerraformPlan);
    }

    #[test]
    fn detects_unified_diff() {
        let d = "diff --git a/x b/x\n@@ -1 +1 @@\n-old\n+new\n";
        assert_eq!(detect(d), Kind::UnifiedDiff);
    }

    #[test]
    fn detects_stacktrace_python() {
        let s = "Traceback (most recent call last):\n  File \"x.py\", line 1, in <module>\n    foo()\nValueError: bad\n";
        assert_eq!(detect(s), Kind::StackTrace);
    }

    #[test]
    fn detects_csv_not_tsv() {
        assert_eq!(detect("a,b,c\n1,2,3\n4,5,6\n"), Kind::Csv);
        assert_eq!(detect("a\tb\tc\n1\t2\t3\n"), Kind::Tsv);
    }

    #[test]
    fn detects_yaml() {
        // Generic YAML (no k8s-manifest shape) routes to Kind::Yaml.
        // Post-#439: need >= 5 yaml-key lines AND >= 70 % density to
        // qualify, matching the new dominance threshold.
        let plain = "name: example\nversion: 1.2.3\nauthor: alice\ndescription: cfg\nport: 8080\nhost: localhost\n";
        assert_eq!(detect(plain), Kind::Yaml);
        // k8s manifest YAML (apiVersion + kind) now routes to the dedicated
        // K8sManifest compactor introduced in iter #32.
        let k8s = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: foo\n";
        assert_eq!(detect(k8s), Kind::K8sManifest);
        // Negative: only 3 yaml-key lines should NOT trigger anymore.
        let sparse =
            "foo: bar\nbaz: 1\nqux: x\nrandom prose without colons\nmore prose\nstuff happens\n";
        assert_ne!(detect(sparse), Kind::Yaml);
    }

    #[test]
    fn yaml_detector_rejects_sparse_key_value_in_prose() {
        // Regression: a prose / script-output text with a handful of
        // incidental `word: value` lines mixed into many other lines
        // must NOT be classified as YAML. Previously: 3 scattered
        // yaml-key lines in the head triggered Kind::Yaml,
        // mis-routing 15k-token node outputs to the yaml compactor.
        let prose_with_some_keys = "\
some longer prose explanation about what this script does.
the next few lines describe a process step by step.
we initialize the runtime now.
step1: starting up
the runtime takes a moment to boot.
during boot we load a config file.
step2: loading config
the config has many entries.
plus a bunch more verbose logging messages.
this line is just text not a colon-value.
several more lines of pure narrative text.
followed by more explanation.
this script doesn't really output anything structured.
step3: done loading
final note for the curious reader.
nothing else of interest here.
the end of the prose section.
just trailing words.
even more words to fill the head.
last line to make twenty.
";
        // 3 yaml-key lines (step1/step2/step3) out of ~20 non-blank
        // lines = 15% density. The new dominance check (>=50%)
        // rejects this — the old absolute-count-3 check accepted it.
        assert_eq!(
            detect(prose_with_some_keys),
            Kind::Unknown,
            "sparse `key:` in prose must not classify as YAML"
        );
    }

    #[test]
    fn yaml_detector_still_accepts_dense_yaml() {
        // Sanity: a real YAML config should still be detected even
        // when it has comments interleaved (comments are excluded
        // from the denominator of the density check).
        let dense = "\
# top-level config
api_endpoint: https://example.com
api_key: redacted
# timeouts
read_timeout: 30
write_timeout: 60
# limits
max_connections: 100
max_retries: 5
";
        assert_eq!(detect(dense), Kind::Yaml);
    }

    #[test]
    fn detects_challenge_before_html() {
        let t = "<!DOCTYPE html><html><body><div class=\"cf-turnstile\" data-sitekey=\"x\"></div></body></html>";
        assert_eq!(detect(t), Kind::Challenge);
    }

    #[test]
    fn auto_detect_seo_prose_with_engine_urls_not_serp() {
        // #709/#710 integration test: SEO prose mentioning search engine
        // URLs must NOT route to Kind::Serp via the full auto::detect
        // chain. Pre-#709 the engine-URL signals alone triggered SERP →
        // routed to extract() → ~empty output → 90% data loss. Now
        // structural signals are required AND a source-or-doc pre-filter
        // catches docs. The expected classification is Unknown (plain
        // prose with no other format markers — passthrough).
        let prose = "Programmatic SaaS SEO playbook\n\n\
                     Optimize for google.com/search and bing.com/search by \
                     auditing your sitemap.xml. duckduckgo.com and \
                     search.brave.com rank pages similarly to Google Search. \
                     Focus on:\n\
                     - Topic clusters with internal linking\n\
                     - Schema.org structured data markup\n\
                     - Page speed metrics that affect rankings\n";
        let kind = detect(prose);
        assert_ne!(
            kind, Kind::Serp,
            "SEO prose mentioning engine URLs must NOT route to SERP (would destroy via extract): got {kind:?}"
        );
    }

    #[test]
    fn auto_detect_markdown_with_html_snippets_not_serp() {
        // #710 lock-in at the auto::detect level: a markdown doc with
        // SERP markup in code blocks must NOT route to SERP. The
        // source-or-doc pre-filter catches it before structural signals
        // would otherwise fire. Expected: Kind::Markdown (the right
        // destination — the doc gets its outline extracted).
        let md = "# SERP markup reference\n\
                  \n\
                  ## Google\n\
                  The results live under `id=\"rso\"` inside the parent `id=\"search\"` div.\n\
                  Example markup:\n\
                  \n\
                  ```html\n\
                  <div id=\"search\"><div id=\"rso\">...</div></div>\n\
                  ```\n\
                  \n\
                  ## Bing\n\
                  Bing uses `id=\"b_results\"` and `class=\"b_algo\"` for organic hits.\n\
                  \n\
                  See [SEO 101](https://example.com/seo) for more.\n";
        let kind = detect(md);
        assert_ne!(
            kind,
            Kind::Serp,
            "markdown documenting SERP markup must NOT route to SERP: got {kind:?}"
        );
    }

    #[test]
    fn auto_detect_real_google_serp_still_routes_to_serp() {
        // #709/#710 regression guard: the prose / markdown rejections
        // must not regress real SERP detection. A genuine Google SERP
        // HTML page still classifies as Kind::Serp.
        let html = "<!DOCTYPE html><html><head><title>foo - Google Search</title></head>\
                    <body><div id=\"search\"><div id=\"rso\">\
                    <a href=\"https://rust-lang.org/\"><h3>Rust language</h3></a>\
                    <div>A language empowering everyone to build reliable software.</div>\
                    </div></div></body></html>";
        assert_eq!(detect(html), Kind::Serp);
    }

    #[test]
    fn challenge_dispatch_emits_remediation_not_inflated_markdown() {
        // Regression for #463: a detected anti-bot challenge page is
        // garbage to the agent — markdown-ifying it (prior behaviour)
        // inflated 20/20 production calls (+129 tokens avg) AND wasted
        // tokens summarising a CAPTCHA wall as if it were the article.
        // Dispatch must instead return the brief stealth remediation.
        let challenge = "<!DOCTYPE html><html><body>\
                         <div class=\"cf-turnstile\" data-sitekey=\"x\"></div>\
                         <p>Verifying you are human. This may take a few seconds.</p>\
                         <script src=\"https://challenges.cloudflare.com/turnstile/v0/api.js\"></script>\
                         </body></html>";
        let (kind, out, _sav) = compact(challenge);
        assert_eq!(kind, Kind::Challenge);
        // The remediation banner identifies the challenge kind.
        assert!(
            out.contains("challenge=cloudflare-turnstile"),
            "remediation banner missing: {out}"
        );
        // ...and the advice must be something the CALLER can act on: the
        // path that does not need a browser at all. Naming the libraries a
        // stealth fetcher is built from was acted on zero times in eleven.
        assert!(
            out.contains("tokenade web saved.html"),
            "remediation must name the saved-page path: {out}"
        );
        // It must also say the obvious next move does not work — three of
        // eleven advisories were followed by a retry of the same URL.
        assert!(
            out.contains("immediate retry"),
            "remediation must rule out a plain retry: {out}"
        );
        // CRITICAL: must NOT inflate. The remediation is bounded ~400
        // chars regardless of input length; even on a small input the
        // markdown'd HTML used to be larger.
        assert!(
            !out.contains("<script"),
            "challenge HTML leaked into output (should be remediation only): {out}"
        );
    }

    #[test]
    fn unknown_passes_through() {
        let t = "totally arbitrary prose that doesn't fit any format.";
        assert_eq!(detect(t), Kind::Unknown);
        let (kind, out, _) = compact(t);
        assert_eq!(kind, Kind::Unknown);
        assert_eq!(out, t);
    }

    #[test]
    fn end_to_end_dispatch_to_notebook() {
        let nb = r#"{"cells":[{"cell_type":"code","source":["a=1\n"]}],"nbformat":4,"nbformat_minor":5,"metadata":{"kernelspec":{"language":"python"}}}"#;
        let (kind, out, sav) = compact(nb);
        assert_eq!(kind, Kind::JupyterNotebook);
        assert!(out.contains("Cell 1 (code)"));
        assert!(out.contains("a=1"));
        assert!(sav.percent() >= 0.0);
    }

    #[test]
    fn end_to_end_terraform_routes_correctly() {
        // Hand-built terraform plan with a tiny change.
        let mut t = String::new();
        for i in 0..10 {
            let _ = writeln!(t, "aws_instance.x{i}: Refreshing state... [id=i-{i:08x}]");
        }
        t.push_str("\n  # aws_instance.web will be updated in-place\n  ~ resource \"aws_instance\" \"web\" {\n      ~ size = 1 -> 2\n    }\n\nPlan: 0 to add, 1 to change, 0 to destroy.\n");
        let (kind, out, sav) = compact(&t);
        assert_eq!(kind, Kind::TerraformPlan);
        assert!(out.contains("Refreshing state"));
        assert!(out.contains("~ size"));
        assert!(sav.percent() > 30.0, "got {:?}", sav);
    }
    /// A cargo build log handed over as CONTENT reaches the cargo
    /// compactor. It used to be reachable only from the command table
    /// (`("cargo", "build")`), so the same bytes folded to nothing when
    /// tokenade was handed the file instead of running the build. Measured
    /// 2026-08-27 on a real 420-crate cold build: 13 493 bytes delivered
    /// whole before, 595 after.
    #[test]
    fn a_cargo_build_log_read_from_a_file_is_claimed_by_cargo() {
        let mut log = String::new();
        for i in 0..300 {
            let _ = writeln!(log, "   Compiling crate_{i} v0.{i}.0");
        }
        log.push_str("    Finished `dev` profile [unoptimized] target(s) in 9.11s\n");
        let (kind, out, _) = compact(&log);
        assert_eq!(kind, Kind::CargoBuild, "{out}");
        assert!(
            out.len() * 10 < log.len(),
            "folded to {} of {}",
            out.len(),
            log.len()
        );
    }

    /// The three conditions are each load-bearing. Remove any one of them and
    /// this test names which.
    #[test]
    fn each_cargo_condition_refuses_something_real() {
        let body = |n: usize| {
            let mut s = String::new();
            for i in 0..n {
                let _ = writeln!(s, "   Compiling crate_{i} v0.{i}.0");
            }
            s
        };
        let fin = "    Finished `dev` profile [unoptimized] target(s) in 9.11s\n";

        // ONE `Compiling` line is a sentence somebody wrote, not a build.
        let one = format!("{}{fin}", body(1));
        assert_ne!(compact(&one).0, Kind::CargoBuild, "one line is not a build");

        // No terminator: a snippet quoting the compile chatter, with nothing
        // to say the build ever ended.
        let headless = body(30);
        assert_ne!(
            compact(&headless).0,
            Kind::CargoBuild,
            "a build with no sign-off is a quotation"
        );

        // Markdown structure: a release note SHOWING a build. The census that
        // set the rule dropped every `.md` on exactly this clause.
        // A note file that TITLES a pasted build log. One heading and no
        // fence is a single marker, which the markdown detector declines —
        // so this is the shape where the document clause is the only thing
        // standing between a document and the cargo compactor.
        let doc = format!("# Release 1.2.3\n\n{}{fin}", body(30));
        assert_ne!(
            compact(&doc).0,
            Kind::CargoBuild,
            "a document that quotes a build is a document"
        );
    }
}

#[cfg(test)]
mod silent_deletion_sweep {
    use super::*;

    /// A compactor may SHORTEN, never silently DELETE a token that carries
    /// meaning.
    ///
    /// `strip_inline_html` once removed `<hash>`, `<pattern>`, `<file>` —
    /// including inside code spans — because any `<word>` was assumed to be a
    /// tag and an unrecognised one fell through every branch into the cursor
    /// advance. `tokenade expand-ref <hash>` reached the agent as
    /// `tokenade expand-ref `. That is a change of meaning wearing the clothes
    /// of a fold, and nothing in the output said so.
    ///
    /// This sweeps the same QUESTION across every kind rather than the same
    /// code: feed each detector a payload it will claim, carrying markers that
    /// are unusual but meaningful, and require that either the marker survives
    /// or the output admits it dropped lines. A compactor that legitimately
    /// summarises says how much it folded; one that quietly eats a token does
    /// not.
    #[test]
    fn no_compactor_silently_eats_an_unrecognised_token() {
        // (label, payload). Each payload is shaped so its detector claims it
        // and carries markers that are unusual but MEANINGFUL. `ZQMARKER` is a
        // token no compactor can have a rule for; the angle-bracket forms are
        // the CLI-documentation idiom that was being deleted.
        let m = "ZQMARKER";
        let cases: Vec<(&str, String)> = vec![
            (
                "markdown",
                (0..40)
                    .map(|i| {
                        format!(
                            "## Section {i}\n\nUsage: `cmd --opt <hash>` puis <pattern> {m}{i}\n\nLitteral: `<br>` reste.\n\n- a\n- b\n"
                        )
                    })
                    .collect(),
            ),
            (
                "logs",
                (0..60)
                    .map(|i| format!("2026-08-25T10:00:{i:02}Z INFO worker={i} msg=\"see <hash> {m}\"\n"))
                    .collect(),
            ),
            (
                "envdump",
                (0..40).map(|i| format!("VAR_{i}=value <hash> {m}\n")).collect(),
            ),
            (
                "json",
                format!(
                    "{{\"items\":[{}],\"note\":\"{m} <hash>\"}}",
                    (0..60)
                        .map(|i| format!("{{\"id\":{i},\"name\":\"n{i}\"}}"))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            ),
            (
                "yaml",
                (0..40)
                    .map(|i| format!("key_{i}:\n  value: v{i}\n  note: \"{m} <hash>\"\n"))
                    .collect(),
            ),
            (
                "csv",
                format!(
                    "id,name,note\n{}",
                    (0..60)
                        .map(|i| format!("{i},n{i},{m}-<hash>\n"))
                        .collect::<String>()
                ),
            ),
            (
                "unified-diff",
                format!(
                    "--- a/f.rs\n+++ b/f.rs\n@@ -1,3 +1,4 @@\n context\n+let x = \"{m} <hash>\";\n context2\n{}",
                    (0..40)
                        .map(|i| format!("@@ -{i},2 +{i},2 @@\n line{i}\n+add{i}\n"))
                        .collect::<String>()
                ),
            ),
            (
                "ndjson",
                (0..60)
                    .map(|i| format!("{{\"i\":{i},\"note\":\"{m} <hash>\"}}\n"))
                    .collect(),
            ),
            (
                "accesslog",
                (0..60)
                    .map(|i| {
                        format!(
                            "127.0.0.1 - - [25/Aug/2026:10:00:{i:02} +0000] \"GET /p/{m} HTTP/1.1\" 200 {i}\n"
                        )
                    })
                    .collect(),
            ),
            (
                "metrics",
                "# HELP app_requests_total Total requests\n# TYPE app_requests_total counter\n"
                    .to_string()
                    + &(0..60)
                        .map(|i| format!("app_requests_total{{route=\"/{m}\",code=\"200\"}} {i}\n"))
                        .collect::<String>(),
            ),
            (
                "gitlog",
                (0..40)
                    .map(|i| {
                        format!("commit {i:040x}\nAuthor: a <a@b.c>\nDate: Mon Aug 25 10:00:00 2026\n\n    msg {m} {i}\n\n")
                    })
                    .collect(),
            ),
            (
                "netstat",
                format!(
                    "Proto Recv-Q Send-Q Local Address           Foreign Address         State\n{}",
                    (0..60)
                        .map(|i| format!("tcp        0      0 127.0.0.1:{}         0.0.0.0:*               LISTEN      {i}/{m}\n", 8000 + i))
                        .collect::<String>()
                ),
            ),
        ];

        let mut lost: Vec<String> = Vec::new();
        let mut seen_kinds: Vec<String> = Vec::new();
        for (label, payload) in cases {
            // The KIND's own compactor, not `auto::compact`. The latter has
            // an anti-pathological floor: when a fold saves almost nothing it
            // returns the INPUT, so the markdown case came back byte-identical
            // (2420 → 2419) and the sweep passed against a reintroduced tag-strip bug
            // — the defect was real, the payload simply never reached it.
            // Testing the dispatcher instead of the compactor measured the
            // floor, not the fold.
            let payload = payload.as_str();
            let kind = detect(payload);
            seen_kinds.push(format!("{label}={kind:?}"));
            let out = match kind {
                Kind::Markdown => crate::markdown::compact(payload),
                _ => compact(payload).1,
            };
            // Two different contracts, and conflating them is what made the
            // first version of this sweep useless.
            //
            //  * A CENSUS compactor replaces the items with their statistics
            //    on purpose — `find` folds to "61 file(s) across 1 top-level
            //    directory" plus a per-extension breakdown, keeping no
            //    individual name. Nothing is hidden; the shape of the answer
            //    is the count. Those kinds are named below.
            //  * Every other kind keeps LINES, folded or verbatim. There, a
            //    token that vanishes with no marker is the tag-strip class.
            //
            // The first version tried to tell them apart by asking whether the
            // output "mentions the line count" — a substring match on a
            // number, which any longer output satisfies by accident. It passed
            // against the reintroduced tag-strip defect, which is the one thing it
            // had to catch. Naming the census kinds is the honest way.
            const CENSUS_KINDS: &[Kind] = &[Kind::FileList, Kind::Disk];
            if CENSUS_KINDS.contains(&kind) {
                continue;
            }
            let admits = out.contains('\u{d7}')
                || out.contains("omitted")
                || out.contains("truncated")
                || out.contains("more line");
            // Only markers the payload actually CARRIES. Checking a fixed
            // list against every case reports absences that were never
            // presences: `<pattern>` appears only in the markdown payload, and
            // the sweep accused the env-dump compactor of deleting a token it
            // had never been given.
            for marker in ["ZQMARKER", "<hash>", "<pattern>", "`<br>`"] {
                if !payload.contains(marker) {
                    continue;
                }
                if !out.contains(marker) && !admits {
                    lost.push(format!(
                        "{label} ({kind:?}): `{marker}` removed and the output says nothing"
                    ));
                }
            }
        }
        // A payload the detector did not claim proves nothing about that
        // compactor — it exercised whatever generic path caught it instead.
        // Report the mapping so coverage is a fact and not an intention.
        println!("KINDS: {}", seen_kinds.join(" "));
        let generic = seen_kinds
            .iter()
            .filter(|k| k.ends_with("=Generic") || k.ends_with("=Unknown"))
            .count();
        assert!(
            generic <= 1,
            "{generic} payload(s) fell through to the generic path, so they test \
             nothing about their kind: {}",
            seen_kinds.join(" ")
        );
        assert!(
            lost.is_empty(),
            "a compactor removed a meaningful token without saying so:\n  {}",
            lost.join("\n  ")
        );
    }
}

#[cfg(test)]
mod misroute_audit {
    use super::*;

    /// For each real-world payload, compare the fold its DETECTED kind gives
    /// against what every other kind's compactor would give. A kind that folds
    /// dramatically better while keeping the payload's distinctive tokens is a
    /// misroute — that is exactly how `git log` was found sitting in the SQL
    /// compactor at 0.988 while its own gave 0.060.
    #[test]
    #[ignore = "measurement over a locally generated corpus; run explicitly"]
    fn report_better_compactors_than_the_detected_one() {
        use super::ALL_KINDS;
        let dir = "/tmp/claude-1000/-home-username--devworkspace-projects-tokenade/67caa6a3-1a47-4a19-b18f-81af922cce79/scratchpad/corpus";
        let Ok(rd) = std::fs::read_dir(dir) else {
            println!("AUDIT: no corpus");
            return;
        };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        files.sort();
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            if text.len() < 2000 {
                continue;
            }
            let name = f.file_name().unwrap().to_string_lossy().to_string();
            let kind = detect(&text);
            let mine = dispatch_kind(&text, kind).len();
            // A candidate must fold BETTER **and keep the payload's meaning**.
            //
            // Without the second half this audit is useless, and its first
            // version was: `ClaudeTranscript`, `UnifiedDiff` and `StackTrace`
            // came top on EVERY payload at ratios near 0.000 — they do not
            // fold foreign input, they discard it. A compactor that returns
            // nothing always "wins" on size, so size alone ranks destruction
            // first. Retention is what separates a fold from a deletion, and
            // it is the same distinction the tag-strip bug was about.
            //
            // The witnesses are the payload's RARE tokens — words appearing
            // once or twice in the whole text, which is what a summary of it
            // would have to name.
            let mut freq: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
            for w in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
                if w.len() >= 6 {
                    *freq.entry(w).or_default() += 1;
                }
            }
            let mut rare: Vec<&str> = freq
                .iter()
                .filter(|(_, n)| **n <= 2)
                .map(|(w, _)| *w)
                .collect();
            rare.sort_unstable();
            let witnesses: Vec<&str> = rare
                .iter()
                .step_by(rare.len().max(20) / 20)
                .copied()
                .take(20)
                .collect();
            let kept =
                |out: &str| -> usize { witnesses.iter().filter(|w| out.contains(**w)).count() };
            let mine_kept = kept(&dispatch_kind(&text, kind));

            let mut best: Vec<(f64, String)> = Vec::new();
            for k in ALL_KINDS {
                if *k == kind {
                    continue;
                }
                let out = dispatch_kind(&text, *k);
                // Better AND no worse at keeping the witnesses.
                if out.len() * 3 < mine && !out.is_empty() && kept(&out) >= mine_kept {
                    best.push((out.len() as f64 / text.len() as f64, format!("{k:?}")));
                }
            }
            best.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            let ratio = mine as f64 / text.len() as f64;
            if best.is_empty() {
                println!(
                    "AUDIT {name:<20} {kind:?} ratio={ratio:.3} kept={mine_kept}/{} — nothing beats it",
                    witnesses.len()
                );
            } else {
                let top: Vec<String> = best
                    .iter()
                    .take(3)
                    .map(|(r, k)| format!("{k}={r:.3}"))
                    .collect();
                println!(
                    "AUDIT {name:<20} {kind:?} ratio={ratio:.3} << {}",
                    top.join(" ")
                );
            }
        }
    }
}

#[cfg(test)]
mod lockfile_before_toml {

    /// The lockfile case through ROUTING. Two mutations survived the first round and
    /// pointed at the same gap: the `lockfile` guards call `compact`
    /// directly, so nothing checked that a `package-lock.json` REACHES
    /// that compactor. It carries the `.json` extension, and that is how
    /// the path hint sent it to the generic JSON compactor.
    const PACKAGE_LOCK: &str = concat!(
        r#"{"name":"demo","version":"1.0.0","lockfileVersion":3,"packages":{"#,
        r#""":{"name":"demo","version":"1.0.0"},"#,
        r#""node_modules/left-pad":{"version":"1.3.0","#,
        r#""resolved":"https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz","#,
        r#""integrity":"sha512-XI5MPzVNApjAyhQzphX8BkmKsKUxD4LdyK24iZeQGinBN9yTQT3bFlCB"}}}"#,
    );

    /// By CONTENT: a lockfile piped, pasted or renamed.
    #[test]
    fn a_json_lockfile_is_detected_by_its_shape() {
        assert_eq!(
            crate::auto::detect(PACKAGE_LOCK),
            crate::auto::Kind::Lockfile,
            "the content must suffice"
        );
    }

    /// By the PATH HINT, which is the real route of a `tokenade read`:
    /// the `.json` extension won, and the JSON noise-key removal never
    /// heard of `resolved` or `integrity`.
    #[test]
    fn the_json_extension_does_not_win_over_a_lockfile() {
        let (kind, out, _) =
            crate::auto::compact_with_path_hint(PACKAGE_LOCK, "package-lock.json");
        assert_eq!(kind, crate::auto::Kind::Lockfile, "routed to {kind:?}: {out}");
        assert!(out.contains("left-pad 1.3.0"), "{out}");
        assert!(!out.contains("registry.npmjs.org"), "{out}");
    }
    use super::*;

    const CARGO_LOCK: &str = "\
# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = \"serde_json\"
version = \"1.0.140\"
source = \"registry+https://github.com/rust-lang/crates.io-index\"
checksum = \"20068b6e96dc6c9bd23e01df8827e6c7e1f2fddd43c21810382803c136b99373\"
dependencies = [
 \"itoa\",
 \"ryu\",
]

[[package]]
name = \"tokenade\"
version = \"1.1.7\"
dependencies = [
 \"serde_json\",
]
";

    const CARGO_TOML: &str = "\
[package]
name = \"tokenade-core\"
version = \"1.1.7\"
edition = \"2021\"

[dependencies]
serde_json = { workspace = true }
reqwest = { workspace = true }
";

    /// A lockfile IS valid TOML, so the generic branch claimed it first while
    /// its own comment said lockfiles were handled elsewhere. MEASURED on a
    /// 135 KB `Cargo.lock`: 135 634 bytes (0.999) from the TOML compactor
    /// against 10 161 (0.075) from the lockfile one, every package name kept
    /// either way. The SPECIFIC detector must run first.
    #[test]
    fn a_lockfile_is_not_claimed_by_the_generic_toml_branch() {
        assert_eq!(detect(CARGO_LOCK), Kind::Lockfile);
        let folded = dispatch_kind(CARGO_LOCK, Kind::Lockfile);
        assert!(
            folded.len() < CARGO_LOCK.len(),
            "the lockfile compactor did not fold it"
        );
        for name in ["serde_json", "tokenade"] {
            assert!(folded.contains(name), "package {name} was lost: {folded}");
        }
    }

    /// …and a hand-written `Cargo.toml` still goes to the TOML branch. The
    /// lockfile detector demands the `@generated` header, or `[[package]]`
    /// TOGETHER WITH a `name = ` line — `[package]` singular has neither, so
    /// moving it up cannot steal ordinary config.
    #[test]
    fn a_hand_written_toml_is_still_toml() {
        assert_eq!(detect(CARGO_TOML), Kind::TomlConfig);
    }
}

#[cfg(test)]
mod locale_parity {
    use super::*;

    /// The SAME command, run in two locales, must be detected as the same kind
    /// and folded about as hard.
    ///
    /// Three separate locale defects turned up one at a time this session: a
    /// French `git status` read as a Makefile, `journalctl` unfoldable on any
    /// non-English machine, and both traced to the same habit — a
    /// parser encoding ENGLISH as if it were the structure. Finding them one
    /// by one is how you find two of three; running the corpus twice is how
    /// you find the third.
    #[test]
    #[ignore = "measurement over a locally generated corpus; run explicitly"]
    fn same_command_two_locales_same_treatment() {
        let base = "/tmp/claude-1000/-home-username--devworkspace-projects-tokenade/67caa6a3-1a47-4a19-b18f-81af922cce79/scratchpad";
        let (en, fr) = (
            format!("{base}/loc_en_US.utf8"),
            format!("{base}/loc_fr_FR.utf8"),
        );
        let Ok(rd) = std::fs::read_dir(&en) else {
            println!("PARITY: no corpus");
            return;
        };
        let mut names: Vec<String> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        for n in names {
            let (Ok(a), Ok(b)) = (
                std::fs::read_to_string(format!("{en}/{n}")),
                std::fs::read_to_string(format!("{fr}/{n}")),
            ) else {
                continue;
            };
            if a.len() < 1000 || b.len() < 1000 {
                continue;
            }
            let (ka, kb) = (detect(&a), detect(&b));
            let ra = dispatch_kind(&a, ka).len() as f64 / a.len() as f64;
            let rb = dispatch_kind(&b, kb).len() as f64 / b.len() as f64;
            let same_kind = ka == kb;
            let close = (ra - rb).abs() < 0.10;
            let flag = if same_kind && close { "ok  " } else { "DIFF" };
            println!("PARITY {flag} {n:<18} en={ka:?}/{ra:.3}  fr={kb:?}/{rb:.3}");
        }
    }
}

#[cfg(test)]
mod kv_blocks_routing {
    use super::*;

    fn pacman(n: usize) -> String {
        (0..n)
            .map(|i| {
                format!(
                    "Name            : pkg{i}\nVersion         : 1.{i}.0\nDescription     : a package number {i}\nArchitecture    : x86_64\nLicenses        : GPL-3.0-or-later\nInstalled Size  : {}.00 KiB\n\n",
                    100 + i
                )
            })
            .collect()
    }

    /// The COMMAND path already folded `pacman -Qi` (33 599 → 4 039 bytes
    /// through `wrap`); the same bytes arriving as CONTENT were handed back
    /// whole. Measured after wiring: 1.000 → 0.073, and identical in both
    /// locales.
    #[test]
    fn a_key_value_record_stream_is_folded_as_content_too() {
        let t = pacman(30);
        assert_eq!(detect(&t), Kind::KvBlocks);
        let out = dispatch_kind(&t, Kind::KvBlocks);
        assert!(
            out.len() * 4 < t.len(),
            "not folded: {} of {}",
            out.len(),
            t.len()
        );
        assert!(
            out.contains("30 record(s)"),
            "the count was not declared: {out}"
        );
        // The first records survive verbatim — this fold keeps records whole,
        // never cutting one in half.
        assert!(out.contains("Name            : pkg0"));
    }

    /// The French form of the same output must be treated the same way. The
    /// keys are translated (`Nom`, `Licences`) and padded differently, so a
    /// detector keying on the English words would fail here.
    #[test]
    fn the_french_form_routes_identically() {
        let t: String = (0..30)
            .map(|i| {
                format!(
                    "Nom                      : pkg{i}\nVersion                  : 1.{i}.0\nDescription              : un paquet numero {i}\nArchitecture             : x86_64\nLicences                 : GPL-3.0-or-later\n\n"
                )
            })
            .collect();
        assert_eq!(detect(&t), Kind::KvBlocks);
    }

    /// …and it must steal nothing. Each of these has its own detector far
    /// above, and this arm runs LAST precisely so they win.
    #[test]
    fn it_steals_neither_yaml_nor_toml_nor_env_nor_prose() {
        let yaml: String = (0..10)
            .map(|i| format!("service_{i}:\n  image: nginx\n  port: 80{i}\n\n"))
            .collect();
        assert_ne!(detect(&yaml), Kind::KvBlocks, "YAML was claimed");

        let toml: String = (0..10)
            .map(|i| format!("[section{i}]\nname = \"a\"\nversion = \"1.0\"\n\n"))
            .collect();
        assert_ne!(detect(&toml), Kind::KvBlocks, "TOML was claimed");

        let env: String = (0..20).map(|i| format!("VAR_{i}=value{i}\n")).collect();
        assert_ne!(detect(&env), Kind::KvBlocks, "an env dump was claimed");

        // Prose with colons in it — the shape this arm could plausibly net.
        let prose = "Note : ceci est une phrase.\n\nAutre : et une seconde.\n\nEncore : une troisieme.\n\nEnfin : la derniere.\n";
        assert_ne!(detect(prose), Kind::KvBlocks, "prose was claimed");

        // A single stanza is not a stream.
        let one = "Name            : pkg\nVersion         : 1.0\nLicenses        : MIT\n";
        assert_ne!(detect(one), Kind::KvBlocks, "one record is not a stream");
    }
}

#[cfg(test)]
mod tabular_command_parity {
    use super::*;

    fn ps_output(n: usize) -> String {
        let mut s = String::from(
            "USER         PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND\n",
        );
        for i in 0..n {
            s.push_str(&format!(
                "username  {:>6}  0.0  0.1  12345  6789 ?        S    10:00   0:00 /usr/bin/worker{i}\n",
                1000 + i
            ));
        }
        s
    }

    /// A table handed over as CONTENT must reach the same compactor the
    /// COMMAND path uses. Measured before wiring: `ps aux` folded to 0.069
    /// through its own module and 0.481 through the generic table fold — 6.9×,
    /// decided only by whether tokenade ran the command itself.
    #[test]
    fn a_named_table_reaches_its_own_compactor_as_content() {
        let t = ps_output(200);
        assert_eq!(detect(&t), Kind::TabularCli);
        let content = dispatch_kind(&t, Kind::TabularCli);
        let command = crate::cmd::dispatch_auto("ps", &t).expect("ps has a module");
        assert_eq!(
            content, command,
            "the content path did not reach the ps compactor"
        );
        assert!(
            content.len() * 3 < t.len(),
            "not folded: {} of {}",
            content.len(),
            t.len()
        );
    }

    /// …and a table whose header names nothing keeps the generic fold rather
    /// than being handed to some arbitrary module.
    #[test]
    fn an_unnamed_table_still_gets_the_generic_fold() {
        let mut t = String::from("ALPHA   BETA   GAMMA\n");
        for i in 0..80 {
            t.push_str(&format!("a{i}      b{i}     c{i}\n"));
        }
        let hint = detectors::tabular_command_hint(&t);
        assert!(hint.is_none(), "an unknown header was mapped to {hint:?}");
    }

    /// The header→command map must name the right command for each shape.
    #[test]
    fn each_known_header_names_its_command() {
        for (head, want) in [
            ("USER PID %CPU COMMAND\n", "ps"),
            ("UID PID PPID CMD\n", "ps"),
            ("COMMAND PID USER FD TYPE NODE NAME\n", "lsof"),
            ("Filesystem Size Used Avail Mounted on\n", "df"),
            ("CONTAINER ID IMAGE COMMAND\n", "docker"),
            ("REPOSITORY TAG IMAGE ID\n", "docker"),
            ("NAME READY STATUS RESTARTS\n", "kubectl"),
            ("Proto Recv-Q Send-Q Local Address\n", "netstat"),
        ] {
            assert_eq!(
                detectors::tabular_command_hint(head),
                Some(want),
                "wrong command for {head:?}"
            );
        }
    }
}

#[cfg(test)]
mod candidate_parity {
    use super::*;

    /// A tar listing whose entries all sit in DISTINCT top-level directories:
    /// the archive compactor has nothing to group, so it prints one line per
    /// entry, while the template fold sees a single shape.
    fn scattered_listing(n: usize) -> String {
        (0..n)
            .map(|i| {
                format!(
                    "-rw-r--r-- 0/0   {:>7} 2026-08-19 03:07 dir{i}/pkg/app.ts\n",
                    90000 + i
                )
            })
            .collect()
    }

    /// The competing candidates must reach BOTH paths. Measured 2026-08-26
    /// before the fix: 94 890 bytes in, 39 538 out through the content path
    /// and ~1 600 through the command path — 24x, decided only by which door
    /// the bytes came through.
    #[test]
    fn the_fold_candidate_reaches_the_content_path() {
        let t = scattered_listing(1500);
        let (kind, structured, sav) = compact(&t);
        assert_eq!(kind, Kind::ArchiveListing);
        // The structured compactor on its own leaves a line per entry…
        assert!(
            structured.lines().count() > 1000,
            "fixture no longer exercises the weak structured path: {} lines",
            structured.lines().count()
        );
        // …and the shared candidate chooser replaces it with the template.
        let (suffix, out, best) =
            crate::filter::best_auto_candidate(&t, sav.tokens_before, kind, structured, sav);
        assert_eq!(suffix, "+fold", "the fold candidate did not win");
        assert!(
            out.len() * 100 < t.len(),
            "expected a template fold, got {} of {}",
            out.len(),
            t.len()
        );
        assert!(best.tokens_after < 200, "{} tokens", best.tokens_after);
    }

    /// …and it must stay a COMPETING candidate: a payload whose rows genuinely
    /// differ produces many templates, so the structured summary keeps the
    /// round. Without this the fix would be a replacement, not a candidate.
    #[test]
    fn a_payload_with_no_shared_shape_keeps_the_structured_answer() {
        let mut t = String::new();
        for i in 0..400 {
            t.push_str(&format!(
                "-rw-r--r-- 0/0 {i} 2026-08-1{} 0{}:0{} d{}/{}\n",
                i % 9,
                i % 9,
                i % 9,
                "x".repeat(1 + i % 17),
                "seg/".repeat(1 + i % 5)
            ));
        }
        let (kind, structured, sav) = compact(&t);
        let (suffix, _out, _best) =
            crate::filter::best_auto_candidate(&t, sav.tokens_before, kind, structured, sav);
        assert_ne!(
            suffix, "+fold",
            "the fold won on a payload with no shared shape — it is replacing, not competing"
        );
    }
}

#[cfg(test)]
mod line_numbered_input {
    use super::*;

    fn article() -> String {
        let mut s = String::from("# A document\n\n");
        for i in 0..120 {
            s.push_str(&format!(
                "## Section {i}\n\nA paragraph of prose that says something about section {i} and \
                 then continues for long enough to be worth folding.\n\n- point one\n- point two\n\n"
            ));
        }
        s
    }

    fn number_it(t: &str) -> String {
        t.lines()
            .enumerate()
            .map(|(i, l)| format!("{:>6}\t{l}\n", i + 1))
            .collect()
    }

    /// `cat -n` broke the detector, the deduper and the disclose summary at
    /// once. Measured 2026-08-26 on a 26 KB `.mdx`: `cat FILE` delivered 3 294
    /// bytes, `cat -n FILE` 14 743, and reading the numbered file back returned
    /// 27 653 of a 27 652-byte input — no fold at all.
    #[test]
    fn a_numbered_document_folds_like_the_document_it_is() {
        let plain = article();
        let numbered = number_it(&plain);
        let (_k, plain_out, _s) = compact(&plain);
        let (kind, num_out, sav) = compact(&numbered);
        assert_ne!(kind, Kind::Unknown, "the gutter still hides the format");
        assert!(
            num_out.len() * 2 < numbered.len(),
            "numbered input folded to {} of {}",
            num_out.len(),
            numbered.len()
        );
        // Within a banner's width of what the same document folds to
        // unnumbered — plus the numbers themselves, which `cat -n` asked for
        // and which now come back on every line delivered verbatim.
        let numbers: usize = num_out.lines().filter_map(crate::line_gutter::strip_one).map(|(n, _)| n.to_string().len() + 1).sum();
        assert!(numbers > 0, "no number came back");
        assert!(
            num_out.len() < plain_out.len() + numbers + 200,
            "numbered {} vs plain {}",
            num_out.len(),
            plain_out.len()
        );
        assert!(sav.tokens_after < sav.tokens_before);
    }

    /// …and it SAYS the numbers went, with the range they covered.
    #[test]
    fn the_gutter_is_declared_not_silently_dropped() {
        let numbered = number_it(&article());
        let (_k, out, _s) = compact(&numbered);
        assert!(out.starts_with("[tokenade:line-numbers] "), "{:.90}", out);
        assert!(out.contains("gutter 1-"), "{:.90}", out);
    }

    /// The class this rule exists to avoid: a `.tsv` whose first column is a
    /// sequential row counter matches every rule a gutter does. It detects as
    /// TSV on its own, so the scan must never see it — stripping there would
    /// delete a data column.
    #[test]
    fn a_sequential_first_column_of_real_data_is_left_alone() {
        let mut t = String::from("id\tname\temail\n");
        for i in 1..=60 {
            t.push_str(&format!("{i}\tuser{i}\tu{i}@example.com\n"));
        }
        let (_k, out, _s) = compact(&t);
        assert!(
            !out.contains("[tokenade:line-numbers]"),
            "a data column was taken for a gutter:\n{out:.200}"
        );
    }

    /// A gutter over content no compactor improves must cost nothing: no
    /// banner, and the numbers the agent asked for still there.
    #[test]
    fn a_gutter_over_unfoldable_content_is_left_alone() {
        let mut t = String::new();
        for i in 1..=30 {
            t.push_str(&format!("{i:>4}\tzq{i}wx {}\n", "y".repeat(i % 7 + 1)));
        }
        let (_k, out, _s) = compact(&t);
        assert!(
            !out.contains("[tokenade:line-numbers]"),
            "paid a banner for nothing:\n{out:.200}"
        );
    }
}

#[cfg(test)]
mod bracketed_log_tags {
    use super::*;

    /// A Maven reactor build: every line begins `[INFO] `, and three separate
    /// detectors claimed it in turn, each returning the input whole while the
    /// `JvmBuild` compactor that folds it sat behind them.
    fn maven(modules: usize) -> String {
        let mut s = String::from("[INFO] Scanning for projects...\n[INFO] Reactor Build Order:\n");
        for i in 0..modules {
            let m = format!("svc-{i:02}");
            s.push_str(&format!(
                "[INFO] \n\
                 [INFO] ------------------------------------------------------------------------\n\
                 [INFO] Building {m} 2.0.0-SNAPSHOT\n\
                 [INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ {m} ---\n\
                 [INFO] Compiling 24 source files with javac [debug release 17] to target/classes\n\
                 [INFO] Running com.acme.{m}.SmokeTest\n\
                 [INFO] Tests run: 12, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.4 s\n\
                 [INFO] Building jar: /w/{m}/target/{m}-2.0.0-SNAPSHOT.jar\n"
            ));
        }
        s.push_str("[INFO] BUILD SUCCESS\n[INFO] Total time:  04:12 min\n");
        s
    }

    /// Measured 2026-08-26: 53 871 bytes in, 53 871 out. `TomlConfig` claimed
    /// it first (`[INFO] Building …` read as a table header), then `Ini`
    /// (sixty identical `[INFO]` tags read as sixty sections), then `Json`
    /// (starts with `[`, and `Building jar: /w/…` supplies the colon-space).
    /// Each returned the input whole. 53 871 -> 6 556 now.
    #[test]
    fn a_maven_reactor_build_reaches_the_compactor_that_folds_it() {
        let t = maven(60);
        assert_eq!(detect(&t), Kind::JvmBuild, "a looser detector claimed it");
        let (_k, out, sav) = compact(&t);
        assert!(
            out.len() * 4 < t.len(),
            "{} of {} bytes",
            out.len(),
            t.len()
        );
        assert!(
            sav.tokens_after * 2 < sav.tokens_before,
            "tokens {} -> {}",
            sav.tokens_before,
            sav.tokens_after
        );
        // The module names are what an agent needs from a reactor build.
        assert!(out.contains("svc-"), "every module name was dropped");
    }

    /// The third floor, on the shape that found it.
    ///
    /// `strace` output is read to learn WHICH files a process touched, and the
    /// compactor answers `40 syscalls (1 unique)` plus a tally — every path
    /// gone, 944 tokens to 19, no disclosure. `structured_or_generic` has
    /// refused that class since the `auto:pkginstall`-on-SSH leak and answers
    /// `auto:strace+rejected`; `auto::compact` did not ask, and a dozen callers
    /// take its result straight to the agent.
    ///
    /// Found by planting the marker in a field the format itself defines — an
    /// `openat` filename — rather than on a line of its own. A foreign line
    /// changes what the document detects as, so every compactor that demands
    /// format purity had never been reached at all.
    #[test]
    fn a_summary_that_no_longer_speaks_for_its_input_is_refused() {
        let marker = "libcustom_af41.so";
        let doc: String = (0..40)
            .map(|i| {
                let f = if i == 8 {
                    format!("/opt/{marker}")
                } else {
                    format!("/usr/lib/x{i}.so")
                };
                format!(
                    "openat(AT_FDCWD, \"{f}\", O_RDONLY|O_CLOEXEC) = {}\n",
                    3 + i
                )
            })
            .collect();
        assert_eq!(detect(&doc), Kind::Strace, "a looser detector claimed it");
        let (_k, out, _sav) = compact(&doc);

        // THE CONTRACT CHANGED, and this records why rather than being quietly
        // relaxed. When this test was written the strace compactor said
        // nothing at all about the syscall lines it dropped, so the only thing
        // standing between the agent and `40 syscalls (1 unique)` was the
        // floor, and the floor returns the INPUT — the marker survived
        // verbatim. A later fix gave the compactor the disclosure its three
        // neighbouring blocks already had, which lifts the output past the
        // floor's flat ceiling, so the floor no longer fires here and the
        // marker is elided.
        //
        // That is the better outcome and the repo's own rule says so: for a
        // NON-error line, disclosed elision is the contract (the router
        // fuzzer's third invariant), and only an ERROR needs the bytes
        // themselves (its fourth). What must never happen is what happened
        // before either change: gone, with nothing to say so.
        assert!(
            out.contains(marker) || out.contains("elided"),
            "the path an agent reads strace FOR vanished unannounced:\n{out}"
        );
        assert!(
            out.contains("syscall line(s) elided"),
            "the drop must name itself, in the vocabulary the syscall-type \
             table above it already uses:\n{out}"
        );
    }

    /// The floor itself, asked where it lives.
    ///
    /// `a_summary_that_no_longer_speaks_for_its_input_is_refused` used to
    /// exercise it end to end; a later fix repaired the compactor that produced its
    /// case, and a sweep over all 29 fuzz generators at five sizes then found
    /// **no** input left that a compactor claims and reduces below thirty
    /// tokens without disclosure. That is a good state for the codebase and a
    /// bad one for a backstop: nothing tested it, and two mutations of it came
    /// back green for the third reason — no test at all.
    ///
    /// So it is pinned at the seam instead, the same way the tests pin the redraw
    /// replay to its entry points: `guarded` is where the other two floors
    /// live, and this one has to be asked beside them.
    #[test]
    fn the_pathological_floor_is_wired_into_guarded() {
        let src = &crate::source_guard::library_module_family("auto");
        let body = src
            .split("fn guarded(")
            .nth(1)
            .expect("guarded() must exist — this guard names it");
        let end = body.find("\nfn ").unwrap_or(body.len());
        let body = &body[..end];
        for needed in [
            "summarises_nothing_read",
            "reduction_is_pathological",
            "out.trim().is_empty()",
        ] {
            assert!(
                body.contains(needed),
                "guarded() no longer asks `{needed}` — the three floors are \
                 asked together or the doors diverge again"
            );
        }
    }

    /// `.log` is a container, not a format.
    ///
    /// It is settled that a path hint wins outright — a filename is an
    /// assertion by the user — and that holds for `.csv` or `.rs`, which say
    /// what the bytes ARE. `.log` says only that somebody redirected a
    /// program's output somewhere. Measured across the fuzz corpora, naming
    /// each one `run.log` sent a journald dump, a cargo build and an access
    /// log to the generic Logs compactor, which folded NOTHING on the first
    /// two while their own compactors fold 4.7× and 3×.
    ///
    /// Refined through the mechanism that already existed for exactly this
    /// shape: the `Xml` → `JUnit` arm is the same idea, the hint having the
    /// family right and the content the format.
    #[test]
    fn a_log_extension_does_not_hide_a_specific_format() {
        // journald, which has its own compactor.
        let journal: String = (0..40)
            .map(|i| {
                format!(
                    "Aug 27 10:0{}:00 host svc[{}]: worker {i} ready\n",
                    i % 10,
                    1000 + i
                )
            })
            .collect();
        assert_eq!(
            detect(&journal),
            Kind::Journal,
            "fixture must reach Journal"
        );
        let (k, _o, s) = compact_with_path_hint(&journal, "/var/log/run.log");
        assert_eq!(k, Kind::Journal, "the container extension hid the format");
        let (_k2, _o2, s2) = compact(&journal);
        assert_eq!(
            s.tokens_after, s2.tokens_after,
            "naming a file must not change what it costs when the content is \
             unambiguous: {} named vs {} unnamed",
            s.tokens_after, s2.tokens_after
        );
    }

    /// `.json` asserts a real format — and a few kinds ARE that format.
    ///
    /// Every OpenAPI spec on earth is called `openapi.json` or `swagger.json`,
    /// and `detectors.rs` says of them, in its own words, that they "ARE valid
    /// JSON but a dedicated compactor reduces them by 95%+". The hint was
    /// hiding that compactor from the files most likely to carry it.
    #[test]
    fn a_json_extension_does_not_hide_a_json_dialect() {
        let mut spec = String::from(
            "{\n  \"openapi\": \"3.0.3\",\n  \"info\": {\"title\": \"Demo\", \"version\": \"1\"},\n  \"paths\": {\n",
        );
        for i in 0..60 {
            spec.push_str(&format!(
                "    \"/items/{i}\": {{\"get\": {{\"summary\": \"read item {i}\", \
                 \"responses\": {{\"200\": {{\"description\": \"ok\"}}}}}}}},\n"
            ));
        }
        spec.push_str("    \"/health\": {\"get\": {\"summary\": \"health\"}}\n  }\n}\n");
        assert_eq!(detect(&spec), Kind::OpenApi, "fixture must reach OpenApi");

        let (k, _o, hinted) = compact_with_path_hint(&spec, "openapi.json");
        assert_eq!(k, Kind::OpenApi, "the .json hint hid its own dialect");
        let (_k2, _o2, plain) = compact(&spec);
        assert_eq!(
            hinted.tokens_after, plain.tokens_after,
            "naming a spec `openapi.json` must not cost more than not naming it"
        );
    }

    /// …and the `.json` arm is NARROW where the `.log` arm is wide, because
    /// the two hints assert different amounts.
    ///
    /// `.log` says nothing about the bytes, so anything specific beats it.
    /// `.json` says they are JSON, and that assertion is right — so only a
    /// JSON dialect may refine it. A content detector that answers `Csv` or
    /// `Markdown` on a `.json` is misdetecting, and what happens when a
    /// misdetection is allowed to beat a correct hint was measured.
    #[test]
    fn a_json_extension_is_not_overridden_by_a_non_json_guess() {
        // Valid JSON whose head a looser detector could read as something
        // else; the hint must hold.
        let doc = "{\n  \"a,b,c\": \"1,2,3\",\n  \"d,e,f\": \"4,5,6\",\n  \"rows\": [\
                   \"x,y,z\", \"p,q,r\", \"s,t,u\"]\n}\n";
        let (k, _o, _s) = compact_with_path_hint(doc, "data.json");
        assert!(
            matches!(
                k,
                Kind::Json | Kind::Ndjson | Kind::OpenApi | Kind::SourceMap
            ),
            "a non-JSON kind overrode a correct .json hint: {k:?}"
        );
    }

    /// …and it only ever NARROWS. A `.log` the content detector cannot place
    /// still gets the Logs compactor, which is what the hint was for — the
    /// refinement must not turn a working hint into `Unknown`.
    #[test]
    fn a_log_the_content_cannot_place_still_gets_the_log_compactor() {
        let odd: String = (0..40)
            .map(|i| format!("[stage {i}] doing the thing, please wait ({i}%)\n"))
            .collect();
        assert!(
            matches!(detect(&odd), Kind::Unknown | Kind::Logs),
            "fixture must be unplaceable by content, got {:?}",
            detect(&odd)
        );
        let (k, _o, sav) = compact_with_path_hint(&odd, "build.log");
        assert_eq!(k, Kind::Logs, "the hint must still apply");
        assert!(
            sav.tokens_after <= sav.tokens_before,
            "{} -> {}",
            sav.tokens_before,
            sav.tokens_after
        );
    }

    /// A SINGLE-project Gradle build — `> Task :compileKotlin`, no project
    /// segment — which is the common case, not the exotic one.
    ///
    /// The Gradle arm was taught to list its projects, and that arm finds
    /// none here: the summary went back to one status line naming nothing, so
    /// that fix's own floor refused it and handed back the raw log. Safe, and 760
    /// tokens where 110 will do. The identifying payload of a single-project
    /// build is the TASK, so it takes the same block — one block, two possible
    /// identifiers, never both.
    #[test]
    fn a_single_project_gradle_build_names_its_tasks() {
        let mut t = String::new();
        for i in 0..40 {
            t.push_str(&format!(
                "> Task :task{i}\nNote: task{i} completed in 0.3s\n"
            ));
        }
        t.push_str("BUILD SUCCESSFUL in 1m 12s\n");
        let (_k, out, sav) = compact(&t);
        assert!(
            sav.tokens_after * 4 < sav.tokens_before,
            "the floor is still refusing it: {} -> {}",
            sav.tokens_before,
            sav.tokens_after
        );
        assert!(out.contains("tasks (40)"), "no task block:\n{out}");
        assert!(out.contains("task0"), "the tasks are not named:\n{out}");
        assert!(
            out.contains("… 20 more tasks elided"),
            "the cap must name itself:\n{out}"
        );
        // Never both blocks: a multi-project build has already said what ran.
        assert!(
            !out.contains("projects ("),
            "a build with no project segment listed projects:\n{out}"
        );
    }

    /// A Gradle build, the same shape with a different tag. 2 437 -> 126 bytes.
    ///
    /// The size assertion is the one this test was born with, and on its own it
    /// passed a fold that answered `Gradle BUILD SUCCESSFUL: 33 tasks
    /// (0 failed)` and NOTHING else — 877 tokens to 11, every project and task
    /// name gone, no disclosure. Its Maven sibling has always asserted the
    /// second half ("the module names are what an agent needs from a reactor
    /// build"); this arm asserted only that the output was small, and small is
    /// what destruction looks like from the outside.
    #[test]
    fn a_gradle_build_reaches_it_too() {
        let mut t = String::from("> Task :buildSrc:compileKotlin UP-TO-DATE\n");
        for i in 0..8 {
            for task in [
                "preBuild",
                "compileDebugKotlin",
                "mergeDebugResources",
                "dexBuilderDebug",
            ] {
                t.push_str(&format!(
                    "> Task :mod{i}:{task}\nNote: mod{i} {task} completed in 0.3s\n"
                ));
            }
        }
        t.push_str("BUILD SUCCESSFUL in 1m 12s\n");
        let (_k, out, _s) = compact(&t);
        assert!(out.len() * 4 < t.len(), "{} of {}", out.len(), t.len());
        // The same half its Maven sibling asserts.
        for m in ["mod0", "mod7", "buildSrc"] {
            assert!(out.contains(m), "project {m} was dropped:\n{out}");
        }
    }

    /// The three tightenings, each on the payload that motivated it — and each
    /// paired with the real format it must still accept, because a detector
    /// tightened past its own format is a worse defect than the one it fixes.
    #[test]
    fn each_tightened_detector_still_accepts_its_own_format() {
        // TOML: a table header is the WHOLE line.
        let real_toml =
            "[package]\nname = \"demo\"\nversion = \"1.0\"\n\n[dependencies]\nserde = \"1\"\n";
        assert!(crate::toml_compact::detect(real_toml), "real TOML rejected");
        assert!(
            !crate::toml_compact::detect(
                "[INFO] Building a 1.0\n[INFO] Building b 1.0\n[INFO] Building c 1.0\n"
            ),
            "a bracketed log tag read as a TOML table header"
        );

        // INI: DISTINCT section names.
        let real_ini = "[server]\nhost = 0.0.0.0\nport = 8080\n\n[database]\nurl = x\nuser = y\n";
        assert!(crate::ini::detect(real_ini), "real INI rejected");
        let repeated_tag: String = (0..30)
            .map(|i| format!("[INFO]\n[INFO] step {i} = done\n"))
            .collect();
        assert!(
            !crate::ini::detect(&repeated_tag),
            "one tag repeated read as many sections"
        );

        // JSON: the bracket must open a JSON VALUE.
        for ok in [
            "{\"a\": 1, \"b\": 2}",
            "[\n  {\"a\": 1},\n  {\"a\": 2}\n]",
            "[\n  \"x\",\n  \"y\"\n]",
        ] {
            assert_eq!(detect(ok), Kind::Json, "real JSON rejected: {ok}");
        }
        assert_ne!(
            detect("[INFO] Scanning for projects...\n[INFO] Building jar: /w/a.jar\n[INFO] done\n"),
            Kind::Json,
            "a bracketed log tag read as JSON"
        );
    }
}

#[cfg(test)]
mod corpus_kind_census {
    use super::*;

    /// Which `Kind` does every fixture in the benchmark corpus get, and which
    /// kinds does NO fixture produce?
    ///
    /// A compactor with a detector that nothing in the corpus reaches is the
    /// shape of a dead branch: it may be correct and simply unexercised, or it
    /// may be unreachable. The two look identical from the code, and only a
    /// corpus separates them.
    #[test]
    #[ignore = "measurement over the benchmark corpus; run explicitly"]
    fn which_kinds_the_corpus_reaches() {
        use std::collections::BTreeMap;
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/fixtures");
        let mut per_kind: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut stack = vec![root.clone()];
        let mut files = 0usize;
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let Ok(bytes) = std::fs::read(&p) else {
                    continue;
                };
                if bytes.len() < 200 || bytes.len() > 4_000_000 {
                    continue;
                }
                let Ok(text) = String::from_utf8(bytes) else {
                    continue;
                };
                files += 1;
                let k = detect(&text);
                let name = p
                    .strip_prefix(&root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .to_string();
                per_kind
                    .entry(k.as_str().to_string())
                    .or_default()
                    .push(name);
            }
        }
        eprintln!("CORPUS files={files}");
        for (k, v) in &per_kind {
            eprintln!("HIT  {k:<18} n={:<3} e.g. {}", v.len(), v[0]);
        }
        let all: Vec<&str> = super::ALL_KINDS.iter().map(|k| k.as_str()).collect();
        let missed: Vec<&&str> = all.iter().filter(|k| !per_kind.contains_key(**k)).collect();
        eprintln!(
            "MISSED {} of {}: {}",
            missed.len(),
            all.len(),
            missed.iter().map(|s| **s).collect::<Vec<_>>().join(" ")
        );
    }
}

#[cfg(test)]
mod hint_refinement {
    use super::*;

    /// An extension pins a FAMILY and the content can name a MEMBER of it.
    /// `.xml` pinned `Kind::Xml`, whose compactor strips comments and
    /// whitespace and nothing else, so a JUnit report READ FROM A FILE never
    /// reached the JUnit compactor however clearly its content said so.
    /// Measured 2026-08-26: 22 671 bytes in, 22 671 out.
    ///
    /// Through `compact_with_path_hint`, because that is the door a file comes
    /// in by and a test of `detect` alone passes either way.
    #[test]
    fn a_junit_report_named_dot_xml_reaches_the_junit_compactor() {
        let mut t = String::from(
            "<?xml version=\"1.0\"?>\n<testsuites tests=\"24\" failures=\"24\" errors=\"0\" time=\"0.4\">\n<testsuite name=\"a.ts\" tests=\"24\" failures=\"24\" errors=\"0\">\n",
        );
        for i in 0..24 {
            t.push_str(&format!(
                "<testcase classname=\"a.ts\" name=\"case {i}\">\n<failure message=\"boom\" type=\"TypeError\">\nTypeError: boom\n at run a.ts:{}:5\n</failure>\n</testcase>\n",
                10 + i
            ));
        }
        t.push_str("</testsuite>\n</testsuites>\n");
        let (kind, out, _sav) = compact_with_path_hint(&t, "reports/new.xml");
        assert_eq!(kind, Kind::JUnit, "the .xml hint beat the content");
        assert!(out.contains("[tokenade:junit]"), "{out:.160}");
        assert!(out.len() * 4 < t.len(), "{} of {}", out.len(), t.len());
    }

    /// …and a hint that names nothing more specific is returned unchanged: the
    /// refinement only ever NARROWS.
    #[test]
    fn a_plain_xml_document_keeps_its_hint() {
        let t = "<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n  <modelVersion>4.0.0</modelVersion>\n  <groupId>com.acme</groupId>\n</project>\n";
        let (kind, _out, _s) = compact_with_path_hint(t, "pom.xml");
        assert_eq!(kind, Kind::Xml);
    }
}

#[cfg(test)]
mod keyword_prefix_misroutes {
    use super::*;

    /// `git log` of a repository that uses dependabot. Its commit bodies carry
    /// `update-type: version-update:semver-major`, and the SQL detector took
    /// the leading alphabetic run of `update-type` for the keyword `UPDATE`.
    /// Two such lines were enough. MEASURED 2026-08-26 on
    /// `benchmarks/fixtures/gin-gitlog.txt`: 8 788 bytes in, 8 788 out through
    /// the SQL compactor, 415 through the git-log one. 21x, on a fixture that
    /// had been in the corpus the whole time.
    #[test]
    fn a_dependabot_commit_body_is_not_sql() {
        let t = "commit 5f4f9643258dc2a65e684b63f12c8d543c936c67\n\
                 Author: dependabot[bot] <49699333+dependabot[bot]@users.noreply.github.com>\n\
                 Date:   Sat May 9 10:20:32 2026 +0800\n\n\
                 \x20   chore(deps): bump the actions group\n\n\
                 \x20   updated-dependencies:\n\
                 \x20   - dependency-name: codecov/codecov-action\n\
                 \x20     dependency-version: 6\n\
                 \x20     update-type: version-update:semver-major\n\
                 \x20   - dependency-name: aquasecurity/trivy-action\n\
                 \x20     update-type: version-update:semver-minor\n\n\
                 commit 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b\n\
                 Author: Someone <s@example.com>\n\
                 Date:   Fri May 8 09:00:00 2026 +0800\n\n\
                 \x20   fix: a thing\n";
        assert!(
            !crate::sql_source::detect(t),
            "a dependabot commit body was claimed as SQL source"
        );
        assert_eq!(detect(t), Kind::GitLog);
    }

    /// The other identifiers that begin with a SQL keyword. Each was a hit.
    #[test]
    fn an_identifier_that_starts_with_a_keyword_is_not_the_keyword() {
        for ident in [
            "update-type: x",
            "create_table_sql = 1",
            "select_one: true",
            "delete-me: yes",
            "begin_frame(now)",
            "drop_shadow: none",
            "with_context: false",
        ] {
            let t: String = std::iter::repeat_n(format!("{ident}\n"), 12).collect();
            assert!(
                !crate::sql_source::detect(&t),
                "`{ident}` read as a SQL keyword"
            );
        }
    }

    /// …and real SQL, whose keywords are followed by whitespace, `;` or `(`,
    /// must still be claimed. A detector tightened past its own format is a
    /// worse defect than the one it fixes.
    #[test]
    fn real_sql_is_still_claimed() {
        let t = "CREATE TABLE users (\n  id int primary key,\n  name text\n);\n\
                 INSERT INTO users VALUES (1, 'a');\n\
                 SELECT * FROM users WHERE id = 1;\n\
                 BEGIN;\nUPDATE users SET name = 'b' WHERE id = 1;\nCOMMIT;\n";
        assert!(crate::sql_source::detect(t), "real SQL was rejected");
    }

    /// `pytest --collect-only` prints five or more angle-bracket lines in a
    /// row and not one of them closes. XML is a TREE. MEASURED 2026-08-26 on
    /// `benchmarks/fixtures/pytest-collect.log`, 32 939 bytes: claimed as XML,
    /// and the XML compactor returned all 32 939.
    #[test]
    fn an_angle_bracket_listing_is_not_xml() {
        let mut t = String::from("<Module tests/test_basic.py>\n");
        for i in 0..40 {
            t.push_str(&format!("  <Class TestThing{i}>\n"));
            t.push_str(&format!("    <Function test_case_{i}[param{i}]>\n"));
        }
        assert!(
            !crate::xml::detect(&t),
            "a pytest listing was claimed as XML"
        );
        assert_ne!(detect(&t), Kind::Xml);
    }

    /// …and real XML, which always closes something, still is.
    #[test]
    fn real_xml_is_still_claimed() {
        let t = "<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n\
                 \x20 <modelVersion>4.0.0</modelVersion>\n\
                 \x20 <groupId>com.acme</groupId>\n\
                 \x20 <artifactId>demo</artifactId>\n\
                 \x20 <version>1.0</version>\n\
                 \x20 <packaging>jar</packaging>\n\
                 </project>\n";
        assert!(crate::xml::detect(t), "real XML was rejected");
    }
}

#[cfg(test)]
mod wide_corpus_sweep {
    use super::*;

    /// Walk a real tree and report what tokenade DELIVERS for each file,
    /// worst first — the files it hands back essentially unchanged.
    ///
    /// `benchmarks/fixtures` holds 40 files and found two misroutes on its
    /// first run. This is the same question against whatever tree
    /// `SWEEP_ROOT` names, so the corpus can be as wide as the machine allows.
    #[test]
    #[ignore = "measurement over a caller-supplied tree; run explicitly"]
    fn what_comes_back_unchanged() {
        let Ok(root) = std::env::var("SWEEP_ROOT") else {
            eprintln!("set SWEEP_ROOT");
            return;
        };
        let skip_ext: &[&str] = &[
            "png", "jpg", "jpeg", "gif", "webp", "ico", "pdf", "zip", "gz", "xz", "so", "a",
            "rlib", "bin", "wasm", "woff", "woff2", "ttf", "mp4", "mp3", "class", "jar",
        ];
        // SOURCE is protected on the real path by `compact_with_path_hint`'s
        // extension reject, so a bare `compact()` on a `.rs` returning 1.000 is
        // the product working, not a gap. Sweeping it drowns the signal.
        let source_ext: &[&str] = &[
            "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "kt", "rb", "php", "c", "h", "cc",
            "cpp", "hpp", "cs", "swift", "scala", "sh", "bash", "zsh", "fish", "lua", "pl", "ex",
            "exs", "erl", "hs", "ml", "vue", "svelte", "dart", "m", "mm",
        ];
        // A whole tree of real repositories is far more than one measurement
        // needs, and an unbounded walk simply never finishes.
        let cap: usize = std::env::var("SWEEP_CAP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1500);
        let mut rows: Vec<(usize, f64, String, String)> = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(&root)];
        let mut scanned = 0usize;
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let ext = p
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if skip_ext.contains(&ext.as_str()) || source_ext.contains(&ext.as_str()) {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&p) else {
                    continue;
                };
                if bytes.len() < 4_000 || bytes.len() > 3_000_000 {
                    continue;
                }
                let Ok(text) = String::from_utf8(bytes) else {
                    continue;
                };
                scanned += 1;
                if scanned > cap {
                    stack.clear();
                    break;
                }
                let kind = detect(&text);
                let (_k, out, sav) = compact(&text);
                let ratio = out.len() as f64 / text.len() as f64;
                // A compactor that gives back 1.000 is NOT a delivered 1.000
                // when the disclose layer is going to summarise and stash it
                // anyway. Measured 2026-08-26 on a 1 300 083-byte CHANGELOG:
                // `compact()` returned all of it and `tokenade read` returned
                // 155 bytes. Reporting the compactor's ratio as the product's
                // made markdown look like the biggest gap in the corpus —
                // 5.39 MB across 254 files — when it is the one the outline
                // path already answers.
                //
                // What is genuinely delivered whole is the content BELOW that
                // threshold: nothing downstream will rescue it.
                let rescued_by_disclose = sav.tokens_after >= crate::disclose::threshold_tokens();
                if ratio > 0.90 && !rescued_by_disclose {
                    rows.push((out.len(), ratio, kind.as_str().to_string(), name));
                }
            }
        }
        rows.sort_by_key(|b| std::cmp::Reverse(b.0));
        eprintln!("SWEEP scanned={scanned} unchanged={}", rows.len());
        for (bytes, ratio, kind, name) in rows.iter().take(30) {
            eprintln!("BIG  {bytes:>9}  {ratio:.3}  {kind:<14} {name}");
        }
        use std::collections::BTreeMap;
        let mut by_kind: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for (bytes, _, kind, _) in &rows {
            let e = by_kind.entry(kind.as_str()).or_default();
            e.0 += 1;
            e.1 += bytes;
        }
        let mut v: Vec<_> = by_kind.into_iter().collect();
        v.sort_by_key(|b| std::cmp::Reverse(b.1 .1));
        for (k, (n, b)) in v.into_iter().take(15) {
            eprintln!("KIND {k:<16} n={n:<4} bytes={b}");
        }
    }
}

#[cfg(test)]
mod patch_payload_language {
    use super::{detect, dispatch_kind, Kind};

    const README_PATCH: &str = "diff --git a/README.md b/README.md\nindex 1111111..2222222 100644\n--- a/README.md\n+++ b/README.md\n@@ -1,6 +1,7 @@\n # Project\n \n ## Install\n \n-Run the thing.\n+Run the thing, carefully.\n+See the notes below.\n \n ## Usage\n";

    /// Trimmed from `opencode/patches/solid-js@1.9.10.patch`, keeping the two
    /// lines that made `sql_source::detect` claim it. They are plain
    /// JavaScript `delete` statements in the diff's CONTEXT — the first
    /// alphabetic run of each reads as the SQL keyword DELETE, and two hits
    /// inside the first 40 non-empty lines is that detector's whole threshold.
    /// No SQL is involved and none of the file is SQL.
    const JS_PATCH: &str = "diff --git a/dist/dev.cjs b/dist/dev.cjs\nindex 7104749..dc3eac9 100644\n--- a/dist/dev.cjs\n+++ b/dist/dev.cjs\n@@ -987,18 +989,21 @@ function cleanNode(node) {\n   if (node.tOwned) {\n-    for (i = node.tOwned.length - 1; i >= 0; i--) cleanNode(node.tOwned[i]);\n+    const tOwned = node.tOwned;\n     delete node.tOwned;\n+    for (i = tOwned.length - 1; i >= 0; i--) cleanNode(tOwned[i]);\n   }\n@@ -1002,7 +1007,7 @@ function cleanNode(node) {\n   if (node.sourceMap) {\n-    node.sourceMap = null;\n+    node.sourceMap = undefined;\n   delete node.sourceMap;\n   }\n";

    #[test]
    fn a_markdown_payload_is_still_a_patch() {
        assert_eq!(detect(README_PATCH), Kind::UnifiedDiff);
    }

    #[test]
    fn a_javascript_payload_is_not_sql() {
        assert_eq!(detect(JS_PATCH), Kind::UnifiedDiff);
    }

    /// The damage the SQL route did, stated as the thing a reader loses.
    ///
    /// `sql_source` strips `--` to end of line, so `diff --git a/x b/x` was
    /// delivered as `diff` and `--- a/x` as an empty line: which file a hunk
    /// belongs to was gone, and on a rename both names were.
    #[test]
    fn the_file_a_hunk_belongs_to_survives() {
        for patch in [README_PATCH, JS_PATCH] {
            let out = dispatch_kind(patch, detect(patch));
            let named = patch
                .lines()
                .next()
                .unwrap()
                .strip_prefix("diff --git a/")
                .unwrap()
                .split(' ')
                .next()
                .unwrap();
            assert!(
                out.contains(named),
                "the delivered fold never names {named}:\n{out}"
            );
            assert!(
                !out.lines().any(|l| l.trim() == "diff"),
                "a truncated `diff` line means the `--` stripper ran:\n{out}"
            );
        }
    }

    /// The anchoring rule, and why it is not `contains`.
    ///
    /// Five markdown documents in the corpus show a diff inside a fenced code
    /// block. They are prose with 4 to 8 headings; rebuilding them as patches
    /// throws every word away.
    #[test]
    fn prose_showing_a_diff_in_a_fence_stays_prose() {
        let doc = "# Compressing a diff\n\nThe compressor folds context lines. Here is what it reads:\n\n```diff\ndiff --git a/src/main.rs b/src/main.rs\nindex 5555555..6666666 100644\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,3 @@\n fn main() {\n-    println!(\"a\");\n+    println!(\"b\");\n }\n```\n\n## Why it works\n\nContext lines repeat between hunks, so they fold well.\n\n## Limits\n\nA rename carries two names and both are kept.\n";
        assert_ne!(
            detect(doc),
            Kind::UnifiedDiff,
            "a document with three headings around one fenced diff is not a patch"
        );
        let out = dispatch_kind(doc, detect(doc));
        for heading in ["Compressing a diff", "Why it works", "Limits"] {
            assert!(out.contains(heading), "lost heading {heading}:\n{out}");
        }
    }
}

#[cfg(test)]
mod source_claimants {
    /// The mirror of `document_claimants`: which Kind claims a SOURCE
    /// file of another language, and how many of its non-comment lines does the
    /// claimant's stripper delete?
    #[test]
    #[ignore = "measurement"]
    fn what_claims_source_and_what_does_it_delete() {
        let Ok(root) = std::env::var("SWEEP_ROOT") else {
            return;
        };
        let mut stack = vec![std::path::PathBuf::from(&root)];
        // (ext, kind) -> (files, payload_in, payload_kept)
        let mut acc: std::collections::BTreeMap<(String, String), (usize, usize, usize)> =
            Default::default();
        let mut worst: Vec<(usize, String, String, String)> = Vec::new();
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                let nm = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if nm.starts_with('.') || nm == "target" || nm == "node_modules" {
                    continue;
                }
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let ext = p
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if !matches!(
                    ext.as_str(),
                    "py" | "rs" | "js" | "ts" | "go" | "rb" | "java" | "c" | "cpp" | "sh"
                ) {
                    continue;
                }
                let Ok(t) = std::fs::read_to_string(&p) else {
                    continue;
                };
                if t.len() < 3_000 || t.len() > 300_000 {
                    continue;
                }
                let k = super::detect(&t);
                let kname = format!("{k:?}");
                // Unknown is the honest answer for a language we have no
                // compactor for; only a CLAIM can delete.
                if k == super::Kind::Unknown {
                    continue;
                }
                let out = super::dispatch_kind(&t, k);
                let payload: Vec<&str> = t
                    .lines()
                    .map(str::trim)
                    .filter(|l| {
                        l.len() > 12
                            && !l.starts_with("//")
                            && !l.starts_with('#')
                            && !l.starts_with("/*")
                            && !l.starts_with('*')
                    })
                    .collect();
                if payload.len() < 20 {
                    continue;
                }
                let kept = payload.iter().filter(|l| out.contains(**l)).count();
                let e2 = acc.entry((ext.clone(), kname.clone())).or_default();
                e2.0 += 1;
                e2.1 += payload.len();
                e2.2 += kept;
                if kept * 2 < payload.len() {
                    worst.push((
                        payload.len() - kept,
                        ext.clone(),
                        kname,
                        p.display().to_string(),
                    ));
                }
            }
        }
        eprintln!("{:>6} {:>9} {:>9}  ext / kind", "files", "payload", "kept");
        for ((ext, k), (n, pin, pk)) in &acc {
            if pk * 10 < pin * 9 {
                eprintln!("{n:6} {pin:9} {pk:9}  .{ext} -> {k}");
            }
        }
        worst.sort_by_key(|b| std::cmp::Reverse(b.0));
        eprintln!("--- source files losing over half their payload ---");
        for (lost, ext, k, path) in worst
            .iter()
            .filter(|w| w.2 == "GraphQl" || w.2 == "JvmBuild" || w.2 == "UnifiedDiff")
            .take(10)
        {
            eprintln!("  -{lost:4} lines  .{ext:4} -> {k:14} {path}");
        }
        eprintln!("total such files: {}", worst.len());
    }
}

#[cfg(test)]
mod empty_fold_census {
    /// Which compactors can return NOTHING on real, non-empty input?
    #[test]
    #[ignore = "measurement"]
    fn who_returns_the_empty_string() {
        let Ok(root) = std::env::var("SWEEP_ROOT") else {
            return;
        };
        let mut stack = vec![std::path::PathBuf::from(&root)];
        let mut acc: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
        let mut examples: Vec<(String, String)> = Vec::new();
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                let nm = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if nm.starts_with('.') || nm == "target" || nm == "node_modules" {
                    continue;
                }
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let ext = p
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if !matches!(
                    ext.as_str(),
                    "rs" | "py" | "go" | "ts" | "js" | "md" | "txt" | "json" | "yaml" | "toml"
                ) {
                    continue;
                }
                let Ok(t) = std::fs::read_to_string(&p) else {
                    continue;
                };
                if t.trim().len() < 2_000 || t.len() > 200_000 {
                    continue;
                }
                let k = super::detect(&t);
                let out = super::dispatch_kind(&t, k);
                let e2 = acc.entry(format!("{k:?}")).or_default();
                e2.0 += 1;
                if out.trim().is_empty() {
                    e2.1 += 1;
                    if examples.len() < 6 {
                        examples.push((format!("{k:?}"), p.display().to_string()));
                    }
                }
            }
        }
        eprintln!("{:>7} {:>7}  kind", "files", "empty");
        for (k, (n, z)) in &acc {
            if *z > 0 {
                eprintln!("{n:7} {z:7}  {k}");
            }
        }
        for (k, p) in &examples {
            eprintln!("  EX {k:12} {p}");
        }
    }
}

#[cfg(test)]
mod retention_of_facts_agents_need {
    /// Every ground-truth item must appear in `out`; the failure NAMES the
    /// missing ones, because "retention dropped" is not actionable and
    /// "these three hashes are gone" is.
    fn assert_kept(label: &str, out: &str, items: &[String]) {
        let lost: Vec<&str> = items
            .iter()
            .map(String::as_str)
            .filter(|i| !out.contains(*i))
            .collect();
        assert!(
            lost.is_empty(),
            "{label}: {} of {} facts lost, e.g. {:?}\n--- output ---\n{}",
            lost.len(),
            items.len(),
            &lost[..lost.len().min(5)],
            &out[..out.len().min(1200)]
        );
    }

    /// A commit hash is the handle for every follow-up command, so losing one
    /// costs the agent the object itself, not a detail of it.
    #[test]
    fn every_commit_hash_of_a_git_log_survives() {
        let mut text = String::new();
        let mut hashes = Vec::new();
        for i in 0..50 {
            let h = format!("{:07x}", 0x1a2b3c + i * 7919);
            text.push_str(&format!("{h} fix(core): change number {i} in the loader\n"));
            hashes.push(h);
        }
        let (_k, out, _s) = super::compact(&text);
        assert_kept("git log --oneline", &out, &hashes);
    }

    /// `path:line` is what the agent opens next. A fold that keeps the match
    /// text but loses the location has kept the least useful half.
    #[test]
    fn every_location_of_a_grep_survives() {
        let mut text = String::new();
        let mut locs = Vec::new();
        for i in 0..40 {
            let loc = format!("crates/core/src/mod_{i}.rs:{}", 10 + i * 3);
            text.push_str(&format!(
                "{loc}:    fn handler_{i}(req: Request) -> Response {{\n"
            ));
            locs.push(loc);
        }
        let (_k, out, _s) = super::compact(&text);
        assert_kept("grep -rn", &out, &locs);
    }

    /// Below the listing fold threshold the names are the whole answer and
    /// all of them are kept. Measured over our own corpus: of 5 218 real `ls`
    /// invocations only 6 were large enough to have their middle elided, so
    /// THIS is the case that describes normal use.
    #[test]
    fn every_filename_of_an_ordinary_listing_survives() {
        let mut text = String::from("total 412\n");
        let mut names = Vec::new();
        for i in 0..50 {
            let n = format!("handler_{i}_stage.rs");
            text.push_str(&format!(
                "-rw-r--r-- 1 dev dev {:5} Aug 29 01:02 {n}\n",
                900 + i * 37
            ));
            names.push(n);
        }
        let (_k, out, _s) = super::compact(&text);
        assert_kept("ls -l (ordinary)", &out, &names);
    }

    /// And when a listing IS big enough to lose names, the output must say so
    /// and hand back a way to get them. Dropping them is a defensible trade;
    /// dropping them silently is not, and this pins which one we do.
    #[test]
    fn a_listing_too_big_to_keep_whole_says_what_it_dropped() {
        let mut text = String::from("total 9000\n");
        for i in 0..400 {
            text.push_str(&format!(
                "-rw-r--r-- 1 dev dev {:5} Aug 29 01:02 handler_{i}_stage.rs\n",
                900 + i * 37
            ));
        }
        let (_k, out, _s) = super::compact(&text);
        if out.contains("handler_399_stage.rs") && out.contains("handler_200_stage.rs") {
            return; // nothing was dropped — nothing to disclose
        }
        // Measured over our own sessions: 16 of these rollups were served and
        // only 2 carried any route back to the paths — the delivery layer
        // stashes on some paths and not others, so the ONE thing that is
        // always true has to be said by the rollup itself. The same complaint
        // is recorded in `filelist.rs` from a user on 2026-05-29.
        assert!(
            out.contains("no individual path is listed")
                || out.contains("elided")
                || out.contains("expand-ref"),
            "names were dropped and the output does not say so:\n{}",
            &out[..out.len().min(1500)]
        );
    }
}

#[cfg(test)]
mod a_module_that_declines_does_not_beat_one_that_folds {
    fn lsof_shaped(rows: usize) -> String {
        let mut t =
            String::from("COMMAND     PID   USER   FD   TYPE   DEVICE   SIZE/OFF   NODE NAME\n");
        for i in 0..rows {
            t.push_str(&format!(
                "chrome    {}   dev    12u  IPv4   1234     0t0        99   /tmp/x{}\n",
                1_000_000 + i * 7919,
                2_000_000 + i * 104_729
            ));
        }
        t
    }

    fn ps_shaped(rows: usize) -> String {
        let mut t = String::from(
            "USER         PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND\n",
        );
        for i in 0..rows {
            t.push_str(&format!(
                "dev      {:7} {:4.1}  0.2 5590120 163972 ?        Sl   Aug25   1:58 /usr/bin/app{}\n",
                1000 + i * 13,
                (i % 40) as f32 / 10.0,
                i
            ));
        }
        t
    }

    #[test]
    fn the_generic_fold_is_used_when_the_module_hands_the_table_back() {
        let t = lsof_shaped(120);
        let (kind, out, _s) = super::compact(&t);
        assert_eq!(
            format!("{kind:?}"),
            "TabularCli",
            "routing changed: {kind:?}"
        );
        let generic = crate::tabular::compact(&t);
        assert!(
            generic.len() * 2 < t.len(),
            "the fixture no longer folds generically, so this proves nothing"
        );
        assert!(
            out.len() <= generic.len(),
            "the module handed the table back ({} of {} bytes) and that answer \
             was served over a generic fold of {}",
            out.len(),
            t.len(),
            generic.len()
        );
    }

    /// ...and a module that DOES fold still wins, which is the whole reason
    /// it is consulted first.
    #[test]
    fn a_module_that_folds_is_still_preferred() {
        let t = ps_shaped(400);
        let (_k, out, _s) = super::compact(&t);
        assert!(
            out.contains("[tokenade:ps-cmd]"),
            "the per-command module must still answer for ps:\n{}",
            &out[..out.len().min(300)]
        );
    }
}

#[cfg(test)]
mod a_compactor_that_returns_nothing_has_failed {
    //! Swept the whole tree for real inputs a compactor hands back EMPTY.
    //! The clearest one is not a misroute at all: `schema { … }` is a valid,
    //! ordinary GraphQL root-operation block, the Kind is right, and the SDL
    //! compactor rebuilds from a parse that finds no type declarations — so
    //! it returns the empty string and `guarded` used to score that as a
    //! perfect saving.

    /// 47 bytes of legitimate SDL. Every collection the rebuild fills stays
    /// empty, so `out` is never written to.
    const SCHEMA_ONLY: &str = "schema {\n  query: Query\n  mutation: Mutation\n}\n";

    #[test]
    fn the_compactor_really_does_return_nothing_here() {
        let kind = super::detect(SCHEMA_ONLY);
        assert_eq!(format!("{kind:?}"), "GraphQl");
        assert!(
            super::dispatch_kind(SCHEMA_ONLY, kind).trim().is_empty(),
            "fixture no longer reproduces the empty fold; re-run the sweep for another"
        );
    }

    #[test]
    fn an_empty_fold_delivers_the_original_text() {
        let (kind, out, _sav) = super::compact(SCHEMA_ONLY);
        assert_eq!(format!("{kind:?}"), "GraphQl");
        assert!(
            !out.trim().is_empty(),
            "a compactor returned nothing and it was DELIVERED"
        );
        assert_eq!(
            out, SCHEMA_ONLY,
            "nothing was compacted, so the original stands"
        );
    }

    #[test]
    fn a_real_fold_is_still_allowed_to_shrink() {
        let sdl = format!(
            "schema {{\n  query: Query\n}}\n\ntype Query {{\n  a: Int\n}}\n{}",
            (0..40)
                .map(|i| format!("type T{i} {{\n  id: ID!\n  name: String\n  bio: String\n}}\n"))
                .collect::<String>()
        );
        let (_k, out, _s) = super::compact(&sdl);
        assert!(!out.trim().is_empty());
        assert!(out.len() < sdl.len(), "a healthy fold still shrinks");
    }

    #[test]
    fn graphql_files_are_routed_by_their_extension() {
        // SDL was reachable by CONTENT only, so a `.graphql` whose first
        // 8 KB carries no strong signal fell to `Unknown` and shipped whole.
        for name in ["schema.graphql", "ops.gql", "/a/b/Schema.GraphQL"] {
            assert_eq!(
                format!("{:?}", super::kind_from_path_hint(name)),
                "Some(GraphQl)",
                "{name}"
            );
        }
    }

    /// The redraw replay must precede every routing decision, and this
    /// module has THREE public entry points that each decide one. A fourth
    /// added without the call would reintroduce the defect silently: measured
    /// on 25 frames of `watch kubectl get pods`, 309 bytes with no filename and
    /// **4 301** with a `.log` name, because the hint route skipped it.
    ///
    /// Read off the SOURCE, because the omission is invisible to behaviour
    /// until someone happens to hand that entry point a redraw.
    /// Every entry point that takes a PATH HINT must refine it by content.
    ///
    /// A source guard, and it exists because the defect it prevents was
    /// introduced by the two entries that fixed the same defect elsewhere.
    /// Two findings taught `compact_with_path_hint` that a hint must not hide
    /// a specialisation of itself; `compact_with_path_hint_indexed` — the one
    /// the READ HOOK calls — kept calling `kind_from_path_hint` bare for two
    /// iterations, so an `openapi.json` cost 837 tokens through the hook and
    /// 627 through its sibling.
    ///
    /// Same shape as `every_public_compact_entry_point_replays_the_redraw_first`
    /// below, and for the same reason: there is more than one door and the
    /// rule has to be asked at all of them.
    #[test]
    fn every_path_hint_entry_point_refines_by_content() {
        // PRODUCTION code only. The tests below legitimately call the mapper
        // directly to check what an extension maps to, and refining there
        // would be asking a different question.
        //
        // Brace-tracked, not split at the first `#[cfg(test)]`: this file
        // carries TEN of them and the first is at line 1041 of 3397, so the
        // split read under a third of the file and would have been blind to
        // any entry point below it.
        let whole = &crate::source_guard::library_module_family("auto");
        let mut seen = 0usize;
        for (n, line) in crate::srcsweep::production_lines(whole) {
            let t = line.trim_start();
            if t.starts_with("//") || !t.contains("kind_from_path_hint(") {
                continue;
            }
            // The declaration itself.
            if t.contains("fn kind_from_path_hint") {
                continue;
            }
            seen += 1;
            assert!(
                line.contains("refine_hint_by_content"),
                "auto.rs:{}: a path hint is used without refining it by \
                 content — a `.log` will hide a `GitLog` and an `openapi.json` \
                 an `OpenApi`:\n  {t}",
                n + 1
            );
        }
        // The sweep must be doing work: a rename of `kind_from_path_hint`
        // would otherwise report a clean pass over nothing.
        assert!(
            seen >= 2,
            "only {seen} hint use(s) found — this guard has gone blind"
        );
    }

    #[test]
    fn every_public_compact_entry_point_replays_the_redraw_first() {
        let src = &crate::source_guard::library_module_family("auto");
        let mut entries = Vec::new();
        let lines = src.lines().enumerate().peekable();
        for (n, line) in lines {
            let t = line.trim_start();
            // Comments quote these names; only the declarations count.
            if t.starts_with("//") {
                continue;
            }
            if !(t.starts_with("pub fn compact") && t.contains('(')) {
                continue;
            }
            let name = t
                .trim_start_matches("pub fn ")
                .split('(')
                .next()
                .unwrap_or("")
                .to_string();
            // The ONE exemption, and it is a decision rather than an omission.
            // `compact_remainder` is handed the leftover lines a compactor
            // already kept, and that compactor was reached through one of the
            // entry points below — so the replay has run upstream and a second
            // pass would be a no-op. Its four callers (`disk`, `tcpdump`,
            // `vmstat`, `netstat`) all sit inside `dispatch_kind`.
            if name == "compact_remainder" {
                continue;
            }
            // The call must be inside the next few lines of the body — it is a
            // normalisation, so it comes before anything reads the text.
            let body: String = src
                .lines()
                .skip(n + 1)
                .take(8)
                .collect::<Vec<_>>()
                .join("\n");
            entries.push((name, body.contains("normalise_redraws")));
        }
        assert!(
            entries.len() >= 3,
            "the scan found {} entry points — it stopped matching the declarations",
            entries.len()
        );
        let missing: Vec<&str> = entries
            .iter()
            .filter(|(_, ok)| !ok)
            .map(|(n, _)| n.as_str())
            .collect();
        assert!(
            missing.is_empty(),
            "these entry points decide a route without replaying the redraw first: {missing:?}"
        );
    }
}

/// A `.yaml` holding Kubernetes manifests is read as manifests: the one
/// ConfigMap among 300 Deployments is named, wherever it sits.
#[test]
fn a_manifest_file_named_yaml_names_every_kind() {
    let mut docs: Vec<String> = (0..300)
        .map(|i| format!("apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: svc-{i}\nspec:\n  replicas: 1\n  template:\n    spec:\n      containers:\n        - name: app\n          image: registry.local/svc-{i}:1.{i}.0\n"))
        .collect();
    docs.insert(150, "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: feature-flags\ndata:\n  NEW_CHECKOUT: \"true\"\n".into());
    let text = format!("---\n{}", docs.join("---\n"));
    let (_k, out, _) = compact_with_path_hint(&text, "deploy.yaml");
    assert!(out.contains("301 document(s)") && out.contains("ConfigMap") && out.contains("feature-flags"), "{}", &out[..out.len().min(400)]);
}

/// One manifest that fits keeps its spec under the generic YAML pass.
#[test]
fn a_single_manifest_named_yaml_keeps_its_spec() {
    let text = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: web\nspec:\n  containers:\n    - name: app\n      image: nginx:1.27\n      env:\n        - name: MODE\n          value: production\n";
    let (_k, out, _) = compact_with_path_hint(text, "pod.yaml");
    assert!(out.contains("value: production"), "{out}");
}

/// `pnpm test` over `tsx --test`: TAP behind a script runner's banner, no
/// `node` in the argv. Read as TAP — not as a markdown outline of
/// `# Subtest:` headings — with the run's verdict kept.
#[test]
fn tap_behind_a_script_runner_is_read_as_tap() {
    let text = include_str!("../tests/fixtures/linux/pnpm_tsx_tap.txt");
    let (k, out, _) = compact(text);
    assert_eq!(k, Kind::Tap);
    for kept in ["# tests 81", "# pass 81", "# fail 0"] {
        assert!(out.contains(kept), "{kept}: {}", &out[..out.len().min(600)]);
    }
    assert!(out.len() * 4 < text.len(), "{} vs {}", out.len(), text.len());
}
