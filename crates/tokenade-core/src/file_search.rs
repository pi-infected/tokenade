//! Per-file chunk store, and the language detection built on the way to it.
//!
//! The `file-search` verb this store was built for is gone: `tokenade read
//! --prompt` answers the same question over the same file, on documents this
//! could not open at all, and `read --lines A-B` goes back to a region by its
//! own line numbers. Nothing writes to the store any more. It is kept, and
//! still swept, because it holds text taken from the user's files on machines
//! that ran the old verb: [`evict_stale`] expires it, [`vacuum_db`] reclaims
//! it, and [`purge_containing`] / [`purge_redactable`] are how `scrub-secret`
//! and `redact` reach a secret that was indexed before it was scrubbed.
//! [`language_from_path`] is used by `pack`.
//!
//! ## How
//!
//! 1. Read the file, chunk into ~12-line windows (with 2-line overlap
//!    so a match spanning a chunk boundary still surfaces). Hash the
//!    file content once.
//! 2. If the global DB at `~/.tokenade/file_search.db` already has
//!    chunks for this content-hash → skip indexing (cache hit, <10ms).
//! 3. Otherwise: embed every chunk via `embedding::embed_text`, write
//!    `(content_hash, chunk_id, line_start, line_end, snippet)` rows
//!    + the embedding vector via sqlite-vec's `vec0` table.
//! 4. Embed the query, KNN against `vec0` filtered to this content_hash,
//!    return the top-K rows with `(line_range, snippet, distance)`.
//!
//! ## Scope: GLOBAL by content hash
//!
//! Two different absolute paths with identical bytes share their
//! chunks (e.g. a config file copied between projects). Saves cost on
//! the embed pass. The actual file path the search was issued from is
//! reflected in the response for human readability.
//!
//! ## Schema
//!
//! ```sql
//! CREATE TABLE file_chunks (
//!     content_hash TEXT NOT NULL,
//!     chunk_id     INTEGER NOT NULL,
//!     line_start   INTEGER NOT NULL,
//!     line_end     INTEGER NOT NULL,
//!     snippet      TEXT NOT NULL,
//!     PRIMARY KEY (content_hash, chunk_id)
//! );
//! -- rowid of file_chunks links to rowid of file_chunk_vec.
//! CREATE VIRTUAL TABLE file_chunk_vec USING vec0(embedding float[384]);
//! ```

mod ast;
mod chunking;
mod language;
pub use ast::*;
pub use chunking::*;
pub use language::*;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One hit returned to the caller — a chunk that matched the query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSearchHit {
    pub line_start: usize,
    pub line_end: usize,
    pub snippet: String,
    /// Cosine distance from sqlite-vec. Lower is better; 0.0 = identical
    /// embedding direction. Useful for the caller to set a relevance cut.
    pub distance: f32,
}

/// Compact summary of the search result returned by the MCP tool /
/// CLI command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSearchResult {
    pub file: String,
    pub content_hash: String,
    pub chunk_count: usize,
    pub indexed_now: bool,
    pub query: String,
    pub hits: Vec<FileSearchHit>,
}

#[derive(Debug)]
pub enum FileSearchError {
    Io(std::io::Error),
    Db(rusqlite::Error),
    Embed(crate::embedding::EmbedError),
}

impl std::fmt::Display for FileSearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileSearchError::Io(e) => write!(f, "io: {e}"),
            FileSearchError::Db(e) => write!(f, "db: {e}"),
            FileSearchError::Embed(e) => write!(f, "embed: {e:?}"),
        }
    }
}

impl std::error::Error for FileSearchError {}

impl From<std::io::Error> for FileSearchError {
    fn from(v: std::io::Error) -> Self {
        FileSearchError::Io(v)
    }
}

impl From<rusqlite::Error> for FileSearchError {
    fn from(v: rusqlite::Error) -> Self {
        FileSearchError::Db(v)
    }
}

impl From<crate::embedding::EmbedError> for FileSearchError {
    fn from(v: crate::embedding::EmbedError) -> Self {
        FileSearchError::Embed(v)
    }
}

