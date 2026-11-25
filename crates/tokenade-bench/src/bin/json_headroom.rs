//! What `auto` still delivers on REAL JSON, and on which.
//!
//! The ranking says `auto:json` −96 % and 606,672 bytes still delivered over 250
//! calls. The average — 2,427 bytes per call — describes nothing: the
//! distribution is **bimodal**. MEASURED 2026-09-08 on the ledger: median
//! **36 bytes per call**, p90 **84**, and **two calls deliver 297,442 bytes
//! out of 342,006** (−13 %), i.e. 95 % of the row's residue. Almost all
//! calls are perfect; two are not at all, and they are the row.
//!
//! The bytes of those two calls are not recoverable — rows without a session,
//! from another session. So we look for the SAME SHAPE in real JSON we have
//! at hand: every `.json` in the repo, competitors included, run through the
//! path a `tokenade read` takes (`auto::compact_with_path_hint`).
//!
//! The report ranks by BYTES STILL DELIVERED, not by rate: a file that
//! returns −13 % on 300 KB weighs more than ten that return −40 % on 2 KB.
//!
//! `cargo run -q -p tokenade-bench --bin json_headroom [--limit N]`

use std::path::{Path, PathBuf};

/// Beyond this, it is no longer a document an agent reads: it is a build
/// artifact, and the read ceiling deals with it before the compactor.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

struct Row {
    path: String,
    /// The bare file name. A SHAPE is often recognised by its name
    /// (`package-lock.json`, `tsconfig.json`) and not its path, and a per-file
    /// report cannot say that a whole class is badly folded.
    name: String,
    before: usize,
    after: usize,
    /// The name the router gave this content.
    kind: String,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for ent in entries.flatten() {
        let p = ent.path();
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            // `target/` and `node_modules/` are artifacts, `.git` is not
            // text. The COMPETITORS stay: their JSON is real.
            if matches!(name.as_ref(), "target" | "node_modules" | ".git") {
                continue;
            }
            walk(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("json") {
            out.push(p);
        }
    }
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    tokenade_bench::reject_unknown_args(&["--limit", "--delivered"], &args);
    let limit: usize = args
        .iter()
        .position(|a| a == "--limit")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);

    // `--delivered` measures BOTH layers through the real CLI. Without it this
    // report ranks on `auto::compact_with_path_hint` alone, which named
    // `ta.json` at the top of the deposit at 1,145,860 bytes where a `tokenade read`
    // delivers 1,891 of it.
    let delivered = args.iter().any(|a| a == "--delivered");
    let bench = match delivered.then(tokenade_bench::Delivery::open).transpose() {
        Ok(b) => b,
        Err(note) => {
            eprintln!("{note}");
            std::process::exit(2);
        }
    };

    let root = repo_root();
    let mut paths = Vec::new();
    walk(&root, &mut paths);
    paths.sort();

