//! BM25 semantic search over the codebase.
//!
//! `index.rs` answers *"where is the symbol named X?"* via case-insensitive
//! substring match. That works when the agent already knows the right
//! identifier — but a Claude/Cursor session more often asks the *concept*
//! version of the question (`"auth flow"`, `"rate limiting"`, `"where do we
//! load config?"`). Without semantic ranking we fall back to `grep`, which
//! is the single biggest reason `nav-heavy` was the worst-performing bench
//! mix in v2 (51.2 %).
//!
//! This module builds a dependency-free **Okapi BM25** index over a virtual
//! document per source file, made of:
//!
//! - the file's path components (so `src/auth/login.rs` boosts `auth`, `login`),
//! - every symbol declared in the file (declared via `crate::index::Symbol`),
//! - comments and docstrings extracted from the file (`///`, `//!`, `//`,
//!   `# ...`, `""" ... """`, `/** ... */`).
//!
//! Tokenization splits on non-word chars **and** on the snake_case / camelCase
//! boundaries, then lowercases — so a query for `parseConfig`, `parse_config`,
//! or `parse config` all hit the same posting. The index serialises to
//! `<root>/.tokenade/semantic.json` so a second session can recall it without
//! re-walking the tree.

use crate::embedding::EmbedError;
use crate::index::{is_skip_dir_name, Index};
use crate::reranker::RerankError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// One indexed document — currently one per file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Doc {
    pub file: String,
    /// Length in tokens (used to normalise term frequency).
    pub len: u32,
    /// Short representative snippet (~500 chars) of what this file is about —
    /// path components + symbol names + first comment lines. Used by the
    /// cross-encoder reranker: scoring `(query, "auth.rs login user_session
    /// verify_credentials handle a user login")` is dramatically stronger
    /// than scoring `(query, "src/auth.rs")` which has no signal.
    #[serde(default)]
    pub snippet: String,
}

/// (`doc_id`, term-frequency-in-doc) — the inverted index payload.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Posting {
    pub doc: u32,
    pub tf: u32,
}

/// Errors propagated from the embedding or reranker stages of the hybrid
/// search pipeline. The CLI/MCP surface this directly to the user — see
/// the "no silent fallback" policy on [`crate::embedding`].
#[derive(Debug, Clone)]
pub enum HybridError {
    /// Embedder failed (model init or inference).
    Embed(EmbedError),
    /// Reranker failed (model init or inference).
    Rerank(RerankError),
}

impl std::fmt::Display for HybridError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HybridError::Embed(e) => write!(f, "{e}"),
            HybridError::Rerank(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for HybridError {}

/// Single ranked hit returned by [`SemanticIndex::search`].
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub file: String,
    pub score: f64,
    /// Up to ~5 query terms that contributed to the score (for explainability).
    pub matched: Vec<String>,
}

/// Okapi-BM25 inverted index. Tunables `k1` and `b` follow the canonical
/// defaults (1.2 / 0.75) which work well across both code and prose.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticIndex {
    pub docs: Vec<Doc>,
    /// term → posting list
    pub inv: HashMap<String, Vec<Posting>>,
    /// Average document length (tokens) across the corpus.
    pub avgdl: f64,
    pub k1: f64,
    pub b: f64,
    /// Per-file mtime used by [`Self::rebuild_incremental`] to detect
    /// changes without re-tokenising the whole corpus.
    #[serde(default)]
    pub mtimes: HashMap<String, u64>,
    /// Which tokenade built this index, same rule as [`crate::index::Index`].
    ///
    /// The postings are the output of [`tokenize`], and a query tokenises
    /// through that same function: change it and the two sides stop
    /// agreeing. `mtimes` cannot see that — every file is unchanged — so
    /// an upgraded install would keep answering out of an index built by
    /// the tokenizer it just replaced, and a retrieval fix would reach
    /// nobody who already had an index. An empty string is a pre-guard
    /// blob and is refused like any other mismatch.
    #[serde(default)]
    pub built_by: String,
    /// True once [`Self::rebase_to`] has rewritten every path relative to a
    /// directory other than the project root. Never serialised (so the
    /// on-disk format is unchanged), and [`Self::save`] REFUSES to persist an
    /// index carrying it — same rule, same reason, as
    /// [`crate::index::Index::rebased`].
    #[serde(skip)]
    pub rebased: bool,
}

impl Default for SemanticIndex {
    fn default() -> Self {
        Self {
            docs: Vec::new(),
            inv: HashMap::new(),
            avgdl: 0.0,
            k1: 1.2,
            b: 0.75,
            mtimes: HashMap::new(),
            built_by: env!("CARGO_PKG_VERSION").to_string(),
            rebased: false,
        }
    }
}

/// Maximum tokens kept per document. Files larger than this contribute
/// their first chunk and emit a warning. Caps a single 1-million-token
/// generated file from blowing up the inverted index.
pub const MAX_TOKENS_PER_DOC: usize = 8_192;

/// Hard ceiling on the number of postings the inverted index will hold.
/// At ~24 bytes per posting that's ~120 MB resident; a small-laptop user
/// should never see the indexer OOM. When hit, we stop adding postings
/// and emit a single warning on stderr.
pub const MAX_TOTAL_POSTINGS: usize = 5_000_000;

mod build;
mod persist;
pub mod search;
mod text;
pub use persist::*;
pub(crate) use search::*;
pub(crate) use text::*;

#[cfg(test)]
include!("semantic_tests.rs");
