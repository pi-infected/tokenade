//! **What code search finds, measured on real labels.**
//!
//! The only recall figure this repo had was on queries by
//! EXACT symbol NAME — the easy case, the one BM25 always wins. Every
//! setting (alpha, boosts, embedding model) was therefore a guess.
//!
//! The labels are free and real: **a commit's message is a
//! query, the files it touches are the expected answers**. A commit
//! says what one was trying to do in the words of intent, not in those
//! of the code — exactly the query an agent asks.
//!
//! ```text
//! rag_eval [--repo <path>] [--commits N] [--alpha A] [--mode bm25|hybrid] [--k K]
//! ```
//!
//! What is excluded, and why: merges (their message describes nothing),
//! commits touching more than `MAX_FILES` files (the query cannot
//! designate twenty files), those whose message is shorter than three words,
//! and any file that no longer exists today — the index does not contain it,
//! so requiring it would measure the repo's history, not search.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::PathBuf;
use tokenade_core::{index, semantic};

mod engines;
mod labels;
mod routes;

pub(crate) use engines::*;
pub(crate) use labels::*;
pub(crate) use routes::*;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |name: &str, def: &str| -> String {
        args.windows(2)
            .find(|w| w[0] == format!("--{name}"))
            .map(|w| w[1].clone())
            .unwrap_or_else(|| def.to_string())
    };
    let repo = PathBuf::from(get("repo", "."));
    let n: usize = get("commits", "800").parse().unwrap_or(800);
    let k: usize = get("k", "10").parse().unwrap_or(10);
    let alpha: f32 = get("alpha", "0.5").parse().unwrap_or(0.5);
    let mode = get("mode", "both");

    if let Some(i) = args.iter().position(|a| a == "--web") {
        let list = PathBuf::from(args.get(i + 1).cloned().unwrap_or_default());
        let cache = PathBuf::from(get("cache", "/tmp/webcache"));
        let budget: usize = get("budget", "2000").parse().unwrap_or(2000);
        web_eval(&list, &cache, budget);
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--stash") {
        let list = PathBuf::from(args.get(i + 1).cloned().unwrap_or_default());
        stash_eval(&list, 0);
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--real") {
        let list = PathBuf::from(args.get(i + 1).cloned().unwrap_or_default());
        let budget: usize = get("budget", "2000").parse().unwrap_or(2000);
        real_eval(&list, &repo, budget);
        return;
    }
    if args.iter().any(|a| a == "--docrag") {
        let pages = repo.join("crates/tokenade-core/tests/fixtures/pages");
        let budget: usize = get("budget", "2000").parse().unwrap_or(2000);
        doc_rag_eval(&pages, budget);
        return;
    }
    let idx = match index::Index::ensure_fresh(&repo) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("index: {e}");
            std::process::exit(1);
        }
    };
    let sem = semantic::SemanticIndex::build_from_index(&idx, &repo);
    let indexed: BTreeSet<String> = sem.docs.iter().map(|d| d.file.clone()).collect();
    if args.iter().any(|a| a == "--dump") {
        for d in sem.docs.iter().step_by(sem.docs.len().max(1) / 6).take(6) {
            println!("── {} ({} chars)\n{}", d.file, d.snippet.len(), d.snippet);
        }
        return;
    }
    let family = get("family", "commit");
    // **Remove a bias of the bench.** The "doc" families take a file's header
    // sentence, and many of those files are TESTS —
    // which `path_weight` ranks down on purpose, because a human looking for "the
    // language detection" wants `utils.rs`, not `utils_tests.rs`. Measuring the
    // weighting on a target it is designed to push aside is measuring
    // the bench. `--prod-only` keeps only the cases whose answers are ALL
    // production code.
    let prod_only = args.iter().any(|a| a == "--prod-only");
    let is_test_path = |f: &str| {
        f.contains("/tests/")
            || f.contains("test_")
            || f.ends_with("_tests.rs")
            || f.ends_with("_test.go")
            || f.contains("/test/")
    };
    let mut cases = match family.as_str() {
        "doc" => doc_cases(&repo, &sem, false),
        "hard" => doc_cases(&repo, &sem, true),
        _ => cases(&repo, n, &indexed),
    };
    if prod_only {
        cases.retain(|c| !c.gold.iter().any(|f| is_test_path(f)));
    }
    if cases.is_empty() {
        eprintln!("no usable (message, files) pair in the last {n} commits");
        std::process::exit(1);
    }
    println!(
        "corpus: {} indexed files · family {family} · {} queries · k={k}",
        indexed.len(),
        cases.len()
    );

    // The two chunking levels, with THE SAME BM25.
    let needs_levels = matches!(mode.as_str(), "level" | "levelchunk" | "niveaux");
    let (file_bm, chunk_bm) = if needs_levels {
        let file_texts: Vec<String> = sem
            .docs
            .iter()
            .map(|d| std::fs::read_to_string(repo.join(&d.file)).unwrap_or_default())
            .collect();
        let f = Bm25::build(&file_texts, (0..sem.docs.len()).collect());
        let (ctexts, cowner) = chunks_of(&repo, &sem);
        let c = Bm25::build(&ctexts, cowner);
        println!(
            "  levels: {} files, {} fragments",
            file_texts.len(),
            c.len.len()
        );
        (Some(f), Some(c))
    } else {
        (None, None)
    };
    let needs_chunks = matches!(mode.as_str(), "chunk" | "rrfchunk" | "all");
    let (chunk_vecs, chunk_owner) = if needs_chunks {
        let t = std::time::Instant::now();
        let (texts, owner) = chunks_of(&repo, &sem);
        let v = tokenade_core::embedding::embed_batch(&texts).unwrap_or_default();
        println!(
            "  windows: {} chunks embedded in {:?}",
            v.len(),
            t.elapsed()
        );
        (v, owner)
    } else {
        (Vec::new(), Vec::new())
    };
    let needs_dense = matches!(mode.as_str(), "dense" | "rrf" | "all");
    let (vecs, _) = if needs_dense {
        let t = std::time::Instant::now();
        let v = doc_vectors(&sem);
        println!(
            "  vectors: {} documents embedded in {:?}",
            v.len(),
            t.elapsed()
        );
        (v, ())
    } else {
        (Vec::new(), ())
    };
    let modes: Vec<&str> = match mode.as_str() {
        "both" => vec!["bm25", "hybrid"],
        "all" => vec![
            "bm25", "bm25pw", "hybrid", "dense", "rrf", "chunk", "rrfchunk",
        ],
        "idees" => vec!["bm25pw", "hybrid", "expand", "expandhy"],
        "niveaux" => vec!["level", "levelchunk"],
        "fr" => vec!["bm25pw", "frstem"],
        m => vec![m],
    };
    for m in modes {
        let (mut recall, mut ndcg, mut mrr, mut done) = (0.0, 0.0, 0.0, 0usize);
        let (mut h_recall, mut h_ndcg, mut h_done) = (0.0, 0.0, 0usize);
        for c in &cases {
            let ranked: Vec<String> = match m {
                "bm25" => sem
                    .search(&c.query, k)
                    .into_iter()
                    .map(|h| h.file)
                    .collect(),
                // Expansion + the shipped path weighting: the honest
                // comparison is against `bm25pw`, not against bare `bm25`.
                "frstem" => {
                    let q = fr_variants(&c.query);
                    let mut hits = sem.search(&q, k * 4);
                    for h in &mut hits {
                        h.score *= tokenade_core::semantic::search::path_weight(&h.file) as f64;
                    }
                    hits.sort_by(|a, b| {
                        b.score
                            .partial_cmp(&a.score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then(a.file.cmp(&b.file))
                    });
                    hits.into_iter().take(k).map(|h| h.file).collect()
                }
                "expand" | "expandhy" => {
                    let q = expand_query(&sem, &c.query, 2);
                    if m == "expand" {
                        let mut hits = sem.search(&q, k * 4);
                        for h in &mut hits {
                            h.score *= tokenade_core::semantic::search::path_weight(&h.file) as f64;
                        }
                        hits.sort_by(|a, b| {
                            b.score
                                .partial_cmp(&a.score)
                                .unwrap_or(std::cmp::Ordering::Equal)
                                .then(a.file.cmp(&b.file))
                        });
                        hits.into_iter().take(k).map(|h| h.file).collect()
                    } else {
                        match sem.search_hybrid(&q, k, alpha) {
                            Ok(h) => h.into_iter().map(|h| h.file).collect(),
                            Err(_) => Vec::new(),
                        }
                    }
                }
                // BM25 + the SHIPPED path weighting (tests ×0.5, entry
                // points ×1.15): today it applies to every
                // hybrid ranking, and nobody had measured it alone.
                "bm25pw" => {
                    let mut hits = sem.search(&c.query, k * 4);
                    for h in &mut hits {
                        h.score *= tokenade_core::semantic::search::path_weight(&h.file) as f64;
                    }
                    hits.sort_by(|a, b| {
                        b.score
                            .partial_cmp(&a.score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then(a.file.cmp(&b.file))
                    });
                    hits.into_iter().take(k).map(|h| h.file).collect()
                }
                "dense" | "rrf" => {
                    let q = match tokenade_core::embedding::embed_text(&c.query) {
                        Ok(q) => q,
                        Err(e) => {
                            eprintln!("embedding unavailable: {e:?}");
                            std::process::exit(1);
                        }
                    };
                    let d = dense_top(&q, &vecs, &sem, k * 4);
                    if m == "dense" {
                        d.into_iter().take(k).collect()
                    } else {
                        let b: Vec<String> = sem
                            .search(&c.query, k * 4)
                            .into_iter()
                            .map(|h| h.file)
                            .collect();
                        rrf(&[b, d], k)
                    }
                }
                "level" | "levelchunk" => {
                    let b = if m == "level" {
                        file_bm.as_ref()
                    } else {
                        chunk_bm.as_ref()
                    };
                    match b {
                        Some(b) => b
                            .search(&c.query, sem.docs.len(), k)
                            .into_iter()
                            .map(|i| sem.docs[i].file.clone())
                            .collect(),
                        None => Vec::new(),
                    }
                }
                "chunk" | "rrfchunk" => {
                    let q = tokenade_core::embedding::embed_text(&c.query).unwrap_or_default();
                    // A file is worth its BEST window: it is what
                    // a chunk engine does when it returns files.
                    let mut best: HashMap<usize, f32> = HashMap::new();
                    for (j, v) in chunk_vecs.iter().enumerate() {
                        let s = tokenade_core::embedding::cosine(&q, v);
                        let e = best.entry(chunk_owner[j]).or_insert(f32::MIN);
                        if s > *e {
                            *e = s;
                        }
                    }
                    let mut v: Vec<(usize, f32)> = best.into_iter().collect();
                    v.sort_by(|a, b| {
                        b.1.partial_cmp(&a.1)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then(a.0.cmp(&b.0))
                    });
                    let d: Vec<String> = v
                        .iter()
                        .take(k * 4)
                        .map(|(i, _)| sem.docs[*i].file.clone())
                        .collect();
                    if m == "chunk" {
                        d.into_iter().take(k).collect()
                    } else {
                        let b: Vec<String> = sem
                            .search(&c.query, k * 4)
                            .into_iter()
                            .map(|h| h.file)
                            .collect();
                        rrf(&[b, d], k)
                    }
                }
                _ => match sem.search_hybrid(&c.query, k, alpha) {
                    Ok(h) => h.into_iter().map(|h| h.file).collect(),
                    Err(e) => {
                        eprintln!("hybrid unavailable: {e:?}");
                        std::process::exit(1);
                    }
                },
            };
            let rels: Vec<f64> = ranked
                .iter()
                .take(k)
                .map(|f| if c.gold.contains(f) { 1.0 } else { 0.0 })
                .collect();
            let found = rels.iter().filter(|r| **r > 0.0).count();
            recall += found as f64 / c.gold.len() as f64;
            let ideal: Vec<f64> = std::iter::repeat_n(1.0, c.gold.len().min(k)).collect();
            ndcg += if ideal.is_empty() {
                0.0
            } else {
                dcg(&rels) / dcg(&ideal)
            };
            mrr += rels
                .iter()
                .position(|r| *r > 0.0)
                .map(|i| 1.0 / (i + 1) as f64)
                .unwrap_or(0.0);
            let this_ndcg = if ideal.is_empty() {
                0.0
            } else {
                dcg(&rels) / dcg(&ideal)
            };
            if c.hard {
                h_recall += found as f64 / c.gold.len() as f64;
                h_ndcg += this_ndcg;
                h_done += 1;
            }
            done += 1;
        }
        let d = done as f64;
        let hd = h_done.max(1) as f64;
        println!(
            "{m:<7} alpha={alpha:<4} Recall@{k} {:.4}  nDCG@{k} {:.4}  MRR {:.4}   │ hard ({h_done}) Recall {:.4}  nDCG {:.4}",
            recall / d,
            ndcg / d,
            mrr / d,
            h_recall / hd,
            h_ndcg / hd
        );
    }
}
