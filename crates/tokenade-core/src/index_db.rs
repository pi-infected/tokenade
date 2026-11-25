//! Consolidated SQLite store for per-project symbol/import-graph
//! indexes.
//!
//! Replaces the per-project `<root>/.tokenade/index.json` file with a
//! single `~/.tokenade/index.db` keyed by project path. Same shape as
//! `semantic_db` (Phase 1: blob storage) — the in-memory `Index`
//! struct is unchanged; only its on-disk layout moves to a global
//! SQLite.
//!
//! Why: previously every project carried a 1–10 MB JSON sidecar.
//! Stale projects accumulated them indefinitely and `tokenade
//! semantic-status` had no visibility into them. The new DB:
//!   * gives `tokenade index-status` a single source of truth
//!   * removes per-project disk noise (one of the four artifacts the
//!     2026-05-29 sweep flagged)
//!   * sets up Phase 2 (relational schema → cross-project symbol
//!     queries server-side)
//!
//! ## Schema
//!
//! ```sql
//! CREATE TABLE project_indexes (
//!     project_path  TEXT PRIMARY KEY,
//!     blob          BLOB NOT NULL,   -- JSON-encoded `Index`
//!     last_updated  INTEGER NOT NULL,
//!     size_bytes    INTEGER NOT NULL,
//!     file_count    INTEGER NOT NULL,
//!     symbol_count  INTEGER NOT NULL
//! );
//! ```

use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

/// Path to the global index DB. Honours `TOKENADE_INDEX_DB` env
/// override (used by tests for isolation).
pub fn db_path() -> PathBuf {
    if let Some(p) = std::env::var_os("TOKENADE_INDEX_DB") {
        return PathBuf::from(p);
    }
    #[cfg(test)]
    return per_test_store("index.db");
    #[cfg(not(test))]
    crate::platform::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tokenade")
        .join("index.db")
}

/// **A unit test never shares a store, nor reaches the user's.** Without an
/// explicit variable, `db_path` pointed every test at `~/.tokenade/`: the
/// developer's real database, opened by every parallel test at once. Two
/// failures came of it — `index db: disk I/O error` on the Windows release
/// build (2026-10-03, `incremental_rebuild_drops_deleted_files`) and a test
/// reading the path a sibling had just pointed under a FILE. libtest runs
/// each test on its own thread, so a file per thread is a file per test.
#[cfg(test)]
pub(crate) fn per_test_store(name: &str) -> PathBuf {
    let thread = format!("{:?}", std::thread::current().id());
    let thread: String = thread.chars().filter(char::is_ascii_digit).collect();
    std::env::temp_dir().join(format!(
        "tokenade-test-{}-{thread}-{name}",
        std::process::id()
    ))
}

/// Open the DB and ensure schema. Best-effort connection — failures
/// (no home, locked fs) bubble up as `Err`.
pub fn open() -> Result<Connection, rusqlite::Error> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(&path)?;
    crate::sqlite_tuning::tune(&conn);
    install_schema(&conn)?;
    Ok(conn)
}

/// The table, in ONE place.
///
/// It was in two: `open` created it and the test helper created its own copy,
/// so the moment production grew a `calls_blob` column every test that saved a
/// row failed with "table project_indexes has no column named calls_blob".
/// A schema with two spellings has two futures — the same rule this codebase
/// applies to formatters and predicates.
fn install_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS project_indexes (
            project_path TEXT PRIMARY KEY,
            blob         BLOB NOT NULL,
            last_updated INTEGER NOT NULL,
            size_bytes   INTEGER NOT NULL,
            file_count   INTEGER NOT NULL DEFAULT 0,
            symbol_count INTEGER NOT NULL DEFAULT 0,
            -- The call graph, stored apart from `blob` because most commands
            -- never read it. Measured on a 3 980-file
            -- repository it is 39.7 MB of a 51.0 MB index and 63.7 ms of the
            -- 106 ms spent deserialising it, and 84 % of code-intel
            -- invocations (semantic, map, skeleton) touch none of it.
            calls_blob   BLOB
        );
        CREATE INDEX IF NOT EXISTS idx_project_indexes_updated
            ON project_indexes(last_updated);
        "#,
    )?;
    // Databases created before the split have no `calls_blob`. `ALTER TABLE`
    // is the migration; the error when it already exists is the normal case.
    let _ = conn.execute("ALTER TABLE project_indexes ADD COLUMN calls_blob BLOB", []);
    Ok(())
}

