//! **Scoring a section: dense, lexical, and what blends them.**
//!
//! Two measures that are wrong in different ways — the embedding misses a
//! rare term, the lexical score misses a rephrasing — and an `alpha` that says
//! how much each one is trusted depending on the shape of the question. A
//! questions-and-answers page is not scored like a manual.

use super::*;

/// Set the `has_code / has_table / has_param` type flags from the body.
pub(super) fn classify(sec: &mut Section) {
    let mut in_fence = false;
    for line in &sec.body_lines {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            sec.has_code = true;
            continue;
        }
        if in_fence {
            sec.has_code = true;
            continue;
        }
        // Indented code block: 4 leading spaces or a tab on a non-blank line.
        if !line.trim().is_empty() && (line.starts_with("    ") || line.starts_with('\t')) {
            sec.has_code = true;
        }
        // Table row: a line that is a pipe-delimited row.
        let tt = line.trim();
        if tt.starts_with('|') && tt.matches('|').count() >= 2 {
            sec.has_table = true;
        }
        // Parameter / definition shape: `- name: ...`, `**name** — ...`,
        // or an HTML `<dt>` definition term.
        if (t.starts_with("- ") || t.starts_with("* ")) && t.contains(':') {
            sec.has_param = true;
        }
        if t.contains("**") && (t.contains('—') || t.contains('–') || t.contains(" - ")) {
            sec.has_param = true;
        }
        if t.contains("<dt>") {
            sec.has_param = true;
        }
    }
}

/// Deduplicated query terms with meta-request stopwords stripped — the same
/// preprocessing the flat baseline applies, so both extractors see the same
/// anchor set.
pub(super) fn query_terms(prompt: &str) -> Vec<String> {
    // Used to be a byte-for-byte copy of `disclose::scoped::scoped_query_terms`,
    // which owns `SCOPE_STOPWORDS`. Two copies of the same tokenisation drift
    // apart as soon as the list is enriched on one side only.
    crate::disclose::scoped::scoped_query_terms(prompt)
}

/// Resolve the hybrid blend weight from `TOKENADE_RAG_ALPHA` (clamped to
/// `[0,1]`), defaulting to [`DEFAULT_ALPHA`].
pub(super) fn alpha() -> f64 {
    std::env::var("TOKENADE_RAG_ALPHA")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or(DEFAULT_ALPHA)
}

/// Dense (embedding) similarity of each candidate section to the query.
///
/// `None` when the embedder is unavailable (offline / lock / inference error) —
/// the caller then serves the flat BM25 baseline. That fallback used to be a
/// `return` buried in a match arm 100 lines into `prompt_scoped_extract_rag`.
pub(super) fn dense_scores(
    terms: &[String],
    prompt: &str,
    sec_texts: &[String],
    cand: &[usize],
    n: usize,
) -> Option<Vec<f64>> {
    // **The joined TERMS, not the raw question.** The question carries its
    // function words, and the model is static: it turns it into a bag of words
    // where "what / is / the" weigh as much as "retry".
    //
    // MEASURED on 2026-09-14 on **73 real agent questions** taken from
    // transcripts (`rag_eval --real`): embedding the raw question changes the
    // returned passage on **27 of the 73**, and the cases read by hand all go
    // the wrong way — the page header and the author's signature come up
    // instead of the requested section. The joined form stays.
    let query_input = terms.join(" ");
    let _ = prompt;
    let mut inputs: Vec<String> = Vec::with_capacity(cand.len() + 1);
    inputs.push(query_input);
    for &i in cand {
        inputs.push(truncate_bytes(&sec_texts[i], EMBED_TRUNC_BYTES).to_string());
    }
    match crate::embedding::embed_batch(&inputs) {
        Ok(vecs) if vecs.len() == cand.len() + 1 => {
            let q = &vecs[0];
            let secs_v = &vecs[1..1 + cand.len()];
            let mut dense = vec![0.0_f64; n];
            for (k, &i) in cand.iter().enumerate() {
                dense[i] = crate::embedding::cosine(q, &secs_v[k]).max(0.0) as f64;
            }
            Some(dense)
        }
        // Any embedding error (offline / lock / inference) → flat baseline.
        _ => None,
    }
}

