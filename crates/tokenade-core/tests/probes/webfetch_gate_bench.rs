//! Where should the WebFetch intercept threshold sit?
//!
//! `WEBFETCH_INTERCEPT_MIN_TOKENS_DEFAULT = 12_000` was never measured: its own
//! comment calls it a "proxy for the answer may sit beyond the native fetch
//! truncation (~100 KB)", and the two MEASURED tags around it attach to the
//! DIRECTION (native wins small, truncates large), not to the number. Meanwhile
//! tokenade's own universal rule puts the switch from "return everything" to
//! "summary + stash" at ~5 000 tokens.
//!
//! This harness settles it against real pages the agent actually fetched, and
//! against the real native answers it got back:
//!
//!   experiments/webfetch-gate/calls.tsv   url, prompt, native result (verbatim)
//!   experiments/webfetch-gate/corpus/     the raw HTML for each of them
//!
//! Both come from `~/.claude/projects` transcripts — only the calls the NATIVE
//! WebFetch served, because those are the ones with a baseline to beat.
//!
//! Run it:
//!     cargo test -p tokenade-core --test webfetch_gate_bench -- --ignored --nocapture
//!
//! What it reports per page: the raw HTML tokens, the distilled tokens (WHAT THE
//! GATE ACTUALLY COMPARES — `sav.tokens_after`, not the raw size), the tokens the
//! native path spent, the tokens our scoped extract would spend, and whether the
//! numeric facts the native answer reported survive in our extract. Numbers are
//! the fact type these prompts ask for, and checking them needs no judge.

use std::path::{Path, PathBuf};

// ── the cost model ───────────────────────────────────────────────────
//
// Counting DELIVERED TOKENS is the wrong unit and the first run of this harness
// got it wrong. The native WebFetch does not hand the agent a page: it runs the
// page through a small model (Haiku) with the prompt and hands back that model's
// summary. So the native path bills THREE things and only one of them is the
// summary:
//
//   1. Haiku INPUT — the whole page, every time, uncached.
//   2. Haiku OUTPUT — the summary.
//   3. the main model's INPUT — the same summary, entering the real context.
//
// Our path bills ONE: the main model's input for the extract we serve. We read
// and distil the page locally, which costs nothing at the API.
//
// Prices per 1M tokens, from the live Helicone table cached in
// `~/.tokenade/llm-costs.json` (2026-08-01): Haiku 4.5 $1 in / $5 out, Opus
// 4.5-4.6 $5 in / $25 out. Cache-read is priced at a tenth of input, the usual
// Anthropic ratio, and is only used for the carry term.
const HAIKU_IN: f64 = 1.0;
const HAIKU_OUT: f64 = 5.0;
const MAIN_IN: f64 = 5.0;
const MAIN_CACHE_READ: f64 = 0.5;

/// How many later turns still carry what we put in the context. Whatever lands
/// in the main context is re-sent on every subsequent turn at the cache-read
/// price, so a bigger payload keeps costing after the turn that fetched it.
/// This cuts AGAINST us (our extract is larger than the native summary), which
/// is exactly why it belongs in the model.
const CARRY_TURNS: f64 = 10.0;

/// Dollar cost of the native path for one page.
///
/// `haiku_input` is the page as HAIKU sees it, and nobody outside Anthropic
/// knows that number: Claude Code does its own HTML→markdown conversion before
/// the sub-model call. So the table reports a BAND — the low bound assumes their
/// conversion is as aggressive as ours (our distilled tokens), the high bound
/// assumes it keeps the raw markup (our raw HTML tokens). The truth is between.
fn native_usd(haiku_input: usize, summary: usize) -> f64 {
    let m = 1_000_000.0;
    haiku_input as f64 / m * HAIKU_IN
        + summary as f64 / m * HAIKU_OUT
        + summary as f64 / m * MAIN_IN
        + summary as f64 / m * MAIN_CACHE_READ * CARRY_TURNS
}

/// Dollar cost of our path: main-model input for what we serve, plus its carry.
fn ours_usd(delivered: usize) -> f64 {
    let m = 1_000_000.0;
    delivered as f64 / m * MAIN_IN + delivered as f64 / m * MAIN_CACHE_READ * CARRY_TURNS
}

/// The threshold under test, and the candidate the 5k disclosure rule implies.
const CURRENT: usize = 12_000;
const CANDIDATE: usize = 5_000;

