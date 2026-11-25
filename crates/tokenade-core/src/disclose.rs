//! Progressive disclosure for large tool outputs.
//!
//! Above [`DEFAULT_THRESHOLD_TOKENS`] (5 000 tokens), returning the
//! full content to the agent pollutes its context for the rest of
//! the conversation. The trade-off flips: prompt-cached follow-up
//! reads cost ~10 % of fresh input, while every token in the agent's
//! context is paid for on every subsequent turn.
//!
//! This module implements the universal "stash + summary" pattern:
//!
//! ```text
//!  any tool output → maybe_stash(text, hint):
//!     if tokens(text) < threshold → return text unchanged
//!     else:
//!         hash    = sha256(text)[..12]
//!         ext     = compose_extension(text, hint)   ← .api.rest.json, .graphql.json, .html, …
//!         path    = ~/.cache/tokenade/stash/<hash>.<ext>
//!         (write text to path if not already present)
//!         summary = head + tail + "[stashed: <path> · <N> tokens · use grep/jq/Read offset]"
//!         return summary
//! ```
//!
//! The agent then drills in via standard tools (grep, jq, Read with
//! offset/limit, css selectors via curl pipes, etc.) on the stashed
//! file. The probabilistic extension is the parsing hint — `.html`
//! → CSS selectors / pup; `.json` → jq; `.api.rest.json` → likely
//! REST envelope, expect `data`/`items`/`results`; `.graphql.json` →
//! `data` + `errors` keys at top level; `.txt` → plain grep.
//!
//! ## Source hints (caller-supplied context)
//!
//! [`SourceHint`] tells `maybe_stash` *where* the bytes came from so
//! the composite extension can encode the trio (origin, format,
//! protocol). Pure content sniffing can't always answer "is this a
//! REST API response or just a JSON config file?" — the origin is
//! the disambiguator.
//!
//! ## Cache
//!
//! Same content (same hash) is stashed once per 7-day TTL window.
//! Eviction shares [`crate::file_search::evict_stale`]-style
//! semantics: cold-tier rows dropped, hot ones survive on access.

mod kind;
pub use kind::*;
mod binary;
mod eviction;
pub(crate) mod scoped;
mod search;
pub use binary::*;
pub use eviction::*;
pub use scoped::*;
pub use search::*;
use std::path::{Path, PathBuf};

mod gate;
mod help_index;
mod ranking;
mod store;
mod summary;
pub use gate::*;
pub use help_index::looks_like_a_help_text;
pub use ranking::*;
pub use store::*;
pub use summary::*;

pub use tk_base::delivery::DEFAULT_THRESHOLD_TOKENS;

/// Cache TTL — stashed files older than this are eligible for
/// eviction. Matches the dedup/file_search/fold_store horizon so all
/// on-disk caches share a single mental model ("anything older than
/// a week may be gone").
pub const CACHE_TTL_SECS: u64 = 7 * 86_400;

/// Head + tail line counts for the summary block. Picked to fit
/// comfortably under ~250 tokens of preview while still showing the
/// shape of the data.
pub const SUMMARY_HEAD_LINES: usize = 20;
pub const SUMMARY_TAIL_LINES: usize = 10;

/// Hard ceiling on the BODY of a disclosure summary, in characters.
///
/// The summary was bounded by LINES and never by bytes, which
/// makes the whole backstop a no-op on any single-line document. Measured
/// on a real npm registry response (`registry.npmjs.org/react`, minified
/// JSON, 6.8 MB on one line): `total_lines` was under the 30-line
/// head+tail cutoff, so the "short document" branch returned **the entire
/// 6.8 MB verbatim** — ~1.4M tokens — under a footer that said "folded
/// summary (omits middle lines)". The banner was the only thing that
/// fired; nothing was folded and nothing was omitted.
///
/// This is not a JSON problem. Minified JS/CSS, a single-line log record,
/// a base64 blob and a one-line SQL dump defeat a line-based fold the same
/// way. A byte bound is the only one that holds for all of them.
///
/// 4 000 chars ≈ 1k tokens. This is a PREVIEW attached to a stash
/// pointer, not the delivery: the agent is meant to ask the stash
/// (`expand-ref --prompt`) or grep the original, and every char spent
/// here is spent on a document we just decided was too big to serve.
/// The first draft used 12 000 (~3k tokens) and the PO was right that it
/// was too generous for something the agent should not be reading in
/// full anyway.
pub const SUMMARY_MAX_BODY_CHARS: usize = 4_000;

