//! Lightweight codebase index: symbols + import graph.
//!
//! Instead of a multi-megabyte tree-sitter grammar set, we use carefully
//! written per-language regexes to extract *declarations* (functions, types,
//! classes) and *imports*. This is the tokensave/codegraph idea (symbol nav +
//! dependency graph, impact radius) delivered with zero heavy deps.
//!
//! The win: an agent asks "where is `parse_config`?" or "what's the impact of
//! changing `User`?" and gets a few hundred tokens of structured answer instead
//! of reading dozens of whole files.

// `Symbol` and `Call` are the records the EXTRACTOR produces; they live with
// it in `treesitter`, and are re-exported here because `index::Symbol` is the
// spelling the whole repo knows.
pub use crate::treesitter::{Call, Symbol};

use regex::Regex;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

impl Symbol {
    /// `<kind> <name>` — how this symbol is announced to a reader.
    ///
    /// Two kinds carry the thing they hang off in [`Self::bases`], and
    /// without it they print the same words over and over:
    ///
    /// - an `impl` block is named after the type it extends, so a file with
    ///   six of them printed six identical lines — `impl TokenadeError` ×6
    ///   becomes `impl Display for TokenadeError`,
    ///   `impl From<std::io::Error> for TokenadeError`, and so on;
    /// …and two more carry it in [`Self::owner`]:
    ///
    /// - a Go method is named without its receiver, so `Execute` printed 34
    ///   identical lines on one real project — it becomes
    ///   `method *Task.Execute`;
    /// - a field or enum variant is named without the type that declares it,
    ///   which is the biggest of the three: measured across 517 indexed
    ///   projects, **38 126 of 90 188 field symbols (42 %)** shared a bare
    ///   name with a sibling in the same file. It becomes
    ///   `field MCPServer.Timeout`.
    ///
    /// All are rendered the way their language writes them.
    pub fn label(&self) -> String {
        format!("{} {}", self.kind, self.qualified_name())
    }

    /// Does this declaration carry a test attribute (`#[test]`,
    /// `#[tokio::test]`, `#[rstest]`, a Python `@pytest.mark…`)?
    ///
    /// Half the answer only: 706 functions in this workspace sit inside a
    /// `mod tests` and carry no attribute of their own (fixtures, helpers),
    /// and 1 766 carry one while their module's span was never captured. The
    /// span half lives in [`Index::impact_of`], which is where the file's
    /// other symbols are in hand.
    pub fn has_test_attribute(&self) -> bool {
        self.decorators
            .iter()
            .any(|d| d.trim().trim_end_matches([']', ')']).ends_with("test"))
    }

    /// The name WITH what it hangs off, and without the kind.
    ///
    /// `impact` groups its output by kind already, so printing `fn` twice on
    /// one line says nothing — it needs this half. Splitting it out is also
    /// what keeps the two surfaces from drifting: `query` prints
    /// `fn TokenadeError.fmt` and `impact` prints `TokenadeError.fmt` under
    /// an `fn (12):` heading, from the same string.
    pub fn qualified_name(&self) -> String {
        if self.kind == "impl" {
            if let Some(t) = self.bases.first() {
                return format!("{t} for {}", self.name);
            }
        }
        if self.owner.is_empty() {
            return self.name.clone();
        }
        // ONE rule for every kind and every language: `Owner.name`.
        //
        // The Go receiver was rendered `method (*Task) Execute` for one
        // iteration, keyed on `kind == "method"`. Java and Kotlin produce
        // that same kind, so a Java method came out `method (T) run` —
        // parentheses that mean "receiver" in Go and nothing anywhere else.
        // A branch that guesses the language from the kind will keep getting
        // that wrong; `*Task.Execute` costs Go a little idiom and is right
        // in every grammar we ship.
        format!("{}.{}", self.owner, self.name)
    }