    let mut rows: Vec<Row> = Vec::new();
    for p in &paths {
        let Ok(meta) = std::fs::metadata(p) else {
            continue;
        };
        if meta.len() == 0 || meta.len() > MAX_BYTES {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(p) else {
            continue;
        };
        let name = p
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (kind, folded, _) = tokenade_core::auto::compact_with_path_hint(&body, &name);
        rows.push(Row {
            path: p
                .strip_prefix(&root)
                .unwrap_or(p)
                .to_string_lossy()
                .into_owned(),
            name: name.clone(),
            before: body.len(),
            after: folded.len(),
            kind: kind.as_str().to_string(),
        });
    }
    if rows.is_empty() {
        eprintln!(
            "json_headroom: NO readable .json under {}.\n\
             \x20 The instrument measured nothing — this is not \"no headroom\".",
            root.display()
        );
        std::process::exit(2);
    }

    // The VERIFICATION: the layer-1 ranking is only worth something if its
    // top entries are still there after layer 2. Measuring the 16,391 files through the
    // real CLI is out of reach (the gate bypass only works in debug, where
    // a read costs ~10 s), so we check the TOP — it is what gets read
    // as a target.
    if let Some(d) = bench.as_ref() {
        let mut top = rows.iter().collect::<Vec<_>>();
        top.sort_by_key(|r| std::cmp::Reverse(r.after));
        top.truncate(limit);
        println!("{}", tokenade_bench::Stages::verification_header());
        let (mut s1, mut s2) = (0usize, 0usize);
        for r in &top {
            let full = repo_root().join(&r.path);
            match d.measure(&["read", &full.to_string_lossy()]) {
                Ok(out) => {
                    s1 += r.after;
                    s2 += out;
                    println!(
                        "  layer 1 {:>10}   delivered {:>10}   {}",
                        thousands(r.after),
                        thousands(out),
                        r.path
                    );
                }
                Err(note) => {
                    eprintln!("{note}");
                    std::process::exit(2);
                }
            }
        }
        println!(
            "  ── {} file(s): {} at layer 1, {} REALLY delivered\n",
            top.len(),
            thousands(s1),
            thousands(s2)
        );
    }

    let before: usize = rows.iter().map(|r| r.before).sum();
    let after: usize = rows.iter().map(|r| r.after).sum();
    // The TOTAL is that of ONE layer, `--delivered` or not: the flag only
    // re-measures the top entries. Titling it "delivery path" is
    // what this instrument used to do, above a total of 100,036,475
    // bytes when its first six rows delivered 11,287 — a factor of 956,
    // in the very instrument where an earlier overestimate of
    // 813× had been fixed. The header now comes from the measured value.
    print!(
        "{}",
        tokenade_bench::Measured {
            stages: tokenade_bench::Stages::CommandOnly,
            items: rows.len(),
            unit: ".json file(s)",
            before,
            after,
            via: "auto::compact_with_path_hint",
        }
        .header()
    );

    // Sorted by what remains DELIVERED, not by rate: a file that returns −13 % on
    // 300 KB weighs more than ten that return −40 % on 2 KB.
    rows.sort_by_key(|b| std::cmp::Reverse(b.after));
    println!("the heaviest AFTER folding:");
    for r in rows.iter().take(limit) {
        let pct = 100.0 * (r.before.saturating_sub(r.after)) as f64 / r.before.max(1) as f64;
        println!(
            "  {:>9} → {:>9}  (−{pct:>3.0} %)  {:<12} {}",
            thousands(r.before),
            thousands(r.after),
            r.kind,
            r.path
        );
    }

    // By NAME: a shape is recognised by its name, and a per-file report cannot
    // say that a whole class is badly folded.
    let mut by_name: std::collections::BTreeMap<&str, (usize, usize, usize)> = Default::default();
    for r in &rows {
        let e = by_name.entry(r.name.as_str()).or_default();
        e.0 += 1;
        e.1 += r.before;
        e.2 += r.after;
    }
    let mut named: Vec<_> = by_name.into_iter().collect();
    named.sort_by_key(|b| std::cmp::Reverse(b.1 .2));
    println!("\nby file NAME, what remains delivered:");
    for (name, (n, before, after)) in named.iter().take(limit) {
        println!(
            "  {:>9} → {:>9}  (−{:>3.0} %)  ×{n:<5} {name}",
            thousands(*before),
            thousands(*after),
            100.0 * (before.saturating_sub(*after)) as f64 / (*before).max(1) as f64
        );
    }

    // And the shape we are looking for: big AND barely folded.
    let stubborn: Vec<&Row> = rows
        .iter()
        .filter(|r| r.before > 100_000 && r.after * 4 > r.before * 3)
        .collect();
    println!(
        "\n{} file(s) over 100 KB returned at more than 75 % — the shape of the \
         ledger's two calls",
        stubborn.len()
    );
    for r in stubborn.iter().take(limit) {
        println!(
            "  {:>9} → {:>9}  {:<12} {}",
            thousands(r.before),
            thousands(r.after),
            r.kind,
            r.path
        );
    }
}
