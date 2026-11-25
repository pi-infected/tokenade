//! **Querying the index** — BM25, hybrid, and what weights a path.
//!
//! Four entry points because a caller does not always ask for the same thing:
//! the bare ranking, the ranking filtered by framework, the full hybrid pass,
//! and the variant that ALSO returns the number of candidates examined —
//! without which a share of results has no denominator.
//!
//! Path weights live here and not in the build: they do not change the index,
//! they change the ORDER. A test file keeps its 0.5 weight, a canonical entry
//! point (`main.rs`, `lib.rs`) gains 1.15×, because BM25 has no way of knowing
//! that `main.rs` says more than `helpers.rs`.

use super::*;

impl SemanticIndex {
    /// Hybrid search with the full v5 pipeline:
    ///   1. BM25 over the inverted index (lexical recall),
    ///   2. cosine over multilingual-e5-small bi-encoder embeddings (the
    ///      semantic signal — synonyms, paraphrases, cross-lingual),
    ///   3. cross-encoder rerank with BGE-reranker-v2-m3 (final precision).
    ///
    /// We do NOT fall back to BM25-only when the embedder or reranker is
    /// unreachable — returning `Err` lets the caller show a clear "no
    /// connection, try later" message rather than silently degrading the
    /// quality of every subsequent search.
    /// Hybrid dense+BM25 search. See [`Self::search_hybrid_with_total`] when
    /// the caller must report how many candidates survived ranking.
    pub fn search_hybrid(
        &self,
        query: &str,
        limit: usize,
        alpha: f32,
    ) -> Result<Vec<Hit>, HybridError> {
        self.search_hybrid_with_total(query, limit, alpha)
            .map(|(hits, _)| hits)
    }

