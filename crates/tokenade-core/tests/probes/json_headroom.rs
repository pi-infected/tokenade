//! What a JSON payload still costs after compaction, priced in tokens.
//!
//! Production says this family compacts by ~70 % while a web page compacts by
//! ~96 %, and JSON is the third-largest thing agents are handed. The corpus is
//! whatever real JSON is on the machine — lockfiles, API captures, JSON Schema,
//! i18n bundles — because inventing a payload is how a compactor gets tuned to a
//! shape it never meets.
//!
//! `#[ignore]`d — a measurement to decide from, not a gate.
//!
//! `SURVEY_ROOTS=/path cargo test -p tokenade-core --test json_headroom -- --ignored --nocapture`

use tokenade_core::tokenizer::estimate;

fn corpus() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
        if depth > 8 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            let n = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if n.starts_with('.') || n == "target" || n == "node_modules" {
                continue;
            }
            if p.is_dir() {
                walk(&p, out, depth + 1);
            } else if n.ends_with(".json") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    for root in std::env::var("SURVEY_ROOTS").unwrap_or_default().split(':') {
        if !root.is_empty() {
            walk(std::path::Path::new(root), &mut files, 0);
        }
    }
    files.sort();
    files
        .into_iter()
        .filter_map(|p| {
            let t = std::fs::read_to_string(&p).ok()?;
            (t.len() > 8_000 && t.len() < 4_000_000).then(|| (p.to_string_lossy().to_string(), t))
        })
        .collect()
}

#[test]
#[ignore = "measurement: what a JSON payload still costs"]
fn what_a_json_payload_still_costs() {
    let files = corpus();
    assert!(!files.is_empty(), "set SURVEY_ROOTS");

    let (mut raw, mut delivered) = (0usize, 0usize);
    // Where the delivered tokens sit, so a candidate has something to aim at.
    let (mut worst, mut n_pass) = (Vec::new(), 0usize);
    for (name, text) in &files {
        let (_kind, out, _sav) = tokenade_core::auto::compact_with_path_hint(text, name);
        let (b, a) = (estimate(text), estimate(&out));
        raw += b;
        delivered += a;
        let pct = 100.0 * (b as f64 - a as f64) / b as f64;
        if pct < 1.0 {
            n_pass += 1;
        }
        worst.push((pct, b - a.min(b), name.clone(), b));
    }
    println!(
        "{} files: {raw} -> {delivered} tokens ({:.1}% saved), {n_pass} passed through untouched",
        files.len(),
        100.0 * (raw - delivered) as f64 / raw as f64
    );
    // The files that cost the most AFTER compaction are where any further work
    // has to pay off; ranking by percentage would surface tiny files.
    worst.sort_by_key(|b| std::cmp::Reverse(b.3));
    println!("--- files where compaction does nothing (<5%), largest first ---");
    let mut inert: Vec<&(f64, usize, String, usize)> = worst.iter().filter(|w| w.0 < 5.0).collect();
    inert.sort_by_key(|b| std::cmp::Reverse(b.3));
    let inert_tokens: usize = inert.iter().map(|w| w.3).sum();
    println!(
        "  {} files, {inert_tokens} tokens ({:.1}% of the corpus)",
        inert.len(),
        100.0 * inert_tokens as f64 / raw as f64
    );
    for (pct, _s, name, before) in inert.iter().take(12) {
        let short: String = name
            .chars()
            .rev()
            .take(58)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        println!("  {short:<58} {before:>8} tok, saved {pct:>5.1}%");
    }
    println!("largest inputs and what they cost after compaction:");
    for (pct, _saved, name, before) in worst.iter().take(15) {
        let short: String = name
            .chars()
            .rev()
            .take(58)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        println!("  {short:<58} {before:>8} tok, saved {pct:>5.1}%");
    }
}