/// Lines per chunk window. ~12 lines is the sweet spot for a 384-d
/// embedder: short enough to be specific, long enough to carry
/// semantic context. Larger windows dilute relevance; smaller ones
/// over-fragment.
pub const CHUNK_LINES: usize = 12;
/// Overlap between consecutive chunks (so a match straddling a
/// boundary still surfaces).
pub const CHUNK_OVERLAP: usize = 2;
/// Maximum K the caller can request — guards against pulling the
/// entire indexed file back via search.
pub const MAX_K: usize = 50;
/// Cap on brace-balanced snippet expansion. If the enclosing block is
/// larger than this many lines we keep the fixed window instead of
/// swallowing a whole 300-line function. Picked so a typical function
/// fits but a giant one falls back to the window (retro-compat).
pub const EXTRACT_MAX_LINES: usize = 80;

/// Cache TTL — chunks not accessed for this long are eligible for
/// eviction by [`evict_stale`]. Matches the dedup_ref / fold_store
/// horizon so all on-disk caches share a single mental model
/// ("anything older than a week may be gone").
pub const CACHE_TTL_SECS: u64 = 7 * 86_400;

/// DB path. Honours `TOKENADE_FILE_SEARCH_DB` env override (tests).
pub fn db_path() -> PathBuf {
    if let Some(p) = std::env::var_os("TOKENADE_FILE_SEARCH_DB") {
        return PathBuf::from(p);
    }
    crate::platform::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tokenade")
        .join("file_search.db")
}

fn open() -> Result<Connection, FileSearchError> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    crate::sqlite_vec_util::register_sqlite_vec_extension();
    let conn = Connection::open(&path)?;
    crate::sqlite_tuning::tune(&conn);
    // Step 1: bare-minimum schema present on both fresh and pre-L61 DBs.
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS file_chunks (
            content_hash TEXT NOT NULL,
            chunk_id     INTEGER NOT NULL,
            line_start   INTEGER NOT NULL,
            line_end     INTEGER NOT NULL,
            snippet      TEXT NOT NULL,
            PRIMARY KEY (content_hash, chunk_id)
        );
        CREATE INDEX IF NOT EXISTS file_chunks_hash ON file_chunks(content_hash);
        "#,
    )?;
    // Step 2: ALTER adds last_accessed for pre-L61 DBs. Errors
    // swallowed (duplicate column on fresh DBs is benign).
    let _ = conn.execute(
        "ALTER TABLE file_chunks ADD COLUMN last_accessed INTEGER NOT NULL DEFAULT 0",
        [],
    );
    // Step 3: index on the column we just guaranteed exists.
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS file_chunks_last_accessed ON file_chunks(last_accessed)",
        [],
    );
    // Every cached vector belongs to the model that produced it. Comparing a
    // stored vector against a query embedded by a DIFFERENT model is
    // meaningless, and nothing downstream can notice: same dimension, no error,
    // just wrong results. So the model's identity is stored and checked here,
    // and a mismatch wipes the cache.
    //
    // The dim check below is NOT that guard: it caught the 384→256 change by
    // luck, and `potion-code-16M` and `potion-multilingual-128M` are BOTH
    // 256-dim — it would let that swap through silently.
    invalidate_on_model_change(&conn)?;
    // Virtual vector table. Dim is sourced from the current embedder
    // (256 for potion-code-16M after L112, was 384 for E5Small). If an
    // existing table has the wrong dim we drop + recreate so writes
    // succeed. Kept as a cheap belt-and-braces on top of the model check.
    let dim = crate::embedding::DIM;
    let existing_dim: Option<i64> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='file_chunk_vec'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|sql| {
            let i = sql.find("float[")?;
            let rest = &sql[i + "float[".len()..];
            let end = rest.find(']')?;
            rest[..end].trim().parse().ok()
        });
    if existing_dim.map(|d| d as usize) != Some(dim) && existing_dim.is_some() {
        conn.execute("DROP TABLE file_chunk_vec", [])?;
    }
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS file_chunk_vec USING vec0(embedding float[{dim}]);",
    ))?;
    Ok(conn)
}