/// Hard ceiling on a SINGLE line inside a summary body.
///
/// Same round-38 lesson one level down: bounding only the total lets one
/// pathological line consume the entire budget — a 6.8 MB minified JSON
/// clamped to the body cap produced exactly two lines, neither readable
/// and neither leaving room for anything else. A preview is useful when
/// it shows the SHAPE of the document, which needs several lines.
pub const SUMMARY_MAX_LINE_CHARS: usize = 400;

/// Hard cap on the stashed file size — beyond this we save head +
/// tail bytes only and warn the caller. Protects the cache from a
/// single multi-GB curl that would fill the disk.
pub const STASH_MAX_BYTES: usize = 50 * 1024 * 1024; // 50 MB

/// Where did the bytes come from? Drives the composite extension
/// suffix so the model gets a richer parsing hint than just the
/// content kind.
#[derive(Debug, Clone)]
pub enum SourceHint {
    /// Read of an on-disk file. The extension on `path` (if any)
    /// wins over content sniffing — files have authoritative
    /// extensions, we shouldn't second-guess them.
    File(PathBuf),
    /// HTTP/HTTPS response — curl, wget, httpie, xh, WebFetch,
    /// Playwright network requests, Python requests, etc.
    ///
    /// `content_type` is the raw HTTP `Content-Type` header
    /// (potentially with `; charset=…` parameters) when the
    /// interceptor was able to capture it. When present it is
    /// AUTHORITATIVE over content-sniffing — the server told us
    /// what it sent. When `None`, falls back to the shape detector.
    Web { content_type: Option<String> },
    /// Known GraphQL response (caller-asserted; e.g. the body of a
    /// POST to a `/graphql` endpoint). Skips the auto-detect since
    /// GraphQL bodies can be plain JSON of arbitrary shape.
    GraphQl,
    /// Output of a shell command (Bash interceptor `wrap`). Content
    /// kind is detected from the bytes.
    Shell,
    /// MCP tool response. Content kind sniffed from bytes.
    Mcp,
    /// Caller has no context — fall back to pure content sniffing.
    Unknown,
}

mod read_target;
pub use read_target::{earns_the_read_threshold, reads_only_source_files, single_file_read_target};

impl SourceHint {
    /// Shortcut for `Web { content_type: None }` — call sites that
    /// don't have access to the HTTP header.
    pub fn web() -> Self {
        SourceHint::Web { content_type: None }
    }
    /// Shortcut for `Web { content_type: Some(ct) }`.
    pub fn web_with_content_type(ct: impl Into<String>) -> Self {
        SourceHint::Web {
            content_type: Some(ct.into()),
        }
    }
}

// The delivery threshold lives in `tk_base::delivery` so every crate — the
// format compactors in `tk-formats` included — asks the same question.
pub use tk_base::delivery::threshold_tokens;

/// Disclosure threshold for a DELIBERATE Read of an on-disk file —
/// deliberately HIGHER than [`DEFAULT_THRESHOLD_TOKENS`] (env override
/// `TOKENADE_READ_DISCLOSE_THRESHOLD`).
///
/// ## Why a Read deserves a higher bar than an incidental tool output
///
/// `maybe_stash`'s 5 000-token cutoff is tuned for *incidental* output:
/// a `curl` body, a command's stdout, an MCP response. There the agent
/// did NOT ask for the bytes — it asked a question and got a wall of
/// data back; stashing the bulk and showing a summary is a clear win.
///
/// A `Read` is the opposite: the agent CHOSE to open this file and
/// almost always wants the whole thing (if it only wanted a slice it
/// would pass offset/limit, or grep). Stashing a deliberately-read file
/// at 5 k forces the agent to reconstitute it via repeated
/// `search_stash`/`Read offset` round-trips — the THOL `doc-digest`
/// benchmark caught exactly this: an 11 k-token handbook stashed →
/// 5× `search_stash` → +20 % session cost vs reading it once.
///
/// The break-even favours serving the file inline: every follow-up
/// reference to a file already in context is prompt-cached (~10 % of
/// fresh input), whereas each `search_stash` round-trip re-pays a whole
/// turn. So the Read cutoff sits where "the agent should really have
/// paginated instead" begins — ~25 k tokens (~100 KB). Above that, the
/// stash + summary discipline kicks back in.
pub const READ_DEFAULT_THRESHOLD_TOKENS: usize = 25_000;