/// Inline cap: above this the intercept serves a scoped extract instead of the
/// whole distilled page (`webfetch_inline_cap`).
///
/// This is the bench's own ceiling on what a serve can deliver, and it is
/// DELIBERATELY held at twice the product's `WEBFETCH_INLINE_CAP_DEFAULT`
/// (4 000). The comment here read "default 8k" and that was simply wrong about
/// the product — caught 2026-09-05, where a test fixture sized against
/// the number written here sailed over the real cap and left a mutation alive.
/// The value is kept, not corrected down, because it prices tokenade's side
/// PESSIMISTICALLY: a bigger inline cap means we bill for more delivered
/// tokens on every page in 4k..8k, so the interception verdict this bench
/// recorded is understated rather than flattered. Do not read a product
/// threshold off this constant.
const INLINE_CAP: usize = 8_000;

struct Row {
    url: String,
    prompt: String,
    native_tokens: usize,
    raw_tokens: usize,
    /// Visible text — the middle estimate of Haiku's input.
    vis_tokens: usize,
    md_tokens: usize,
    /// `None` when the prompt gives no lexical anchor — the gate passes those
    /// through at ANY size, so they are the control group.
    scoped_tokens: Option<usize>,
    /// Numeric facts in the native answer, and how many of them our extract
    /// (or, for the unanchored ones, the distilled page) still contains.
    facts: usize,
    facts_kept: usize,
    facts_kept_full: usize,
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../experiments/webfetch-gate")
}

fn sha1_12(s: &str) -> String {
    // The corpus filenames come from `sha1sum | cut -c1-12`; recomputing them
    // here would drag in a sha1 dependency for nothing, so the fetch script
    // wrote the mapping down instead.
    let index = std::fs::read_to_string(root().join("corpus/index.tsv")).unwrap_or_default();
    for line in index.lines() {
        let mut f = line.split('\t');
        let (Some(name), Some(_code), Some(url)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if url == s {
            return name.to_string();
        }
    }
    String::new()
}

/// Number-like facts: `4.2`, `12%`, `1,500`, `2026`. Deduped, and single digits
/// are dropped — they match by accident far too often to mean anything.
///
/// **This metric is a WEAK LOWER BOUND and must not be read as a quality score.**
/// Proven on this very corpus: the nginx row scores 0/6 while the extract
/// demonstrably contains both `keepalive_timeout` and `75s` — the exact answer the
/// prompt asked for. The native summary had simply written its numbers
/// differently ("75 seconds", a rounded figure, a number from elsewhere on the
/// page). String containment cannot see through paraphrase, so a LOW score here
/// proves nothing at all.
///
/// Only the COST columns of this harness are sound. Deciding whether the native
/// answer is BETTER needs a judge over (prompt, native answer, our extract), and
/// that is a different bench with its own reps policy.
fn facts_of(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let push = |cur: &mut String, out: &mut Vec<String>| {
        let t = cur.trim_matches(|c: char| !c.is_ascii_digit());
        if t.len() >= 2 && t.chars().any(|c| c.is_ascii_digit()) && !out.iter().any(|s| s == t) {
            out.push(t.to_string());
        }
        cur.clear();
    };
    for c in text.chars() {
        if c.is_ascii_digit() || (!cur.is_empty() && matches!(c, '.' | ',' | '%')) {
            cur.push(c);
        } else {
            push(&mut cur, &mut out);
        }
    }
    push(&mut cur, &mut out);
    out
}

/// The page's VISIBLE TEXT, as a middle estimate of what Haiku is fed.
///
/// Neither bound of the band is credible on its own: our distilled markdown is
/// the floor (nobody strips boilerplate as hard as we do) and the raw HTML is a
/// ceiling nothing sensible reaches — `anthropic.com/pricing` is 383 671 tokens
/// of markup for 4 964 tokens of content, and a WebFetch that shipped that to a
/// sub-model would be absurd. Any HTML→markdown conversion, however naive, keeps
/// the visible text and drops `<script>`/`<style>` — so this is the number to
/// reason with, and it needs no guess about Anthropic's converter.
fn visible_tokens(html: &str) -> usize {
    // `char_indices` only ever yields char boundaries, so slicing `lower` at one
    // is safe. Walking raw byte offsets instead panicked on the first Cyrillic
    // page in the corpus — the same byte-boundary trap the sitemap reader hit.
    // `to_ascii_lowercase` keeps the byte layout, so offsets are shared.
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len() / 4);
    let mut in_tag = false;
    let mut skip_to = 0usize;
    for (i, c) in html.char_indices() {
        if i < skip_to {
            continue;
        }
        if c == '<' {
            let mut skipped = false;
            for (open, close) in [("<script", "</script>"), ("<style", "</style>")] {
                if lower[i..].starts_with(open) {
                    skip_to = match lower[i..].find(close) {
                        Some(end) => i + end + close.len(),
                        None => html.len(),
                    };
                    skipped = true;
                    break;
                }
            }
            in_tag = !skipped;
            continue;
        }
        if c == '>' {
            in_tag = false;
            continue;
        }
        if !in_tag {
            out.push(c);
        }
    }
    tokenade_core::tokenizer::estimate(&out)
}

