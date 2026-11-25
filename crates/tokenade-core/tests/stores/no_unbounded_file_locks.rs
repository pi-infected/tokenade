//! No tokenade file lock waits forever.
//!
//! `fs2`'s `lock_exclusive()` blocks with no ceiling. Every call site that had
//! one also discarded its result (`let _ = …`) — a lock error has always meant
//! "carry on" — so the unbounded wait bought nothing, and cost this: in this
//! repository's own suite, with dozens of wrapped commands running at once, a
//! `tokenade wrap` had still not started its work after 30 SECONDS, six times
//! across one session, where the same test alone takes 0.25 s. The savings
//! ledger was bounded first; the stall came back, because a usage
//! reporter, the redaction counter and the recalc marker each held one too —
//! all three on the startup path of EVERY invocation.
//!
//! `platform::lock_briefly` is the one rule. This sweep is what keeps a second
//! spelling from appearing next to it.

use std::path::{Path, PathBuf};

/// Sites that block on purpose, each with the reason it is not a startup cost.
/// An entry here is a claim that a human read the call and decided.
const DELIBERATE_BLOCKING: &[(&str, &str)] = &[
    (
        "crates/tk-agents/src/mcp_wrap.rs",
        "`mcp-wrap install` rewrites another agent's config; a partial rewrite is worse than a wait",
    ),
    (
        "crates/tokenade-core/src/search.rs",
        "index build — the caller has nothing to return until it is done",
    ),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_sources(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

/// The blocking-lock sites in one file: `(the file had any call at all, the
/// undeclared ones)`.
///
/// PRODUCTION lines only — a test that holds a lock to prove the bound works
/// is not a startup cost, and the helper that strips `#[cfg(test)]` modules
/// exists for exactly this.
fn blocking_lock_sites(rel: &str, body: &str) -> (bool, Vec<String>) {
    // The declaration bears on the MODULE, not on the file: `mcp_wrap.rs` was
    // split and its lock moved down into `mcp_wrap/config_io.rs`, which made
    // this guard announce an unbounded wait where the decision had been made
    // and written long ago. A declared path therefore also covers `<it>/…`.
    let declared = DELIBERATE_BLOCKING.iter().any(|(p, _)| {
        rel.ends_with(p)
            || p.strip_suffix(".rs")
                .is_some_and(|trunk| rel.contains(&format!("{trunk}/")))
    });
    let mut any = false;
    let mut out = Vec::new();
    for (i, line) in tokenade_core::srcsweep::production_lines(body) {
        // The call, not a mention of it: prose in a doc comment is not a wait.
        let code = line.split("//").next().unwrap_or("");
        if !code.contains(".lock_exclusive()") {
            continue;
        }
        any = true;
        if declared {
            continue;
        }
        out.push(format!("{rel}:{i}: {}", line.trim()));
    }
    (any, out)
}

/// The rule itself, on source that is not in the tree — so weakening the sweep
/// shows up here even when the tree is clean.
#[test]
fn the_rule_reports_an_undeclared_wait_and_spares_a_declared_one() {
    let src = "fn f() {\n    let _ = handle.lock_exclusive();\n}\n";
    let (any, sites) = blocking_lock_sites("crates/tokenade-core/src/somewhere.rs", src);
    assert!(any);
    assert_eq!(
        sites.len(),
        1,
        "an undeclared wait must be reported: {sites:?}"
    );

    let (any, sites) = blocking_lock_sites("crates/tk-agents/src/mcp_wrap.rs", src);
    assert!(any);
    assert!(sites.is_empty(), "a declared one must be spared: {sites:?}");

    // `try_lock_exclusive` is not a wait, and neither is prose about one.
    let benign = "fn f() {\n    let _ = handle.try_lock_exclusive();\n    // calls lock_exclusive() one day\n}\n";
    let (any, sites) = blocking_lock_sites("crates/tokenade-core/src/somewhere.rs", benign);
    assert!(!any, "nothing here waits");
    assert!(sites.is_empty());
}

#[test]
fn every_blocking_file_lock_is_bounded_or_declared() {
    let root = workspace_root();
    let mut files = Vec::new();
    for d in ["crates/tokenade-core/src", "crates/tokenade-cli/src"] {
        rust_sources(&root.join(d), &mut files);
    }
    assert!(files.len() > 100, "the sweep found no sources to read");

    let mut offenders = Vec::new();
    let mut seen_any = false;
    for path in &files {
        let Ok(body) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let (found, mut sites) = blocking_lock_sites(&rel, &body);
        seen_any |= found;
        offenders.append(&mut sites);
    }
    assert!(
        seen_any,
        "the sweep matched no call at all, so it proves nothing"
    );
    assert!(
        offenders.is_empty(),
        "a file lock waits with no ceiling; use `platform::lock_briefly`, or declare it in \
         DELIBERATE_BLOCKING with the reason:\n  {}",
        offenders.join("\n  ")
    );
}
