//! **Merging rankings that cannot be compared.**
//!
//! Two engines do not score on the same scale: adding their scores makes no
//! sense, merging their RANKS does. And the same result served by two engines
//! must count once — so the deduplication key is the normalised address, not
//! the title.

use super::*;

/// Reciprocal Rank Fusion (searxng-rust, MIT). `score = Σ 1/(60 + (rank+1))`,
/// unweighted, 1-indexed. Pathology: no minimum-engine filter — a single-engine
/// #1 (0.0164) loses to a two-engine pair at ranks 8+8 (0.0294).
/// **DECLARED RESERVE**: one of the three comparison formulas. The product
/// applies a different one; these stay because their value lies in being
/// COMPARABLE, and a removed formula can no longer be compared.
#[allow(dead_code)]
pub(crate) fn rank_rrf(per_engine: &[Vec<serp::Result>]) -> Vec<serp::Result> {
    let mut acc: std::collections::HashMap<String, (f64, usize, serp::Result)> =
        std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for hits in per_engine {
        for (rank, hit) in hits.iter().enumerate() {
            let key = canonical_url(&hit.url);
            let seq = order.len();
            let e = acc.entry(key.clone()).or_insert_with(|| {
                order.push(key.clone());
                (0.0, seq, hit.clone())
            });
            e.0 += 1.0 / (60.0 + (rank + 1) as f64);
        }
    }
    let mut scored: Vec<_> = order.iter().filter_map(|k| acc.remove(k)).collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    scored
        .into_iter()
        .map(|(_, _, r)| r)
        .take(MAX_HITS)
        .collect()
}

/// SearXNG (AGPL — formula is a fact). `weight = Π engine_weights × engine_count`
/// (our engine weights are all 1, so `weight = engine_count`), then
/// `score = Σ_positions weight/position`. Agreement is quadratic, no `k` damping.
/// **DECLARED RESERVE**: see [`rank_rrf`].
#[allow(dead_code)]
pub(crate) fn rank_searxng(per_engine: &[Vec<serp::Result>]) -> Vec<serp::Result> {
    let mut acc: std::collections::HashMap<String, (usize, f64, usize, serp::Result)> =
        std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for hits in per_engine {
        for (rank, hit) in hits.iter().enumerate() {
            let key = canonical_url(&hit.url);
            let seq = order.len();
            let e = acc.entry(key.clone()).or_insert_with(|| {
                order.push(key.clone());
                (0, 0.0, seq, hit.clone())
            });
            e.0 += 1;
            e.1 += 1.0 / (rank + 1) as f64;
        }
    }
    let mut scored: Vec<_> = order.iter().filter_map(|k| acc.remove(k)).collect();
    scored.sort_by(|a, b| {
        let sa = a.0 as f64 * a.1;
        let sb = b.0 as f64 * b.1;
        sb.partial_cmp(&sa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.2.cmp(&b.2))
    });
    scored
        .into_iter()
        .map(|(_, _, _, r)| r)
        .take(MAX_HITS)
        .collect()
}

/// ddgs (MIT). `100·agreement + 50·(host has wikipedia.org) + 20·(query token in
/// title) + 10·(query token in body)`.
/// **DECLARED RESERVE**: see [`rank_rrf`].
#[allow(dead_code)]
pub(crate) fn rank_ddgs(per_engine: &[Vec<serp::Result>], query: &str) -> Vec<serp::Result> {
    let needles = needles_for(query);
    let mut acc: std::collections::HashMap<String, (usize, usize, serp::Result)> =
        std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for hits in per_engine {
        for hit in hits {
            let key = canonical_url(&hit.url);
            let seq = order.len();
            let e = acc.entry(key.clone()).or_insert_with(|| {
                order.push(key.clone());
                (0, seq, hit.clone())
            });
            e.0 += 1;
        }
    }
    let mut scored: Vec<_> = order.iter().filter_map(|k| acc.remove(k)).collect();
    scored.sort_by(|a, b| {
        let score = |agree: usize, r: &serp::Result| -> i64 {
            let title = r.title.to_lowercase();
            let body = format!("{} {}", r.snippet, r.url).to_lowercase();
            100 * agree as i64
                + if r.url.contains("wikipedia.org") {
                    50
                } else {
                    0
                }
                + if needles.iter().any(|n| title.contains(n.as_str())) {
                    20
                } else {
                    0
                }
                + if needles.iter().any(|n| body.contains(n.as_str())) {
                    10
                } else {
                    0
                }
        };
        score(b.0, &b.2).cmp(&score(a.0, &a.2)).then(a.1.cmp(&b.1))
    });
    scored
        .into_iter()
        .map(|(_, _, r)| r)
        .take(MAX_HITS)
        .collect()
}

pub(super) fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
        .trim_start_matches("www.")
        .to_ascii_lowercase()
}