/// Store the project's index blob, replacing any prior entry.
pub fn save_blob(
    conn: &Connection,
    project_path: &Path,
    blob: &[u8],
    calls_blob: &[u8],
    file_count: usize,
    symbol_count: usize,
) -> Result<(), rusqlite::Error> {
    let key = project_path.display().to_string();
    // #734, applied where it was missing — see `semantic_db::save_blob`.
    // Measured 2026-09-03: 85 of 180 rows here were test tempdirs, 47 %.
    if crate::stores::write_would_leak_into_real_store(&key, conn.path()) {
        return Ok(());
    }
    let now = crate::platform::now_unix_secs();
    // `size_bytes` stays the FULL footprint, both halves — it is what the
    // dashboard and the disk GC read, and splitting the storage must not make
    // an index look smaller than the space it takes.
    let total = (blob.len() + calls_blob.len()) as i64;
    conn.execute(
        "INSERT INTO project_indexes(project_path, blob, calls_blob, last_updated, size_bytes, file_count, symbol_count)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(project_path) DO UPDATE SET
             blob=?2, calls_blob=?3, last_updated=?4, size_bytes=?5, file_count=?6, symbol_count=?7",
        params![key, blob, calls_blob, now as i64, total, file_count as i64, symbol_count as i64],
    )?;
    Ok(())
}

/// Mark this project as USED, without rewriting its index.
///
/// `last_updated` is what every eviction pass orders by — the 60-day cold TTL
/// and the size cap both read it as "when was this project last touched". It
/// used to mean that by accident, because every command re-saved the whole
/// index. Since the save is skipped when nothing changed
/// it means "last CHANGED", and a repository queried
/// daily but not edited ages exactly like an abandoned one: dropped at 60
/// days, and evicted before a project edited once last week whenever the store
/// hits its byte budget.
///
/// One integer, no blob: the 74 ms save stays skipped.
pub fn touch(conn: &Connection, project_path: &Path) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE project_indexes SET last_updated=?1 WHERE project_path=?2",
        params![
            crate::platform::now_unix_secs() as i64,
            project_path.display().to_string()
        ],
    )?;
    Ok(())
}

/// The call graph half, fetched only by the commands that read it.
pub fn load_calls_blob(
    conn: &Connection,
    project_path: &Path,
) -> Result<Option<Vec<u8>>, rusqlite::Error> {
    let key = project_path.display().to_string();
    let mut stmt = conn.prepare("SELECT calls_blob FROM project_indexes WHERE project_path=?1")?;
    let mut rows = stmt.query(params![key])?;
    match rows.next()? {
        Some(row) => Ok(row.get::<_, Option<Vec<u8>>>(0)?),
        None => Ok(None),
    }
}

/// Load the project's index blob.
pub fn load_blob(
    conn: &Connection,
    project_path: &Path,
) -> Result<Option<Vec<u8>>, rusqlite::Error> {
    Ok(load_blob_with_stamp(conn, project_path)?.map(|(b, _)| b))
}

/// As [`load_blob`], plus the row's `last_updated`.
///
/// One statement, because the timestamp is NOT cheap to fetch on its own:
/// SQLite stores columns in declaration order and `blob` sits before
/// `last_updated`, so a `SELECT last_updated` walks the blob's overflow pages
/// to reach it. Measured on a 51 MB index, a separate read plus an
/// unconditional `UPDATE` cost 36 ms per command — more than the whole saving
/// the touch exists to protect.
pub fn load_blob_with_stamp(
    conn: &Connection,
    project_path: &Path,
) -> Result<Option<(Vec<u8>, u64)>, rusqlite::Error> {
    let key = project_path.display().to_string();
    let mut stmt =
        conn.prepare("SELECT blob, last_updated FROM project_indexes WHERE project_path=?1")?;
    let mut rows = stmt.query(params![key])?;
    if let Some(row) = rows.next()? {
        let blob = row.get::<_, Vec<u8>>(0)?;
        let stamp = row.get::<_, i64>(1)?.max(0) as u64;
        Ok(Some((blob, stamp)))
    } else {
        Ok(None)
    }
}

