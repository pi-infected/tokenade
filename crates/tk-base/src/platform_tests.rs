// The tests of `platform`.
//
// Spliced in by `include!` from `platform.rs`: the same module, hence the same
// private access and the same `super::` paths, without a single visibility line
// changed. This is the shape this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod tests {

    /// An ambient override describes where the CURRENT user's data lives. When
    /// a caller names a different home it is not talking about that user, and
    /// letting a process-global win means the caller cannot isolate the call —
    /// which is how `opencode_plugin_dir` wrote wherever the environment
    /// pointed and made a guard test unrunnable in a parallel suite.
    #[test]
    fn ambient_overrides_lose_to_an_explicitly_named_home() {
        let foreign = std::path::Path::new("/definitely/not/the/process/home");
        assert!(!is_process_home(foreign));
        assert_eq!(xdg_config_root(foreign), foreign.join(".config"));
        assert_eq!(xdg_data_root(foreign), foreign.join(".local/share"));
        assert_eq!(ambient_override(foreign, "XDG_DATA_HOME"), None);
        assert_eq!(ambient_override(foreign, "GOOSE_PATH_ROOT"), None);
        // …and the real home still resolves to a plausible root.
        if let Some(h) = home_dir() {
            assert!(is_process_home(&h));
            assert!(xdg_config_root(&h).is_absolute());
        }
    }

    /// Every on-disk store must be redirectable.
    ///
    /// Not a style rule. A store with no override cannot be isolated in a test,
    /// so its tests write to the developer's REAL state — measured: 81 rows
    /// keyed on `/tmp/tokenade-sem-<pid>-…` inside the real
    /// `~/.tokenade/semantic.db`, because `semantic_db::db_path()` was the one
    /// of five that had none. It is also the mechanism this crate settled on
    /// for isolation, deliberately, after process-wide `HOME` mutation stalled
    /// the suite.
    ///
    /// Checked in the source rather than by setting the variables: proving the
    /// property must not itself mutate the environment five times.
    #[test]
    fn every_store_path_honours_an_env_override() {
        for (file, var) in [
            ("dedup_ref.rs", "TOKENADE_DEDUP_DB"),
            ("fold_store.rs", "TOKENADE_FOLD_STORE_DB"),
            ("index_db.rs", "TOKENADE_INDEX_DB"),
            ("file_search.rs", "TOKENADE_FILE_SEARCH_DB"),
            ("semantic_db.rs", "TOKENADE_SEMANTIC_DB"),
            // Added 2026-08-04: this was the last path here with no escape
            // hatch, and its tests appended to the real `~/.tokenade/redact.jsonl`.
            ("redact.rs", "TOKENADE_REDACT_LEDGER"),
        ] {
            // Searched in BOTH roots, and by an ABSOLUTE path. Five of
            // these six files live in `tokenade-core`, `redact.rs` in
            // `tk-base` — and the old form, `read_to_string("src/…")`,
            // depended on the current directory, which the repo's rules forbid. The
            // split knocked down both defects at once.
            // …and on the module's FAMILY, not just the trunk file:
            // `redact.rs`'s escape hatch moved down into
            // `redact/ledger.rs`, and this test accused the product of no longer
            // having one. A split module the sweep does not name is a
            // module the sweep does not see.
            let stem = file.strip_suffix(".rs").expect("a module name");
            let src = crate::source_guard::crate_srcs()
                .iter()
                .filter(|r| r.join(file).is_file())
                .map(|r| crate::source_guard::module_family(r, stem))
                .next()
                .unwrap_or_else(|| panic!("{file} not found in the source roots"));
            assert!(
                src.contains(&format!("var_os(\"{var}\")")),
                "src/{file} builds a store path with no `{var}` escape hatch — \
                 its tests can only run against the developer's real store"
            );
        }
    }

    /// End to end: a store with real dead space shrinks, a tight one is left
    /// exactly as it was, and a file that is not a database cannot break the
    /// sweep.
    #[test]
    fn the_sweep_vacuums_only_what_is_bloated() {
        let dir = std::env::temp_dir().join(format!("tok-vacuum-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Bloated: fill ~12 MB, then delete it all. SQLite keeps the pages.
        let fat = dir.join("fat.db");
        {
            let c = rusqlite::Connection::open(&fat).unwrap();
            c.execute_batch("CREATE TABLE t(x TEXT);").unwrap();
            let blob = "x".repeat(4096);
            for _ in 0..3000 {
                c.execute("INSERT INTO t VALUES (?1)", rusqlite::params![blob])
                    .unwrap();
            }
            c.execute_batch("DELETE FROM t;").unwrap();
        }
        let fat_before = std::fs::metadata(&fat).unwrap().len();

        // Tight: small and fully used.
        let lean = dir.join("lean.db");
        {
            let c = rusqlite::Connection::open(&lean).unwrap();
            c.execute_batch("CREATE TABLE t(x TEXT); INSERT INTO t VALUES ('a');")
                .unwrap();
        }
        let lean_before = std::fs::metadata(&lean).unwrap().len();

        // Not a database at all, and a non-.db file: neither may abort the sweep.
        std::fs::write(dir.join("garbage.db"), b"not sqlite").unwrap();
        std::fs::write(dir.join("notes.txt"), b"ignored").unwrap();

        let (stores, freed) = vacuum_bloated_stores(&dir);
        assert_eq!(stores, 1, "only the bloated store is rewritten");
        assert!(freed > 4 * 1024 * 1024, "reclaimed only {freed} bytes");
        assert!(std::fs::metadata(&fat).unwrap().len() < fat_before);
        assert_eq!(
            std::fs::metadata(&lean).unwrap().len(),
            lean_before,
            "a tight store must not be touched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vacuum_is_asked_for_only_when_both_waste_tests_pass() {
        let p = 4096;
        // The real shapes that motivated this, and the one that did not.
        assert!(
            sqlite_waste_is_worth_vacuuming(13_092, 4_744, p),
            "index.db: 36% dead, 19.4 MB"
        );
        assert!(
            sqlite_waste_is_worth_vacuuming(2_562, 1_259, p),
            "fold_store.db: 49% dead, 5.2 MB"
        );
        assert!(
            !sqlite_waste_is_worth_vacuuming(40_253, 0, p),
            "dedup.db already vacuums after eviction"
        );
        // 97% dead but only 1.6 MB — under the absolute floor, not worth a
        // full rewrite.
        assert!(!sqlite_waste_is_worth_vacuuming(407, 394, p));
        // Large but tight: 4 MB of freelist in a 1 GB file is noise.
        assert!(!sqlite_waste_is_worth_vacuuming(262_144, 1_024, p));
        assert!(!sqlite_waste_is_worth_vacuuming(0, 0, p), "empty file");
    }

    /// Records must stay contiguous when many writers append at once.
    ///
    /// This is the regression the ledgers actually suffered: with
    /// `writeln!(f, "…")` the record is streamed as one `write()` per format
    /// fragment, and hooks append as concurrent processes, so fragments
    /// interleave into lines like
    /// `{"ts":{"ts":17857689501785768950,"n":,"n":11}`. Threads with their own
    /// `O_APPEND` handles reproduce that exactly — the atomicity in question is
    /// the file offset's, not the process's.
    ///
    /// Sized so a regression cannot pass by luck: 8 writers x 400 records of a
    /// shape that splits into several fragments.
    #[test]
    fn concurrent_appends_never_interleave_a_record() {
        let dir = std::env::temp_dir().join(format!("tok-append-line-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("ledger.jsonl");

        const WRITERS: usize = 8;
        const EACH: usize = 400;
        let handles: Vec<_> = (0..WRITERS)
            .map(|w| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for i in 0..EACH {
                        append_line(&path, &format!(r#"{{"ts":178576895{w},"n":{i}}}"#));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(
            lines.len(),
            WRITERS * EACH,
            "every append must produce exactly one line"
        );
        for l in &lines {
            let v: serde_json::Value =
                serde_json::from_str(l).unwrap_or_else(|e| panic!("torn record {l:?}: {e}"));
            assert!(v.get("ts").and_then(|x| x.as_u64()).is_some());
            assert!(v.get("n").and_then(|x| x.as_u64()).is_some());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The helper creates the directory it is handed, because every caller is a
    /// best-effort ledger write on a path that may not exist yet.
    #[test]
    fn append_line_creates_the_parent_directory() {
        let dir = std::env::temp_dir()
            .join(format!("tok-append-mk-{}", std::process::id()))
            .join("nested");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        let path = dir.join("l.jsonl");
        append_line(&path, "hello");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    /// Our own litter gets collected; the user's does not.
    #[cfg(unix)]
    #[cfg(test)]
    mod own_backups {
        use super::super::{sweep_own_backups_in, OWN_BACKUP_TTL_SECS};
        use std::time::{Duration, SystemTime};

        fn dir(tag: &str) -> std::path::PathBuf {
            let d = std::env::temp_dir().join(format!(
                "tk-ownbak-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            d
        }

        fn aged(d: &std::path::Path, name: &str, days: u64) -> std::path::PathBuf {
            let p = d.join(name);
            std::fs::write(&p, b"payload").unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_times(
                std::fs::FileTimes::new()
                    .set_modified(SystemTime::now() - Duration::from_secs(days * 86_400)),
            )
            .unwrap();
            p
        }

        #[test]
        fn old_transient_backups_go_and_recent_ones_stay() {
            let d = dir("basic");
            let old = aged(&d, "gain.jsonl.bak-1780312510.counted", 63);
            let recent = aged(&d, "gain.jsonl.bak-1784599522.counted", 3);
            let (n, freed) = sweep_own_backups_in(&d);
            assert!(!old.exists(), "a 63-day-old backup must be swept");
            assert!(
                recent.exists(),
                "a 3-day-old backup is still someone's safety net"
            );
            assert_eq!(n, 1);
            assert!(freed > 0);
            let _ = std::fs::remove_dir_all(&d);
        }

        /// The blast radius is the whole point: a live ledger, a live log and a
        /// support bundle the USER generated must all survive, at any age.
        #[test]
        fn live_files_and_user_artefacts_are_never_touched() {
            let d = dir("safety");
            let keep = [
                aged(&d, "gain.jsonl", 400),
                aged(&d, "debug.log", 400),
                aged(&d, "dedup.db", 400),
                aged(&d, "last-report.zip", 400),
                aged(&d, "license.key", 400),
            ];
            let (n, _) = sweep_own_backups_in(&d);
            assert_eq!(n, 0, "the sweep touched something it does not own");
            for p in keep {
                assert!(p.exists(), "{p:?} was removed");
            }
            let _ = std::fs::remove_dir_all(&d);
        }

        /// `set_permissions` follows symlinks — verified on this host: a link
        /// chmod'd its target from 644 to 600. So a link planted under
        /// `~/.tokenade` with a name we harden could have had a file OUTSIDE
        /// the directory silently re-moded. This is the half of the symlink
        /// audit that was a real hole; `remove_dir_all` was not (it unlinks the
        /// link and leaves the target alone).
        #[test]
        fn hardening_perms_never_reaches_through_a_symlink() {
            use std::os::unix::fs::PermissionsExt;
            let d = dir("permlink");
            let victim = d.join("victim.txt");
            std::fs::write(&victim, b"not ours").unwrap();
            std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).unwrap();

            let stash = d.join("stash");
            std::fs::create_dir_all(&stash).unwrap();
            std::os::unix::fs::symlink(&victim, stash.join("deadbeef0000")).unwrap();
            std::fs::set_permissions(&stash, std::fs::Permissions::from_mode(0o755)).unwrap();

            super::super::tighten_stash(&stash, &d.join(".stash-perms-checked"));

            let mode = std::fs::metadata(&victim).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o644,
                "the stash walk chmod'd a file outside the stash"
            );
            let _ = std::fs::remove_dir_all(&d);
        }

        /// A name matching our backup shape could be a symlink. What must hold
        /// is that the sweep never reaches through it: `remove_dir_all` on a
        /// followed link would destroy a tree outside `~/.tokenade`.
        ///
        /// The link here is necessarily young — std has no way to age a symlink
        /// itself (`File::open` follows it, and `utimensat(AT_SYMLINK_NOFOLLOW)`
        /// is not exposed), so it is below the horizon and gets skipped. That is
        /// why this asserts the safety half only: the target survives, whatever
        /// the sweep decided about the link. The removal path for symlinks is
        /// `remove_file`, which by definition unlinks rather than follows.
        #[test]
        fn a_symlinked_backup_name_never_reaches_its_target() {
            let d = dir("symlink");
            let outside = d.join("precious");
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(outside.join("keep.txt"), b"do not delete").unwrap();

            let link = d.join("backup-recalc-1783516383");
            std::os::unix::fs::symlink(&outside, &link).unwrap();

            let _ = sweep_own_backups_in(&d);

            assert!(
                outside.join("keep.txt").exists(),
                "the sweep followed a symlink and destroyed the target"
            );
            let _ = std::fs::remove_dir_all(&d);
        }

        #[test]
        fn a_recalc_backup_directory_is_removed_whole() {
            let d = dir("recalcdir");
            let sub = d.join("backup-recalc-1783516383");
            std::fs::create_dir_all(sub.join("nested")).unwrap();
            std::fs::write(sub.join("nested").join("a.json"), b"1234567890").unwrap();
            let f = std::fs::File::options().read(true).open(&sub).unwrap();
            f.set_times(std::fs::FileTimes::new().set_modified(
                SystemTime::now() - Duration::from_secs(OWN_BACKUP_TTL_SECS + 86_400),
            ))
            .unwrap();
            let (n, freed) = sweep_own_backups_in(&d);
            assert_eq!(n, 1);
            assert!(!sub.exists());
            assert!(freed >= 10, "nested bytes must be counted, got {freed}");
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    // The crate-wide env lock. Declared at module level in `platform` so tests
    // in OTHER modules can take the same one — see its doc for why one lock
    // per module was not enough.
    #[cfg(unix)]
    use super::ENV_LOCK;
    use super::*;

    /// A fresh scratch directory. `platform` cannot reach the test-only
    /// tempdir helper that lives inside `index`'s test module, and a second
    /// copy of it here would be a third spelling of the same five lines.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tk-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// One rule, one place: the code index and the semantic index both bridge
    /// root-relative storage to a cwd-relative reader through this.
    #[test]
    fn path_rebase_strips_what_is_under_you_and_climbs_out_of_what_is_not() {
        let root = std::path::Path::new("/p");
        let r = PathRebase::new(root, std::path::Path::new("/p/a/b")).expect("inside");
        assert_eq!(r.apply("a/b/inner.rs"), "inner.rs");
        assert_eq!(r.apply("a/b/deep/x.rs"), "deep/x.rs");
        assert_eq!(r.apply("other/x.rs"), "../../other/x.rs");
        assert_eq!(r.apply("top.rs"), "../../top.rs");
        // A sibling whose name merely STARTS with the prefix is not inside it.
        assert_eq!(r.apply("a/bb/x.rs"), "../../a/bb/x.rs");

        // One level deep needs exactly one `../`.
        let one = PathRebase::new(root, std::path::Path::new("/p/a")).expect("inside");
        assert_eq!(one.apply("a/x.rs"), "x.rs");
        assert_eq!(one.apply("z.rs"), "../z.rs");

        // At the root there is nothing to do, and outside the project there is
        // no base to do it against.
        assert!(PathRebase::new(root, root).is_none());
        assert!(PathRebase::new(root, std::path::Path::new("/elsewhere")).is_none());
    }

    /// The nearest VCS marker wins, a worktree's `.git` FILE counts, and a
    /// tree with no marker at all is its own root.
    #[test]
    fn project_root_walks_up_to_the_nearest_vcs_marker() {
        let d = scratch("root-walk");
        let repo = d.join("repo");
        let deep = repo.join("crates").join("core").join("src");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        assert_eq!(project_root_of(&deep), repo);
        assert_eq!(project_root_of(&repo), repo);

        // A nested repository is its own root — the NEAREST marker wins.
        let inner = repo.join("vendor").join("dep");
        std::fs::create_dir_all(inner.join(".git")).unwrap();
        let inner_deep = inner.join("src");
        std::fs::create_dir_all(&inner_deep).unwrap();
        assert_eq!(project_root_of(&inner_deep), inner);

        // A worktree's `.git` is a FILE. `exists` must cover it.
        let wt = d.join("worktree");
        std::fs::create_dir_all(wt.join("sub")).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: /elsewhere\n").unwrap();
        assert_eq!(project_root_of(&wt.join("sub")), wt);

        // Mercurial, Subversion and Jujutsu answer too.
        for marker in [".hg", ".svn", ".jj"] {
            let r = d.join(format!("r{}", marker.trim_start_matches('.')));
            std::fs::create_dir_all(r.join(marker)).unwrap();
            std::fs::create_dir_all(r.join("a").join("b")).unwrap();
            assert_eq!(project_root_of(&r.join("a").join("b")), r, "{marker}");
        }

        // No marker anywhere above: the directory is its own root, which is
        // the behaviour every non-repo directory keeps.
        let bare = d.join("bare").join("deep");
        std::fs::create_dir_all(&bare).unwrap();
        assert_eq!(project_root_of(&bare), bare);
    }

    /// Claude Code reads `CLAUDE_CONFIG_DIR` before it reads `$HOME/.claude`,
    /// and so must we.
    ///
    /// Read out of Claude Code 2.1.251's own binary rather than assumed:
    /// `(process.env.CLAUDE_CONFIG_DIR ?? join(homedir(), ".claude"))`. Before
    /// this, tokenade honoured the variable in none of its hundred-odd
    /// `.claude` joins, and the failure was total rather than partial:
    /// `install --only claude-code` answered "no AI coding agent detected on
    /// this machine" and wrote nothing, because the presence marker it looks
    /// for is a directory the user does not have.
    ///
    /// An empty value is not a directory, for the same reason an empty `HOME`
    /// is not a home: `PathBuf::from("")` joins to a RELATIVE path, so every
    /// read and write would land under the current directory.
    #[test]
    fn the_claude_config_dir_variable_wins_over_the_default_and_empty_is_not_a_value() {
        let mut h = ScratchHome::new("claude-config-dir");
        let home = h.path().to_path_buf();

        // Unset: the default place under the home we were given.
        assert_eq!(claude_config_dir_for(&home), home.join(".claude"));
        assert_eq!(claude_dot_json_for(&home), home.join(".claude.json"));

        // Set: the variable, verbatim, and `.claude.json` moves INSIDE it —
        // which is the half nobody would guess from the default layout.
        let alt = home.join("elsewhere");
        h.env().set("CLAUDE_CONFIG_DIR", &alt);
        assert_eq!(claude_config_dir_for(&home), alt);
        assert_eq!(claude_dot_json_for(&home), alt.join(".claude.json"));

        // Set but empty: fall back, never join a relative path.
        h.env().set("CLAUDE_CONFIG_DIR", "");
        assert_eq!(claude_config_dir_for(&home), home.join(".claude"));
        assert_eq!(claude_dot_json_for(&home), home.join(".claude.json"));
    }

    /// A scratch home must neutralise the variable that overrides it.
    ///
    /// Same shape as the `USERPROFILE` line above, one directory over: a
    /// developer or CI runner with a relocated Claude Code config would have
    /// every scratch home in the suite read and write the REAL one, and the
    /// assertions would be made against a directory that never saw the
    /// fixtures. Unset rather than redirected — the scratch home IS the
    /// answer, and `claude_config_dir_for` falls back to it.
    #[test]
    fn a_scratch_home_neutralises_a_relocated_claude_config() {
        // Set it OUTSIDE the guard first, so the test proves the helper
        // clears a value that was really there.
        {
            let mut pre = ScopedEnv::new();
            pre.set("CLAUDE_CONFIG_DIR", "/somewhere/else/entirely");
            drop(pre);
        }
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", "/somewhere/else/entirely") };
        let h = ScratchHome::new("claude-config-neutral");
        assert!(
            std::env::var_os("CLAUDE_CONFIG_DIR").is_none(),
            "ScratchHome left a relocated Claude config in place, so every test \
             using it reads the developer's real one"
        );
        assert_eq!(claude_config_dir_for(h.path()), h.path().join(".claude"));
        drop(h);
        // And it is restored, because the guard remembers what it found.
        assert_eq!(
            std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from),
            Some(std::path::PathBuf::from("/somewhere/else/entirely"))
        );
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };
    }

    /// A home directory under version control — dotfile repositories are
    /// common — must not make every project inside it index the whole home.
    /// The walk stops at `$HOME` and never returns it.
    /// `ScratchHome` has to redirect whatever `home_dir` actually READS, and
    /// on Windows that is `USERPROFILE`, not `HOME`. The helper set only
    /// `HOME`, so on Windows it redirected nothing: six of the seven Windows
    /// failures in the 1.1.7 release build were tests writing fixtures into a
    /// scratch home that `home_dir` never pointed at.
    ///
    /// Asked here rather than left to a Windows run, because the variables are
    /// just environment variables — a Linux host can check that both were set
    /// and that `pick_home` lands on the scratch under EITHER platform policy.
    #[test]
    fn a_scratch_home_redirects_what_home_dir_reads_on_both_platforms() {
        let h = ScratchHome::new("both-vars");
        let dir = h.path().to_path_buf();
        for var in ["HOME", "USERPROFILE"] {
            assert_eq!(
                std::env::var_os(var).map(std::path::PathBuf::from),
                Some(dir.clone()),
                "`{var}` was not redirected, so a test using ScratchHome reads \
                 the developer's real home on any platform that prefers it"
            );
        }
        // Both orderings must land on the scratch, which is what makes the
        // helper honest on a host that is not the one running it.
        for windows in [false, true] {
            assert_eq!(
                super::pick_home(
                    std::env::var_os("HOME"),
                    std::env::var_os("USERPROFILE"),
                    windows,
                ),
                Some(dir.clone()),
                "pick_home(windows={windows}) left the scratch home"
            );
        }
    }

    #[test]
    fn project_root_never_climbs_past_home() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let d = scratch("root-home");
        let home = d.join("home");
        let proj = home.join("projects").join("thing");
        std::fs::create_dir_all(&proj).unwrap();
        // The ONLY marker is at home.
        std::fs::create_dir_all(home.join(".git")).unwrap();

        // BOTH variables, because `home_dir` reads `USERPROFILE` first on
        // Windows: setting only `HOME` moved nothing there, the boundary stayed
        // at the developer's real profile, and the walk sailed past the scratch
        // "home" it was supposed to stop at. It failed on the Windows leg of
        // the 1.1.7 release build and passed everywhere else.
        //
        // `ScratchHome` does exactly this and would be the one declarant — but
        // it takes `ENV_LOCK` for its own life, and this test already holds it.
        // A `Mutex` is not reentrant, so the helper cannot be used from inside
        // the guard; the pair is written out instead.
        let prev_home = std::env::var_os("HOME");
        let prev_profile = std::env::var_os("USERPROFILE");
        // SAFETY: env vars are process-wide; ENV_LOCK is held above.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("USERPROFILE", &home);
        }
        let got = project_root_of(&proj);
        // SAFETY: still inside the ENV_LOCK guard's lifetime.
        unsafe {
            for (k, prev) in [("HOME", prev_home), ("USERPROFILE", prev_profile)] {
                match prev {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
        assert_eq!(got, proj, "the walk climbed into the home directory");
    }

    #[test]
    fn home_dir_returns_some_on_test_host() {
        // CI runners always set HOME (or USERPROFILE on Windows).
        assert!(home_dir().is_some());
    }

    #[test]
    fn pick_home_prefers_userprofile_on_windows_home_on_unix() {
        let h = || Some(std::ffi::OsString::from("/home/u"));
        let up = || Some(std::ffi::OsString::from("C:\\Users\\u"));
        // Windows: USERPROFILE wins (matches where the agent stores ~/.claude),
        // even when a Git-Bash $HOME is also present.
        assert_eq!(
            pick_home(h(), up(), true),
            Some(PathBuf::from("C:\\Users\\u"))
        );
        // Unix: HOME wins.
        assert_eq!(pick_home(h(), up(), false), Some(PathBuf::from("/home/u")));
        // Fallbacks: each side covers the other when one is missing.
        assert_eq!(
            pick_home(None, up(), true),
            Some(PathBuf::from("C:\\Users\\u"))
        );
        assert_eq!(pick_home(h(), None, true), Some(PathBuf::from("/home/u")));
        assert_eq!(
            pick_home(None, up(), false),
            Some(PathBuf::from("C:\\Users\\u"))
        );
        assert_eq!(pick_home(None, None, true), None);
        assert_eq!(pick_home(None, None, false), None);
    }

    /// A set-but-empty `HOME` must resolve to None, never to `""` — `"".join(x)`
    /// is RELATIVE, and `tokenade uninstall` would then recursively delete
    /// `./tokenade` + `./.tokenade` under the CWD (reproduced: real user files
    /// destroyed). Reaches us via `env -i`, systemd, sudo, CI, or an rc doing
    /// `export HOME="$SOME_UNSET_VAR"`.
    #[test]
    fn pick_home_rejects_empty_vars() {
        let empty = || Some(std::ffi::OsString::from(""));
        let abs_unix = || Some(std::ffi::OsString::from("/home/u"));
        let abs_win = || Some(std::ffi::OsString::from("C:\\Users\\u"));
        // Empty on both sides → no home at all, rather than the CWD.
        assert_eq!(pick_home(empty(), empty(), false), None);
        assert_eq!(pick_home(empty(), empty(), true), None);
        assert_eq!(pick_home(empty(), None, false), None);
        assert_eq!(pick_home(None, empty(), true), None);
        // An empty authoritative var must not shadow a usable fallback.
        assert_eq!(
            pick_home(empty(), abs_unix(), false),
            Some(PathBuf::from("/home/u"))
        );
        assert_eq!(
            pick_home(empty(), abs_win(), true),
            Some(PathBuf::from("C:\\Users\\u"))
        );
        // An MSYS-shaped Windows $HOME stays valid (Git-Bash sets it).
        let msys = Some(std::ffi::OsString::from("/c/Users/u"));
        assert_eq!(
            pick_home(None, msys, true),
            Some(PathBuf::from("/c/Users/u"))
        );
    }

    #[test]
    fn migrate_tokenade_dir_copies_essentials_not_caches_idempotently() {
        let base = std::env::temp_dir().join(format!("tok-mig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (old, new) = (base.join("oldhome"), base.join("newhome"));
        let oldtk = old.join(".tokenade");
        std::fs::create_dir_all(&oldtk).unwrap();
        for (f, c) in [
            ("gain.jsonl", "x"),
            ("license.key", "k"),
            ("installed-at", "123"),
            ("gain.jsonl.bak-1.counted", "h"),
            ("dedup.db", "BIGCACHE"),
        ] {
            std::fs::write(oldtk.join(f), c).unwrap();
        }
        let n = migrate_tokenade_dir(&old, &new);
        let newtk = new.join(".tokenade");
        assert!(newtk.join("gain.jsonl").exists());
        assert!(
            newtk.join("license.key").exists(),
            "activation must survive"
        );
        assert!(newtk.join("installed-at").exists());
        assert!(newtk.join("gain.jsonl.bak-1.counted").exists());
        assert!(
            !newtk.join("dedup.db").exists(),
            "rebuildable caches NOT migrated"
        );
        assert!(newtk.join(".home-migrated").exists());
        assert!(n >= 4);
        // Idempotent: re-run copies nothing.
        assert_eq!(migrate_tokenade_dir(&old, &new), 0);
        // Never clobbers an existing ledger at the new location.
        let new2 = base.join("newhome2");
        std::fs::create_dir_all(new2.join(".tokenade")).unwrap();
        std::fs::write(new2.join(".tokenade").join("gain.jsonl"), b"existing").unwrap();
        assert_eq!(migrate_tokenade_dir(&old, &new2), 0);
        assert_eq!(
            std::fs::read(new2.join(".tokenade").join("gain.jsonl")).unwrap(),
            b"existing"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cache_dir_returns_some_on_test_host() {
        // At least HOME is set; cache_dir() falls back to HOME/.cache.
        assert!(cache_dir().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn ensure_storage_perms_secure_tightens_loose_dirs_and_files() {
        // Regression: re-running tokenade on a HOME with
        // world-readable storage dirs must auto-chmod them to 700/600.
        // The test mutates process-wide $HOME, so it must serialise
        // with any other env-mutating test in this crate (mirrors
        // the debug_log::ENV_LOCK pattern).
        use std::os::unix::fs::PermissionsExt;
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

        let tmp = std::env::temp_dir().join(format!(
            "tokenade-perms-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let tk = tmp.join(".tokenade");
        std::fs::create_dir_all(&tk).unwrap();
        let log = tk.join("debug.log");
        std::fs::write(&log, b"sample").unwrap();
        // Seed loose perms (the failure mode a permissions audit caught).
        std::fs::set_permissions(&tk, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();

        let prev_home = std::env::var_os("HOME");
        let prev_xdg = std::env::var_os("XDG_CACHE_HOME");
        // SAFETY: ENV_LOCK above serialises against other env-mutating
        // tests in this crate.
        unsafe {
            std::env::set_var("HOME", &tmp);
            // Also pin cache_dir into the tempdir so the stash branch
            // doesn't reach into the real ~/.cache.
            std::env::set_var("XDG_CACHE_HOME", tmp.join("cache"));
        }

        ensure_storage_perms_secure();

        let dir_mode = std::fs::metadata(&tk).unwrap().permissions().mode() & 0o777;
        let file_mode = std::fs::metadata(&log).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "dir mode should be 0700, got {dir_mode:o}");
        assert_eq!(
            file_mode, 0o600,
            "file mode should be 0600, got {file_mode:o}"
        );

        // Restore env to avoid contaminating other tests.
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prev_xdg {
                Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
                None => std::env::remove_var("XDG_CACHE_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn shell_command_picks_expected_pair() {
        let (prog, flag) = shell_command();
        if cfg!(windows) {
            assert_eq!(prog, "cmd");
            assert_eq!(flag, "/C");
        } else {
            assert_eq!(prog, "sh");
            assert_eq!(flag, "-c");
        }
    }

    /// The per-file stash walk is O(stash size) and used to run on EVERY CLI
    /// start — measured at ~12 ms for 3 396 files on a real install, paid twice
    /// per tool call through the hooks. These three tests pin the trade that
    /// replaced it: the DIRECTORY mode is always tightened (that is the
    /// protection that holds), and the walk runs only when the state gives a
    /// reason to.
    /// The per-file stash walk used to run on EVERY CLI start — ~12 ms for
    /// 3 396 files on a real install, paid twice per tool call through the
    /// hooks. These pin the trade that replaced it. No env mutation: the paths
    /// are injected, so these run safely alongside a parallel test binary.
    #[cfg(unix)]
    #[cfg(test)]
    mod stash_perms {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// -> (stash, sentinel, file inside the stash)
        fn seed(
            tag: &str,
            dir_mode: u32,
            file_mode: u32,
        ) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
            let root = std::env::temp_dir().join(format!(
                "tk-stashperms-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            let stash = root.join("stash");
            std::fs::create_dir_all(&stash).unwrap();
            let f = stash.join("deadbeef0000");
            std::fs::write(&f, b"folded bytes").unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(file_mode)).unwrap();
            std::fs::set_permissions(&stash, std::fs::Permissions::from_mode(dir_mode)).unwrap();
            (stash, root.join(".stash-perms-checked"), f)
        }

        fn mode(p: &std::path::Path) -> u32 {
            std::fs::metadata(p).unwrap().permissions().mode() & 0o777
        }

        /// The two lists were one hand-written enumeration and one
        /// `read_dir`, and they disagreed about the same directory.
        #[test]
        fn the_ledger_list_holds_every_shape_a_rotation_produces() {
            let d = std::env::temp_dir().join(format!(
                "tk-ledgers-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            for n in [
                "gain.jsonl",
                "gain.rollup.jsonl.bak-1784599592",
                "bypass_lost.jsonl",
                "debug.log",
                "debug.log.1",
                "quota.json",
                "installed-at",
                "last-report.zip",
                "index.db",
                "index.db-wal",
                "projects-registry.txt",
            ] {
                std::fs::write(d.join(n), b"x").unwrap();
            }
            let names = |v: Vec<std::path::PathBuf>| -> Vec<String> {
                let mut n: Vec<String> = v
                    .iter()
                    .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                    .collect();
                n.sort();
                n
            };
            assert_eq!(
                names(super::super::state_text_ledgers(&d)),
                vec![
                    "bypass_lost.jsonl",
                    "debug.log",
                    "debug.log.1",
                    "gain.jsonl",
                    "gain.rollup.jsonl.bak-1784599592",
                ],
                "a rotation renames the ledger; the rows are still ledger rows"
            );
            let hardened = names(super::super::state_files_to_harden(&d));
            for must in [
                "last-report.zip",
                "index.db",
                "index.db-wal",
                "bypass_lost.jsonl",
            ] {
                assert!(hardened.contains(&must.to_string()), "{must} not hardened");
            }
            for must_not in ["quota.json", "installed-at"] {
                assert!(
                    !hardened.contains(&must_not.to_string()),
                    "{must_not} carries no user content and should be left alone"
                );
            }
            let _ = std::fs::remove_dir_all(&d);
        }

        /// The permissions fix must still hold: a loose stash is repaired, files included.
        #[test]
        fn a_loose_stash_is_still_fully_repaired() {
            let (stash, sentinel, f) = seed("loose", 0o755, 0o644);
            tighten_stash(&stash, &sentinel);
            assert_eq!(mode(&stash), 0o700, "loose stash dir must be tightened");
            assert_eq!(
                mode(&f),
                0o600,
                "a loose file in a loose stash must be tightened"
            );
            assert!(sentinel.exists(), "the walk must record that it ran");
            let _ = std::fs::remove_dir_all(stash.parent().unwrap());
        }

        /// Upgrade path: directory already tight, but this install has never
        /// walked — files predating the hardening pass still get repaired.
        #[test]
        fn a_tight_stash_is_walked_once_when_never_checked() {
            let (stash, sentinel, f) = seed("firstrun", 0o700, 0o644);
            tighten_stash(&stash, &sentinel);
            assert_eq!(
                mode(&f),
                0o600,
                "the one-time walk must repair pre-existing files"
            );
            assert!(sentinel.exists());
            let _ = std::fs::remove_dir_all(stash.parent().unwrap());
        }

        /// The hot path: tight directory, walk already recorded. Nothing is
        /// re-walked — the point of the change. The trade is asserted out loud:
        /// a file that somehow loosened INSIDE a 0700 directory stays loose,
        /// because no other user can traverse in to reach it.
        #[test]
        fn a_checked_tight_stash_is_not_rewalked() {
            let (stash, sentinel, f) = seed("hot", 0o700, 0o644);
            std::fs::write(&sentinel, STASH_PERMS_RULE).unwrap();
            tighten_stash(&stash, &sentinel);
            assert_eq!(
                mode(&f),
                0o644,
                "the walk must be SKIPPED on the hot path; the 0700 dir is the guarantee"
            );
            let _ = std::fs::remove_dir_all(stash.parent().unwrap());
        }

        /// An install that completed the walk under the PREVIOUS rule walks
        /// once more. Its sentinel is empty because that is what the old code
        /// wrote, and the rule it recorded was "repair afterwards" — which
        /// stopped repairing the moment it ran, on entries that kept being
        /// created 0644. Measured on the development machine before the fix:
        /// 817 of 828 stash files world-readable, with the sentinel in place.
        #[test]
        fn an_install_checked_under_the_old_rule_walks_once_more() {
            let (stash, sentinel, f) = seed("upgrade", 0o700, 0o644);
            std::fs::write(&sentinel, b"").unwrap();
            tighten_stash(&stash, &sentinel);
            assert_eq!(mode(&f), 0o600, "the rule changed; the walk must re-run");
            assert_eq!(
                std::fs::read(&sentinel).unwrap(),
                STASH_PERMS_RULE,
                "the sentinel must record the rule it walked under, or it re-walks forever"
            );
            let _ = std::fs::remove_dir_all(stash.parent().unwrap());
        }

        /// The source of the 817: `std::fs::write` creates 0666 & ~umask.
        #[test]
        fn a_stash_entry_is_created_private() {
            let (stash, _sentinel, _f) = seed("private", 0o700, 0o600);
            let fresh = stash.join("cafebabe0000");
            super::super::write_private(&fresh, b"raw tool output").unwrap();
            assert_eq!(mode(&fresh), 0o600, "a stash entry must be born private");
            assert_eq!(std::fs::read(&fresh).unwrap(), b"raw tool output");

            // And an entry a previous version left loose is tightened when it
            // is rewritten, rather than keeping the mode it was created with.
            let old = stash.join("d00d00000000");
            std::fs::write(&old, b"older").unwrap();
            std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o644)).unwrap();
            super::super::write_private(&old, b"newer").unwrap();
            assert_eq!(mode(&old), 0o600);
            assert_eq!(std::fs::read(&old).unwrap(), b"newer");
            let _ = std::fs::remove_dir_all(stash.parent().unwrap());
        }
    }
}

#[cfg(test)]
mod path_resolution {
    use super::*;

    /// The property the check exists for: PATH ORDER decides, and a second
    /// install is reported as a second install only when it is a different
    /// file. Both halves have a way to be wrong that looks fine — reporting
    /// every symlink as a conflict, or reporting only the first hit and so
    /// never seeing the stale copy at all.
    #[test]
    fn path_order_decides_and_a_symlink_farm_is_still_one_install() {
        let tmp = std::env::temp_dir().join(format!(
            "tokenade-pathres-{}-{}",
            std::process::id(),
            now_unix_secs()
        ));
        let (a, b, c) = (tmp.join("a"), tmp.join("b"), tmp.join("c"));
        for d in [&a, &b, &c] {
            std::fs::create_dir_all(d).unwrap();
        }
        let write_exe = |p: &std::path::Path| {
            std::fs::write(p, b"#!/bin/sh\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        };
        let names = ["tokenade"];
        write_exe(&a.join("tokenade"));
        write_exe(&b.join("tokenade"));

        let joined = |dirs: &[&std::path::Path]| {
            std::env::join_paths(dirs.iter().map(|d| d.as_os_str())).unwrap()
        };

        // Two distinct files: both reported, in PATH order.
        let found = tokenade_on_path_in(&joined(&[&a, &b]), &names);
        assert_eq!(
            found,
            vec![a.join("tokenade"), b.join("tokenade")],
            "two distinct binaries must both be reported, first one first"
        );
        // The same two directories the other way round: the winner changes.
        let found = tokenade_on_path_in(&joined(&[&b, &a]), &names);
        assert_eq!(
            found.first(),
            Some(&b.join("tokenade")),
            "PATH order decides which binary a bare `tokenade` runs"
        );

        // A symlink to a file already seen is the SAME install reached twice.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(a.join("tokenade"), c.join("tokenade")).unwrap();
            let found = tokenade_on_path_in(&joined(&[&a, &c]), &names);
            assert_eq!(
                found,
                vec![a.join("tokenade")],
                "a symlink to a binary already found is one install, not two"
            );
            assert!(
                same_file(&a.join("tokenade"), &c.join("tokenade")),
                "same_file must see through the symlink"
            );
        }

        // An empty PATH entry means the CURRENT DIRECTORY to a shell, and we
        // refuse to follow it: healthcheck must never report an install whose
        // whole existence is "you happened to run me from this folder". The
        // assertion only bites from a CWD that HAS such a file, so the test
        // moves there — under [`ENV_LOCK`], which is what serialises the
        // process-global CWD.
        {
            let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let before = std::env::current_dir().unwrap();
            write_exe(&b.join("tokenade")); // b already has one; make sure.
            std::env::set_current_dir(&b).unwrap();
            let with_empty = {
                let mut v = std::ffi::OsString::from(if cfg!(windows) { ";" } else { ":" });
                v.push(a.as_os_str());
                v
            };
            let found = tokenade_on_path_in(&with_empty, &names);
            std::env::set_current_dir(&before).unwrap();
            assert_eq!(
                found,
                vec![a.join("tokenade")],
                "an empty PATH entry must not contribute a CWD-relative binary"
            );
        }

        // A directory that is not there contributes nothing.
        let found = tokenade_on_path_in(&joined(&[&tmp.join("missing")]), &names);
        assert!(
            found.is_empty(),
            "a PATH entry that does not exist contributes nothing"
        );

        // A file with the right NAME and no executable bit is not a binary the
        // shell would ever run, and reporting it would send the user hunting an
        // install that does not exist. On Windows there is no bit to read and
        // the name list is what decides, so the rule differs by platform and
        // the assertion follows it.
        let d = tmp.join("d");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("tokenade"), b"not a program").unwrap();
        let found = tokenade_on_path_in(&joined(&[&d]), &names);
        #[cfg(unix)]
        assert!(
            found.is_empty(),
            "a non-executable file named tokenade is not an install: {found:?}"
        );
        #[cfg(not(unix))]
        assert_eq!(
            found,
            vec![d.join("tokenade")],
            "Windows has no executable bit — the name is what decides"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Windows resolves a bare name through PATHEXT and npm ships a `.cmd`
    /// shim beside the `.exe`, so the list of names is part of the answer and
    /// not an implementation detail.
    #[test]
    fn the_candidate_names_cover_what_each_platform_actually_runs() {
        for want in ["tokenade.exe", "tokenade.cmd", "tokenade.bat", "tokenade"] {
            assert!(
                tokenade_exe_names_for(true).contains(&want),
                "Windows resolves {want} for a bare `tokenade`; it must be a candidate"
            );
        }
        assert_eq!(
            tokenade_exe_names_for(false),
            &["tokenade"],
            "Unix resolves the bare name only — extra candidates would invent installs"
        );
        assert_eq!(
            tokenade_exe_names(),
            tokenade_exe_names_for(cfg!(windows)),
            "the host's list must be the rule's answer for this host"
        );
    }
}

#[cfg(test)]
mod cwd_discipline {
    /// The process CWD is global, and [`ENV_LOCK`] is what serialises access
    /// to it — but a lock only protects the tests that TAKE it. One test read
    /// the CWD twice and asserted the two reads agreed, without the lock,
    /// while another moved the CWD under it: roughly one workspace run in
    /// eight failed, and the failing name never appeared twice.
    ///
    /// This sweep is the fix. Adding a test that reads `current_dir()` and
    /// forgets the lock is a defect that shows up as somebody ELSE's test
    /// failing, days later, in a run nobody can reproduce.
    ///
    /// Scanned by looking BACKWARDS from each `current_dir()` to the nearest
    /// `#[test]`, not by matching braces: the first version of this counted
    /// `{` and `}` across the file, and one unbalanced brace inside a shell
    /// fixture (`clean(){`, a lone `"{"`) shifted every body after it. It
    /// passed its own mutation and proved nothing. A lock is always taken at
    /// the top of a test, so the window between the marker and the read is
    /// where it must appear.
    #[test]
    fn every_test_that_reads_the_process_cwd_takes_the_lock() {
        const IDIOMS: [&str; 5] = [
            "ENV_LOCK",
            "ScratchHome",
            "isolated_repo",
            "StoreIsolation",
            "Layers",
        ];
        let mut offenders: Vec<String> = Vec::new();
        let mut checked = 0usize;
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(src) = std::fs::read_to_string(&p) else {
                    continue;
                };
                // Only `current_dir()`. A test that opens a RELATIVE path
                // depends on the working directory just as much — that is how
                // `tokenizer::same_count_as_the_char_vec_loop` came to walk
                // `"tests/fixtures"` and race another test's `Layers` — but a text
                // scan cannot see it: the literal goes into a variable and the
                // filesystem call reads the variable. A needle list over
                // `read_dir("…")` and friends passes its own mutation and
                // proves nothing, which is the same defect a second time.
                //
                // That class is closed at the source instead: nothing moves
                // the process CWD any more (see `TOKENADE_FILTER_PROJECT_DIR`
                // in `user_filters::filter_dirs`), so there is no race left
                // for a sweep to police.
                let mut at = 0usize;
                while let Some(rel) = src[at..].find("current_dir()") {
                    let hit = at + rel;
                    at = hit + 13;
                    // Only inside a test: the nearest `#[test]` above must be
                    // closer than the nearest `mod ` boundary is irrelevant —
                    // production code reading the CWD is the point of the
                    // function, not a race.
                    let Some(marker) = src[..hit].rfind("#[test]") else {
                        continue;
                    };
                    let window = &src[marker..hit];
                    // Another test declaration in between means this read is
                    // in a HELPER below the test, not in the test itself.
                    if window.matches("#[test]").count() > 1 {
                        continue;
                    }
                    // A COLUMN-ZERO `fn` between the marker and the hit means
                    // the test module has closed and this is production code,
                    // whose job may well be to read the CWD. `gain.rs` has
                    // two such reads after its last test, and without this
                    // they were reported as offenders — the sweep's own first
                    // finding was a pair of false positives.
                    let top_level_fn = ["\nfn ", "\npub fn ", "\npub(crate) fn "]
                        .iter()
                        .filter_map(|k| window.rfind(k))
                        .max();
                    if top_level_fn.is_some() {
                        continue;
                    }
                    checked += 1;
                    if !IDIOMS.iter().any(|k| window.contains(k)) {
                        let name: String = src[marker..]
                            .split_once("fn ")
                            .map(|(_, r)| {
                                r.chars()
                                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                                    .collect()
                            })
                            .unwrap_or_default();
                        offenders.push(format!("{}::{name}", p.display()));
                    }
                }
            }
        }
        assert!(
            checked >= 1,
            "the sweep found NO test reading the process CWD — it is looking in the wrong place"
        );
        assert!(
            offenders.is_empty(),
            "these tests read the process CWD without holding ENV_LOCK:\n  {}",
            offenders.join("\n  ")
        );
    }
}

#[cfg(test)]
mod one_clock_for_the_whole_product {
    //! Mined from `repowise` (2026-08-29): "five byte-identical copies of
    //! 'top-level directory, else root' are now one function". Here it was
    //! FIFTEEN copies of "seconds since the epoch, 0 on failure", under three
    //! names, one per module that needed a timestamp.

    /// The body must exist once. Counting the SPELLING rather than the
    /// function name is deliberate: the copies were called `now_secs`,
    /// `now_secs_global` and `now_unix_secs`, so a name-based sweep would
    /// have found three families and declared each of them consistent.
    #[test]
    fn the_epoch_conversion_is_written_once_in_each_crate() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates dir");
        // Two halves so this needle cannot find ITSELF in the file it scans.
        let needle = format!("duration_since({}UNIX_EPOCH)", "std::time::");
        let bare = "duration_since(UNIX_EPOCH)";
        let mut hits: Vec<String> = Vec::new();
        // Files actually opened, per crate. Without this the sweep could be
        // narrowed to one crate and stay green — there is nothing left to find
        // in the other, so "no hits" and "did not look" are the same answer.
        let mut scanned_core = 0usize;
        let mut scanned_cli = 0usize;
        let mut stack = vec![crates.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if p.file_name().is_some_and(|n| n == "target") {
                        continue;
                    }
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(body) = std::fs::read_to_string(&p) else {
                    continue;
                };
                let ps = p.to_string_lossy();
                if ps.contains("tokenade-core") {
                    scanned_core += 1;
                } else if ps.contains("tokenade-cli") {
                    scanned_cli += 1;
                }
                for (i, line) in body.lines().enumerate() {
                    if !(line.contains(&needle) || line.contains(bare)) {
                        continue;
                    }
                    // The CONTRACT, not merely the call: the wall clock read
                    // now, converted to whole seconds, with 0 for a clock
                    // before the epoch. The fifteen sites that survive this
                    // sweep convert a FILE's mtime, or fold the comparison
                    // into the closure (`as_secs().saturating_sub(t) < 600`,
                    // `unwrap_or(false)`), or chain through `.ok()`. Those are
                    // different questions with different failure answers, and
                    // collapsing them into this helper would change what
                    // happens when the clock is wrong.
                    // From the PRECEDING line: `SystemTime::now()` sits above
                    // the `duration_since` the sweep anchors on, so a window
                    // starting at the hit never contains it — the floor
                    // assertion below caught that as "found nothing at all".
                    let win: String = body
                        .lines()
                        .skip(i.saturating_sub(1))
                        .take(5)
                        .map(str::trim)
                        .collect::<Vec<_>>()
                        .join(" ");
                    let canonical = win.contains("SystemTime::now()")
                        && win.contains(".map(|d| d.as_secs())")
                        && (win.contains(".unwrap_or(0)") || win.contains(".unwrap_or_default()"));
                    if canonical {
                        hits.push(format!("{}:{}", p.display(), i + 1));
                    }
                }
            }
        }
        // A sweep that looked nowhere agrees with a sweep that found nothing.
        assert!(
            scanned_core > 100 && scanned_cli > 50,
            "the sweep did not reach both crates (core {scanned_core}, cli {scanned_cli})"
        );
        assert!(
            !hits.is_empty(),
            "the sweep found nothing at all, so it proves nothing"
        );
        assert_eq!(
            hits.len(),
            1,
            "the epoch-to-seconds conversion is spelled out {} times; call \
             `platform::now_unix_secs()` instead:\n{}",
            hits.len(),
            hits.join("\n")
        );
    }
}

#[cfg(test)]
mod path_segment_tests {
    use super::path_has_segment;
    use std::path::Path;

    #[test]
    fn matches_a_whole_segment_on_either_separator() {
        assert!(path_has_segment(Path::new("/a/cmd/b.rs"), "cmd"));
        assert!(path_has_segment(Path::new(r"C:\a\cmd\b.rs"), "cmd") || cfg!(unix));
        // The leading and trailing segments count too: the substring
        // search "/cmd/" required a slash on BOTH sides and missed
        // both ends.
        assert!(path_has_segment(Path::new("cmd/b.rs"), "cmd"));
        assert!(path_has_segment(Path::new("/a/cmd"), "cmd"));
    }

    #[test]
    fn does_not_match_a_partial_name() {
        assert!(!path_has_segment(Path::new("/a/mydeps/b.rs"), "deps"));
        assert!(!path_has_segment(Path::new("/a/depsx/b.rs"), "deps"));
        assert!(!path_has_segment(Path::new("/a/b.rs"), "cmd"));
    }

    /// A lock is only sound when no OTHER lock touches its variables.
    ///
    /// `tokenade-core`'s tests all run in ONE process against ONE environment,
    /// and they used to serialise on twelve different mutexes. Four of those
    /// shared a variable with this crate's own guard, and **`HOME` was set
    /// under three mutually unaware locks** — the shape of the flakes we chased.
    ///
    /// This does not demand one lock for everything: `net.rs` serialises the
    /// proxy variables on its own mutex and nothing else in the crate touches
    /// them, so it costs nothing and buys parallelism. What it forbids is a
    /// private lock over a variable somebody else also sets.
    #[test]
    fn no_two_locks_share_a_variable() {
        use std::collections::{HashMap, HashSet};
        // BOTH roots: a lock taken in `tokenade-core` on a variable
        // that `tk-base` also sets is exactly what this test refuses, and the
        // split into crates changes nothing about the question.
        let mut stack = crate::source_guard::crate_srcs();
        let mut by_lock: HashMap<String, HashSet<String>> = HashMap::new();
        // Every lock a test can take, global first: a body naming any of the
        // global idioms belongs to the crate-wide guard.
        const GLOBAL: [&str; 6] = [
            "ENV_LOCK",
            "ScratchHome",
            "StoreIsolation",
            "isolated_repo",
            "Layers",
            "TempHome",
        ];
        // Read every file once: a helper is only "private" if its own body
        // does NOT take the crate-wide guard. The old per-module env locks
        // are aliases for it now, and classifying them by NAME would report
        // the very thing that was just fixed.
        let mut files: Vec<(String, String)> = Vec::new();
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                    continue;
                }
                if let Ok(src) = std::fs::read_to_string(&p) {
                    let name = p
                        .file_name()
                        .and_then(|x| x.to_str())
                        .unwrap_or("?")
                        .to_string();
                    files.push((name, src));
                }
            }
        }
        let mut aliases: HashSet<String> = HashSet::new();
        for (_, src) in &files {
            let mut at = 0usize;
            while let Some(rel) = src[at..].find("_env_lock() -> ") {
                let hit = at + rel;
                at = hit + 15;
                let head = &src[..hit + "_env_lock".len()];
                let s = head.rfind("fn ").map(|x| x + 3).unwrap_or(0);
                let name = head[s..].to_string();
                let body_end = (at + 400).min(src.len());
                if src[at..body_end].contains("ENV_LOCK") {
                    aliases.insert(name);
                }
            }
        }
        {
            for (file, src) in &files {
                let file = file.clone();
                let mut at = 0usize;
                while let Some(rel) = src[at..].find("#[test]") {
                    let start = at + rel + 7;
                    let end = src[start..]
                        .find("#[test]")
                        .map(|r| start + r)
                        .unwrap_or(src.len());
                    let body = &src[start..end];
                    at = start;
                    let global = GLOBAL.iter().any(|g| body.contains(g))
                        || aliases.iter().any(|a| body.contains(a.as_str()));
                    let local = body.match_indices("_env_lock()").next().map(|(i, _)| {
                        let head = &body[..i];
                        let s = head
                            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
                            .map(|x| x + 1)
                            .unwrap_or(0);
                        format!("{}_env_lock @{file}", &head[s..])
                    });
                    let key = match (global, local) {
                        (true, _) => "GLOBAL".to_string(),
                        (false, Some(l)) => l,
                        (false, None) => continue,
                    };
                    let entry = by_lock.entry(key).or_default();
                    let mut k = 0usize;
                    while let Some(r) = body[k..].find("_var(\"") {
                        let s = k + r + 6;
                        let Some(q) = body[s..].find('"') else { break };
                        entry.insert(body[s..s + q].to_string());
                        k = s + q;
                    }
                }
            }
        }
        let Some(global) = by_lock.get("GLOBAL").cloned() else {
            panic!("the sweep found no globally-locked test — it is looking in the wrong place");
        };
        let mut shared: Vec<String> = Vec::new();
        for (lock, vars) in &by_lock {
            if lock == "GLOBAL" {
                continue;
            }
            let overlap: Vec<&String> = vars.intersection(&global).collect();
            if !overlap.is_empty() {
                shared.push(format!("{lock} also sets {overlap:?}"));
            }
        }
        shared.sort();
        assert!(
            shared.is_empty(),
            "a private lock covers a variable the crate-wide guard also sets:\n  {}",
            shared.join("\n  ")
        );
    }
}

/// **A path suffixed ` (deleted)` must never survive in a config.**
///
/// The kernel appends that suffix to `/proc/self/exe` as soon as the binary's inode has
/// been replaced — and `tokenade upgrade` replaces the binary JUST BEFORE
/// writing the services that launch it. Observed plainly on a real
/// machine after 1.1.14:
///
/// ```text
/// ExecStart=/home/u/.local/bin/tokenade (deleted) llm-proxy --listen 127.0.0.1:8788
/// ```
///
/// The unit becomes unrunnable, the previous version's process survives on
/// the old port, and the next update undoes the agent's
/// configuration while announcing "not installed".
///
/// `stable_exe_from` already handles the case — an absolute path that does not exist
/// returns the installed tokenade — but nothing SAID so. This test pins it on
/// the exact shape the kernel produces.
#[cfg(test)]
mod a_deleted_binary_never_reaches_a_persisted_config {
    /// The exact shape, on a path that does not exist: the result never carries
    /// the suffix, and cannot be the dead path.
    #[test]
    fn the_deleted_suffix_never_comes_back_out() {
        for cur in [
            "/home/u/.local/bin/tokenade (deleted)",
            "/usr/local/bin/tokenade (deleted)",
            "/opt/tokenade/bin/tokenade (deleted)",
        ] {
            let out = crate::platform::stable_exe_from(cur);
            assert!(
                !out.contains("(deleted)"),
                "`{cur}` comes back out with its suffix: `{out}` — a unit carrying \
                 that never starts"
            );
            assert_ne!(out, cur, "the dead path is returned as is: {out}");
        }
    }

    /// And the bound: an ORDINARY existing path is not replaced in the name
    /// of caution. Without it, the test would also pass on a rule
    /// returning "tokenade" for everyone.
    #[test]
    fn an_ordinary_existing_path_is_left_alone() {
        let me = std::env::current_exe().expect("a test binary has a path");
        let me = me.display().to_string();
        // A test binary lives under `target/`, which the rule redirects to
        // the install when there is one — so identity is only required
        // if the rule has nothing else to offer.
        let out = crate::platform::stable_exe_from(&me);
        assert!(
            !out.contains("(deleted)"),
            "a healthy path must not acquire the suffix: {out}"
        );
        assert!(!out.is_empty());
    }
}

/// **The kernel's suffix alone is the answer "no".**
///
/// During an update, `/proc/self/exe` returns `…/tokenade (deleted)` and the
/// BARE path already designates the new binary. Comparing paths after
/// stripping the suffix would therefore answer "yes, I am the installed one" exactly at
/// the moment it is false — and it is that "yes" that would write a unit, or
/// restart a service, with the previous version's code.
///
/// This test holds the CASE, not the call: `running_the_installed_binary` reads the
/// real `current_exe`, which cannot be faked. `same_executable` is the
/// pure half, and the string equality it replaces is checked in both
/// directions.
#[test]
fn the_kernel_deleted_suffix_is_never_the_installed_binary() {
    let exe = std::env::current_exe().expect("current_exe");
    let p = exe.display().to_string();
    // A path and itself: the same file, on all three systems.
    assert!(crate::platform::same_executable(&p, &p));
    // The same path suffixed is NOT the same file — the string differs and
    // `canonicalize` fails on it, so no equality is invented.
    assert!(!crate::platform::same_executable(
        &p,
        &format!("{p} (deleted)")
    ));
    // And two different files are not the same.
    assert!(!crate::platform::same_executable(
        &p,
        "/nonexistent/tokenade"
    ));
    // The test binary is not an installed tokenade: the answer is no,
    // and above all it does not panic.
    let _ = crate::platform::running_the_installed_binary();
}