/// Union the structural extract with the flat BM25 baseline.
///
/// The two extractors render the same bytes differently, so the dedup is on
/// NORMALISED content, not on exact lines. The union is never much worse than
/// either alone, and recall is worth more than the extra tokens here.
pub(super) fn merge_flat_baseline(
    out: &mut String,
    markdown: &str,
    prompt: &str,
    fusion_budget: usize,
) {
    if let Some(flat) = prompt_scoped_extract(markdown, prompt, fusion_budget) {
        // Dedup on NORMALISED CONTENT, not on exact lines.
        //
        // The two extractors render the same bytes differently: the structural
        // pick emits a section's lines verbatim, while the flat extractor's item
        // path collapses a multi-line unit into ONE trimmed line. Those strings
        // never compare equal, so a line-exact `have` set called every one of
        // them fresh and appended the same content a second time. MEASURED
        // 2026-08-01 on `tokenade.net/llms.txt`: 47 137 characters served from a
        // 28 471-character page — 1.66x the page, from a 500-token budget.
        //
        // Comparing each candidate against the whole emitted text, whitespace
        // collapsed, catches the reshaped copies too. `contains` is the right
        // test rather than set membership precisely because one side may be a
        // fragment of a longer joined line.
        let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let have = squash(out);
        let fresh: Vec<&str> = flat
            .lines()
            .filter(|l| {
                let t = l.trim();
                if t.is_empty() {
                    return false;
                }
                !have.contains(&squash(t))
            })
            .collect();
        // A couple of stray lines is noise; a real uncovered region is worth
        // the tokens.
        if fresh.len() >= 2 {
            insert_in_document_order(out, markdown, &fresh);
        }
    }
}

/// Place the fusion block where it sits in the DOCUMENT, not after everything.
///
/// The structural pick is emitted in document order, as runs separated by
/// `…`; the flat lines it missed used to be appended after the last run.
/// MEASURED 2026-10-03 on calibre's `quick_start/eng.epub`, "what is DRM":
/// the pick held the second block of section 2.7, the fusion held its heading
/// and its first line — `DRM is an acronym for Digital Rights Management.` —
/// and the definition came out AFTER the paragraph that builds on it. Same
/// lines, same count: only where they go changes.
fn insert_in_document_order(out: &mut String, markdown: &str, fresh: &[&str]) {
    let block = format!("{}\n", fresh.join("\n"));
    let at = |text: &str| -> Option<usize> {
        let first = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("> "))?;
        markdown.find(first)
    };
    let runs: Vec<&str> = out.split("…\n\n").collect();
    // Only into the SAME section: a fusion block from a table of contents
    // three chapters up, moved ahead of the passage that answers, buried the
    // answer under a list of titles ("how do I auto convert an e-book", same
    // epub). A heading between the two means another section — stay last.
    let same_section = |from: usize, to: usize| {
        markdown[from..to]
            .lines()
            .skip(1)
            .all(|l| !l.trim_start().starts_with('#'))
    };
    let later = at(&block).and_then(|pos| {
        let k = runs.iter().position(|r| at(r).is_some_and(|p| p > pos))?;
        same_section(pos, at(runs[k])?).then_some(k)
    });
    *out = match later {
        Some(k) => {
            let mut parts: Vec<String> = runs.iter().map(|r| r.to_string()).collect();
            parts.insert(k, block);
            parts.join("…\n\n")
        }
        None => format!("{out}\n…\n\n{block}"),
    };
}