    /// `<kind> <name> — <file>:<line>`, the location line every code-intel
    /// surface prints.
    ///
    /// Was spelled out at four sites — `render_with_context`'s no-context
    /// branch and both of its fallbacks, `code_intel::output`, and the bench
    /// harness — one of which carried a comment asserting it matched another
    /// ("Same em-dash location line render_with_context(0) emits"). A comment
    /// claiming two places agree is the shape a shared declarer replaces.
    pub fn location_line(&self) -> String {
        format!("{} — {}:{}", self.label(), self.file, self.line)
    }
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Index {
    pub root: String,
    pub symbols: Vec<Symbol>,
    /// file -> list of files it imports (resolved best-effort within the repo).
    pub imports: BTreeMap<String, Vec<String>>,
    pub files_indexed: usize,
    /// Per-file change fingerprint ([`file_fingerprint`]: high-res mtime folded
    /// with size). Persisted so the next `rebuild_incremental` only re-parses
    /// files whose fingerprint changed — catching same-second and same-mtime
    /// edits a bare seconds-mtime missed. Kept the `mtimes` name/type for
    /// on-disk compatibility; an index written by an older build stores raw
    /// seconds here, so the first rebuild after upgrade re-parses everything
    /// once (fingerprints won't match), then stabilises. Missing on v3 indices
    /// loaded from disk — they fall back to a full rebuild.
    /// The tokenade build that produced this index. A cached index is a
    /// cache of one particular EXTRACTOR, and nothing else on disk records
    /// that: incremental rebuilds only re-parse files whose fingerprint
    /// changed, so a file nobody touched keeps whatever its symbols looked
    /// like when it was first parsed. Every improvement to symbol extraction
    /// was therefore invisible on existing repositories, forever. Stamping
    /// the build and rejecting a foreign one on load costs one full rebuild
    /// per repository per upgrade and makes those fixes actually arrive.
    /// Version-stamped rather than hand-bumped on purpose: a marker somebody
    /// has to remember to raise is a marker that silently goes stale.
    #[serde(default)]
    pub built_by: String,
    #[serde(default)]
    pub mtimes: BTreeMap<String, u64>,
    /// Call-graph edges harvested by the tree-sitter extractor. Each
    /// row is one call site: `caller_name` calls `callee_name` at
    /// `file:line`. The graph is intentionally lossy — we don't track
    /// argument types, dispatch resolution, or generic instantiation;
    /// just the syntactic call relation, which is what an agent needs
    /// to answer "who calls X?" in seconds.
    ///
    /// Empty on indices built before this field existed; `Index::callers`
    /// degrades gracefully when missing.
    ///
    /// NOT serialised into the main blob — it lives in its own `calls_blob`
    /// column and is fetched by [`Self::load_calls`], because 84 % of
    /// code-intel invocations never read it and it is 78 % of the bytes.
    /// Still DEserialised when present, so an index written before the split
    /// keeps its graph.
    #[serde(default, skip_serializing)]
    pub calls: Vec<Call>,
    /// True once [`Self::rebase_to`] has rewritten every path to be relative
    /// to a directory other than `root`. Never serialised, and [`Self::save`]
    /// REFUSES to persist such an index — the store must only ever hold
    /// root-relative paths, and making that impossible beats remembering it.
    #[serde(skip)]
    pub rebased: bool,
}

mod lang_specs;
use lang_specs::lang_for;

// The index is a DATUM (`Symbol`, `Index`) and six jobs that use it. The file
// carries the datum; each child carries a job, and `pub use` puts them back at
// the `index::` path the repo already spells everywhere.
mod calls;
mod extract;
mod is_test;
mod query;
mod store;
mod structure;
mod walk;
pub use calls::*;
pub use extract::*;
use is_test::*;

pub use store::*;

pub use walk::*;

// #557 — `use std::io` (unused) and `fn _path_marker` (no-op
// touch-keeper) removed. All consumers reference `std::io::*`
// fully-qualified above.

// Minimal in-tree tempdir helper so tests need no extra crate.
#[cfg(test)]
include!("index_tests.rs");
