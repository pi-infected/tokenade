//! Release-build cost of the `dom_dedup` pre-pass.
//!
//! Measured in-process rather than through the CLI, so the timing covers the
//! pre-pass alone.
//!
//! Ignored by default — it needs a corpus. Run it with:
//!   DOM_DEDUP_CORPUS=/tmp/corpusA cargo test --release -p tokenade-core \
//!     --test dom_dedup_timing -- --ignored --nocapture
use std::time::Instant;

#[test]
#[ignore = "needs a corpus directory in DOM_DEDUP_CORPUS"]
fn measures_pre_pass_cost() {
    let Ok(dirs) = std::env::var("DOM_DEDUP_CORPUS") else {
        return;
    };
    let mut pages: Vec<String> = Vec::new();
    for dir in dirs.split(':') {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "html") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    pages.push(text);
                }
            }
        }
    }
    assert!(!pages.is_empty(), "no pages found in {dirs}");
    let total_bytes: usize = pages.iter().map(|p| p.len()).sum();

    // Full pipeline with the pass disabled, then enabled. The env var is read
    // inside the pass, so flipping it here exercises the real code path.
    // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process per
    // test, so no other thread reads the environment while it changes.
    unsafe { std::env::set_var("TOKENADE_DOM_DEDUP", "0") };
    let start = Instant::now();
    for page in &pages {
        std::hint::black_box(tokenade_core::web::html_to_markdown(page));
    }
    let without = start.elapsed();

    // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process per
    // test, so no other thread reads the environment while it changes.
    unsafe { std::env::remove_var("TOKENADE_DOM_DEDUP") };
    let start = Instant::now();
    for page in &pages {
        std::hint::black_box(tokenade_core::web::html_to_markdown(page));
    }
    let with = start.elapsed();

    // Isolate the pass itself from the rest of the pipeline.
    let start = Instant::now();
    for page in &pages {
        std::hint::black_box(tokenade_core::dom_dedup::dedup_repeated_siblings(page));
    }
    let pass_only = start.elapsed();

    let n = pages.len() as f64;
    println!(
        "pages={} bytes={}\n  pipeline without dedup: {:>8.1?} ({:.1} ms/page)\n  \
         pipeline with dedup:    {:>8.1?} ({:.1} ms/page)\n  \
         dedup pass alone:       {:>8.1?} ({:.1} ms/page)\n  \
         overhead: {:+.1}%",
        pages.len(),
        total_bytes,
        without,
        without.as_secs_f64() * 1000.0 / n,
        with,
        with.as_secs_f64() * 1000.0 / n,
        pass_only,
        pass_only.as_secs_f64() * 1000.0 / n,
        100.0 * (with.as_secs_f64() - without.as_secs_f64()) / without.as_secs_f64(),
    );
}