/// Resolve the deliberate-Read disclosure threshold. Reads
/// `TOKENADE_READ_DISCLOSE_THRESHOLD` on every call (same live-knob
/// semantics as [`threshold_tokens`]); empty/unparseable falls back to
/// [`READ_DEFAULT_THRESHOLD_TOKENS`]. Independent of the general
/// `TOKENADE_DISCLOSE_THRESHOLD` — operators who lowered the incidental
/// cutoff don't accidentally re-introduce the doc-digest anti-pattern.
pub fn read_threshold_tokens() -> usize {
    if let Ok(v) = std::env::var("TOKENADE_READ_DISCLOSE_THRESHOLD") {
        if let Ok(n) = v.parse::<usize>() {
            return n;
        }
    }
    (READ_DEFAULT_THRESHOLD_TOKENS as f64 * tk_base::delivery::pressure_factor()) as usize
}

/// Stash root directory. Resolution order (first wins):
///
/// 1. `$TOKENADE_STASH_DIR` if set — dedicated knob, doesn't touch
///    any other cache. **Tests use this**: setting only this env var
///    isolates the stash without redirecting the embedder's model
///    cache (the L100 20 GB tmpfs leak source).
/// 2. `$XDG_CACHE_HOME/tokenade/stash/` (Linux default).
/// 3. `$HOME/Library/Caches/tokenade/stash/` (macOS default).
/// 4. `%LOCALAPPDATA%\tokenade\stash\` (Windows default).
/// 5. Fallback to `./tokenade-stash/` in the current dir.
///
/// **Why a dedicated env var?** The legacy code overrode
/// `XDG_CACHE_HOME` in tests to isolate the stash, which also
/// redirected fastembed's model cache and caused 53 copies of a
/// 465 MB model to land in tmpfs. With this var the two concerns
/// are split.
pub fn stash_dir() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(p) = TEST_STASH_DIR.with(|c| c.borrow().clone()) {
            return p;
        }
        crate::index_db::per_test_store("stash")
    }
    #[cfg(not(test))]
    if let Some(p) = std::env::var_os("TOKENADE_STASH_DIR") {
        return PathBuf::from(p);
    }
    #[cfg(not(test))]
    crate::platform::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tokenade")
        .join("stash")
}

// A unit test's stash, set for ITS thread only. `TOKENADE_STASH_DIR` is
// process-wide: a test that pointed it at a temp dir and deleted the dir took
// every parallel test's stash with it — `what_the_hard_cap_samples_away_…`
// read a footer naming a file that no longer existed (2026-10-03). Without an
// override, a test gets a store of its own (`index_db::per_test_store`), and
// never the user's `~/.cache/tokenade/stash`.
#[cfg(test)]
thread_local! {
    static TEST_STASH_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_test_stash_dir(dir: Option<PathBuf>) {
    TEST_STASH_DIR.with(|c| *c.borrow_mut() = dir);
}

/// Record that THIS session produced `hash`, in the per-session state
/// directory that already exists and is already swept.
///
/// The post-compaction digest offers "raw bytes that tokenade folded earlier"
/// and picked its refs by MTIME inside a six-hour window. The stash is one
/// directory shared by every agent on the machine, so half the time that
/// window holds another session's work: measured over this machine's ledger,
/// **1 828 of 3 676 moments (50 %)** had more than one session active in the
/// preceding six hours, and one day held fifteen. An offer that points at
/// bytes this agent never saw is worse than no offer — it reads as its own
/// lost detail.
///
/// `gain`'s own session module already learned this lesson for the statusline
/// ("time-window scoping counted CONCURRENT sessions on the same machine,
/// which is how a fresh session showed millions"); the digest never got it.
///
/// Best-effort and bounded: a missing or unwritable file simply means the
/// digest offers nothing, which is the honest degradation.
fn note_session_ref(hash: &str) {
    let Some(sess) = crate::session::session_key() else {
        return;
    };
    let sess: String = sess
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    if sess.is_empty() || hash.is_empty() {
        return;
    }
    let dir = crate::session::sessions_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("{sess}.refs"));
    // Bounded: ~13 bytes a line, so this holds a few thousand refs and then
    // keeps the newest half. A session state file must not grow without end.
    const MAX_BYTES: u64 = 64 * 1024;
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_BYTES {
        if let Ok(body) = std::fs::read_to_string(&path) {
            let keep: Vec<&str> = body.lines().skip(body.lines().count() / 2).collect();
            let _ = std::fs::write(&path, format!("{}\n", keep.join("\n")));
        }
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{hash}");
    }
}

