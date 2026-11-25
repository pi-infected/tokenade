//! Find DUPLICATED implementations — the targeting tool for the
//! consolidations the project's working rules ask for.
//!
//! `cargo run -p tokenade-bench --bin duplication [-- --min-lines 8]`
//!
//! ## Why it exists
//!
//! The most frequent defect in this repo is "proven on one consumer,
//! not on the sibling" — thirteen recorded occurrences. Its hardest form
//! is the EXACT copy: one copy gets fixed, the other keeps the bug, and nothing
//! says so. Until now I found them by chance, by stumbling on them.
//!
//! ## What it does NOT count as duplication
//!
//! - Bodies shorter than `--min-lines` lines: two identical accessors
//!   are not duplication, they are two accessors.
//! - `#[cfg(test)]` blocks: a repeated fixture is a fixture.
//! ## Identical text is not always identical meaning
//!
//! MEASURED on the first pass: `prune_missing` appears identically in
//! `index_db.rs` and `semantic_db.rs` — and is NOT mergeable, because
//! each one calls ITS OWN `list_projects` and `forget`. The report is therefore
//! a list of candidates to look at, not a list of merges to make. Of the
//! three groups of the first pass, two were real copies and one
//! was this illusion.
//!
//! - This repo's intended pattern: every `cmd/<os>/<prog>.rs` exposes
//!   `detect`/`try_compact`/`compact`. **927 `try_compact` are not 926
//!   copies to merge**, it is the architecture — one compactor per command,
//!   whose list `build.rs` generates. So only bodies
//!   IDENTICAL up to whitespace are reported, which a real compactor never is.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The half of the file that ships — through the repo's SINGLE declarer.
///
/// This instrument used to cut at the first `#[cfg(test)]` anchored in column zero,
/// the third spelling of the question in this repo and the second wrong one:
/// everything after the first test module disappeared, production
/// code included. `srcsweep::production_lines` follows braces and
/// reports the lines on BOTH sides of an interleaved test module.
fn production_only(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    for (_, line) in tokenade_core::srcsweep::production_lines(body) {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Replaces the CONTENT of strings and comments with spaces, keeping
/// length and line breaks, so that offsets stay
/// valid on the original text.
///
/// Without it the brace counter desynchronises: MEASURED on this tree,
/// **2,856 strings contain an unmatched brace** (`"{"`, `"}"`, and
/// even `"an odd { brace"` in a test fixture). A tool that looks for
/// identical bodies and slices the bodies badly does not report
/// duplications, it reports its own noise.
fn mask_literals(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = vec![b' '; b.len()];
    let mut i = 0;
    while i < b.len() {
        // Outside a literal: copy as is, except on entering a literal/comment.
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            let mut depth = 1;
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if b[i] == b'*' && i + 1 < b.len() && b[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    if b[i] == b'\n' {
                        out[i] = b'\n';
                    }
                    i += 1;
                }
            }
            continue;
        }
        // Raw string `r"…"` / `r#"…"#`, hashes counted.
        if b[i] == b'r' && i + 1 < b.len() && (b[i + 1] == b'"' || b[i + 1] == b'#') {
            let mut k = i + 1;
            let mut hashes = 0;
            while k < b.len() && b[k] == b'#' {
                hashes += 1;
                k += 1;
            }
            if k < b.len() && b[k] == b'"' {
                i = k + 1;
                'raw: while i < b.len() {
                    if b[i] == b'"' {
                        let mut j = i + 1;
                        let mut seen = 0;
                        while j < b.len() && b[j] == b'#' && seen < hashes {
                            seen += 1;
                            j += 1;
                        }
                        if seen == hashes {
                            i = j;
                            break 'raw;
                        }
                    }
                    if b[i] == b'\n' {
                        out[i] = b'\n';
                    }
                    i += 1;
                }
                continue;
            }
        }
        if b[i] == b'"' || b[i] == b'\'' {
            let quote = b[i];
            // A lifetime (`'a`) does not open a character literal.
            if quote == b'\'' {
                let next = b.get(i + 1).copied().unwrap_or(b' ');
                let after = b.get(i + 2).copied().unwrap_or(b' ');
                if next.is_ascii_alphabetic() && after != b'\'' {
                    out[i] = b[i];
                    i += 1;
                    continue;
                }
            }
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if b[i] == quote {
                    i += 1;
                    break;
                }
                if b[i] == b'\n' {
                    out[i] = b'\n';
                }
                i += 1;
            }
            continue;
        }
        out[i] = b[i];
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The body of each `fn`, from the opening `{` to the balanced `}`, with its name.
///
/// Braces are counted on the MASKED text (see `mask_literals`), the
/// bodies are sliced from the ORIGINAL: both have the same length, so the
/// offsets coincide.
fn functions(src: &str) -> Vec<(String, String)> {
    let masked = mask_literals(src);
    let mut out = Vec::new();
    let b = masked.as_bytes();
    let src_orig = src;
    let src = masked.as_str();
    let mut i = 0;
    while let Some(rel) = src[i..].find("fn ") {
        let start = i + rel;
        // `fn` must start a word (not `transfn`, not `.fn`).
        let before_ok = start == 0 || !b[start - 1].is_ascii_alphanumeric() && b[start - 1] != b'_';
        let name_end = src[start + 3..]
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|k| start + 3 + k)
            .unwrap_or(src.len());
        let name = src[start + 3..name_end].to_string();
        let Some(open_rel) = src[name_end..].find('{') else {
            break;
        };
        let open = name_end + open_rel;
        let mut depth = 0usize;
        let mut end = open;
        for (k, c) in src[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + k + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        let kept = before_ok && !name.is_empty() && end > open;
        if kept {
            out.push((name, src_orig[open..end].to_string()));
        }
        // Only advance past a BODY if that body was kept. A false
        // `fn ` (the end of `transfn `) would otherwise find the brace of the
        // NEXT function and jump over it: the real function vanished
        // from the report without anything saying so.
        i = if kept { end } else { start + 3 };
    }
    out
}