/// How stale the "last used" mark may get before a read refreshes it.
///
/// Eviction reasons about days (a 60-day cold TTL) and about a byte budget, so
/// an hour of resolution is far more than either needs — and it turns a write
/// on every command into one write per hour per project.
pub const TOUCH_MIN_INTERVAL_SECS: u64 = 3_600;

/// Should a read refresh this project's "last used" mark?
pub fn touch_is_due(stamp: u64) -> bool {
    crate::platform::now_unix_secs().saturating_sub(stamp) >= TOUCH_MIN_INTERVAL_SECS
}

/// One row of [`list_projects`].
pub use crate::store_scrub::ProjectSummary;

/// List every project in the index DB with summary stats, sorted
/// most-recently-updated first.
pub fn list_projects(conn: &Connection) -> Result<Vec<ProjectSummary>, rusqlite::Error> {
    crate::store_scrub::list_projects(conn, "project_indexes", "last_updated")
}

/// Remove the entry for a project (e.g. the project dir was deleted).
pub fn forget(conn: &Connection, project_path: &Path) -> Result<usize, rusqlite::Error> {
    crate::store_scrub::forget_project(conn, "project_indexes", project_path)
}

/// Reclaim disk space freed by deleted rows. Mirror of
/// [`semantic_db::vacuum_db`]. Returns (bytes_before, bytes_after).
pub fn vacuum_db() -> (u64, u64) {
    crate::store_scrub::vacuum_store(&db_path(), open)
}

/// Drop entries for projects whose root dir no longer exists.
pub fn prune_missing(conn: &Connection) -> Result<usize, rusqlite::Error> {
    let projects = list_projects(conn)?;
    let mut removed = 0;
    for (path, _, _, _, _) in projects {
        if !path.exists() {
            removed += forget(conn, &path)?;
        }
    }
    Ok(removed)
}

/// Drop zero-content orphan rows past a short TTL (#447).
///
/// Mirrors the semantic_db pattern (#446): tests / aborted init save
/// project rows with `symbol_count=0`, then never index real content;
/// these accumulate forever (production audit: 433 such rows in a 51 MB
/// index.db). Real projects always index at least one symbol; if a
/// real project ever has `symbol_count=0` it gets re-indexed on the
/// next save trivially.
///
/// 1-day TTL because zero-content rows hold nothing of value.
pub fn evict_stale(conn: &Connection) -> Result<usize, rusqlite::Error> {
    const ORPHAN_TTL_SECS: u64 = 86_400;
    let cutoff = crate::platform::now_unix_secs().saturating_sub(ORPHAN_TTL_SECS) as i64;
    conn.execute(
        "DELETE FROM project_indexes WHERE symbol_count = 0 AND last_updated < ?1",
        params![cutoff],
    )
}

/// Total-byte budget for the project-index store (OPP-2). `index.db`
/// keeps one blob per project ever indexed and never drops a *live*
/// project's row — [`evict_stale`] only catches zero-symbol orphans and
/// [`prune_missing`] only catches deleted directories. Production audit:
/// 108 MB across 23 projects, monotonically growing. Re-indexing a
/// project is cheap and happens automatically on its next save, so
/// bounding the store by evicting the least-recently-updated projects is
/// nearly free. Env-overridable via `TOKENADE_INDEX_MAX_BYTES`; mirrors
/// [`crate::dedup_ref::dedup_max_bytes`].
const INDEX_MAX_BYTES: u64 = 64 * 1024 * 1024; // 64 MiB

/// Cold-project TTL. A project whose index hasn't been updated in this
/// long is dropped on the size-cap pass regardless of total size: 60
/// days untouched ≈ abandoned, and a false positive re-indexes trivially
/// on the next touch. Longer than the dedup 7-day TTL because an index
/// is far more expensive to rebuild than a single deduped output.
const INDEX_COLD_TTL_SECS: u64 = 60 * 86_400; // 60 days

