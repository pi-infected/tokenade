//! **A variable a message says to set must be a variable something READS.**
//!
//! The twin of [`super::advertised_flags_exist`], for the product's 142
//! `TOKENADE_*` variables. The rule is the same and it does not need to know
//! the grammar of accessors: a REAL variable is spelled at least twice — once
//! alone, where it is passed to a reader (`env::var`,
//! `env_compat::is_truthy_one`, `hp`, …), and once in the sentence that
//! announces it. A variable that NEVER appears alone in a string can only
//! have been announced.
//!
//! What this catches, and nothing else catches: a banner saying "set
//! `TOKENADE_NO_FOO=1` to disable this" while the code reads
//! `TOKENADE_NO_FOOS`. The message is right, the product does not respond,
//! and the agent following the instruction loses its turn — exactly the
//! shape of the `--no-fold` the flags guard found.
//!
//! MEASURED on 2026-09-13: **180 names appear in a production string, and
//! only three are never spelled alone** — the three are declared below, and
//! none is a defect.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The names no RUST code reads, with what reads them instead. An entry here
/// is a verifiable claim, not an exemption.
const READ_ELSEWHERE: &[(&str, &str)] = &[
    (
        "TOKENADE_RICHNESS_BLESS",
        "read by the richness ratchet, `tests/corpus/richness_corpus.rs`; \
         announced by the \"file names\" half of that ratchet, mounted in \
         `--lib` (`corpus_key.rs`)",
    ),
    (
        "TOKENADE_BIN",
        "read by the installed shell shims (`@@TOKENADE_BIN@@` is replaced \
         there at install time); it is the script that consults the variable, \
         not the binary",
    ),
    (
        "TOKENADE_TARGET",
        "read by the PowerShell install script embedded in `cmake.rs` \
         (`$env:TOKENADE_TARGET`)",
    ),
];

fn crate_srcs() -> Vec<PathBuf> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf();
    [
        "tk-base",
        "tk-fold",
        "tokenade-core",
        "tokenade-cli",
        "tokenade-bench",
    ]
    .iter()
    .map(|c| crates.join(c).join("src"))
    .filter(|p| p.is_dir())
    .collect()
}

/// The string literals of a line of code — comments excluded, and the text
/// returned RAW (escapes not undone).
///
/// Raw, because that is what tells a variable from a SENTINEL: `web_fetch`
/// carries `"\x1fTOKENADE_META\x1f"`, an internal end-of-stream marker. An
/// environment variable name cannot travel inside a control character — no
/// shell could set it. Undoing escapes first made two sentinels pass for two
/// dead variables.
fn literals(line: &str) -> Vec<String> {
    if line.trim_start().starts_with("//") {
        return Vec::new();
    }
    let b: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '"' {
            let mut s = String::new();
            i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' && i + 1 < b.len() {
                    s.push(b[i]);
                    i += 1;
                }
                s.push(b[i]);
                i += 1;
            }
            out.push(s);
        }
        i += 1;
    }
    out
}

#[test]
fn every_advertised_env_var_is_read_somewhere() {
    let mut alone: BTreeMap<String, bool> = BTreeMap::new();
    let mut first_site: BTreeMap<String, String> = BTreeMap::new();
    let mut files = 0usize;

    let mut stack: Vec<PathBuf> = crate_srcs();
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
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if !name.ends_with(".rs") || name.ends_with("_tests.rs") {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&p) else {
                continue;
            };
            files += 1;
            for (n, line) in src.lines().enumerate() {
                for lit in literals(line) {
                    // A sentinel, not a variable: see `literals`.
                    if lit.contains("\\x") || lit.contains("\\u") {
                        continue;
                    }
                    for word in lit
                        .split(|c: char| {
                            !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                        })
                        .filter(|w| w.starts_with("TOKENADE_") && w.len() > "TOKENADE_".len())
                    {
                        let e = alone.entry(word.to_string()).or_insert(false);
                        *e |= lit == word;
                        first_site
                            .entry(word.to_string())
                            .or_insert_with(|| format!("{}:{}", p.display(), n + 1));
                    }
                }
            }
        }
    }

    assert!(
        files >= 900,
        "only {files} file(s) read: the guard no longer looks at anything"
    );
    assert!(
        alone.len() >= 100,
        "only {} variable(s) found: the guard no longer looks at anything",
        alone.len()
    );

    let faults: Vec<String> = alone
        .iter()
        .filter(|(_, read)| !**read)
        .map(|(k, _)| k.clone())
        .filter(|k| !READ_ELSEWHERE.iter().any(|(n, _)| n == k))
        .map(|k| format!("  {k} — announced at {}", first_site[&k]))
        .collect();
    assert!(
        faults.is_empty(),
        "{} announced variable(s) that no code reads. Either the reader is \
         missing, or the name is misspelled on one side — and in both cases \
         the agent following the instruction loses its turn. Otherwise, list it \
         in `READ_ELSEWHERE` with what reads it:\n{}",
        faults.len(),
        faults.join("\n")
    );

    // The ratchet in the other direction: an exemption with no reason left.
    let stale: Vec<&str> = READ_ELSEWHERE
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| alone.get(*n).copied().unwrap_or(false) || !alone.contains_key(*n))
        .collect();
    assert!(
        stale.is_empty(),
        "stale `READ_ELSEWHERE` entr(y/ies) — the variable is read by \
         Rust, or no longer exists. Remove them:\n  {}",
        stale.join("\n  ")
    );
}