/// The stash hashes THIS session produced, newest last. Empty when nothing
/// was recorded — which is the correct answer for a session that folded
/// nothing, and the safe one for a state file we could not read.
/// **DECLARED RESERVE.** No production caller: retrieval is
/// done by named hash, never by « everything this session folded ».
/// Kept because it is the only view that would make an argument-less
/// `expand-ref` possible.
#[allow(dead_code)]
pub(crate) fn session_ref_hashes(session: &str) -> Vec<String> {
    session_ref_hashes_in(&crate::session::sessions_dir(), session)
}

/// The directory-injected form. Split out so a test can ask the question
/// without setting `HOME`, which is process-wide: the first version of the
/// guard for this took `ENV_LOCK` and still broke an unrelated index test
/// running in parallel, because that one reads `HOME` without the lock.
pub fn session_ref_hashes_in(dir: &std::path::Path, session: &str) -> Vec<String> {
    let sess: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    if sess.is_empty() {
        return Vec::new();
    }
    let path = dir.join(format!("{sess}.refs"));
    std::fs::read_to_string(path)
        .map(|b| {
            b.lines()
                .map(str::trim)
                .filter(|l| l.len() >= 8)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn stash_path(hash: &str, ext: &str) -> PathBuf {
    stash_dir().join(format!("{hash}.{ext}"))
}

/// Look up a stash file by hash (any extension). Returns `Some((content,
/// extension))` when a file matching `<hash>.*` exists in the stash dir,
/// `None` otherwise. The content is re-redacted on read as a belt-and-
/// suspenders measure (legacy stashes may pre-date the #475 redact pass).
///
/// #704 — `expand_ref` falls back to this when `dedup_ref::lookup` misses,
/// so the global "use `expand_ref hash=…` to recover stashed bytes" rule
/// works for content that was disclose-stashed AS WELL AS content that
/// only lives in dedup.db. Without this, agents got "hash not found" on
/// any disclose-summary-style ref despite following the documented path.
pub fn read_stash_by_hash(hash: &str) -> Option<(String, String)> {
    let (text, ext) = read_stash_raw(hash)?;
    // Re-redact text on read (defense in depth, mirrors #488 for
    // dedup.db). Binary stashes (compose_extension routes binaries
    // through `stash_as_binary`, ext like "png", "zip", …) go
    // through lossy UTF-8 decode — the redact pass is a no-op on
    // bytes it doesn't recognise, so this is safe either way.
    Some((crate::redact::redact(&text), ext))
}

/// Un-redacted stash read. PRIVATE, and it stays private: every public path
/// out of this module redacts before returning, so a caller cannot obtain raw
/// stash bytes by accident. Exists only so [`grep_stash_by_hash`] can filter
/// before paying for redaction.
fn read_stash_raw(hash: &str) -> Option<(String, String)> {
    if hash.is_empty() {
        return None;
    }
    let dir = stash_dir();
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some((stem, ext)) = name.split_once('.') else {
            continue;
        };
        if stem != hash {
            continue;
        }
        let bytes = std::fs::read(entry.path()).ok()?;
        return Some((String::from_utf8_lossy(&bytes).to_string(), ext.to_string()));
    }
    None
}

/// Grep a stashed blob by hash, returning `(matching lines as (1-based line
/// number, redacted text), total line count, estimated tokens of the WHOLE
/// blob)`. The last one lets the caller credit the recovery identically to the
/// unfiltered path, which never sees the blob it just avoided materialising.
///
/// Filters BEFORE redacting, and that ordering is the whole point. Redaction
/// runs at ~0.47 s/MB in release, so redacting first made
/// `expand-ref <hash> --grep` pay a ~12 s full-blob pass on a 26 MB stash to
/// return ~300 lines — the recovery path the product advertises everywhere was
/// slow enough that an agent would rather re-read the file it was meant to
/// avoid. Only matching lines are redacted, because only they are emitted.
///
/// "Raw" here means "as stored": stashes are redacted at WRITE time
/// ([`stash_for_reference`], [`maybe_stash`]), so a secret is already gone from
/// the file and cannot be matched — a needle that is itself a secret correctly
/// finds nothing. The read-side pass this skips is the defense-in-depth one for
/// legacy stashes written before that rule, which is why matching lines are
/// still redacted individually before they leave.
/// Max characters of a single matching line returned by a grep. A minified JS
/// bundle, a one-line JSON dump or a CSV with no newlines is ONE line that IS
/// the whole file — an unbounded "matching line" would re-inflate exactly what
/// the stash exists to avoid. The match is centred in the window so the needle
/// stays visible.
pub const GREP_LINE_MAX_CHARS: usize = 400;

/// Max matching lines returned per pattern. A frequent needle would otherwise
/// return thousands of lines.
pub const GREP_MAX_HITS: usize = 40;

const STASH_MAX_DIR_BYTES: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB

/// Testable inner of [`evict_to_size_cap`]. Takes the stash directory
/// path + the cap explicitly so tests can use a tempdir with a tiny
/// cap and synthetic mtimes via `set_modified`. Reads the dir once
/// (collect metadata up front) so concurrent writes mid-sweep don't
/// double-count.
pub(crate) fn evict_to_size_cap_inner(dir: &Path, cap: u64) -> std::io::Result<(usize, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok((0, 0));
    };
    // Snapshot (path, len, mtime) once. Skip non-files, unreadable
    // metadata, and entries with no mtime. mtime is the LRU key —
    // `ensure_stashed` already bumps mtime on access (see line ~718),
    // so "oldest mtime" really means "least-recently-touched".
    let mut snapshot: Vec<(PathBuf, u64, std::time::SystemTime)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let meta = path.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let mtime = meta.modified().ok()?;
            Some((path, meta.len(), mtime))
        })
        .collect();
    let total: u64 = snapshot.iter().map(|(_, len, _)| *len).sum();
    if total <= cap {
        return Ok((0, 0));
    }
    // Replaceable entries go FIRST, then oldest-first within each group.
    //
    // The TTL sweep already spares sole copies; a cap that ignored the same
    // distinction would just delete them on a different schedule — and pick
    // them ahead of a replaceable blob ten times their size purely because
    // their mtime was older. The cap stays hard (a sole copy IS evicted once
    // nothing replaceable is left); it simply stops being the first thing
    // sacrificed.
    snapshot.sort_by_key(|(path, _, mtime)| {
        let stem = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.split_once('.').map(|(a, _)| a).unwrap_or(n).to_string())
            .unwrap_or_default();
        (is_sole_copy(&stem), *mtime)
    });
    let mut over = total - cap;
    let mut removed = 0usize;
    let mut bytes_freed = 0u64;
    for (path, len, _) in snapshot {
        if over == 0 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
            bytes_freed += len;
            // saturating_sub: if len > over (last evicted file overshoots
            // the cap by a bit), clamp to 0 so the loop exits cleanly
            // on the next iter.
            over = over.saturating_sub(len);
        }
    }
    Ok((removed, bytes_freed))
}

