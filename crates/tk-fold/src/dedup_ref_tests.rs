// The tests of `crate::dedup_ref`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `dedup_ref`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod worth_a_ref_tests {
    use super::*;

    /// A repeated command's pointer answers the question the repeat asked.
    ///
    /// The commands that repeat are polls — `tail -30 <log>`,
    /// `sleep 90; tail -16 <log>`, `until [ -s <log> ]` — and for a poll the
    /// tail IS the answer. `exec-dedup`, the sibling route for the very same
    /// "byte-identical to the previous run" situation, has always given the
    /// last five lines; this one gave a hash and a size, and MEASURED
    /// 2026-09-05 an agent went around tokenade 14 times to read bytes it had
    /// just been refused.
    #[test]
    fn a_repeated_commands_pointer_carries_its_tail() {
        let content = format!("{}\nbuilding…\nstep 7/9\nstill running\n", "x".repeat(4000));
        let m = ref_marker_with_tail("abc123abc123", content.len(), &content);
        assert!(m.contains("§ref:abc123abc123§"), "{m}");
        assert!(m.contains("still running"), "the tail is missing: {m}");
        assert!(m.contains("step 7/9"), "only one line of tail: {m}");
        assert!(!m.contains(&"x".repeat(100)), "the body came back: {m}");
        // Still a fold worth making.
        assert!(
            m.len() * 2 <= content.len(),
            "the tail ate the saving: {}",
            m.len()
        );

        // Short content keeps the bare pointer: the tail would eat the margin.
        let short = "one\ntwo\nthree\n".repeat(12); // ~168 B
        let bare = ref_marker_with_tail("abc123abc123", short.len(), &short);
        assert_eq!(bare, ref_marker("abc123abc123", short.len()));

        // Empty content cannot invent a tail.
        assert_eq!(
            ref_marker_with_tail("abc123abc123", 0, ""),
            ref_marker("abc123abc123", 0)
        );
    }

    #[test]
    fn the_floor_is_twice_the_marker_and_nothing_below_it_is_a_fold() {
        assert!(!worth_a_ref(0), "an empty output is not worth a pointer");
        assert!(
            !worth_a_ref(188),
            "measured: 188 bytes of content against a 154-byte marker, plus a \
             turn to get them back"
        );
        assert!(
            !worth_a_ref(REF_MARKER_BYTES),
            "breaking even is not a saving"
        );
        assert!(!worth_a_ref(2 * REF_MARKER_BYTES - 1));
        assert!(worth_a_ref(2 * REF_MARKER_BYTES), "the floor is inclusive");
        assert!(worth_a_ref(50_000));
    }
}
#[cfg(test)]
mod tests {