/// Structural + dense-RAG scoped extract. See the module docs.
///
/// Returns `None` (whole-page fallback, same contract as the baseline) when
/// the prompt tokenises to nothing. Otherwise returns a coherent set of
/// section subtrees packed to `budget_tokens`, or transparently falls back
/// to [`prompt_scoped_extract`] on headingless / anchorless input or any
/// embedding error.
/// The blocks eligible for the dense embed pass.
///
/// Huge pages: embedding many hundreds of blocks is where the batch
/// fails or stalls (measured: ~800-block kernel-parameters page → embed
/// error → silent flat fallback). Prefilter to the top blocks by BM25 —
/// the rest keep a zero dense score but stay lexically eligible.
pub(super) fn embed_candidates(bm25: &[f64], n: usize) -> Vec<usize> {
    // 192 dates from a time when embedding was expensive (the comment above
    // cites a page of ~800 blocks that made the batch FAIL). The model is now
    // static and mmap'ed: 0.38 ms per block in release, so 800 blocks would
    // cost 0.3 s. The cap could therefore go up — MEASURED, it is useless
    // here: across the 44 pages of the web corpus **none reaches 192 blocks**,
    // and on 73 real agent questions, raising the cap to 4096 changes the
    // returned passage on **2 of 73**. It stays where it is, and the
    // measurement is written down so nobody has to redo it.
    const EMBED_MAX_SECTIONS: usize = 192;
    let embed_max_sections = EMBED_MAX_SECTIONS;
    if n <= embed_max_sections {
        return (0..n).collect();
    }
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&x, &y| {
        bm25[y]
            .partial_cmp(&bm25[x])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(x.cmp(&y))
    });
    let mut top: Vec<usize> = idx.into_iter().take(embed_max_sections).collect();
    top.sort_unstable();
    top
}

/// Whether a section's heading — or any breadcrumb above it — starts with `w`.
pub(super) fn head_starts(s: &Section, w: &str) -> bool {
    s.heading.to_ascii_lowercase().starts_with(w)
        || s.breadcrumb
            .iter()
            .any(|b| b.to_ascii_lowercase().starts_with(w))
}

/// Q&A page shape (reader-built Stack Exchange pages, FAQ pages): the
/// question restates the query almost verbatim, so pure similarity keeps
/// landing on it while the user wants the ANSWERS (measured). When both
/// shapes are present, answer sections get a counter-boost.
pub(super) fn is_qa_page(sections: &[Section]) -> bool {
    sections.iter().any(|s| head_starts(s, "question"))
        && sections.iter().any(|s| head_starts(s, "answer"))
}

/// Every per-block signal the ranker blends, gathered once. The two maxima
/// ride along because they are normalisers, not signals: recomputing them per
/// caller is how two surfaces end up normalising against different numbers.
pub(super) struct BlockSignals<'a> {
    pub(super) sections: &'a [Section],
    pub(super) sec_texts: &'a [String],
    pub(super) terms: &'a [String],
    pub(super) bm25: &'a [f64],
    pub(super) bm25_max: f64,
    pub(super) dense: &'a [f64],
    pub(super) dense_max: f64,
}