/// Drop every cached chunk when the embedding model changed.
///
/// `file_chunks` is keyed by CONTENT HASH, so an unchanged file is a cache hit
/// forever — including across a model swap, which is exactly when its vectors
/// stop meaning anything. Wiping is correct rather than wasteful: re-indexing a
/// file is cheap, serving silently wrong search results is not.
fn invalidate_on_model_change(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS embed_model (id INTEGER PRIMARY KEY CHECK (id = 1), fingerprint TEXT NOT NULL);",
    )?;
    let current = crate::embedding::model_fingerprint();
    let stored: Option<String> = conn
        .query_row(
            "SELECT fingerprint FROM embed_model WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .ok();
    if stored.as_deref() == Some(current) {
        return Ok(());
    }
    // Unknown provenance (pre-fingerprint DB) counts as a mismatch: those rows
    // were written by an older model and cannot be trusted either.
    if stored.is_some() || table_exists(conn, "file_chunk_vec") {
        let _ = conn.execute("DELETE FROM file_chunks", []);
        let _ = conn.execute("DROP TABLE IF EXISTS file_chunk_vec", []);
        crate::debug_log::log(
            "embedding",
            "file_search_cache_invalidated",
            serde_json::json!({ "was": stored, "now": current }),
        );
    }
    conn.execute(
        "INSERT INTO embed_model (id, fingerprint) VALUES (1, ?1)
         ON CONFLICT(id) DO UPDATE SET fingerprint = excluded.fingerprint",
        [current],
    )?;
    Ok(())
}

fn table_exists(conn: &rusqlite::Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type IN ('table','view') AND name = ?1",
        [name],
        |_| Ok(()),
    )
    .is_ok()
}

/// Compute the content hash for a file — same 12-char SHA-256 prefix
/// the dedup_ref module uses, so an indexed file's hash matches the
/// hash that appears in `§ref:HASH§` markers.
/// **DECLARED RESERVE**, like this whole module: nothing indexes any more, so
/// nothing has a hash to match against a `§ref:HASH§`.
#[allow(dead_code)]
pub(crate) fn hash_file(content: &str) -> String {
    crate::dedup_ref::hash_content(content.as_bytes())
}

/// Whether the DB already has chunks for this content hash.
pub fn is_indexed(content_hash: &str) -> bool {
    let Ok(conn) = open() else {
        return false;
    };
    conn.query_row(
        "SELECT 1 FROM file_chunks WHERE content_hash=?1 LIMIT 1",
        params![content_hash],
        |_| Ok(()),
    )
    .is_ok()
}

/// Drop chunks whose `last_accessed` is older than [`CACHE_TTL_SECS`].
/// Returns the number of chunk rows removed. The corresponding vec
/// rows are deleted in the same transaction. Called by the maintenance
/// sweeps, since nothing writes to this store any more.
pub fn evict_stale() -> Result<usize, FileSearchError> {
    let mut conn = open()?;
    let cutoff = crate::platform::now_unix_secs().saturating_sub(CACHE_TTL_SECS) as i64;
    let tx = conn.transaction()?;
    let stale: Vec<i64> = {
        let mut s = tx.prepare("SELECT rowid FROM file_chunks WHERE last_accessed < ?1")?;
        let rows = s.query_map(params![cutoff], |r| r.get::<_, i64>(0))?;
        rows.filter_map(|r| r.ok()).collect()
    };
    for rid in &stale {
        let _ = tx.execute("DELETE FROM file_chunk_vec WHERE rowid=?1", params![rid]);
    }
    let removed = tx.execute(
        "DELETE FROM file_chunks WHERE last_accessed < ?1",
        params![cutoff],
    )?;
    tx.commit()?;
    Ok(removed)
}