fn kept(facts: &[String], haystack: &str) -> usize {
    facts
        .iter()
        .filter(|f| haystack.contains(f.as_str()))
        .count()
}

fn load_rows() -> Vec<Row> {
    let calls = std::fs::read_to_string(root().join("calls.tsv")).unwrap_or_default();
    let mut rows = Vec::new();
    for line in calls.lines() {
        let mut f = line.split('\t');
        let (Some(url), Some(prompt), Some(_chars), Some(native)) =
            (f.next(), f.next(), f.next(), f.next())
        else {
            continue;
        };
        let name = sha1_12(url);
        if name.is_empty() {
            continue;
        }
        let path = root().join("corpus").join(format!("{name}.html"));
        let Ok(html) = std::fs::read_to_string(&path) else {
            continue;
        };
        // MUST be the pipeline the hook uses, not `html_to_markdown`: the first
        // run of this harness called the raw converter and reported md sizes up
        // to 16x too high (nginx docs: 19 278 tokens vs the 1 200 production
        // actually produces), which put pages in the wrong band and would have
        // argued for moving the threshold on numbers no channel ever sees.
        let d = tokenade_core::web_pipeline::distill_page(&html, url, None);
        let (md, sav) = (d.md, d.savings);
        let md_tokens = sav.tokens_after;
        let scoped = tokenade_core::rag::prompt_scoped_extract_dispatch(
            &md,
            prompt,
            tokenade_core::disclose::webfetch_scoped_budget_for(md_tokens),
        );
        let facts = facts_of(native);
        rows.push(Row {
            url: url.to_string(),
            prompt: prompt.to_string(),
            native_tokens: tokenade_core::tokenizer::estimate(native),
            raw_tokens: sav.tokens_before,
            vis_tokens: visible_tokens(&html),
            md_tokens,
            scoped_tokens: scoped.as_deref().map(tokenade_core::tokenizer::estimate),
            facts: facts.len(),
            facts_kept: scoped.as_deref().map(|s| kept(&facts, s)).unwrap_or(0),
            facts_kept_full: kept(&facts, &md),
        });
    }
    rows
}

/// What tokenade would DELIVER for a page, under a given threshold.
fn delivered(r: &Row, threshold: usize) -> Option<usize> {
    if r.md_tokens < threshold {
        return None; // passthrough — the native answer is what the agent gets
    }
    r.scoped_tokens.map(|t| t.min(INLINE_CAP))
}

