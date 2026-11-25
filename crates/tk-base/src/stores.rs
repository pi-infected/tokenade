//! **The inventory of persistent stores, in one place.**
//!
//! Each one lives under `~/.tokenade/` by default and accepts an environment
//! override. They were enumerated nowhere, so each consumer
//! knew the subset it had come across.
//!
//! ## What it cost
//!
//! MEASURED 2026-09-08: the bench (`tokenade-bench::Bench`) believed itself ISOLATED and
//! only redirected **three out of five** — `TOKENADE_CONFIG_DIR`,
//! `TOKENADE_STASH_DIR`, `TOKENADE_DEDUP_DB`. The index, the fold store, the
//! file search and the semantic store stayed the
//! operator's. Hence:
//!
//! * every measurement read stores the bench believed were empty;
//! * `Bench::forget()`, written to make each capture independent,
//!   only wiped what it redirected — the guarantee was false for four
//!   stores out of five;
//! * `probe --here index` spent eighteen seconds rebuilding the machine's REAL
//!   index, from an instrument whose whole contract is to touch
//!   nothing.
//!
//! A hand-kept list is what lets the sixth slip through: a source
//! guard requires every `TOKENADE_*_DB` override in the crate to appear here.

/// A store: the variable that redirects it, and its file name under
/// `~/.tokenade/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Store {
    pub env: &'static str,
    pub file: &'static str,
}