/// Hybrid blend × field / type / label / QA / link-density multipliers,
/// highest score first, ties broken by document order (determinism).
pub(super) fn score_sections(sig: &BlockSignals<'_>) -> Vec<(usize, f64)> {
    let &BlockSignals {
        sections,
        sec_texts,
        terms,
        bm25,
        bm25_max,
        dense,
        dense_max,
    } = sig;
    let n = sections.len();
    let a = alpha();
    let qa_page = is_qa_page(sections);
    let mut scored: Vec<(usize, f64)> = (0..n)
        .map(|i| {
            let bm = if bm25_max > 0.0 {
                bm25[i] / bm25_max
            } else {
                0.0
            };
            let dn = if dense_max > 0.0 {
                dense[i] / dense_max
            } else {
                0.0
            };
            let base = a * bm + (1.0 - a) * dn;
            let field = field_mult(&sections[i], terms);
            let type_mult =
                if sections[i].has_code || sections[i].has_param || sections[i].has_table {
                    type_bonus()
                } else {
                    1.0
                };
            let qa_mult = if qa_page && head_starts(&sections[i], "answer") {
                qa_answer_mult()
            } else {
                1.0
            };
            // Link-dense blocks (TOCs, "access"/download link clusters,
            // surviving footers) score high lexically but carry no answer
            // text — mirror the extraction-side link-density prune as a
            // soft demotion (measured: a README's table-of-contents and an
            // arXiv "Access Paper" box outranked the actual content).
            let nav_pen = if link_density(&sec_texts[i]) > nav_density_thresh() {
                nav_link_penalty()
            } else {
                1.0
            };
            (i, base * field * type_mult * qa_mult * nav_pen)
        })
        .filter(|(_, s)| *s > 0.0)
        .collect();
    // Highest score first, ties broken by document order (determinism).
    scored.sort_by(|x, y| {
        y.1.partial_cmp(&x.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(x.0.cmp(&y.0))
    });
    scored
}

/// What one section costs the budget: its block, its breadcrumb label, and the
/// blank line between runs.
///
/// The emit loop writes a breadcrumb ABOVE each new run, so the budget has
/// to include it. It did not, and that is the whole reason a 500-token
/// budget could serve 6 117 tokens: sizing measured `section_block`, emission
/// wrote `breadcrumb_label()` + `section_block`. Charging every section for
/// its own label over-charges the sections inside a contiguous run (they
/// share one label) — conservative in the only safe direction, since the
/// alternative is a budget that silently is not one.
pub(super) fn section_token_costs(sections: &[Section]) -> Vec<usize> {
    sections
        .iter()
        .map(|s| {
            crate::tokenizer::estimate(&section_block(s))
                + crate::tokenizer::estimate(&s.breadcrumb_label())
                + 2
        })
        .collect()
}

/// Pre-split section (`group`) → the blocks it was split into.
pub(super) fn group_members(sections: &[Section]) -> std::collections::BTreeMap<usize, Vec<usize>> {
    let mut members: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for (i, s) in sections.iter().enumerate() {
        members.entry(s.group).or_default().push(i);
    }
    members
}

/// Distinct-anchor-region cap. Measured (2026-07-11 recall@K sweep):
/// recall rises steeply 1→4 regions then PLATEAUS — a 5th region adds
/// ~0pp (holdout) for +tokens. Capping at 4 keeps the packer from
/// spreading budget across long-tail regions that don't pay. Overridable
/// via `TOKENADE_RAG_MAX_REGIONS` (tuning / measurement).
pub(super) fn region_cap() -> usize {
    std::env::var("TOKENADE_RAG_MAX_REGIONS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&k| k > 0)
        .unwrap_or(RAG_MAX_REGIONS_DEFAULT)
}

/// Which query terms each block carries, for the coverage bonus below.
pub(super) fn blocks_query_terms<'a>(
    sec_toks: &[Vec<String>],
    terms: &'a [String],
) -> Vec<Vec<&'a String>> {
    sec_toks
        .iter()
        .map(|toks| {
            terms
                .iter()
                .filter(|t| toks.iter().any(|w| &w == t))
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod fusion_goes_in_its_place {
    /// The definition that opens a section comes BEFORE the paragraph the
    /// structural pick chose from further down the same section.
    #[test]
    fn a_fusion_block_precedes_the_passage_that_follows_it_in_the_document() {
        let md = "### 2.7: DRM\nDRM is an acronym for Digital Rights Management.\nIntro line two.\n\nDRM enables various rights.\n";
        let mut out = "> Tasks > 2.7: DRM\nDRM enables various rights.\n".to_string();
        super::insert_in_document_order(
            &mut out,
            md,
            &[
                "### 2.7: DRM",
                "DRM is an acronym for Digital Rights Management.",
            ],
        );
        assert_eq!(
            out,
            "### 2.7: DRM\nDRM is an acronym for Digital Rights Management.\n…\n\n> Tasks > 2.7: DRM\nDRM enables various rights.\n"
        );
    }

    /// A fusion block from ANOTHER section (a table of contents up front) does
    /// not jump ahead of the passage that answers.
    #[test]
    fn a_block_from_another_section_stays_at_the_end() {
        let md = "## Contents\n2.3 Basics\n2.4 Auto conversion\n## Introduction\nWelcome.\n### 2.4: Auto conversion\nSelect the book.\n";
        let mut out = "> 2.4: Auto conversion\nSelect the book.\n".to_string();
        super::insert_in_document_order(&mut out, md, &["2.3 Basics", "2.4 Auto conversion"]);
        assert_eq!(
            out,
            "> 2.4: Auto conversion\nSelect the book.\n\n…\n\n2.3 Basics\n2.4 Auto conversion\n"
        );
    }

    /// A fusion block from AFTER every run still goes last.
    #[test]
    fn a_block_that_comes_after_stays_at_the_end() {
        let md = "A first.\nB second.\nC third.\n";
        let mut out = "A first.\n".to_string();
        super::insert_in_document_order(&mut out, md, &["B second.", "C third."]);
        assert_eq!(out, "A first.\n\n…\n\nB second.\nC third.\n");
    }
}
