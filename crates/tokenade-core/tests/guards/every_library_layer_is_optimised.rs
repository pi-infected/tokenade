//! **A library layer compiles at `opt-level = 3`, not at "z".**
//!
//! This repo's `[profile.release]` optimises for SIZE (`opt-level = "z"`) and
//! raises back to 3, one by one, the crates where the time is spent. The
//! table naming them is hand-written, and it named `tokenade-core` alone —
//! that was true as long as everything lived there.
//!
//! The split carried the hot code into `tk-base`, `tk-fold`, `tk-formats` and
//! `tk-agents`, and the table stayed on the old address: the four layers fell
//! back to the size setting without a test saying so. MEASURED on 2026-09-14
//! on `per_command` (379 fixtures): **58 ms without the four lines, 55 ms
//! with**.
//!
//! The rule is therefore derived from the DIRECTORY, like `crate_srcs`: every
//! `tk-*` crate — plus the `tokenade-core` facade — must have its line. A new
//! layer arrives with its setting, or this guard turns red.

use tk_base::source_guard::crates_dir;

/// The crates whose setting is decided elsewhere, with the reason.
const NOT_A_LAYER: &[(&str, &str)] = &[
    (
        "tokenade-cli",
        "the binary: it links, it carries no hot loop",
    ),
    (
        "tokenade-bench",
        "the measurement harness must not be faster than what it measures",
    ),
];

fn workspace_manifest() -> String {
    let root = crates_dir().parent().expect("workspace root").to_path_buf();
    std::fs::read_to_string(root.join("Cargo.toml")).expect("root Cargo.toml")
}

#[test]
fn every_library_layer_is_optimised() {
    let manifest = workspace_manifest();
    let mut layers: Vec<String> = std::fs::read_dir(crates_dir())
        .expect("crates/")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("tk-") || n == "tokenade-core")
        .collect();
    layers.sort();
    // FLOOR: a guard reading nothing is green, and that is the worst of
    // verdicts. Five layers exist the day this is written.
    assert!(
        layers.len() >= 5,
        "only {} layer(s) found under {}: the guard no longer reads anything",
        layers.len(),
        crates_dir().display()
    );
    let mut unset = Vec::new();
    for l in &layers {
        let head = format!("[profile.release.package.{l}]");
        let ok = manifest
            .split(&head)
            .nth(1)
            .is_some_and(|rest| rest.trim_start().starts_with("opt-level = 3"));
        if !ok {
            unset.push(l.clone());
        }
    }
    assert!(
        unset.is_empty(),
        "these library layers compile at the SIZE setting, \
         because the `Cargo.toml` table does not name them:\n    {}\n\
         Add `[profile.release.package.<crate>]` + `opt-level = 3`.",
        unset.join("\n    ")
    );
    // And the reverse: an entry matching no crate is a dead address —
    // exactly the defect this guard repairs.
    for (name, _) in NOT_A_LAYER {
        assert!(
            !layers.iter().any(|l| l == name),
            "`{name}` is listed as \"not a layer\" although it is one"
        );
    }
}