/// All the stores an isolated environment must redirect.
pub const ISOLATABLE: &[Store] = &[
    Store {
        env: "TOKENADE_DEDUP_DB",
        file: "dedup.db",
    },
    Store {
        env: "TOKENADE_INDEX_DB",
        file: "index.db",
    },
    Store {
        env: "TOKENADE_SEMANTIC_DB",
        file: "semantic.db",
    },
    Store {
        env: "TOKENADE_FOLD_STORE_DB",
        file: "fold_store.db",
    },
    Store {
        env: "TOKENADE_FILE_SEARCH_DB",
        file: "file_search.db",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// **No store is missing.**
    ///
    /// SOURCE guard: every `TOKENADE_*_DB` override read in the crate must
    /// appear in [`ISOLATABLE`]. Without it, a sixth store would be added
    /// and the bench would leave it pointing at the operator's — which is
    /// exactly what happened to four of them.
    #[test]
    fn every_store_override_is_listed() {
        let files = crate::source_guard::all_rust_sources();
        let mut found: std::collections::BTreeSet<String> = Default::default();
        for (_, code) in &files {
            let mut rest = code.as_str();
            while let Some(i) = rest.find("var_os(\"TOKENADE_") {
                let tail = &rest[i + "var_os(\"".len()..];
                if let Some(end) = tail.find('"') {
                    let name = &tail[..end];
                    if name.ends_with("_DB") {
                        found.insert(name.to_string());
                    }
                }
                rest = &rest[i + 8..];
            }
        }
        let scanned = files.len();
        assert!(scanned > 100, "only {scanned} file(s) swept");
        assert!(
            !found.is_empty(),
            "no override found: the guard looks at nothing"
        );
        for name in &found {
            assert!(
                ISOLATABLE.iter().any(|s| s.env == *name),
                "`{name}` redirects a store and is not listed in `ISOLATABLE`: \
                 a bench that believes itself isolated will write into the operator's"
            );
        }
    }

    /// Two stores share neither their variable nor their file.
    #[test]
    fn no_two_stores_collide() {
        for (i, a) in ISOLATABLE.iter().enumerate() {
            for b in &ISOLATABLE[i + 1..] {
                assert_ne!(a.env, b.env, "two stores, one variable");
                assert_ne!(a.file, b.file, "two stores, one file");
            }
        }
    }
}

/// **The variables that change WHAT A COMPACTION RETURNS.**
///
/// A measurement must not depend on the environment of whoever runs it.
/// MEASURED 2026-09-09: the same binary, the same program, the same capture —
/// `windows/tasklist_v_win` — returns **138 lines / 7,825 bytes** in a
/// clean environment and **33 lines / 3,085** when `TOKENADE_PRESSURE=0.35`
/// lingers in the shell, because pressure divides the stashing threshold
/// by three and changes what the second stage folds. The session
/// wrapper exports it; every manual run inherits it, and two figures
/// a factor of 2.5 apart contradicted each other with nothing saying why.
///
/// Written once, next to [`ISOLATABLE`] and for the same reason: the
/// stores say WHERE the command writes, this says WHAT IT DECIDES.
pub const COMPACTION_ENV: &[&str] = &[
    // The stashing threshold, scaled by session depth.
    "TOKENADE_PRESSURE",
    "TOKENADE_ADAPTIVE_THRESHOLDS",
    // The two thresholds, set directly.
    "TOKENADE_DISCLOSE_THRESHOLD",
    "TOKENADE_READ_DISCLOSE_THRESHOLD",
    // The scale of read views (`task`, `entropy`, …).
    "TOKENADE_READ_MODE",
    // The hook and the wrapper: a measurement does not re-invite itself into itself.
    "TOKENADE_WRAPPING",
    "TOKENADE_HOOK_DISABLED",
    // `TOKENADE_HOOK_ACTIVE` was removed: the bench had been clearing it for
    // a long time, and the guard below showed on its first shot that **nothing
    // reads it** in this repo — not a single production line left, not a
    // test. Only one occurrence remains, in a diff capture used
    // as a fixture. Clearing a dead variable gives the illusion of a
    // precaution.
];

#[cfg(test)]
mod compaction_env_tests {
    use super::*;

    /// Every listed variable is READ somewhere in the product — a list
    /// naming a dead variable gives the illusion of a guard.
    ///
    /// **Without excluding THIS file.** First version: it searched for the name
    /// in all sources and found it in its OWN declaration, so
    /// renaming an entry to `TOKENADE_PRESSURE_ABSENTE` left it green —
    /// a mutation survived on that. It is the guard that matches
    /// itself, a class of bug already paid for once.
    #[test]
    fn every_named_variable_is_read_by_the_product() {
        // The whole workspace, not just this crate: `TOKENADE_HOOK_ACTIVE`
        // is read on the CLI side, and a guard bounded to the declaring crate would
        // have concluded it is dead.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .to_path_buf();
        let files = crate::source_guard::rust_sources(&root);
        let mut checked = 0usize;
        for var in COMPACTION_ENV {
            let seen = files.iter().any(|(path, code)| {
                path.file_name().and_then(|f| f.to_str()) != Some("stores.rs") && code.contains(var)
            });
            checked += 1;
            assert!(
                seen,
                "`{var}` is read nowhere outside this inventory: the list lies"
            );
        }
        assert_eq!(
            checked,
            COMPACTION_ENV.len(),
            "the guard no longer walks the list"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────
// "Would this write land in the user's REAL store?"
// ─────────────────────────────────────────────────────────────────────
//
// These two functions lived in `gain/paths.rs`, the ledger module,
// because the ledger was the first to need to know whether it was writing
// into a real store. Yet it is not an accounting question:
// it is an ISOLATION question, the same one the symbol index and
// the semantic index ask — and they are the ones that pulled `gain`, a product layer,
// from the code layer.
//
// Here, the module that inventories the persistent stores carries it,
// and the three callers name it downwards.

/// True when the given cwd path looks like a test tempdir. Used to drop
/// writes that would otherwise contaminate a PRODUCTION store with rows tagged
/// to throwaway tempdirs.
///
/// It guarded the gain ledger alone for a year, and the two other stores that
/// key rows by project path were left open. Measured 2026-09-03 on a real
/// install: **85 of 180 rows in `index.db` (47 %) and 38 of 104 in
/// `semantic.db` (37 %)** were test tempdirs, the newest written that morning.
/// `tokenade semantic-status` reported 52 projects where 14 exist. The original
/// guard's comment says why this belongs at the store rather than in the harness — it
/// is "belt-and-suspenders … catches any future test helper that forgets to
/// also redirect HOME" — and a rule that lives in one place has to be CALLED
/// from every place.
///
/// Patterns covered:
/// - `…/tokenade-cli-*` (cli/main.rs `tmp()` helper)
/// - `…/tokenade-core-*` (core tests' temp helper, if any)
/// - `…/cargo-test-*` (rare but observed when tests use std::env::temp_dir
///   inside `target/tmp/...`)
///
/// We match on the basename component (not the absolute path) because the
/// system's temp_dir varies (`/tmp` on Linux, `/var/folders/...` on macOS,
/// `C:\Users\...\Temp` on Windows) but the prefix is what tests inject.
/// True when a row keyed on `project_path` would land in the USER's real
/// store because a harness redirected the working directory and not `HOME`.
///
/// `is_test_cwd` alone is too blunt for the project-keyed stores: a properly
/// isolated test redirects BOTH, so its cwd looks like a tempdir while its
/// store is its own, and refusing that write only breaks the test. The leak
/// that guard describes is precisely the asymmetry — "sets env::current_dir to a
/// tempdir but does NOT redirect HOME" — so that is what this asks. The gain
/// ledger keeps the blunt check because it has an explicit override
/// (`TOKENADE_FORCE_GAIN`) that the index stores do not.
pub fn write_would_leak_into_real_store(project_path: &str, db_path: Option<&str>) -> bool {
    if !is_test_cwd(project_path) {
        return false;
    }
    // An in-memory connection is nobody's store. Unit tests open one and hand
    // it whatever path the case needs; refusing those breaks the suite and
    // protects nothing — the first version of this guard did exactly that to
    // `evict_stale_drops_zero_symbol_orphans_past_ttl`, which is about
    // eviction and uses a tempdir-shaped path only incidentally.
    match db_path {
        None => return false,
        Some("") => return false,
        Some(_) => {}
    }
    let home = crate::platform::home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    !is_test_cwd(&home)
}

pub fn is_test_cwd(path: &str) -> bool {
    // Cheap substring check first — if the path doesn't include one of
    // the known prefixes anywhere, it's definitely not a test cwd.
    path.contains("/tokenade-cli-")
        || path.contains("/tokenade-core-")
        || path.contains("/tokenade-mcp-")
        || path.contains("/cargo-test-")
}

// ─────────────────────────────────────────────────────────────────────
// "Have I done it recently already?"
// ─────────────────────────────────────────────────────────────────────
//
// A throttle stamp, once per period, under `~/.tokenade`. It lived
// in `report`, the layer that reports figures, because the
// reporter was the first to need it. Yet it is neither a report nor
// a figure: it is a state file, and its three callers are the
// reporter, the stash sweep — a primitive — and the CLI-side session
// lifecycle.
//
// A third bespoke stamp must not be born: `hooks_check_due` is
// older and keeps its name because `uninstall` deletes its file by that
// name. Everything added goes through this pair — a stable `name`, and
// the caller owns its meaning.

use std::path::Path;

/// Generic once-per-TTL throttle stamp under `~/.tokenade`.
///
/// [`hooks_check_due`] predates this and keeps its own name because it already
/// writes a sentinel with a fixed filename that `uninstall` clears by name.
/// Anything added later should use this pair rather than grow a third bespoke
/// one: pass a stable `name` and the caller owns the meaning.
pub fn stamp_due(home: &Path, name: &str, ttl_secs: u64) -> bool {
    let m = crate::platform::mtime_epoch(&stamp_path(home, name));
    m == 0 || crate::platform::now_unix_secs().saturating_sub(m) >= ttl_secs
}

/// Stamp `name` to now (best-effort). See [`stamp_due`].
pub fn mark_stamp(home: &Path, name: &str) {
    let p = stamp_path(home, name);
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&p, crate::platform::now_unix_secs().to_string());
}

fn stamp_path(home: &Path, name: &str) -> std::path::PathBuf {
    // Filenames are ours, never user input, but keep the join defensive so a
    // stray separator can never escape ~/.tokenade.
    let safe: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    home.join(".tokenade").join(format!("stamp-{safe}"))
}