/// Forget the index for a specific content hash. Useful when callers
/// know a file was rewritten and want to evict the stale chunks.
pub fn forget(content_hash: &str) -> Result<usize, FileSearchError> {
    let mut conn = open()?;
    let tx = conn.transaction()?;
    // Delete the vec rows first (they FK-implicitly link via rowid).
    let rowids: Vec<i64> = {
        let mut s = tx.prepare("SELECT rowid FROM file_chunks WHERE content_hash=?1")?;
        let it = s.query_map(params![content_hash], |r| r.get::<_, i64>(0))?;
        it.filter_map(|r| r.ok()).collect()
    };
    for rid in &rowids {
        let _ = tx.execute("DELETE FROM file_chunk_vec WHERE rowid=?1", params![rid]);
    }
    let removed = tx.execute(
        "DELETE FROM file_chunks WHERE content_hash=?1",
        params![content_hash],
    )?;
    tx.commit()?;
    Ok(removed)
}

/// The scan both purges below run.
const SELECT_CHUNKS: &str = "SELECT content_hash, snippet FROM file_chunks";

/// VACUUM the chunk store. Returns `(before, after)` file sizes.
///
/// A delete here is not a removal until the file is rewritten: SQLite keeps
/// the freed pages in its own freelist and the text stays readable in them.
/// Measured on the development machine 2026-08-27, on a store holding ONE live
/// chunk: 136 of 407 pages free, and `strings` reads source code out of them
/// from chunks deleted weeks ago. The session-start sweep would reclaim that,
/// but only past a 4 MiB waste floor this store had not reached.
pub fn vacuum_db() -> (u64, u64) {
    crate::store_scrub::vacuum_store(&db_path(), open)
}

/// Drop every indexed file whose stored text carries `needle`, for
/// `scrub-secret`. Returns `(matched, removed)` in FILES; a dry run leaves the
/// rows in place and reports what it would take.
///
/// Deletion, never substitution: the embedding beside each chunk was computed
/// from the text, so patching the text alone would leave a vector describing
/// words the snippet no longer contains. [`forget`] is the unit that drops
/// both, and the file re-indexes from source on the next question.
pub fn purge_containing(needle: &str, dry_run: bool) -> (usize, usize) {
    let Ok(conn) = open() else {
        return (0, 0);
    };
    let victims = crate::store_scrub::victim_keys(&conn, SELECT_CHUNKS, |snippet| {
        crate::store_scrub::contains_bytes(snippet, needle)
    })
    .unwrap_or_default();
    drop(conn);
    if dry_run {
        return (victims.len(), 0);
    }
    let removed = forget_all(&victims);
    (victims.len(), removed)
}

/// Drop every indexed file whose stored text would be changed by the redact
/// patterns, for `scrub-cache`. Returns the number of FILES removed (or, in a
/// dry run, matched).
///
/// The needle-free sibling of [`purge_containing`]: it does not know WHICH
/// secret leaked, only that the patterns recognise one. The last versions that
/// wrote chunks redacted them first, so this only finds the ones written
/// before that.
pub fn purge_redactable(dry_run: bool) -> usize {
    // See `dedup_ref::scrub_via_redact_inner`: opening creates the file, and
    // a dry run must not.
    if dry_run && !db_path().exists() {
        return 0;
    }
    let Ok(conn) = open() else {
        return 0;
    };
    let victims =
        crate::store_scrub::victim_keys(&conn, SELECT_CHUNKS, crate::store_scrub::is_redactable)
            .unwrap_or_default();
    drop(conn);
    if dry_run {
        return victims.len();
    }
    forget_all(&victims)
}

/// [`forget`] each hash, VACUUMing once if anything went. Returns the number of
/// hashes actually removed.
fn forget_all(hashes: &[String]) -> usize {
    let mut removed = 0usize;
    for h in hashes {
        if forget(h).unwrap_or(0) > 0 {
            removed += 1;
        }
    }
    if removed > 0 {
        // Or the text stays readable in the file's free pages and the tool
        // reports "scrubbed" about a string `grep` still finds.
        let _ = vacuum_db();
    }
    removed
}

#[cfg(test)]
include!("file_search_tests.rs");