#[test]
#[ignore = "needs the local corpus: experiments/webfetch-gate (see fetch_corpus.sh)"]
fn where_should_the_webfetch_intercept_threshold_sit() {
    let rows = load_rows();
    assert!(
        !rows.is_empty(),
        "no corpus at {} — run fetch_corpus.sh first",
        root().display()
    );
    println!("\n{} pages\n", rows.len());
    println!(
        "{:>7} {:>7} {:>7} {:>7} {:>9}  url",
        "raw", "md", "native", "scoped", "facts"
    );
    for r in &rows {
        println!(
            "{:>7} {:>7} {:>7} {:>7} {:>4}/{:<4}  {}",
            r.raw_tokens,
            r.md_tokens,
            r.native_tokens,
            r.scoped_tokens
                .map(|t| t.to_string())
                .unwrap_or_else(|| "-".into()),
            r.facts_kept,
            r.facts,
            r.url.chars().take(72).collect::<String>()
        );
    }

    // ── the band that decides it, in DOLLARS ────────────────────────
    for (label, lo, hi) in [
        ("md < 5k", 0usize, CANDIDATE),
        ("5k \u{2264} md < 12k", CANDIDATE, CURRENT),
        ("md \u{2265} 12k", CURRENT, usize::MAX),
    ] {
        let band: Vec<&Row> = rows
            .iter()
            .filter(|r| r.md_tokens >= lo && r.md_tokens < hi)
            .collect();
        if band.is_empty() {
            println!("\n{label}: 0 pages");
            continue;
        }
        // Only the anchored pages are comparable: the gate passes an unanchored
        // prompt through at ANY threshold, so there is no "ours" for them.
        let anchored: Vec<&&Row> = band.iter().filter(|r| r.scoped_tokens.is_some()).collect();
        let nat_lo: f64 = anchored
            .iter()
            .map(|r| native_usd(r.md_tokens, r.native_tokens))
            .sum();
        let nat_mid: f64 = anchored
            .iter()
            .map(|r| native_usd(r.vis_tokens, r.native_tokens))
            .sum();
        let nat_hi: f64 = anchored
            .iter()
            .map(|r| native_usd(r.raw_tokens, r.native_tokens))
            .sum();
        let ours: f64 = anchored
            .iter()
            .filter_map(|r| r.scoped_tokens)
            .map(|t| ours_usd(t.min(INLINE_CAP)))
            .sum();
        let nat_tok: usize = anchored.iter().map(|r| r.native_tokens).sum();
        let our_tok: usize = anchored.iter().filter_map(|r| r.scoped_tokens).sum();
        println!(
            "\n{label}: {} pages ({} anchored, {} unanchored \u{2192} passthrough at any threshold)",
            band.len(),
            anchored.len(),
            band.len() - anchored.len()
        );
        println!("  delivered tokens : native {nat_tok} vs ours {our_tok}");
        println!(
            "  COST (usd)       : native ${nat_lo:.4} (our-md) / ${nat_mid:.4} (visible text) / \
             ${nat_hi:.4} (raw) vs ours ${ours:.4}  \u{2192} {}",
            if ours < nat_mid {
                format!(
                    "ours cheaper by {:.1}x at the visible-text estimate",
                    nat_mid / ours
                )
            } else if ours > nat_hi {
                format!(
                    "NATIVE cheaper by {:.1}\u{2013}{:.1}x",
                    ours / nat_hi,
                    ours / nat_lo
                )
            } else {
                "overlapping \u{2014} the band straddles the crossover".to_string()
            }
        );
    }

    // ── what moving the threshold would change, in DOLLARS ──────────
    // Whole-corpus totals: every page is counted, intercepted or not, so the
    // rows are comparable to each other.
    for th in [usize::MAX, CURRENT, CANDIDATE, 4_000, 3_000, 1] {
        let mut lo = 0.0;
        let mut mid = 0.0;
        let mut hi = 0.0;
        let mut n = 0;
        for r in &rows {
            match delivered(r, th) {
                Some(t) => {
                    n += 1;
                    let c = ours_usd(t);
                    lo += c;
                    mid += c;
                    hi += c;
                }
                None => {
                    lo += native_usd(r.md_tokens, r.native_tokens);
                    mid += native_usd(r.vis_tokens, r.native_tokens);
                    hi += native_usd(r.raw_tokens, r.native_tokens);
                }
            }
        }
        let name = match th {
            usize::MAX => "never intercept".to_string(),
            1 => "always (anchored)".to_string(),
            n => format!("threshold {n}"),
        };
        println!(
            "\n{name:>20}: intercepts {n:>2}/{} \u{2014} corpus cost ${lo:.4} (our-md) / \
             ${mid:.4} (VISIBLE TEXT) / ${hi:.4} (raw)",
            rows.len()
        );
    }

    // Pages where the native answer carried facts our extract loses are the
    // only ones that can justify keeping the bar high — name them.
    println!(
        "\nrows the weak number-echo metric marks as losses (NOT proof: nginx scores \
         0/6 and contains the answer):"
    );
    let mut any = false;
    for r in &rows {
        if r.scoped_tokens.is_some() && r.facts > 0 && r.facts_kept < r.facts {
            any = true;
            println!(
                "  {:>6} md, echoed {}/{} in extract vs {}/{} in the full page — {} — {:?}",
                r.md_tokens,
                r.facts_kept,
                r.facts,
                r.facts_kept_full,
                r.facts,
                r.url.chars().take(60).collect::<String>(),
                r.prompt.chars().take(50).collect::<String>()
            );
        }
    }
    if !any {
        println!("  none");
    }
}