/// Two bodies are "the same" up to whitespace. Deliberately strict: an
/// approximate resemblance is a design decision, an IDENTITY is
/// a fact.
fn normalise(body: &str) -> String {
    body.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn count_lines(body: &str) -> usize {
    body.lines().count()
}

fn rust_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    tokenade_bench::reject_unknown_args(&["--min-lines"], &args);
    let min_lines: usize = args
        .windows(2)
        .find(|w| w[0] == "--min-lines")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(8);

    let ws = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let mut files = Vec::new();
    for c in ["tokenade-core", "tokenade-cli"] {
        rust_files(&ws.join("crates").join(c).join("src"), &mut files);
    }
    files.sort();

    // normalised body → (name, file) of each site
    let mut groups: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        let rel = f
            .strip_prefix(&ws)
            .unwrap_or(f)
            .to_string_lossy()
            .into_owned();
        for (name, body) in functions(&production_only(&src)) {
            if count_lines(&body) < min_lines {
                continue;
            }
            groups
                .entry(normalise(&body))
                .or_default()
                .push((name, rel.clone()));
        }
    }

    let mut dups: Vec<(usize, usize, &Vec<(String, String)>)> = groups
        .iter()
        .filter(|(_, sites)| sites.len() > 1)
        .map(|(body, sites)| (sites.len(), body.split(' ').count(), sites))
        .collect();
    dups.sort_by_key(|b| std::cmp::Reverse(b.0 * b.1));

    println!(
        "{} files, bodies >= {min_lines} lines, excluding #[cfg(test)]",
        files.len()
    );
    if dups.is_empty() {
        println!("no duplicated implementation");
        return;
    }
    println!("{} group(s) of IDENTICAL bodies:\n", dups.len());
    for (n, tokens, sites) in dups.iter().take(20) {
        let names: Vec<&str> = sites.iter().map(|(nm, _)| nm.as_str()).collect();
        println!("  ×{n}  ~{tokens} tokens  fn {}", names.join(" / "));
        for (_, file) in sites.iter() {
            println!("        {file}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_is_taken_from_its_brace_to_the_balanced_one() {
        let src = "fn a() {\n    if x {\n        y();\n    }\n}\nfn b() { z(); }\n";
        let fns = functions(src);
        assert_eq!(fns.len(), 2, "{fns:?}");
        assert_eq!(fns[0].0, "a");
        assert!(fns[0].1.contains("y();"));
        assert!(
            fns[0].1.ends_with('}') && fns[0].1.matches('{').count() == 2,
            "the body of `a` must include its nested `if`: {:?}",
            fns[0].1
        );
        assert_eq!(fns[1].0, "b");
    }

    /// The case that would have produced noise: 2,856 strings in this tree
    /// carry an unmatched brace. Without masking, the counter
    /// desynchronises and the "identical bodies" are no longer bodies.
    #[test]
    fn a_brace_inside_a_literal_does_not_move_the_body() {
        let src = "fn a() {\n    println!(\"an odd { brace\");\n}\nfn b() {\n    other();\n}\n";
        let fns = functions(src);
        assert_eq!(fns.len(), 2, "the literal shifted the slicing: {fns:?}");
        assert_eq!(fns[0].0, "a");
        assert!(
            fns[0].1.contains("an odd { brace"),
            "the returned body must be the ORIGINAL: {:?}",
            fns[0].1
        );
        assert_eq!(fns[1].0, "b");
        assert!(fns[1].1.contains("other();"));
    }

    /// A `//` or a `/* */` carrying a brace must not count either,
    /// and a lifetime `'a` does not open a character literal.
    #[test]
    fn comments_and_lifetimes_do_not_open_anything() {
        let src = "fn a() {\n    // ferme } ici\n    /* et { la */\n    let x: &'static str = \"y\";\n}\nfn b() {\n    z();\n}\n";
        let fns = functions(src);
        assert_eq!(fns.len(), 2, "{fns:?}");
        assert_eq!(fns[1].0, "b");
    }

    /// `fn` must start a word, otherwise `transfn` or `myfn` invent
    /// functions and the bodies shift.
    #[test]
    fn a_fn_inside_a_word_is_not_a_function() {
        let fns = functions("let transfn = 1; fn real() { x(); }\n");
        assert_eq!(fns.len(), 1);
        assert_eq!(fns[0].0, "real");
    }

    #[test]
    fn only_whitespace_separates_two_identical_bodies() {
        assert_eq!(normalise("{ a();\n b(); }"), normalise("{   a(); b();   }"));
        assert_ne!(normalise("{ a(); }"), normalise("{ a(); c(); }"));
    }

    /// An INTERLEAVED test module does not make what follows it disappear.
    #[test]
    fn a_test_block_in_the_middle_keeps_what_follows() {
        let src = "fn keep() { a(); }\n    // #[cfg(test)] cité dans un commentaire\nfn also() { b(); }\n#[cfg(test)]\nmod t {\n    fn gone() { c(); }\n}\nfn after() { d(); }\n";
        let prod = production_only(src);
        assert!(prod.contains("fn keep"), "{prod}");
        assert!(
            prod.contains("fn also"),
            "an INDENTED cfg(test) does not cut: {prod}"
        );
        assert!(!prod.contains("fn gone"), "{prod}");
        assert!(
            prod.contains("fn after"),
            "the production after it was lost: {prod}"
        );
    }
}
