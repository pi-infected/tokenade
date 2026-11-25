//! A production file has a readable size, and the list of exceptions
//! shrinks.
//!
//! The rule of this work is: production ≤ ~800 lines per file. It is not
//! held — and a rule no test measures is only an intention. This test does
//! for SIZE what `layers_only_point_downwards` does for edges: it counts, it
//! writes the debt one line per file, and it refuses that it grows.
//!
//! What is counted is the half that SHIPS, through `srcsweep`, not the
//! file's line count. Moving 4,000 lines of `#[cfg(test)]` to a sibling file
//! does not make a module simpler; this test must therefore not reward that
//! move. It counts what the compiler delivers.
//!
//! The sibling file `x_tests.rs` is not counted at all: it only contains
//! tests, its production half is empty, and it does not have to hold the
//! bound.
//!
//! **Nor a GENERATED file.** `cmd/spec_table.rs` carries 3,368 lines and it
//! is the biggest entry of the list — but it is a TABLE, one line per
//! compactor, written by a generator that says at the top "Do not edit by
//! hand" and explains why it is on one line: rustfmt would make it 33,874
//! lines that the next regeneration silently undoes. Splitting it makes no
//! sense, and leaving it in the debt suggested there were 91 god files to
//! split where there are 90. A generated file is recognised by its own
//! header, not by a list written here.

use std::path::{Path, PathBuf};

/// A file a generator writes SAYS so in its first lines. Only those are
/// read: a `// GENERATED` in the middle of a file is about something else.
fn is_generated(body: &str) -> bool {
    let head: String = body
        .lines()
        .take(6)
        .collect::<Vec<_>>()
        .join("\n")
        .to_uppercase();
    head.contains("GENERATED") || head.contains("DO NOT EDIT")
}

/// Beyond this, a file is declared or the test fails.
const LIMIT: usize = 800;

/// The files that exceed it, with their measured number of PRODUCTION lines.
/// A file absent from here that exceeds fails the test; a file from here
/// that has GROWN too; a file from here back under the bound must be
/// removed. It is a ratchet, not a list of exceptions.
/// MEASURED, end of day 2026-09-13: **ONE file**, **807 production lines**,
/// seven above the bound — and it is a TABLE, one line per recognised
/// language.
///
/// Splitting it would bring nothing: it changes for a single reason — one
/// more language — and a boundary drawn through its middle would have one
/// look for a language in two files. That is the argument of
/// `cmd/spec_table.rs`, exempted because it is GENERATED; this one is
/// hand-written, so it declares itself here, where it is seen.
///
/// The path: **184 files / 265,542 lines** in the morning, **55 / 57,399**
/// on resuming, **1 / 807** tonight. The first gap was not a split — it was
/// moving `#[cfg(test)]` out into sibling files, which dropped half the list
/// without a single module becoming simpler. That is what this test refuses
/// to reward, and that is why it counts the half that SHIPS. The second gap
/// is splitting: about two hundred child files, each named after the
/// question it answers.
///
/// **The bound is now held by all the rest of the repo.** What gets added
/// here will be a regression, not a legacy.
const OVERSIZED: &[(&str, usize)] = &[
    // 2.0.0: the one-off `cargo fmt --all` pass pushed these over 800.
    ("tokenade-core/src/command_rewrite/segment.rs", 861),
    ("tokenade-cli/src/hooks/session_lifecycle/credit.rs", 829),
    ("tokenade-cli/src/llm_proxy/server.rs", 821),
    ("tokenade-cli/src/llm_proxy/wire.rs", 811),
    ("tokenade-core/src/docs/rtf.rs", 810),
    // Declared at the 1.2.0 ship, without splitting on the eve of a release:
    // gain accounting (exact replay, sub-agents, proxy) grew `record.rs` and
    // `scan.rs`; routing gained Dart and worktrunk; `detectors.rs` and
    // `auto_cmd.rs` crossed 800 in small touches. To be split first in the
    // next cycle.
    ("tokenade-core/src/gain/record.rs", 831),
    ("tokenade-core/src/gain/scan.rs", 846),
    ("tokenade-core/src/filter/dispatch.rs", 829),
    ("tokenade-core/src/auto/detectors.rs", 823),
    ("tokenade-cli/src/compactors/auto_cmd.rs", 814),
    // The supervisor gained a service name PER PORT (one agent = one proxy =
    // one unit), its migration and the census of all units. The subject
    // stays one: "how this operating system starts the proxy".
    ("tokenade-core/src/index/lang_specs/specs.rs", 806),
];