/// Resolve the active index-store byte cap. Reads the env var on each
/// call (no caching). Empty / unparseable / zero values fall back to the
/// compile-time default — same forgiving semantics as
/// [`crate::dedup_ref::dedup_max_bytes`].
pub fn index_max_bytes() -> u64 {
    match std::env::var("TOKENADE_INDEX_MAX_BYTES") {
        Ok(s) => s
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(INDEX_MAX_BYTES),
        Err(_) => INDEX_MAX_BYTES,
    }
}

/// Cold-prune + LRU size-cap eviction for `project_indexes` (OPP-2).
/// First drops projects untouched for [`INDEX_COLD_TTL_SECS`], then, if
/// the store still exceeds [`index_max_bytes`], evicts the
/// least-recently-updated remaining projects until back under the cap.
/// VACUUMs once if anything was removed. Returns `(rows_removed,
/// bytes_freed)`. Best-effort.
///
/// The most-recently-updated rows are kept, so the project currently
/// being worked on (its `last_updated` was just bumped by `save_blob`)
/// is never the eviction target.
pub fn evict_to_size_cap(conn: &Connection) -> Result<(usize, u64), rusqlite::Error> {
    evict_to_size_cap_inner(conn, index_max_bytes(), INDEX_COLD_TTL_SECS)
}