    /// [`Self::search_hybrid`], plus how many candidates the ranking actually
    /// retained before the cut.
    ///
    /// A ranked search has no natural "number of matches" — every document has
    /// a score — but the number that survived scoring IS a property of the
    /// repository, where the size of the caller's own window is not. Without
    /// it, `--output-mode count` could only ever repeat the limit back.
    pub fn search_hybrid_with_total(
        &self,
        query: &str,
        limit: usize,
        alpha: f32,
    ) -> Result<(Vec<Hit>, usize), HybridError> {
        let bm25_hits = self.search(query, limit * 4);
        if bm25_hits.is_empty() {
            return Ok((bm25_hits, 0));
        }
        // Bi-encoder vectors for the query + every BM25 candidate. We batch
        // the docs in a single call to amortise the ONNX session cost.
        let q_vec = crate::embedding::embed_text(query).map_err(HybridError::Embed)?;
        // **What the dense arm compares: the path, and nothing else.**
        //
        // This site embeds `h.file` — the string `src/auth.rs` — and the
        // documentation of `Doc::snippet` claims that a snippet (path +
        // symbols + first comments) would be "dramatically stronger".
        // MEASURED on 546 real queries (commit message → files touched,
        // `rag_eval`), the three forms are equivalent: nDCG@10 0.3192 (path),
        // 0.3142 (snippet), 0.3163 (both). The reason is structural — the
        // dense arm only RE-SCORES the candidates BM25 already found, so
        // enriching its text does not change who is a candidate. The path
        // stays, and the measurement is here so that the next attempt starts
        // with recall, not with the text.
        let texts: Vec<String> = bm25_hits.iter().map(|h| h.file.clone()).collect();
        let doc_vecs = crate::embedding::embed_batch(&texts).map_err(HybridError::Embed)?;

        // Normalise BM25 scores to [0, 1] so blending with cosine isn't
        // dominated by raw IDF magnitude.
        let max_bm = bm25_hits
            .iter()
            .map(|h| h.score)
            .fold(0.0_f64, f64::max)
            .max(1.0);
        let mut blended: Vec<Hit> = bm25_hits
            .into_iter()
            .enumerate()
            .map(|(i, mut h)| {
                let bm = (h.score / max_bm) as f32;
                let cos = doc_vecs
                    .get(i)
                    .map(|v| crate::embedding::cosine(&q_vec, v).max(0.0))
                    .unwrap_or(0.0);
                // Down-weight test files. They use the same vocabulary
                // as the implementations they exercise, so BM25 and the
                // bi-encoder both rank them next to real source. In
                // practice an engineer asking "where's the language
                // detection logic" wants `utils.py`, not
                // `tests/filters_tests.py`. We multiply the blended
                // score by 0.5 for test paths — enough to push tests
                // below same-quality source without hiding them
                // entirely (a unique test-only term still wins).
                let blended_score = (alpha * bm + (1.0 - alpha) * cos) * path_weight(&h.file);
                h.score = blended_score as f64;
                h
            })
            .collect();
        // Tie-break matters more here than anywhere else in this file: the
        // cut below feeds `rerank_window` candidates to the cross-encoder, so
        // an unstable tie decides WHICH files the reranker ever sees.
        blended.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.file.cmp(&b.file))
        });
        // Candidates the ranking kept — the honest answer to "how many".
        let retained = blended.iter().filter(|h| h.score > 0.0).count();

        // Cross-encoder rerank on the top `limit*3` candidates. We pass the
        // per-file snippet (path + symbols + first comments) rather than
        // the bare path — paths give the cross-encoder almost no signal,
        // snippets let it actually score relevance.
        let rerank_window = (limit * 3).min(blended.len());
        let mut doc_by_file: HashMap<&str, &Doc> = HashMap::with_capacity(self.docs.len());
        for d in &self.docs {
            doc_by_file.insert(d.file.as_str(), d);
        }
        let candidates: Vec<String> = blended
            .iter()
            .take(rerank_window)
            .map(|h| {
                doc_by_file
                    .get(h.file.as_str())
                    .map(|d| {
                        if d.snippet.is_empty() {
                            h.file.clone()
                        } else {
                            d.snippet.clone()
                        }
                    })
                    .unwrap_or_else(|| h.file.clone())
            })
            .collect();
        let reranked =
            crate::reranker::rerank(query, &candidates, limit).map_err(HybridError::Rerank)?;

        // SELECTION, not scoring. `reranker::rerank` is an identity shim since
        // the cross-encoder was dropped — it returns the candidates in the
        // order given, with a flat score. Copying that flat score over the
        // fused one erased the only relevance signal there is: every hit of
        // every query came back at 1.00, so the number told the reader
        // nothing, and a query that matched nothing looked as certain as one
        // that matched exactly. Worse, the caller's framework boost multiplies
        // this score — against a flat one, a 25% nudge became an absolute
        // reordering. Keep the fused score; take only the choice of documents.
        let mut out = Vec::with_capacity(limit);
        for r in reranked {
            if let Some(h) = blended.get(r.index).cloned() {
                out.push(h);
            }
        }
        Ok((out, retained))
    }

    /// Rank documents against `query`. Returns up to `limit` hits sorted by
    /// descending BM25 score. Empty query → empty result. This is the
    /// no-network path — pure BM25, no embedding model needed. Used for
    /// the lexical-recall first stage of [`Self::search_hybrid`].
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        if self.docs.is_empty() {
            return Vec::new();
        }
        let mut terms = tokenize(query);
        // Safety net: if the user typed pure stopwords (or punctuation),
        // we used to silently return zero hits — agents then re-read raw
        // files. Fall back to treating the lowercased query as a single
        // literal term so the inverted index still has a chance to match.
        // If the literal isn't in the index either, we additionally do a
        // substring scan over document snippets below.
        if terms.is_empty() {
            let raw = query.trim().to_lowercase();
            if raw.len() >= 2 {
                terms.push(raw);
            } else {
                return Vec::new();
            }
        }
        let n = self.docs.len() as f64;
        // doc_id -> (cumulative score, set of contributing terms)
        let mut scores: HashMap<u32, (f64, Vec<String>)> = HashMap::new();
        for t in &terms {
            let postings = match self.inv.get(t) {
                Some(p) => p,
                None => continue,
            };
            let df = postings.len() as f64;
            // BM25 IDF with the +1 smoothing so it never goes negative.
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            for p in postings {
                let dl = self.docs[p.doc as usize].len as f64;
                let tf = p.tf as f64;
                let norm = tf * (self.k1 + 1.0)
                    / (tf + self.k1 * (1.0 - self.b + self.b * dl / self.avgdl.max(1.0)));
                let entry = scores.entry(p.doc).or_insert((0.0, Vec::new()));
                entry.0 += idf * norm;
                if entry.1.len() < 5 && !entry.1.contains(t) {
                    entry.1.push(t.clone());
                }
            }
        }
        let mut hits: Vec<Hit> = scores
            .into_iter()
            .map(|(doc, (score, matched))| Hit {
                file: self.docs[doc as usize].file.clone(),
                score,
                matched,
            })
            .collect();
        // Tie-break on the path. `scores` is a `HashMap`, so `into_iter`
        // hands us an order that changes between processes; a score-only
        // sort would then rank equal-scoring files differently on every run,
        // and `truncate(limit)` turns that into a different ANSWER, not just
        // a different order. Same rule as `ranking::rrf_scores`.
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.file.cmp(&b.file))
        });
        hits.truncate(limit);

        // Final safety net: if BM25 returned nothing AND the raw lowercased
        // query is a meaningful string, scan the doc snippets for literal
        // substring matches. Slower than the inverted-index path (O(docs ×
        // snippet_len) instead of O(postings)), but it only runs when the
        // index legitimately can't help — typical case: a single-character
        // edit before the user fully typed the symbol, or a query whose
        // term genuinely doesn't appear in the indexed text (only in raw
        // source). Better to return a fuzzy match than zero.
        if hits.is_empty() {
            let needle = query.trim().to_lowercase();
            if needle.len() >= 3 {
                let mut substr_hits: Vec<Hit> = Vec::new();
                for doc in &self.docs {
                    let hay = doc.snippet.to_lowercase();
                    if hay.contains(&needle) || doc.file.to_lowercase().contains(&needle) {
                        substr_hits.push(Hit {
                            file: doc.file.clone(),
                            // Synthetic low score so a callers can tell it
                            // came from the fallback path (real BM25 scores
                            // are strictly positive).
                            score: -1.0,
                            matched: vec![format!("substr:{needle}")],
                        });
                    }
                }
                substr_hits.truncate(limit);
                return substr_hits;
            }
        }
        hits
    }
}