/// The gate compares the DISTILLED size, not the raw HTML. Worth pinning: a
/// threshold read as raw bytes would fire on a completely different set of
/// pages (the corpus below distils by ~10-30x), and the constant's doc comment
/// mentions "~100 KB" of raw HTML, which is where that confusion starts.
#[test]
#[ignore = "needs the local corpus: experiments/webfetch-gate"]
fn the_threshold_is_compared_against_distilled_tokens_not_raw_html() {
    let rows = load_rows();
    assert!(!rows.is_empty(), "no corpus");
    let raw_over: usize = rows.iter().filter(|r| r.raw_tokens >= CURRENT).count();
    let md_over: usize = rows.iter().filter(|r| r.md_tokens >= CURRENT).count();
    println!(
        "\npages over {CURRENT}: {raw_over} by raw HTML tokens, {md_over} by distilled tokens \
         (of {})",
        rows.len()
    );
    let ratios: Vec<f64> = rows
        .iter()
        .filter(|r| r.md_tokens > 0)
        .map(|r| r.raw_tokens as f64 / r.md_tokens as f64)
        .collect();
    let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
    println!("raw/distilled ratio: mean {mean:.1}x");
    assert!(
        raw_over >= md_over,
        "distillation cannot make a page bigger"
    );
}

/// Guard: the corpus manifest and the fetched pages must line up, or the bench
/// silently measures a subset.
#[test]
#[ignore = "needs the local corpus: experiments/webfetch-gate"]
fn every_manifest_row_has_its_page() {
    let calls = std::fs::read_to_string(root().join("calls.tsv")).unwrap_or_default();
    let wanted = calls.lines().filter(|l| !l.trim().is_empty()).count();
    let got = load_rows().len();
    println!("\nmanifest {wanted} rows, corpus resolved {got}");
    assert!(
        got * 100 >= wanted * 80,
        "only {got}/{wanted} manifest rows resolved to a fetched page"
    );
    assert!(Path::new(&root().join("corpus/index.tsv")).exists());
}