/// Build the summary block emitted in place of the full content.
///
/// Dispatches to a format-specific smart summary when one fits:
///   * `*.json` / `*.ndjson` / `api.rest.json` / `graphql.json` →
///     [`crate::json::schema_fold`] (if the body is a homogeneous
///     array) else falls back to head/tail of pretty-printed JSON.
///   * `md` / `html` → [`crate::markdown::outline`] — emit just the
///     heading hierarchy + per-section preview.
///   * everything else → head + tail line slices.
/// Clamp a summary body to [`SUMMARY_MAX_BODY_CHARS`], keeping both ends.
///
/// Every `build_summary` branch is a *preview*, so the invariant it owes the
/// caller is a bounded one. Line-based folds cannot provide it (see the
/// constant's note); this is the backstop's backstop, applied to whichever
/// branch produced the body.
///
/// Keeps 2/3 head and 1/3 tail — the same bias as the line fold, since the
/// head of a document identifies it and the tail is where totals and errors
/// live. Splits on `char_indices` so it can never cut a UTF-8 sequence.
/// Characters the rescued failure block may occupy inside the preview. The
/// rest of the budget is context, and context is what a preview may lose.
const RESCUE_BUDGET_CHARS: usize = SUMMARY_MAX_BODY_CHARS / 2;