/// Build a ~500-char snippet that gives the cross-encoder reranker real
/// context to score against: file path components + first 12 symbol names +
/// first few comment lines. Truncated to roughly the length one would
/// reasonably present to a human glancing at the file.
pub(crate) const SNIPPET_CHAR_CAP: usize = 500;

/// Score multiplier for a file path: combines [`test_path_weight`] with
/// [`entry_point_boost`] so callers get a single number per file.
/// **Public so that the BENCH measures the SHIPPED rule, not a copy.**
///
/// `rag_eval` compares the ranking with and without this weighting; a
/// reimplementation in the bench would measure the bench. Same reason as
/// `srcsweep` for the guards: a single declarer.
pub fn path_weight(file: &str) -> f32 {
    test_path_weight(file) * entry_point_boost(file)
}

/// Boost the ranking of canonical entry-point files. When a human asks
/// "how does this codebase start" or "where's the main loop", they
/// almost always want one of these — but BM25 has no way to know that
/// `main.rs` is more meaningful than `helpers.rs`. We bump entry
/// points by 1.15× so they outrank similar-score files.
///
/// Pattern set covers the typical roots for: Rust, Go, Python, JS/TS,
/// Java/Kotlin, C/C++, Ruby, Elixir. We deliberately only check the
/// filename, not the directory — `src/main.rs` and `cmd/foo/main.go`
/// both get the boost, but a deeply nested `vendor/.../main.go` is
/// already filtered upstream by the index walk (see
/// [`should_skip_dir`](crate::index::should_skip_dir)).
pub(crate) fn entry_point_boost(file: &str) -> f32 {
    let last = file
        .rsplit(['/', '\\'])
        .next()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        last.as_str(),
        "main.rs"
            | "lib.rs"
            | "main.go"
            | "main.py"
            | "app.py"
            | "__init__.py"
            | "__main__.py"
            | "wsgi.py"
            | "asgi.py"
            | "index.ts"
            | "index.tsx"
            | "index.js"
            | "index.jsx"
            | "main.ts"
            | "main.tsx"
            | "app.ts"
            | "app.tsx"
            | "app.js"
            | "server.ts"
            | "server.js"
            | "main.java"
            | "application.java"
            | "main.kt"
            | "main.cpp"
            | "main.c"
            | "main.rb"
            | "app.rb"
            | "config.ru"
            | "application.ex"
            | "mod.rs"
    )
    .then_some(1.15_f32)
    .unwrap_or(1.0)
}

/// Score multiplier for a file path, de-emphasising files that rarely
/// answer "where is X" questions. Delegates to the single classifier
/// [`crate::ranking::classify_file`] so there is exactly one notion of
/// file class across exploration ranking; test files keep their prior
/// `0.5` weight, source stays `1.0`, docs/data/junk are demoted further.
pub(crate) fn test_path_weight(file: &str) -> f32 {
    crate::ranking::class_search_weight(crate::ranking::classify_file(file))
}

pub(crate) fn build_snippet(file: &str, symbols: &[String], comments: &str) -> String {
    let mut s = String::with_capacity(SNIPPET_CHAR_CAP + 32);
    // Path stem first — "src/auth/login.rs" → "auth login".
    let path_words: Vec<String> = path_components(file);
    s.push_str(&path_words.join(" "));
    s.push_str(" — ");
    // Up to 12 symbol names, comma-separated.
    let sym_take = symbols.iter().take(12).cloned().collect::<Vec<_>>();
    s.push_str(&sym_take.join(", "));
    // First non-empty comment lines, capped to fill the budget.
    let mut budget = SNIPPET_CHAR_CAP.saturating_sub(s.len());
    if budget > 0 {
        s.push_str(" — ");
        budget = budget.saturating_sub(3);
        for line in comments.lines() {
            let trim = line.trim();
            if trim.is_empty() {
                continue;
            }
            if budget == 0 {
                break;
            }
            // Find the largest char boundary that fits in budget — we can't
            // slice in the middle of a multi-byte char without panicking.
            let mut take = trim.len().min(budget);
            while take > 0 && !trim.is_char_boundary(take) {
                take -= 1;
            }
            s.push_str(&trim[..take]);
            s.push(' ');
            budget = budget.saturating_sub(take + 1);
        }
    }
    // Final safety cap (use byte boundary).
    if s.len() > SNIPPET_CHAR_CAP {
        let mut cut = SNIPPET_CHAR_CAP;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}