/// Testable inner of [`evict_to_size_cap`] — explicit cap + cold TTL so
/// tests can drive a tiny budget against synthetic rows.
pub(crate) fn evict_to_size_cap_inner(
    conn: &Connection,
    cap: u64,
    cold_ttl: u64,
) -> Result<(usize, u64), rusqlite::Error> {
    let now = crate::platform::now_unix_secs();
    let mut removed = 0usize;
    let mut bytes_freed = 0u64;

    // 1. Cold prune — projects untouched past the cold TTL, any size.
    let cold_cutoff = now.saturating_sub(cold_ttl) as i64;
    {
        let cold: Vec<(String, i64)> = {
            let mut stmt = conn.prepare(
                "SELECT project_path, size_bytes FROM project_indexes WHERE last_updated < ?1",
            )?;
            let mapped = stmt.query_map(params![cold_cutoff], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?;
            mapped.flatten().collect()
        };
        for (path, sz) in cold {
            if conn
                .execute(
                    "DELETE FROM project_indexes WHERE project_path=?1",
                    params![path],
                )
                .unwrap_or(0)
                > 0
            {
                removed += 1;
                bytes_freed += sz.max(0) as u64;
            }
        }
    }

    // 2. Size cap on what remains — evict oldest `last_updated` first.
    let total: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM project_indexes",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if (total as u64) > cap {
        let mut over = (total as u64).saturating_sub(cap);
        let victims: Vec<(String, i64)> = {
            // NEVER evict the most-recently-updated project, even when it alone
            // busts the cap. A single real repo's index can exceed the whole
            // budget (measured: 82 MiB for one 91k-symbol project vs a 64 MiB
            // cap), and an unfloored LRU then deletes the row it just wrote:
            // the working set is wiped, re-indexed on the next command, wiped
            // again on the next pass — pure CPU burn, forever. The cap is a
            // best-effort budget for COLD projects; overshooting it to keep the
            // live one is strictly better than thrashing. This never regressed
            // before only because the GC that calls it was effectively dead.
            let mut stmt = conn.prepare(
                "SELECT project_path, size_bytes FROM project_indexes \
                 ORDER BY last_updated ASC, project_path ASC",
            )?;
            let mapped =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            let all: Vec<(String, i64)> = mapped.flatten().collect();
            let keep_newest = all.len().saturating_sub(1); // index of the newest
            let mut v = Vec::new();
            for (i, row) in all.into_iter().enumerate() {
                if over == 0 || i == keep_newest {
                    break;
                }
                let len = row.1.max(0) as u64;
                over = over.saturating_sub(len);
                v.push(row);
            }
            v
        };
        for (path, sz) in victims {
            if conn
                .execute(
                    "DELETE FROM project_indexes WHERE project_path=?1",
                    params![path],
                )
                .unwrap_or(0)
                > 0
            {
                removed += 1;
                bytes_freed += sz.max(0) as u64;
            }
        }
    }

    if removed > 0 {
        // VACUUM cannot run inside a transaction; we hold none here.
        let _ = conn.execute("VACUUM", []);
    }
    Ok((removed, bytes_freed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // The production schema, not a copy of it.
        super::install_schema(&conn).unwrap();
        conn
    }

    /// Two tests never share the default store, and neither is the user's.
    #[test]
    fn a_test_without_the_variable_gets_a_store_of_its_own() {
        let _env = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // SAFETY: env vars are process-wide; ENV_LOCK is held above.
        unsafe { std::env::remove_var("TOKENADE_INDEX_DB") };
        let here = db_path();
        let there = std::thread::spawn(db_path).join().unwrap();
        assert_ne!(here, there, "two test threads share one store");
        if let Some(home) = crate::platform::home_dir() {
            assert!(
                !here.starts_with(home.join(".tokenade")),
                "a test reached the real store: {}",
                here.display()
            );
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let conn = tmp_db();
        let p = PathBuf::from("/home/me/proj-a");
        let blob = b"{\"root\":\"/home/me/proj-a\"}".to_vec();
        save_blob(&conn, &p, &blob, b"", 42, 123).unwrap();
        let loaded = load_blob(&conn, &p).unwrap().unwrap();
        assert_eq!(loaded, blob);
    }

    #[test]
    fn second_save_replaces_first() {
        let conn = tmp_db();
        let p = PathBuf::from("/home/me/proj-a");
        save_blob(&conn, &p, b"v1", b"", 1, 1).unwrap();
        save_blob(&conn, &p, b"v2-larger", b"", 5, 50).unwrap();
        let loaded = load_blob(&conn, &p).unwrap().unwrap();
        assert_eq!(loaded, b"v2-larger");
        let projects = list_projects(&conn).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].3, 5);
    }

    #[test]
    fn forget_removes_single_entry() {
        let conn = tmp_db();
        let p = PathBuf::from("/home/me/proj");
        save_blob(&conn, &p, b"x", b"", 1, 1).unwrap();
        assert_eq!(forget(&conn, &p).unwrap(), 1);
        assert!(load_blob(&conn, &p).unwrap().is_none());
    }

    /// A single project whose index alone busts the cap must SURVIVE. An
    /// unfloored LRU deletes the row it just wrote, so the live project is
    /// re-indexed and wiped on every pass — a CPU treadmill. Overshooting the
    /// budget is the correct trade here.
    #[test]
    fn a_test_tempdir_never_reaches_the_project_index() {
        // The sibling of `semantic_db`'s guard. This store was the worse of
        // the two: 85 of 180 rows on a real install, 47 %.
        // The guard asks whether the write would land in the USER's store:
        // a tempdir cwd with a REAL home. An isolated test redirects both and
        // must still be able to save, or the guard breaks the suite instead of
        // the leak — which is exactly what happened on the first attempt.
        // HOME is process-wide and this test moves it, so it must take the
        // crate-wide guard: without it a sibling `index_db` test saving under
        // a tempdir path ran while HOME said "/home/somebody-real" and had its
        // write refused. Same lesson as every other env-touching test here —
        // a lock only protects the parties that take it.
        let _g = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // BOTH vars, and not as belt-and-braces: `platform::home_dir` reads
        // USERPROFILE first on Windows, so a test that moves only HOME moves
        // nothing there — the guard then asks about the REAL profile and
        // refuses the isolated write. Invisible on Unix, where HOME wins.
        let prev_home = std::env::var_os("HOME");
        let prev_profile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", "/home/somebody-real");
        std::env::set_var("USERPROFILE", "/home/somebody-real");
        // A FILE-backed connection: the guard asks what the write targets, and
        // an in-memory database is nobody's store.
        let dir = std::env::temp_dir().join(format!(
            "tokenade-guard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = Connection::open(dir.join("store.db")).unwrap();
        install_schema(&conn).unwrap();
        for p in [
            "/tmp/claude-1000/tokenade-cli-cwd-12345-67890",
            "/tmp/tokenade-core-gain-noop-1-2",
            "/tmp/cargo-test-1234",
        ] {
            save_blob(&conn, std::path::Path::new(p), b"b", b"c", 1, 1).unwrap();
        }
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_indexes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "a test tempdir was written into the project index");

        save_blob(
            &conn,
            std::path::Path::new("/home/me/projects/tokenade"),
            b"b",
            b"c",
            3,
            7,
        )
        .unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_indexes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "a real project was dropped by the tempdir guard");

        // Now the isolated case: HOME is a tempdir too, so the store is the
        // test's own and the write must go through.
        std::env::set_var("HOME", "/tmp/claude-1000/tokenade-cli-cwd-999-1");
        std::env::set_var("USERPROFILE", "/tmp/claude-1000/tokenade-cli-cwd-999-1");
        save_blob(
            &conn,
            std::path::Path::new("/tmp/claude-1000/tokenade-cli-cwd-999-1"),
            b"b",
            b"c",
            1,
            1,
        )
        .unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_indexes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            n, 2,
            "an isolated test (cwd AND home both temp) was refused, so the \
             guard breaks the suite rather than the leak"
        );
        match prev_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match prev_profile {
            Some(h) => std::env::set_var("USERPROFILE", h),
            None => std::env::remove_var("USERPROFILE"),
        }
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn size_cap_never_evicts_the_live_project_even_when_it_busts_the_cap() {
        let conn = tmp_db();
        let now = crate::platform::now_unix_secs() as i64;
        // One project, 82 MiB, against a 64 MiB cap — the measured real case.
        let big = 82 * 1024 * 1024;
        conn.execute(
            "INSERT INTO project_indexes(project_path, blob, last_updated, size_bytes, file_count, symbol_count)
             VALUES('/live', x'00', ?1, ?2, 1, 91225)",
            params![now, big],
        )
        .unwrap();
        let (removed, _) = evict_to_size_cap_inner(&conn, 64 * 1024 * 1024, 60 * 86_400).unwrap();
        assert_eq!(removed, 0, "live project evicted — index would thrash");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_indexes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "live project row gone");
    }

    /// The floor protects only the NEWEST row: colder projects still get
    /// evicted to honour the budget.
    #[test]
    fn size_cap_still_evicts_colder_projects_down_to_the_newest() {
        let conn = tmp_db();
        let now = crate::platform::now_unix_secs() as i64;
        for (i, path) in ["/old2", "/old1", "/live"].iter().enumerate() {
            conn.execute(
                "INSERT INTO project_indexes(project_path, blob, last_updated, size_bytes, file_count, symbol_count)
                 VALUES(?1, x'00', ?2, ?3, 1, 100)",
                params![path, now - 1000 + i as i64, 40 * 1024 * 1024],
            )
            .unwrap();
        }
        // 120 MiB total vs a 64 MiB cap → the two cold ones go, /live stays.
        let (removed, _) = evict_to_size_cap_inner(&conn, 64 * 1024 * 1024, 60 * 86_400).unwrap();
        assert_eq!(removed, 2);
        let survivor: String = conn
            .query_row("SELECT project_path FROM project_indexes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(survivor, "/live");
    }

    #[test]
    fn evict_stale_drops_zero_symbol_orphans_past_ttl() {
        // Regression for #447: test-tempdir rows accumulate at
        // symbol_count=0; the orphan TTL clause must drop them.
        let conn = tmp_db();
        let orphan = PathBuf::from("/tmp/claude-1000/tokenade-mcp-test-X");
        save_blob(&conn, &orphan, b"", b"", 0, 0).unwrap();
        // Antedate past the orphan TTL (1 day).
        let stale = crate::platform::now_unix_secs().saturating_sub(2 * 86_400) as i64;
        conn.execute(
            "UPDATE project_indexes SET last_updated = ?1 WHERE project_path = ?2",
            params![stale, orphan.display().to_string()],
        )
        .unwrap();
        // Fresh real entry — must survive.
        let real = PathBuf::from("/home/me/real-project");
        save_blob(&conn, &real, b"v1", b"", 50, 500).unwrap();

        let dropped = evict_stale(&conn).unwrap();
        assert_eq!(dropped, 1, "orphan must be evicted");
        assert!(load_blob(&conn, &orphan).unwrap().is_none());
        assert!(load_blob(&conn, &real).unwrap().is_some());
    }

    #[test]
    fn evict_stale_keeps_fresh_zero_symbol_entries() {
        // Just-saved zero-symbol entry (e.g. project being indexed
        // right now) must survive — only stale orphans get evicted.
        let conn = tmp_db();
        let recent = PathBuf::from("/home/me/just-created");
        save_blob(&conn, &recent, b"", b"", 0, 0).unwrap();
        let dropped = evict_stale(&conn).unwrap();
        assert_eq!(dropped, 0);
        assert!(load_blob(&conn, &recent).unwrap().is_some());
    }

    fn backdate(conn: &Connection, p: &Path, secs_ago: u64) {
        let ts = crate::platform::now_unix_secs().saturating_sub(secs_ago) as i64;
        conn.execute(
            "UPDATE project_indexes SET last_updated = ?1 WHERE project_path = ?2",
            params![ts, p.display().to_string()],
        )
        .unwrap();
    }

    #[test]
    fn size_cap_evicts_oldest_projects_until_under_budget() {
        let conn = tmp_db();
        // Three 1000-byte project blobs, distinct update times.
        let blob = vec![b'x'; 1000];
        let (a, b, c) = (
            PathBuf::from("/p/old"),
            PathBuf::from("/p/mid"),
            PathBuf::from("/p/new"),
        );
        for p in [&a, &b, &c] {
            save_blob(&conn, p, &blob, b"", 10, 100).unwrap();
        }
        backdate(&conn, &a, 300);
        backdate(&conn, &b, 200);
        backdate(&conn, &c, 100);
        // Cap 2500 → 3000 total must shed exactly the oldest (1000 → 2000).
        // Cold TTL huge so the cold-prune step is inert here.
        let (removed, freed) = evict_to_size_cap_inner(&conn, 2500, 10_000 * 86_400).unwrap();
        assert_eq!(removed, 1);
        assert_eq!(freed, 1000);
        assert!(load_blob(&conn, &a).unwrap().is_none(), "oldest evicted");
        assert!(load_blob(&conn, &b).unwrap().is_some());
        assert!(load_blob(&conn, &c).unwrap().is_some());
    }

    #[test]
    fn size_cap_cold_prune_drops_abandoned_projects_regardless_of_size() {
        let conn = tmp_db();
        let cold = PathBuf::from("/p/abandoned");
        let warm = PathBuf::from("/p/active");
        save_blob(&conn, &cold, b"tiny", b"", 5, 50).unwrap();
        save_blob(&conn, &warm, b"tiny", b"", 5, 50).unwrap();
        backdate(&conn, &cold, 61 * 86_400); // past 60-day cold TTL
                                             // Huge cap so the size step is inert; only the cold prune fires.
        let (removed, _) = evict_to_size_cap_inner(&conn, 1 << 30, 60 * 86_400).unwrap();
        assert_eq!(removed, 1);
        assert!(load_blob(&conn, &cold).unwrap().is_none());
        assert!(load_blob(&conn, &warm).unwrap().is_some());
    }

    #[test]
    fn size_cap_is_noop_when_under_budget_and_all_warm() {
        let conn = tmp_db();
        save_blob(&conn, &PathBuf::from("/p/x"), b"small", b"", 1, 1).unwrap();
        let (removed, freed) =
            evict_to_size_cap_inner(&conn, 64 * 1024 * 1024, 60 * 86_400).unwrap();
        assert_eq!((removed, freed), (0, 0));
    }

    #[test]
    fn index_max_bytes_honours_env_override() {
        // No io_lock here (no shared module lock in this test mod); the
        // var name is unique to this crate so cross-test bleed is nil.
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::set_var("TOKENADE_INDEX_MAX_BYTES", "8192");
        }
        assert_eq!(index_max_bytes(), 8192);
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::set_var("TOKENADE_INDEX_MAX_BYTES", "0");
        }
        assert_eq!(index_max_bytes(), INDEX_MAX_BYTES);
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::remove_var("TOKENADE_INDEX_MAX_BYTES");
        }
    }
}