/// How many ORIGINAL lines one line of an already-collapsed view stands for.
///
/// Three shapes, and the third is the one that made the arithmetic wrong:
///   * `… [N lines elided by tokenade] …` — a fold marker. It is not itself a
///     line of the original; it STANDS for the N that are gone.
///   * a line ending `(×N)` — a collapsed run, and the line is one of the N.
///   * a `[tokenade…]`/`[disclose…]` banner — ours, worth 0 of the original.
///   * anything else — itself, 1.
fn original_weight(line: &str) -> usize {
    let t = line.trim();
    // The marker's spelling lives in `crate::elision`, with the writing.
    // It was spelled here AND in `filter::render_kept`: the day one of them
    // changes a word, the other stops counting, silently.
    if let Some(n) = crate::elision::weight_of(t) {
        return n;
    }
    if t.starts_with("[tokenade") || t.starts_with("[disclose") {
        return 0;
    }
    t.strip_suffix(')')
        .and_then(|l| l.rsplit_once("(×"))
        .and_then(|(_, n)| n.parse::<usize>().ok())
        .filter(|n| *n > 1)
        .unwrap_or(1)
}

/// Short hint string telling the agent which tool fits the stashed
/// extension. Helps prevent "Read full file" reflex on a 50k stashed
/// JSON when `jq` would do.
fn parsing_hint_for(ext: &str) -> &'static str {
    if ext.ends_with("json") || ext == "ndjson" {
        "use jq for structured queries (e.g. `jq '.items[] | select(.x>0)'`)"
    } else if ext == "graphql.json" {
        "GraphQL envelope: `jq '.data'` for the payload, `.errors` for failures"
    } else if ext == "html" {
        "use grep with class/id patterns, or `xmllint --html --xpath` for structured drill-down"
    } else if ext == "xml" || ext == "api.rest.xml" {
        "use `xmllint --xpath` or `xq` for structured queries"
    } else if ext == "csv" || ext == "tsv" || ext == "api.rest.csv" {
        "use awk / cut / `csvkit` for column slicing"
    } else if ext == "bin" {
        "binary blob — use `file` to inspect, or `xxd | head` to peek"
    } else if ext.ends_with(".log") || ext == "log" {
        "use grep + sort | uniq -c for patterns, head/tail/sed for ranges"
    } else {
        "use grep / Read with offset/limit for specific sections"
    }
}

/// Split `text` into a head and tail of at most `half_cap` bytes each, for the
/// stash-size truncation. A fixed byte cap can fall mid-codepoint on non-Latin
/// text (CJK is 3 bytes/char, emoji 4) and slicing a `&str` there panics — so
/// the head is backed off and the tail advanced to the nearest char boundary.
/// This keeps a large non-ASCII blob stashable instead of crashing the stash.
fn head_tail_at_cap(text: &str, half_cap: usize) -> (&str, &str) {
    let head = crate::caps::truncate_at_char_boundary(text, half_cap);
    let mut tail_start = text.len().saturating_sub(half_cap);
    while tail_start < text.len() && !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    (head, &text[tail_start..])
}

#[cfg(test)]
include!("disclose_tests.rs");