/// Two rows from the table that decide whether the threshold is even the right
/// question. Both are pages the gate INTERCEPTS (md ≥ 12k) or would serve, so a
/// defect here is a defect on exactly the traffic we take over.
#[test]
#[ignore = "needs the local corpus: experiments/webfetch-gate"]
fn diagnose_the_two_rows_that_decide_the_verdict() {
    let rows_src = std::fs::read_to_string(root().join("calls.tsv")).unwrap_or_default();
    for (needle_url, needles) in [
        // The prompt asked for a default value. The page states it in a
        // definition table; if the distilled page has it and our extract does
        // not, the loss is RETRIEVAL, not extraction.
        (
            "nginx.org/en/docs/http/ngx_http_core_module",
            vec!["keepalive_timeout", "75s"],
        ),
        // `scoped` came out BIGGER than the page it was scoped from (11 785 vs
        // 7 118), which no budget should allow.
        ("tokenade.net/llms.txt", vec![]),
    ] {
        for line in rows_src.lines() {
            let mut f = line.split('\t');
            let (Some(url), Some(prompt), _, Some(_native)) =
                (f.next(), f.next(), f.next(), f.next())
            else {
                continue;
            };
            if !url.contains(needle_url) {
                continue;
            }
            let name = sha1_12(url);
            let Ok(html) =
                std::fs::read_to_string(root().join("corpus").join(format!("{name}.html")))
            else {
                continue;
            };
            let d = tokenade_core::web_pipeline::distill_page(&html, url, None);
            let md_tokens = d.savings.tokens_after;
            let budget = tokenade_core::disclose::webfetch_scoped_budget_for(md_tokens);
            let extract = tokenade_core::rag::prompt_scoped_extract_dispatch(&d.md, prompt, budget);
            let ex = extract.unwrap_or_default();
            println!(
                "\n{url}\n  md {md_tokens} tok · budget {budget} tok · extract {} tok",
                tokenade_core::tokenizer::estimate(&ex)
            );
            println!(
                "  prompt: {:?}",
                prompt.chars().take(90).collect::<String>()
            );
            // Which extractor produced the overshoot? Call both directly.
            let flat = tokenade_core::disclose::prompt_scoped_extract(&d.md, prompt, budget)
                .map(|t| tokenade_core::tokenizer::estimate(&t))
                .unwrap_or(0);
            let ragx = tokenade_core::rag::prompt_scoped_extract_rag(&d.md, prompt, budget)
                .map(|t| tokenade_core::tokenizer::estimate(&t))
                .unwrap_or(0);
            println!("  flat alone {flat} tok · rag alone {ragx} tok · budget {budget}");
            for n in &needles {
                println!(
                    "  {n:>18}: raw {} · distilled {} · extract {}",
                    html.contains(n),
                    d.md.contains(n),
                    ex.contains(n)
                );
            }
            // The scoped extract must never exceed the page it was cut from.
            let ex_tok = tokenade_core::tokenizer::estimate(&ex);
            if !ex.is_empty() {
                let dup: usize = {
                    let mut seen = std::collections::HashSet::new();
                    ex.lines()
                        .filter(|l| l.trim().len() > 20)
                        .filter(|l| !seen.insert(l.trim().to_string()))
                        .count()
                };
                // Composition: the RAG path emits structural picks, then a
                // fusion tail after a "…" separator. Sizing them separately says
                // which half broke its budget.
                let parts: Vec<&str> = ex.split("\n…\n").collect();
                println!(
                    "  parts: {}",
                    parts
                        .iter()
                        .map(|p| tokenade_core::tokenizer::estimate(p).to_string())
                        .collect::<Vec<_>>()
                        .join(" + ")
                );
                println!(
                    "  chars: page {} vs extract {} · extract lines {} · repeated lines {dup}",
                    d.md.chars().count(),
                    ex.chars().count(),
                    ex.lines().count()
                );
            }
            if ex_tok > md_tokens {
                println!(
                    "  !! INFLATION: extract {ex_tok} tok > page {md_tokens} tok \
                     (budget said {budget})"
                );
            }
            break;
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Statistics: is the answer stable, or is it 42 pages of noise?
// ═══════════════════════════════════════════════════════════════════════

/// One page reduced to the four numbers the cost model needs.
struct Sample {
    md: usize,
    vis: usize,
    raw: usize,
    /// What tokenade would put in the main context if it intercepted.
    delivered: usize,
    /// What the native path's summary costs, in tokens.
    summary: usize,
    /// True when `delivered`/`summary` are MEASURED for this exact page and
    /// prompt, false when they come from the model/empirical distribution.
    paired: bool,
}

/// Deterministic LCG. A bootstrap needs randomness; a repo with a determinism
/// suite needs the same answer twice. Numerical Recipes constants.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Prices for one scenario. `main_in` is the model the AGENT runs; `haiku_in`
/// is the sub-model Claude Code reads pages with.
#[derive(Clone, Copy)]
struct Prices {
    label: &'static str,
    main_in: f64,
    haiku_in: f64,
    haiku_out: f64,
    carry_turns: f64,
}

impl Prices {
    fn native(&self, haiku_input: usize, summary: usize) -> f64 {
        let m = 1_000_000.0;
        haiku_input as f64 / m * self.haiku_in
            + summary as f64 / m * self.haiku_out
            + summary as f64 / m * (self.main_in + self.main_in / 10.0 * self.carry_turns)
    }
    fn ours(&self, delivered: usize) -> f64 {
        let m = 1_000_000.0;
        delivered as f64 / m * (self.main_in + self.main_in / 10.0 * self.carry_turns)
    }
}

/// Which page size Haiku is assumed to read.
#[derive(Clone, Copy, PartialEq)]
enum HaikuSees {
    OurMarkdown,
    VisibleText,
    RawHtml,
}

fn haiku_input(s: &Sample, m: HaikuSees) -> usize {
    match m {
        HaikuSees::OurMarkdown => s.md,
        HaikuSees::VisibleText => s.vis,
        HaikuSees::RawHtml => s.raw,
    }
}

/// Corpus cost under one threshold. A page is intercepted when its distilled
/// size clears the threshold AND we have something to serve for it.
fn corpus_cost(samples: &[&Sample], th: usize, p: Prices, m: HaikuSees) -> f64 {
    samples
        .iter()
        .map(|s| {
            if s.md >= th && s.delivered > 0 {
                p.ours(s.delivered.min(INLINE_CAP))
            } else {
                p.native(haiku_input(s, m), s.summary)
            }
        })
        .sum()
}

/// Load BOTH tiers.
///
/// Tier A — the 42 pages with a real prompt and the real native answer, so
/// `delivered` and `summary` are measured.
///
/// Tier B — `experiments/cf-clearance/corpus` (147 pages of the CrUX long tail,
/// already in the repo for the web tests). No prompt was ever recorded for
/// these, so nothing can be measured per page; they are here for the SIZE
/// distribution, which is what the cost model actually turns on. `delivered`
/// falls back to the budget the gate would grant (validated against tier A
/// below) and `summary` is drawn from the 62 real native answers.
///
/// Mixing them is the point: tier A is Paul's browsing, tier B is the web.
fn load_samples() -> (Vec<Sample>, Vec<usize>, f64) {
    let observed: Vec<usize> = std::fs::read_to_string(root().join("native_summary_tokens.txt"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();

    let mut out: Vec<Sample> = load_rows()
        .into_iter()
        .map(|r| Sample {
            md: r.md_tokens,
            vis: r.vis_tokens,
            raw: r.raw_tokens,
            delivered: r.scoped_tokens.unwrap_or(0),
            summary: r.native_tokens,
            paired: true,
        })
        .collect();

    // CALIBRATION. Tier B has no prompt, so `delivered` has to be modelled from
    // the budget — and the budget is NOT what we serve: the documented fusion
    // tail adds up to another half budget on top, and the measured pages come
    // out at 1.78x nominal. Using the raw budget for 139 of 181 pages would
    // understate our own cost by that factor on three quarters of the sample,
    // i.e. flatter the conclusion this test exists to check. Calibrate on the
    // pages where the truth is known.
    let calib = {
        let (mut sum, mut n) = (0.0f64, 0usize);
        for s in out.iter().filter(|s| s.paired && s.delivered > 0) {
            let b = tokenade_core::disclose::webfetch_scoped_budget_for(s.md).min(s.md);
            if b > 0 {
                sum += s.delivered as f64 / b as f64;
                n += 1;
            }
        }
        if n == 0 {
            1.0
        } else {
            sum / n as f64
        }
    };

    let tier_b =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../experiments/cf-clearance/corpus");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&tier_b)
        .map(|es| {
            es.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "html"))
                .collect()
        })
        .unwrap_or_default();
    names.sort(); // determinism: the summary draw is keyed by position
    for (k, path) in names.iter().enumerate() {
        let Ok(html) = std::fs::read_to_string(path) else {
            continue;
        };
        let d = tokenade_core::web_pipeline::distill_page(&html, "https://example.invalid/", None);
        let md = d.savings.tokens_after;
        if md == 0 {
            continue;
        }
        let budget = tokenade_core::disclose::webfetch_scoped_budget_for(md);
        out.push(Sample {
            md,
            vis: visible_tokens(&html),
            raw: d.savings.tokens_before,
            // Calibrated, and never more than the page itself.
            delivered: (((budget as f64) * calib) as usize).min(md),
            summary: if observed.is_empty() {
                212
            } else {
                observed[k % observed.len()]
            },
            paired: false,
        });
    }
    (out, observed, calib)
}

#[test]
#[ignore = "needs both corpora: experiments/webfetch-gate + experiments/cf-clearance/corpus"]
fn the_threshold_verdict_survives_bootstrap_and_a_price_sweep() {
    let (samples, observed, calib) = load_samples();
    let paired: Vec<&Sample> = samples.iter().filter(|s| s.paired).collect();
    let all: Vec<&Sample> = samples.iter().collect();
    assert!(
        all.len() >= 100,
        "only {} pages — the point of this test is N",
        all.len()
    );
    println!(
        "\n{} pages ({} paired/measured, {} size-only) · {} observed native answers",
        all.len(),
        paired.len(),
        all.len() - paired.len(),
        observed.len()
    );

    // ── Does the tier-B `delivered` model hold? Check it on tier A, where the
    //    real extract is known. If the budget systematically over- or
    //    under-states what we serve, every tier-B row is biased.
    let (mut ratio_sum, mut n) = (0.0, 0usize);
    for s in &paired {
        if s.delivered == 0 {
            continue;
        }
        let budget = tokenade_core::disclose::webfetch_scoped_budget_for(s.md).min(s.md);
        if budget > 0 {
            ratio_sum += s.delivered as f64 / budget as f64;
            n += 1;
        }
    }
    let model_ratio = ratio_sum / n.max(1) as f64;
    println!(
        "delivered/budget on the measured pages: mean {model_ratio:.2}x over {n} pages \
         — tier B calibrated by {calib:.2}x so the model cannot understate our own cost"
    );

    // ── Bootstrap over pages, for every price scenario × Haiku assumption.
    const RESAMPLES: usize = 4_000;
    let thresholds = [usize::MAX, 12_000usize, 5_000, 4_000, 3_000, 1];
    let scenarios = [
        Prices {
            label: "Opus 4.5/4.6 main ($5/M), 10-turn carry",
            main_in: 5.0,
            haiku_in: 1.0,
            haiku_out: 5.0,
            carry_turns: 10.0,
        },
        // The arm that most disfavours interception: a main model 15x Haiku's
        // input price makes every token WE put in the context expensive, while
        // the page Haiku reads stays cheap. This is the sensitivity Paul asked
        // for — if the verdict holds here, the price ratio is not driving it.
        Prices {
            label: "Opus 4.1 main ($15/M) — worst case for us",
            main_in: 15.0,
            haiku_in: 1.0,
            haiku_out: 5.0,
            carry_turns: 10.0,
        },
        Prices {
            label: "Sonnet main ($3/M)",
            main_in: 3.0,
            haiku_in: 1.0,
            haiku_out: 5.0,
            carry_turns: 10.0,
        },
        Prices {
            label: "Opus 4.5 main, NO carry (single turn)",
            main_in: 5.0,
            haiku_in: 1.0,
            haiku_out: 5.0,
            carry_turns: 0.0,
        },
    ];
    for m in [
        HaikuSees::OurMarkdown,
        HaikuSees::VisibleText,
        HaikuSees::RawHtml,
    ] {
        let mname = match m {
            HaikuSees::OurMarkdown => "Haiku reads OUR distilled md (floor)",
            HaikuSees::VisibleText => "Haiku reads the VISIBLE TEXT (credible)",
            HaikuSees::RawHtml => "Haiku reads RAW HTML (ceiling)",
        };
        for p in scenarios {
            println!("\n── {mname} · {}", p.label);
            let base = corpus_cost(&all, usize::MAX, p, m);
            for th in thresholds {
                // Bootstrap: resample pages with replacement, recompute the
                // cost RATIO against never-intercepting on the same resample so
                // the two policies see identical pages (paired comparison —
                // that is what kills most of the variance).
                let mut rng = Rng(0x5EED_1234_ABCD_0001);
                let mut ratios: Vec<f64> = Vec::with_capacity(RESAMPLES);
                let mut wins = 0usize;
                for _ in 0..RESAMPLES {
                    let pick: Vec<&Sample> =
                        (0..all.len()).map(|_| all[rng.below(all.len())]).collect();
                    let never = corpus_cost(&pick, usize::MAX, p, m);
                    let this = corpus_cost(&pick, th, p, m);
                    if never > 0.0 {
                        ratios.push(this / never);
                    }
                    if this < never {
                        wins += 1;
                    }
                }
                ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let lo = ratios[(RESAMPLES as f64 * 0.025) as usize];
                let hi = ratios[(RESAMPLES as f64 * 0.975) as usize];
                let point = corpus_cost(&all, th, p, m) / base;
                let name = match th {
                    usize::MAX => "never".to_string(),
                    1 => "always".to_string(),
                    n => format!("{n}"),
                };
                println!(
                    "   th {name:>6}: cost {:+6.1}% vs never  [95% CI {:+.1}%..{:+.1}%]  \
                     P(cheaper)={:.3}",
                    (point - 1.0) * 100.0,
                    (lo - 1.0) * 100.0,
                    (hi - 1.0) * 100.0,
                    wins as f64 / RESAMPLES as f64
                );
            }
        }
    }
}