fn crates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf()
}

/// The production `.rs` of every crate of the workspace.
///
/// The list is DERIVED from the `crates/` directory, not written by hand. Two
/// crates were extracted on 2026-09-13 — `tk-base` in the morning, `tk-fold`
/// in the afternoon — and a list of names would have missed the second
/// silently: a ratchet that does not walk a root does not complain about it,
/// it merely stays green.
fn production_files() -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut krates: Vec<String> = std::fs::read_dir(crates_root())
        .expect("crates/")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("src").is_dir())
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    krates.sort();
    assert!(
        krates.len() >= 4,
        "only {} crate(s) found: the ratchet no longer looks at anything",
        krates.len()
    );
    for krate in krates {
        let root = crates_root().join(&krate).join("src");
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                    continue;
                }
                let name = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
                // A tests sibling file carries no production.
                if name.ends_with("_tests.rs") {
                    continue;
                }
                // `src/tests/` is only mounted under `#[cfg(test)]`, from the
                // crate root: the `#[cfg(test)]` is on the `mod`, not in the
                // file, so `srcsweep` sees production there. They are tests.
                if p.components().any(|c| c.as_os_str() == "tests") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&p) else {
                    continue;
                };
                // A table written by a generator is not a god file: see this
                // module's header.
                if is_generated(&text) {
                    continue;
                }
                let n = tokenade_core::srcsweep::production_lines(&text).len();
                let rel = p
                    .strip_prefix(crates_root())
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, n));
            }
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

#[test]
fn every_file_over_the_limit_is_declared_with_its_size() {
    let declared: std::collections::BTreeMap<&str, usize> =
        OVERSIZED.iter().map(|(f, n)| (*f, *n)).collect();
    let measured = production_files();

    let mut undeclared = Vec::new();
    let mut grown = Vec::new();
    for (f, n) in &measured {
        if *n <= LIMIT {
            continue;
        }
        match declared.get(f.as_str()) {
            None => undeclared.push(format!("    (\"{f}\", {n}),")),
            Some(d) if n > d => grown.push(format!("`{f}`: declared {d}, measured {n}")),
            _ => {}
        }
    }

    assert!(
        undeclared.is_empty(),
        "{} production file(s) exceed {LIMIT} lines without being declared.\n\
         Either the file is split, or the line is added to `OVERSIZED` — but \
         size does not pass silently:\n{}",
        undeclared.len(),
        undeclared.join("\n")
    );
    assert!(
        grown.is_empty(),
        "{} declared file(s) have GROWN. The ratchet does not go back up:\n  {}",
        grown.len(),
        grown.join("\n  ")
    );
}

#[test]
fn the_declared_list_is_counted_and_shrinking() {
    let measured: std::collections::BTreeMap<String, usize> =
        production_files().into_iter().collect();

    // A path written TWICE inflates the debt counter without any file
    // growing: the list really carried 183 lines for 92 files, and nothing
    // said so. The duplicate is read here, not in a 184-line diff.
    let mut seen = std::collections::BTreeSet::new();
    let duplicates: Vec<&str> = OVERSIZED
        .iter()
        .filter(|(f, _)| !seen.insert(*f))
        .map(|(f, _)| *f)
        .collect();
    assert!(
        duplicates.is_empty(),
        "{} path(s) declared twice — the debt counter lies:\n  {}",
        duplicates.len(),
        duplicates.join("\n  ")
    );

    let mut gone = Vec::new();
    for (f, d) in OVERSIZED {
        match measured.get(*f) {
            None => gone.push(format!("`{f}` no longer exists")),
            Some(n) if *n <= LIMIT => gone.push(format!("`{f}` went back down to {n} ≤ {LIMIT}")),
            Some(n) if n < d => gone.push(format!("`{f}`: declared {d}, measured {n} — paid")),
            _ => {}
        }
    }
    assert!(
        gone.is_empty(),
        "The declared debt is stale. A paid debt that stays written suggests \
         it is due; update these lines:\n  {}",
        gone.join("\n  ")
    );
}