    /// Round-trips against an ISOLATED store, like its nine neighbours.
    ///
    /// It used to write into the real `~/.tokenade/dedup.db` — 177 MB on this
    /// machine, close enough to the size cap that a concurrent test's eviction
    /// pass dropped the row between `register` and `lookup`. It failed in two
    /// runs out of four, which is the worst kind of guard: one people learn to
    /// re-run. `isolated_db` both redirects the store AND takes the io lock;
    /// this test took neither, and a lock protects only the parties that take
    /// it.
    #[test]
    fn register_then_lookup_round_trips_on_content_hash() {
        let (_g, db) = isolated_db("round-trip");
        assert_eq!(
            super::db_path(),
            db,
            "the store was not redirected: this test would write to the user's real dedup.db"
        );
        let dir = std::env::temp_dir().join(format!("tokenade-ddrt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let body: String = (0..500)
            .map(|i| format!("line {i} of a file worth deduping\n"))
            .collect();
        let h = super::hash_content(body.as_bytes());
        super::register(&dir, &h, &body);
        let got = super::lookup(&dir, &h);
        assert!(
            got.is_some(),
            "register/lookup round-trip failed for hash {h}"
        );
        assert_eq!(got.unwrap(), body, "lookup returned different bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }
    use super::*;
    use std::sync::Mutex;

    /// Cargo runs tests from a single binary in parallel threads.
    /// Both the env-var override and the open DB connection are
    /// process-wide, so we serialise tests that touch them.
    fn io_lock() -> &'static Mutex<()> {
        static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn isolated_db(label: &str) -> (std::sync::MutexGuard<'static, ()>, PathBuf) {
        let guard = io_lock().lock().unwrap_or_else(|p| p.into_inner());
        let p = std::env::temp_dir().join(format!(
            "tokenade-dedup-{label}-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&p);
        // SAFETY: env vars are process-wide. The io_lock guarantees
        // only one test reads/writes TOKENADE_DEDUP_DB at a time.
        unsafe {
            std::env::set_var("TOKENADE_DEDUP_DB", &p);
        }
        (guard, p)
    }

    /// Register a blob under `hash` so a range naming it is the shape
    /// production actually creates: `read_diffs` registers the delivered slice
    /// and THEN the range. Tests that skipped this were exercising a state only
    /// reachable after eviction — which is exactly the state
    /// `range_is_covered` must refuse, so they broke when it started
    /// refusing it.
    fn seed_content(hash: &str) {
        register(&dummy_root(), hash, &format!("body for {hash}"));
    }

    fn dummy_root() -> PathBuf {
        std::env::temp_dir().join("tokenade-dedup-test-root-unused")
    }

    /// A range whose blob has been evicted must stop proving coverage.
    ///
    /// The caller substitutes the slice with `expand-ref <cover_hash>`, so
    /// returning an evicted hash hands the agent a recovery command that
    /// resolves to nothing — it loses the lines with no way back. Measured on a
    /// dogfood install: 823 of 1090 range rows named content the size-cap
    /// eviction had already dropped.
    #[test]
    fn an_evicted_blob_stops_proving_range_coverage() {
        let (_g, _p) = isolated_db("range-evicted");
        let sid = "sess-E";
        let fk = range_file_key("/x/big.js");
        let ver = "111|9000";
        seed_content("hLIVE");
        register_read_range(sid, &fk, ver, 0, 500, "hLIVE");
        assert!(
            range_is_covered(sid, &fk, ver, 100, 220, 0).is_some(),
            "with the blob present this range covers the slice"
        );

        // Evict just the blob, exactly as the size cap does.
        let conn = open_conn().unwrap();
        conn.execute("DELETE FROM dedup_content WHERE hash=?1", params!["hLIVE"])
            .unwrap();

        assert!(
            range_is_covered(sid, &fk, ver, 100, 220, 0).is_none(),
            "the bytes are gone, so the range cannot claim them as delivered"
        );
    }

    /// Evicting a blob must not leave the index tables naming it. Nothing else
    /// removed those rows, so they grew without bound against a capped table:
    /// 2797 of 4567 pointers dangled on a dogfood install.
    #[test]
    fn eviction_cascades_to_pointers_and_ranges() {
        let (_g, _p) = isolated_db("evict-cascade");
        let big = "x".repeat(4096);
        let h = hash_content(big.as_bytes());
        register(&dummy_root(), &h, &big);
        register_path_pointer(&dummy_root(), "pk-1", &h);
        register_read_range("sess", &range_file_key("/f.rs"), "1|1", 0, 100, &h);

        let conn = open_conn().unwrap();
        let count = |t: &str| -> i64 {
            conn.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(count("dedup_path_pointers"), 1);
        assert_eq!(count("dedup_read_ranges"), 1);

        // Cap of 0 bytes → everything is over the cap and gets evicted.
        let (removed, _) = evict_to_size_cap_conn(&conn, 0);
        assert!(removed > 0, "the blob must actually be evicted");
        assert_eq!(count("dedup_content"), 0);
        assert_eq!(count("dedup_path_pointers"), 0, "pointer left dangling");
        assert_eq!(count("dedup_read_ranges"), 0, "range left dangling");
    }

    #[test]
    fn read_range_coverage_contained_hits_and_gaps_miss() {
        let (_g, _p) = isolated_db("range-cover");
        let sid = "sess-A";
        let fk = range_file_key("/x/big.js");
        let ver = "111|9000";
        // Deliver lines [0,500) and [500,2000) (two adjacent slices).
        seed_content("h1");
        register_read_range(sid, &fk, ver, 0, 500, "h1");
        seed_content("h2");
        register_read_range(sid, &fk, ver, 500, 2000, "h2");
        // Fully contained sub-ranges → covered (returns a hash).
        assert!(
            range_is_covered(sid, &fk, ver, 100, 220, 0).is_some(),
            "subset of [0,500)"
        );
        assert!(range_is_covered(sid, &fk, ver, 150, 170, 0).is_some());
        assert!(
            range_is_covered(sid, &fk, ver, 400, 1800, 0).is_some(),
            "spans merge of both"
        );
        assert!(
            range_is_covered(sid, &fk, ver, 0, 2000, 0).is_some(),
            "exact union"
        );
        // Partial / beyond → NOT covered (no data loss).
        assert!(
            range_is_covered(sid, &fk, ver, 1800, 2600, 0).is_none(),
            "extends past 2000"
        );
        assert!(
            range_is_covered(sid, &fk, ver, 2040, 2070, 0).is_none(),
            "fully beyond"
        );
    }

    #[test]
    fn read_range_recency_window_excludes_stale_in_session_ranges() {
        let (_g, _p) = isolated_db("range-recency");
        let fk = range_file_key("/x/long.js");
        register(&dummy_root(), "h", "the delivered bytes"); // blob for the baseline lookup
        seed_content("h");
        register_read_range("sess", &fk, "1|1", 0, 500, "h");
        // Same session, fresh → covered with any window.
        assert!(range_is_covered("sess", &fk, "1|1", 0, 100, 900).is_some());
        assert!(lookup_session_file_latest("sess", &fk, 900).is_some());
        // Backdate the row to 20 min ago (older than a 15-min context window).
        let old = crate::platform::now_unix_secs().saturating_sub(20 * 60) as i64;
        let conn = open_conn().unwrap();
        conn.execute("UPDATE dedup_read_ranges SET last_used=?1", params![old])
            .unwrap();
        // With a 900s (15-min) window → excluded: likely scrolled out of context.
        assert!(
            range_is_covered("sess", &fk, "1|1", 0, 100, 900).is_none(),
            "a range older than the context window must not be trusted"
        );
        assert!(lookup_session_file_latest("sess", &fk, 900).is_none());
        // With the window disabled (0 → TTL only) → still covered.
        assert!(range_is_covered("sess", &fk, "1|1", 0, 100, 0).is_some());
    }

    #[test]
    fn read_range_is_session_scoped_and_version_scoped() {
        let (_g, _p) = isolated_db("range-scope");
        let fk = range_file_key("/x/server.js");
        seed_content("h");
        register_read_range("sess-1", &fk, "100|500", 0, 400, "h");
        // SAME range, DIFFERENT session → NOT covered (fresh context).
        assert!(
            range_is_covered("sess-2", &fk, "100|500", 10, 50, 0).is_none(),
            "a different session must not reuse another session's context"
        );
        // SAME session, DIFFERENT version (file edited: mtime|size changed) → miss.
        assert!(
            range_is_covered("sess-1", &fk, "200|512", 10, 50, 0).is_none(),
            "an edited file (new version) invalidates stale ranges"
        );
        // SAME session + version → hit.
        assert!(range_is_covered("sess-1", &fk, "100|500", 10, 50, 0).is_some());
    }

    #[test]
    fn lookup_session_file_latest_returns_most_recent_session_scoped() {
        let (_g, _p) = isolated_db("slice-latest");
        let fk = range_file_key("/x/srv.js");
        let root = dummy_root();
        // Register two deliveries (different versions = an edit happened).
        register(&root, "hOLD", "old body v1");
        seed_content("hOLD");
        register_read_range("sess", &fk, "1|100", 0, 40, "hOLD");
        register(&root, "hNEW", "new body v2");
        seed_content("hNEW");
        register_read_range("sess", &fk, "2|110", 0, 40, "hNEW");
        // Latest (version-agnostic) → the v2 content.
        let (h, c) = lookup_session_file_latest("sess", &fk, 0).unwrap();
        assert_eq!(h, "hNEW");
        assert_eq!(c, "new body v2");
        // A different session sees nothing (context isolation).
        assert!(lookup_session_file_latest("other", &fk, 0).is_none());
        // Empty session id → None.
        assert!(lookup_session_file_latest("", &fk, 0).is_none());
    }

    #[test]
    fn clear_session_ranges_drops_only_that_session() {
        let (_g, _p) = isolated_db("range-clear");
        let fk = range_file_key("/x/a.js");
        seed_content("h");
        register_read_range("keep", &fk, "1|1", 0, 100, "h");
        seed_content("h");
        register_read_range("drop", &fk, "1|1", 0, 100, "h");
        clear_session_ranges("drop");
        assert!(
            range_is_covered("drop", &fk, "1|1", 0, 50, 0).is_none(),
            "PreCompact cleared it"
        );
        assert!(
            range_is_covered("keep", &fk, "1|1", 0, 50, 0).is_some(),
            "other session intact"
        );
    }

    #[test]
    fn read_range_degenerate_inputs_are_safe() {
        let (_g, _p) = isolated_db("range-degen");
        let fk = range_file_key("/x/b.js");
        register_read_range("", &fk, "1|1", 0, 100, "h"); // empty session → no-op
        register_read_range("s", &fk, "1|1", 100, 100, "h"); // empty range → no-op
        assert!(range_is_covered("", &fk, "1|1", 0, 50, 0).is_none());
        assert!(
            range_is_covered("s", &fk, "1|1", 50, 40, 0).is_none(),
            "inverted range"
        );
    }

    #[test]
    fn hash_is_deterministic_and_12_hex() {
        let a = hash_content(b"hello");
        let b = hash_content(b"hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 12);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        // RFC 6234 test vector for "abc"
        let abc = hash_content(b"abc");
        assert_eq!(abc, "ba7816bf8f01");
    }

    #[test]
    fn hash_differs_for_different_inputs() {
        assert_ne!(hash_content(b"foo"), hash_content(b"bar"));
    }

    #[test]
    fn round_trip_register_and_lookup() {
        let _db = isolated_db("rt");
        let root = dummy_root();
        let content = "the content";
        let h = hash_content(content.as_bytes());
        assert!(lookup(&root, &h).is_none(), "fresh cache should miss");
        register(&root, &h, content);
        assert_eq!(lookup(&root, &h).as_deref(), Some(content));
    }

    #[test]
    fn command_key_is_stable_and_scoped_by_cwd() {
        let a = command_key("/proj/a", "git status");
        assert_eq!(a, command_key("/proj/a", "git status"));
        assert_ne!(a, command_key("/proj/b", "git status"));
        assert_ne!(a, command_key("/proj/a", "git log"));
        // Must not collide with a Read path_key over similar bytes.
        assert_ne!(a, path_key("/proj/a|git status", 0, 0));
    }

    #[test]
    fn lookup_by_path_with_age_roundtrip_and_repoint() {
        let _db = isolated_db("cmdage");
        let root = dummy_root();
        let key = command_key("/proj/a", "kubectl get pods");
        assert!(lookup_by_path_with_age(&root, &key).is_none());

        let h1 = hash_content(b"output v1");
        register(&root, &h1, "output v1");
        register_path_pointer(&root, &key, &h1);
        let (got_hash, got_content, age) = lookup_by_path_with_age(&root, &key).unwrap();
        assert_eq!(got_hash, h1);
        assert_eq!(got_content, "output v1");
        assert!(
            age < 60,
            "age must be seconds-since-registration, got {age}"
        );

        // Re-pointing to new content must replace, not duplicate.
        let h2 = hash_content(b"output v2");
        register(&root, &h2, "output v2");
        register_path_pointer(&root, &key, &h2);
        let (got_hash2, got_content2, _) = lookup_by_path_with_age(&root, &key).unwrap();
        assert_eq!(got_hash2, h2);
        assert_eq!(got_content2, "output v2");
    }

    #[test]
    fn lookup_by_path_with_age_misses_when_content_expired() {
        let _db = isolated_db("cmdexp");
        let root = dummy_root();
        let key = command_key("/proj/a", "docker ps");
        let h = hash_content(b"containers");
        register(&root, &h, "containers");
        register_path_pointer(&root, &key, &h);
        // Expire the CONTENT blob; the pointer alone must not hit.
        let conn = open_conn().unwrap();
        let ancient = (crate::platform::now_unix_secs() - (CACHE_TTL_SECS + 60)) as i64;
        conn.execute(
            "UPDATE dedup_content SET last_used=?1 WHERE hash=?2",
            params![ancient, h],
        )
        .unwrap();
        assert!(lookup_by_path_with_age(&root, &key).is_none());
    }

    #[test]
    fn ref_marker_includes_size_and_expand_hint() {
        let m = ref_marker("abc123", 1234);
        assert!(m.contains("abc123"));
        assert!(m.contains("1234 bytes"));
        assert!(m.contains("tokenade expand-ref"));
    }

    #[test]
    fn lookup_bumps_last_used_for_lru_refresh() {
        let _db = isolated_db("lru");
        let root = dummy_root();
        let content = "x";
        let h = hash_content(content.as_bytes());
        register(&root, &h, content);
        // Manually back-date the row so the LRU bump is observable.
        let conn = open_conn().unwrap();
        let backdate = (crate::platform::now_unix_secs() - 3600) as i64;
        conn.execute(
            "UPDATE dedup_content SET last_used=?1 WHERE hash=?2",
            params![backdate, h],
        )
        .unwrap();
        let lu_before: i64 = conn
            .query_row(
                "SELECT last_used FROM dedup_content WHERE hash=?1",
                params![h],
                |r| r.get(0),
            )
            .unwrap();
        let _ = lookup(&root, &h);
        let lu_after: i64 = conn
            .query_row(
                "SELECT last_used FROM dedup_content WHERE hash=?1",
                params![h],
                |r| r.get(0),
            )
            .unwrap();
        assert!(lu_after > lu_before, "lookup must bump last_used (LRU)");
    }

    #[test]
    fn stale_entries_are_dropped_on_lookup() {
        let _db = isolated_db("stale");
        let root = dummy_root();
        let h = hash_content(b"old");
        register(&root, &h, "stored");
        // Back-date past the TTL.
        let conn = open_conn().unwrap();
        let ancient = (crate::platform::now_unix_secs() - (CACHE_TTL_SECS + 60)) as i64;
        conn.execute(
            "UPDATE dedup_content SET last_used=?1 WHERE hash=?2",
            params![ancient, h],
        )
        .unwrap();
        assert!(lookup(&root, &h).is_none(), "stale must return None");
        let still_there: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dedup_content WHERE hash=?1",
                params![h],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(still_there, 0, "lookup should have purged the stale row");
    }

    #[test]
    fn refresh_refs_bumps_residents_and_reports_sizes() {
        let _db = isolated_db("refresh");
        let root = dummy_root();
        let a = hash_content(b"alpha");
        let b = hash_content(b"beta!!");
        register(&root, &a, "alpha"); // 5 bytes
        register(&root, &b, "beta!!"); // 6 bytes
                                       // Back-date both so the bump is observable.
        let conn = open_conn().unwrap();
        let backdate = (crate::platform::now_unix_secs() - 3600) as i64;
        conn.execute("UPDATE dedup_content SET last_used=?1", params![backdate])
            .unwrap();

        let absent = "deadbeefdead".to_string();
        let mut got = refresh_refs(&[a.clone(), b.clone(), absent]);
        got.sort();
        let mut want = vec![(a.clone(), 5u64), (b.clone(), 6u64)];
        want.sort();
        assert_eq!(got, want, "only residents returned, with byte_count");

        // Both residents must have been LRU-refreshed.
        for h in [&a, &b] {
            let lu: i64 = conn
                .query_row(
                    "SELECT last_used FROM dedup_content WHERE hash=?1",
                    params![h],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(lu > backdate, "refresh_refs must bump last_used");
        }
    }

    #[test]
    fn refresh_refs_empty_is_noop() {
        let _db = isolated_db("refresh-empty");
        assert!(refresh_refs(&[]).is_empty());
    }

    #[test]
    fn evict_stale_removes_old_rows() {
        let _db = isolated_db("evict");
        let root = dummy_root();
        let young = hash_content(b"young");
        let old = hash_content(b"old");
        register(&root, &young, "y");
        register(&root, &old, "o");
        // Age the "old" row out via direct SQL.
        let conn = open_conn().unwrap();
        let ancient = (crate::platform::now_unix_secs() - (CACHE_TTL_SECS + 60)) as i64;
        conn.execute(
            "UPDATE dedup_content SET last_used=?1 WHERE hash=?2",
            params![ancient, old],
        )
        .unwrap();
        let removed = evict_stale(&root).unwrap();
        assert_eq!(removed, 1);
        assert!(lookup(&root, &young).is_some());
        assert!(lookup(&root, &old).is_none());
    }

    #[test]
    fn dedup_max_bytes_honours_env_override() {
        let _g = io_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: env vars are process-wide; io_lock serialises access.
        unsafe {
            std::env::set_var("TOKENADE_DEDUP_MAX_BYTES", "4096");
        }
        assert_eq!(dedup_max_bytes(), 4096);
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::set_var("TOKENADE_DEDUP_MAX_BYTES", "0");
        }
        assert_eq!(
            dedup_max_bytes(),
            DEDUP_MAX_BYTES,
            "zero falls back to default"
        );
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::set_var("TOKENADE_DEDUP_MAX_BYTES", "garbage");
        }
        assert_eq!(
            dedup_max_bytes(),
            DEDUP_MAX_BYTES,
            "garbage falls back to default"
        );
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::remove_var("TOKENADE_DEDUP_MAX_BYTES");
        }
    }

    #[test]
    fn size_cap_evicts_oldest_first_until_under_budget() {
        let _db = isolated_db("sizecap");
        let root = dummy_root();
        // Three 1000-byte rows; cap at 2500 → exactly one (the oldest)
        // must be evicted to get under budget (3000 → 2000).
        let body = "x".repeat(1000);
        let (h_old, h_mid, h_new) = (
            hash_content(b"old"),
            hash_content(b"mid"),
            hash_content(b"new"),
        );
        for h in [&h_old, &h_mid, &h_new] {
            register(&root, h, &body);
        }
        let conn = open_conn().unwrap();
        let base = crate::platform::now_unix_secs() as i64;
        // Distinct last_used so LRU order is unambiguous.
        for (h, age) in [(&h_old, 300), (&h_mid, 200), (&h_new, 100)] {
            conn.execute(
                "UPDATE dedup_content SET last_used=?1 WHERE hash=?2",
                params![base - age, h],
            )
            .unwrap();
        }
        let (removed, freed) = evict_to_size_cap_conn(&conn, 2500);
        assert_eq!(removed, 1, "exactly the oldest row should be evicted");
        assert_eq!(freed, 1000);
        assert!(lookup(&root, &h_old).is_none(), "oldest must be gone");
        assert!(lookup(&root, &h_mid).is_some());
        assert!(lookup(&root, &h_new).is_some());
    }

    #[test]
    fn size_cap_is_noop_when_under_budget() {
        let _db = isolated_db("sizecap-noop");
        let root = dummy_root();
        register(&root, &hash_content(b"a"), "small");
        let conn = open_conn().unwrap();
        let (removed, freed) = evict_to_size_cap_conn(&conn, 150 * 1024 * 1024);
        assert_eq!((removed, freed), (0, 0));
    }

    #[test]
    fn path_pointer_round_trip() {
        let _db = isolated_db("ptr");
        let root = dummy_root();
        let content = "file v1";
        let h = hash_content(content.as_bytes());
        register(&root, &h, content);
        let key = path_key("/abs/path.rs", 0, 0);
        register_path_pointer(&root, &key, &h);
        let (got_hash, got_content) = lookup_by_path(&root, &key).unwrap();
        assert_eq!(got_hash, h);
        assert_eq!(got_content, content);
    }

    #[test]
    fn path_pointer_returns_none_when_content_evicted() {
        let _db = isolated_db("ptr-evict");
        let root = dummy_root();
        let content = "file v1";
        let h = hash_content(content.as_bytes());
        register(&root, &h, content);
        let key = path_key("/abs/path2.rs", 0, 0);
        register_path_pointer(&root, &key, &h);
        // Evict the content; pointer remains but lookup must fail.
        let conn = open_conn().unwrap();
        conn.execute("DELETE FROM dedup_content WHERE hash=?1", params![h])
            .unwrap();
        assert!(lookup_by_path(&root, &key).is_none());
    }

    /// The store is content-addressed, so the filename is a CLAIM about the
    /// bytes inside — and a cloned repository controls both. Importing on the
    /// filename's word alone would let it plant an entry that `expand-ref`
    /// serves back as genuine recovered output.
    #[test]
    fn a_planted_legacy_blob_whose_content_does_not_match_its_name_is_refused() {
        let _db = isolated_db("migrate-poison");
        let root = std::env::temp_dir().join(format!(
            "tokenade-dedup-poison-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let legacy = root.join(".tokenade").join("dedup");
        std::fs::create_dir_all(&legacy).unwrap();

        // Planted: the name is a valid-looking hash of OTHER bytes.
        let planted = hash_content(b"what the victim expects");
        std::fs::write(
            legacy.join(format!("{planted}.txt")),
            "attacker text no command ever produced",
        )
        .unwrap();
        // Genuine: written content-addressed, as an older tokenade would.
        let honest = hash_content(b"real prior output");
        std::fs::write(legacy.join(format!("{honest}.txt")), "real prior output").unwrap();

        let summary = migrate_local_store(&root).unwrap_or_default();
        // Exactly ONE of the two blobs may cross: the honest one.
        assert!(
            summary.contains("migrated 1 blob(s)"),
            "planted blob must be refused and the genuine one kept, got: {summary:?}"
        );
        assert!(!planted.is_empty() && !honest.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn migrate_legacy_dir_imports_blobs_and_pointers() {
        let _db = isolated_db("migrate");
        let root = std::env::temp_dir().join(format!(
            "tokenade-dedup-migrate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let legacy = root.join(".tokenade").join("dedup");
        std::fs::create_dir_all(legacy.join("_paths")).unwrap();
        let h = hash_content(b"legacy content");
        std::fs::write(legacy.join(format!("{h}.txt")), "legacy content").unwrap();
        let key = path_key("/old.rs", 0, 0);
        std::fs::write(legacy.join("_paths").join(format!("{key}.ptr")), &h).unwrap();

        let action = migrate_local_store(&root);
        assert!(action.is_some());
        let action = action.unwrap();
        assert!(action.contains("migrated 1 blob(s)"));
        assert!(action.contains("1 path pointer(s)"));

        // Legacy dir gone.
        assert!(!legacy.exists());
        // Global DB now serves the content.
        let dummy = dummy_root();
        assert_eq!(lookup(&dummy, &h).as_deref(), Some("legacy content"));
        let (got_hash, got_content) = lookup_by_path(&dummy, &key).unwrap();
        assert_eq!(got_hash, h);
        assert_eq!(got_content, "legacy content");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn migrate_no_op_when_no_legacy_dir() {
        let _db = isolated_db("no-migrate");
        let root = std::env::temp_dir().join(format!(
            "tokenade-dedup-no-legacy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        assert!(migrate_local_store(&root).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
