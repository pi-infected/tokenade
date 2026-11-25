// The tests of `lib`.
//
// Spliced by `include!` from `lib.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility line
// changed. It is the form this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

// The CODE lines that spell a number of stages by hand.
//
// Extracted from the guard to be checkable on a known text: the guard
// itself cannot say that it has stopped looking.

#[cfg(test)]
mod tests {
    use super::*;

    /// The announcement cost is MEASURED on real folds. An instrument that
    /// measured nothing does not return a number.
    #[test]
    fn the_announcement_cost_is_measured_on_real_folds() {
        let c = announcement_cost().expect("the corpus must produce banners");
        assert!(c > 0, "an empty banner announces nothing");
        // ATTESTED, not chosen: no observed banner is shorter, and
        // at least one equals it. A constant set by hand dies here.
        let seen = observed_announcements();
        assert!(!seen.is_empty(), "no banner observed");
        assert!(
            seen.iter().all(|l| *l >= c),
            "{c} is not a floor: the shortest observed is {:?}",
            seen.iter().min()
        );
        assert!(seen.contains(&c), "{c} is the length of no real banner");
    }

    #[test]
    fn only_our_own_lines_count_as_an_announcement() {
        assert!(is_an_announcement("[tokenade:diff] 1610 line(s)"));
        assert!(is_an_announcement("  [disclose:stash a1b2]"));
        assert!(!is_an_announcement("total 48"));
        // A CONTENT line that talks about us is not our announcement: it
        // does not start with the bracket.
        assert!(!is_an_announcement("see [tokenade:diff] above"));
    }

    /// The single enumerator sees the THREE platforms. `per_command` had
    /// its own and read only macos and windows: 30 linux captures, on the
    /// product's platform, missing from a measurement that called itself complete.
    #[test]
    fn the_corpus_carries_every_platform() {
        let caps = captures();
        for os in ["linux", "macos", "windows"] {
            assert!(
                caps.iter().any(|c| c.os == os),
                "no {os} capture — the enumerator lost a platform"
            );
        }
        // A capture under a locale directory keeps its OS. This
        // assertion was under an `if let`: the mutation that makes every
        // directory an OS empties the filter, and the body no longer ran.
        // A case that must EXIST is looked up with `expect`, not with `if let`.
        let c = caps
            .iter()
            .find(|c| c.locale != "EN/FR")
            .expect("the corpus carries captures under a locale directory");
        assert_ne!(c.os, c.locale, "the OS must not be the locale directory");
        assert!(["linux", "macos", "windows"].contains(&c.os.as_str()), "os={}", c.os);
        assert!(c.key.contains(&c.locale), "{}", c.key);
    }

    #[test]
    fn a_silence_without_a_canary_is_not_a_clean_bill() {
        assert_eq!(
            conclude(true, Vec::new()),
            Verdict::Clean,
            "canary fired and nothing found: it is a true negative"
        );
        match conclude(false, Vec::new()) {
            Verdict::InstrumentNotValidated(why) => {
                assert!(why.contains("canary"), "{why}");
            }
            other => panic!("a silence without a canary must be refused, not returned clean: {other:?}"),
        }
    }

    /// And a FINDING without a canary is no better: the probe may have
    /// found its own noise.
    #[test]
    fn a_finding_without_a_canary_is_refused_too() {
        assert!(matches!(
            conclude(false, vec!["quelque chose".into()]),
            Verdict::InstrumentNotValidated(_)
        ));
        assert_eq!(
            conclude(true, vec!["x".into()]),
            Verdict::Found(vec!["x".into()])
        );
    }

    #[test]
    fn two_benches_do_not_share_a_dedup_store() {
        let a = Bench::new().expect("bench a");
        let b = Bench::new().expect("bench b");
        assert_ne!(
            a.state_dir(),
            b.state_dir(),
            "two benches sharing their state: the second arm of an A/B would see \
             the duplicate the first one just recorded"
        );
        assert!(a.state_dir().exists() && b.state_dir().exists());
    }

    #[test]
    fn a_fixture_keeps_the_extension_that_routes_it() {
        let b = Bench::new().expect("bench");
        let p = b.fixture("probe", "html", "<p>x</p>").expect("write");
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("html"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "<p>x</p>");
    }

    #[test]
    #[should_panic(expected = "reaches no compactor")]
    fn a_fixture_without_an_extension_is_refused() {
        let b = Bench::new().expect("bench");
        let _ = b.fixture("probe", "", "x");
    }

    /// A bench must be born even with no binary built, otherwise the whole
    /// workspace suite falls over on a fresh clone. The absence is stated in `run`.
    #[test]
    fn a_bench_is_born_even_with_nothing_built() {
        let b = Bench::new().expect("construction does not depend on the binary");
        assert!(b.state_dir().exists());
        if !b.exe().exists() {
            let err = b.run(&["--version"]).expect_err("without a binary, run must fail");
            assert!(
                err.to_string().contains("cargo build"),
                "the failure must say what to do: {err}"
            );
        }
    }

    /// A relative argument must name the same file for the caller and
    /// for the bench, whose current directory is elsewhere. Without it the
    /// command answers "No such file" and the probe reads "0 bytes delivered,
    /// −100 %": a refusal disguised as a perfect compaction.
    #[test]
    fn a_relative_argument_still_names_the_callers_file() {
        let b = Bench::new().expect("bench");
        if !b.exe().exists() {
            return; // covered by `a_bench_is_born_even_with_nothing_built`
        }
        let rel = "Cargo.toml";
        assert!(
            std::path::Path::new(rel).exists(),
            "the test runs from the crate root, where this file exists"
        );
        let run = b.run(&["read", rel]).expect("run");
        assert!(
            !String::from_utf8_lossy(&run.stderr).contains("No such file"),
            "the relative path was not resolved: {}",
            String::from_utf8_lossy(&run.stderr)
        );
    }

    #[test]
    fn a_debug_timing_is_not_offered_for_comparison() {
        let mk = |profile| Run {
            stdout: Vec::new(),
            stderr: Vec::new(),
            code: Some(0),
            ms: 1234,
            profile,
        };
        assert_eq!(mk(Profile::Release).ms_comparable(), Some(1234));
        assert_eq!(
            mk(Profile::Debug).ms_comparable(),
            None,
            "43x gap with release: an absolute value in debug means nothing"
        );
        // The ratio remains available, because a before/after in the SAME
        // profile is valid.
        assert_eq!(mk(Profile::Debug).ms_ratio_only(), 1234);
    }

    /// The exception is NAMED, so it must be guarded: a non-isolated bench
    /// that measured a compaction would return a wrong A/B without saying anything.
    #[test]
    fn a_non_isolated_bench_refuses_anything_but_a_state_question() {
        let b = Bench::reading_real_state().expect("bench");
        assert!(!b.is_isolated());
        assert!(Bench::new().expect("bench").is_isolated());
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = b.run(&["read", "quelque-chose.txt"]);
        }))
        .is_err();
        assert!(
            panicked,
            "a non-isolated bench must REFUSE a compaction measurement: without \
             a fresh dedup store the second arm sees the first one's duplicate"
        );
    }

    #[test]
    fn a_signal_death_is_not_a_survival() {
        let mk = |code| Run {
            stdout: Vec::new(),
            stderr: Vec::new(),
            code,
            ms: 0,
            profile: Profile::Release,
        };
        assert!(mk(Some(0)).survived());
        assert!(mk(Some(1)).survived(), "an ORDINARY failure is still a survival");
        assert!(
            !mk(None).survived(),
            "killed by a signal: a test that accepts this accepts the OOM kill it \
             was meant to prevent"
        );
    }
    /// The child runs in a temporary directory, so a RELATIVE path
    /// to the binary is resolved there against that directory.
    /// `TOKENADE_BENCH_EXE=target/debug/tokenade` passed `p.exists()` — which
    /// reads from the bench's directory — then the launch answered
    /// `No such file or directory (os error 2)` without naming the path that
    /// was missing. It is the arguments defect, on the executable side.
    #[test]
    fn a_relative_exe_path_is_made_absolute_before_the_child_moves() {
        // `cargo test` sets the current directory to the crate root, so
        // `Cargo.toml` is a relative path that exists — without touching the
        // process's current directory, which the other tests share.
        let (abs, found) = Bench::absolutise(PathBuf::from("Cargo.toml"));
        assert!(found, "the file exists: it must be found");
        assert!(
            abs.is_absolute(),
            "a relative path must be made absolute, otherwise the child does not find it: {abs:?}"
        );
        assert!(abs.ends_with("Cargo.toml"), "{abs:?}");
        // A path that does not exist cannot be canonicalised: return it
        // as is and say it is missing, rather than invent.
        let (kept, found) = Bench::absolutise(PathBuf::from("/n/existe/pas/tokenade"));
        assert!(!found, "a missing binary cannot be announced as found");
        assert_eq!(kept, PathBuf::from("/n/existe/pas/tokenade"));
    }
}

#[cfg(test)]
mod arg_tests {
    use super::reject_unknown_args;

    /// The function exits the process when it refuses, so the ACCEPTED cases
    /// are what an in-process test can check — and it is the half that
    /// breaks when hardening: a bench that refuses its own flags is no
    /// longer of use. The refusal itself is exercised by the source guard below and
    /// by real use.
    #[test]
    fn an_instrument_that_folded_nothing_is_dead_not_clean() {
        use super::instrument_is_dead;
        // Zero folds on a real corpus: that is not "no defect".
        assert!(instrument_is_dead(0, 554_148));
        assert!(instrument_is_dead(5_000, 554_148));
        // One fold in a hundred is enough to say the instrument is looking.
        assert!(!instrument_is_dead(5_542, 554_148));
        assert!(!instrument_is_dead(300_000, 554_148));
        // No call: nothing to conclude, above all not "dead".
        assert!(!instrument_is_dead(0, 0));
    }

    #[test]
    fn a_known_flag_and_its_value_pass() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        reject_unknown_args(&["--min-lines"], &a(&["--min-lines", "4"]));
        reject_unknown_args(&["--min-lines"], &a(&["--min-lines=4"]));
        reject_unknown_args(&["--focus"], &a(&["--focus", "auto:html"]));
        // A value that looks like a flag is consumed by its own.
        reject_unknown_args(&["--focus"], &a(&["--focus", "--weird-name"]));
        // A positional argument is not a flag.
        reject_unknown_args(&[], &a(&["read", "fichier.txt"]));
    }
}

#[cfg(test)]
mod stale_exe_tests {
    use super::*;
    use std::time::Duration;

    /// A throwaway tree carrying a single file, and the instant of its
    /// construction. The mtimes are the REAL ones: no dependency added
    /// to fake them, the threshold moves instead.
    fn tree(tag: &str, rel: &str) -> (PathBuf, std::time::SystemTime) {
        let root = std::env::temp_dir().join(format!(
            "tokenade-stale-{}-{tag}",
            std::process::id()
        ));
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "// \n").unwrap();
        let t = std::fs::metadata(&p).unwrap().modified().unwrap();
        (root, t)
    }

    /// MEASURED 2026-09-08: the same census returned 273,366 then 284,024
    /// bytes delivered on the SAME base, because `target/debug/tokenade`
    /// dated from a previous iteration. An instrument that measures a stale
    /// binary returns a figure on code that no longer exists.
    /// **A TEST file makes no binary stale.**
    ///
    /// Paid for once: editing `tests/support/corpus.rs` made a
    /// measurement refuse and cost a full rebuild of the CLI. The two staleness
    /// guards ask the same question, and a single function answers it.
    #[test]
    fn a_test_file_does_not_stale_a_binary() {
        let (root, t) = tree("tests-dont-count", "tests/support/corpus.rs");
        assert!(
            is_not_compiled_into_the_binary(&root.join("tests/support/corpus.rs")),
            "a file under `tests/` was counted as going into the binary"
        );
        assert_eq!(
            source_newer_than_except(
                &root,
                t - Duration::from_secs(60),
                &is_not_compiled_into_the_binary
            ),
            None
        );
        std::fs::remove_dir_all(&root).ok();

        // And the bound on the other side: a `src/` source always makes it stale.
        let (root, t) = tree("src-still-counts", "src/filter.rs");
        assert!(!is_not_compiled_into_the_binary(&root.join("src/filter.rs")));
        assert_eq!(
            source_newer_than_except(
                &root,
                t - Duration::from_secs(60),
                &is_not_compiled_into_the_binary
            ),
            Some(root.join("src/filter.rs"))
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_source_newer_than_the_build_is_named() {
        // It DESCENDS: a source modified in a subdirectory makes the
        // binary stale as much as one at the root.
        let (root, t) = tree("deep", "a/b/fresh.rs");
        assert_eq!(
            source_newer_than(&root, t - Duration::from_secs(60)),
            Some(root.join("a/b/fresh.rs"))
        );
        // And it does not shout on a tree older than the build.
        assert_eq!(source_newer_than(&root, t + Duration::from_secs(60)), None);
        // A missing tree is not a recent source: claim nothing.
        assert_eq!(source_newer_than(&root.join("nowhere"), t - Duration::from_secs(60)), None);
        std::fs::remove_dir_all(&root).ok();

        // It is not Rust: a newer artifact does not make the
        // binary stale, otherwise the guard would shout on every rewritten fixture.
        let (root, t) = tree("data", "a/capture.txt");
        assert_eq!(source_newer_than(&root, t - Duration::from_secs(60)), None);
        std::fs::remove_dir_all(&root).ok();
    }

    /// Forgetting the state must REALLY erase, otherwise the dedup answers from a
    /// neighbouring capture intermittently.
    #[test]
    fn forgetting_clears_what_a_previous_measure_left() {
        let b = Bench::new().unwrap();
        let dedup = b.state_dir().join("dedup.db");
        let stash = b.state_dir().join("stash");
        std::fs::create_dir_all(&stash).unwrap();
        std::fs::write(&dedup, "x").unwrap();
        std::fs::write(stash.join("a.log"), "x").unwrap();

        b.forget();
        assert!(!dedup.exists(), "the dedup store survives forgetting");
        // And ALL the others: three stores out of five were redirected, so
        // forgetting only erased three and the operator's index served as the
        // answer.
        for st in tokenade_core::stores::ISOLATABLE {
            let p = b.state_dir().join(st.file);
            std::fs::write(&p, "x").unwrap();
        }
        b.forget();
        for st in tokenade_core::stores::ISOLATABLE {
            assert!(
                !b.state_dir().join(st.file).exists(),
                "`{}` survives forgetting",
                st.file
            );
        }
        assert!(!stash.exists(), "the stash survives forgetting");
        // And it is idempotent: an already fresh state is not an error.
        b.forget();
    }

    /// The three guards are only worth something if every delivery instrument goes
    /// through [`Delivery`]. A guard placed in the library and never wired
    /// is dead, and exactly that was found again one day after it had been
    /// written. Here the SOURCE guard ALSO says that `Delivery` carries
    /// all three: spreading them among the callers is what allowed
    /// `json_headroom` to overestimate by a factor of 600 while
    /// `residue_census` measured right.
    #[test]
    fn every_delivery_instrument_goes_through_the_shared_bench() {
        let root = Bench::workspace_root();
        let lib = the_one_file_carrying("impl Delivery {\n");
        // BOUNDED to the block. A first version took everything up to the end of the
        // file, so it included the LIST below: the guard
        // matched itself and three mutations that empty `Delivery`
        // survived. A source guard must exclude its own expectation.
        // The line break DISAMBIGUATES: without it, `find` lands on the
        // string literal of this very line, which comes earlier in the
        // file — the guard then read its own test as a block of
        // code.
        let start = lib.find("impl Delivery {\n").expect("the impl of Delivery");
        let end = lib[start..].find("\n}\n").expect("the end of the block") + start;
        let body = &lib[start..end];
        assert!(
            !body.contains("every_delivery_instrument"),
            "the guard is looking at its own test: it can no longer fail"
        );
        // One forget PER measurement: `forget` present only once let a
        // measurement method do without it, and the dedup then answers instead of the
        // compactor. A method that DELEGATES to another measurement is covered
        // by it — counting `forget`s against `pub fn measure`s
        // went red the first time a measurement became the shortcut
        // of another, which is the right way to add one.
        let unguarded: Vec<&str> = body
            .split("pub fn measure")
            .skip(1)
            .filter(|m| {
                let head = m.split("\n    }").next().unwrap_or(m);
                !head.contains("self.bench.forget();") && !head.contains("self.measure")
            })
            .map(|m| m.lines().next().unwrap_or(""))
            .collect();
        assert!(
            unguarded.is_empty(),
            "these measurements do not forget the state and do not delegate to one that \
             does — the dedup will answer instead of the compactor, \
             intermittently: {unguarded:?}"
        );
        assert!(
            body.matches("self.bench.forget();").count() >= 2,
            "the guard no longer finds the forgets it checks"
        );
        for guard in [
            "source_newer_than_exe()",
            "self.bench.forget();",
            // The capture is measured UNDER ITS PROGRAM, never under `cat`:
            // eight captures were reported at 100 % because `cat`'s routing
            // was being measured. The argv is built by `argv_for`,
            // which also carries the number of STAGES requested.
            "argv_for(stages, prog)",
        ] {
            assert!(body.contains(guard), "`Delivery` does not carry `{guard}`");
        }
        // Driven by the DIRECTORY, not by a hand-written list: a
        // new instrument that ranks its rows by delivered bytes would have entered
        // the bench without anything reminding it of the three guards. Three
        // iterations in a row ran into a single-stage report;
        // the hand-kept list is what lets the
        // fourth through.
        let bins = root.join("crates/tokenade-bench/src/bin");
        let mut checked = 0usize;
        for ent in std::fs::read_dir(&bins).unwrap().flatten() {
            let p = ent.path();
            if p.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap();
            if !src.contains("\"--delivered\"") {
                continue;
            }
            checked += 1;
            assert!(
                src.contains("Delivery::open"),
                "`{}` offers `--delivered` without going through `Delivery`",
                p.display()
            );
            // And an instrument that censuses COMMANDS must measure them UNDER
            // THEIR PROGRAM: `measure` alone runs `wrap "cat <fixture>"` and
            // puts a figure on `cat`'s routing. MEASURED 2026-09-08 on the 265
            // captures — 284,036 bytes announced versus 156,503 actually
            // delivered, a residue overestimated by 81 %.
            if src.contains("Route::Command") {
                assert!(
                    src.contains("measure_as_program(") || src.contains("measure_stages("),
                    "`{}` censuses commands without measuring them under their program",
                    p.display()
                );
            }
        }
        assert!(checked >= 3, "only {checked} delivery instrument(s) found");
    }

    // **A capture is measured under ITS program, not under `cat`.**
    //
    // The product routes on the command line. Writing the capture into a
    // file and running `wrap "cat <file>"` measures `cat`'s routing.
    // MEASURED 2026-09-08 on the 265 captures: 284,036 bytes announced versus
    // 156,503 actually delivered, a residue overestimated by **81 %**, and eight
    // captures given at 100 % while their compactor exists and folds
    // (`linux/systemctl_units`: 5,944 → 5,944 through `cat`, **532 under
    // `systemctl`**).
    //
    // The measurement therefore goes through `filter <prog>` on stdin. **Not through a fake
    // executable placed in the `PATH`**: this repo has already ruled out that choice, and
    // an attempt to take it up again cost two hard shutdowns of the workstation — the
    // shim's body invoked `cat`, the corpus contains a `cat` capture, and
    // the shim relaunched itself.

    /// The `src/` file that carries a seam, DERIVED rather than written by
    /// hand — and there must be only one.
    ///
    /// Two guards of this module read a source to check what it
    /// contains. Both named `lib.rs`, and both went red the
    /// day `impl Delivery` and `reject_unknown_args` changed files:
    /// the message said "the impl of Delivery", which reads as a defect
    /// of the product where it was the test that had the wrong address. Uniqueness
    /// is checked too: two copies of the seam, and the guard would only speak
    /// about one of the two, silently.
    fn the_one_file_carrying(needle: &str) -> String {
        let src = Bench::workspace_root().join("crates/tokenade-bench/src");
        let found: Vec<_> = std::fs::read_dir(&src)
            .expect("crates/tokenade-bench/src")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "rs"))
            // Test files do not count: this one SPELLS the seam
            // it is looking for, and would count itself.
            .filter(|p| !p.to_string_lossy().ends_with("_tests.rs"))
            .filter_map(|p| std::fs::read_to_string(&p).ok().map(|b| (p, b)))
            .filter(|(_, b)| b.contains(needle))
            .collect();
        assert_eq!(
            found.len(),
            1,
            "`{needle}` must live in a single src/ file, found: {:?}",
            found.iter().map(|(p, _)| p).collect::<Vec<_>>()
        );
        found.into_iter().next().unwrap().1
    }

    #[test]
    fn a_capture_is_measured_under_its_own_program_without_a_shim() {
        let lib = the_one_file_carrying("impl Delivery {\n");
        let start = lib.find("impl Delivery {\n").expect("the impl of Delivery");
        let end = lib[start..].find("\n}\n").expect("the end of the block") + start;
        let body = &lib[start..end];

        // The measurement routes by the PROGRAM…
        assert!(
            body.contains("argv_for(stages, prog)"),
            "the measurement no longer attributes the capture to its program"
        );
        // …and `argv_for` does start from `filter <prog>`, not from `cat`.
        assert_eq!(
            argv_for(Stages::CommandOnly, "pacman -Q"),
            vec!["filter", "pacman", "-Q"],
            "the capture would be measured under another program"
        );
        // …and nothing there fabricates an executable or diverts the PATH.
        for banned in ["set_permissions", "PATH", "shim", "0o755"] {
            assert!(
                !body.contains(banned),
                "`Delivery` fabricates a shim again (`{banned}`): a choice already ruled out, \
                 and it cost two machine shutdowns"
            );
        }
    }

    /// Standard input REALLY reaches the child, and the program changes the
    /// result.
    ///
    /// MEASURED 2026-09-08 on `linux/systemctl_units`: under `cat`, 5,944 bytes
    /// returned as is; under `systemctl`, **532**. Without this assertion, a
    /// measurement that writes nothing to stdin would return "0 bytes delivered", which reads
    /// as a perfect compaction.
    ///
    /// It goes through [`Bench`] and NOT through [`Delivery`]: `Delivery::open` refuses
    /// a binary older than the tree, which is right for a
    /// measurement and fatal for a test — under mutation the source is always more
    /// recent than the binary, so this test exited silently and the mutation
    /// "stdin is no longer pushed" survived its first version.
    #[test]
    fn stdin_reaches_the_child_and_the_program_decides() {
        // The DEBUG binary, explicitly: the one `cargo build -p tokenade-cli` makes.
        let exe = Bench::target_dir().join("debug").join("tokenade");
        assert!(
            exe.exists(),
            "this test measures the real CLI: `cargo build -p tokenade-cli` first ({})",
            exe.display()
        );
        let b = Bench::with_exe(exe).expect("the bench");
        let body = std::fs::read_to_string(
            Bench::workspace_root()
                .join("crates/tokenade-core/tests/fixtures/linux/systemctl_units.txt"),
        )
        .expect("the corpus's systemctl capture");

        let under = |prog: &str| -> usize {
            b.forget();
            b.run_with_stdin(&["filter", prog], Some(&body))
                .unwrap_or_else(|e| panic!("measurement under `{prog}`: {e}"))
                .bytes()
        };
        let prog = under("systemctl");
        assert!(prog > 0, "nothing came out: stdin did not reach the child");
        assert!(
            prog * 2 < body.len(),
            "under `systemctl` the capture must be folded by more than half, \
             {prog} out of {}",
            body.len()
        );
        // And the program is indeed what decides: `cat` does not know these
        // columns and returns the input.
        let cat = under("cat");
        assert!(
            cat > prog,
            "the program changes nothing in the result: {cat} versus {prog}"
        );
    }

    /// **Every instrument goes through the seam that refuses a stale binary.**
    ///
    /// The stale-binary guard only looked at the PRODUCT binary; mine,
    /// ten hours stale, returned three verdicts on code I had not
    /// built. Fixing the class means asking the same question of the
    /// other binary, on the seam ALL go through — a guard one has
    /// to remember to call is called only by those who remembered.
    /// **The PACKAGE binary is not an instrument of `src/bin/`.**
    ///
    /// `src/main.rs` and `src/benches/**` are linked only by
    /// `tokenade-bench` itself. Counting them made `duplication` refuse to
    /// run for an edit that could not change what it measures — and
    /// the refusal was inescapable, `cargo build --bins` finishing in 0.15 s without
    /// rebuilding anything.
    #[test]
    fn the_package_binary_does_not_stale_an_instrument_of_bin() {
        let src = Bench::workspace_root().join("crates/tokenade-bench/src");
        let holder = the_one_file_carrying("let another_instrument = |p: &Path| {");
        for needle in [
            "belongs_to_the_package_binary",
            "i_am_the_package_binary",
            "\"tokenade-bench\"",
        ] {
            assert!(
                holder.contains(needle),
                "the staleness guard no longer distinguishes the package binary: `{needle}` disappeared"
            );
        }
        // Non-vacuity: both paths do exist.
        assert!(src.join("main.rs").is_file());
        assert!(src.join("benches").is_dir());
    }

    /// **A tests sibling never makes an instrument stale.**
    ///
    /// It is mounted by `include!` under `#[cfg(test)]`. In a binary
    /// build, the inclusion is not expanded: cargo does not see the file,
    /// relinks nothing, and the refusal becomes INESCAPABLE — no rebuild
    /// can lift it. The case happened as soon as the tests moved out of the god
    /// files into sibling files.
    #[test]
    fn a_tests_sibling_does_not_stale_an_instrument() {
        assert!(is_not_compiled_into_the_binary(Path::new(
            "/r/crates/tokenade-bench/src/lib_tests.rs"
        )));
        // …and the production source, for its part, does make it stale.
        assert!(!is_not_compiled_into_the_binary(Path::new(
            "/r/crates/tokenade-bench/src/lib.rs"
        )));
        // The name must be a file SUFFIX, not a substring.
        assert!(!is_not_compiled_into_the_binary(Path::new(
            "/r/crates/tokenade-bench/src/instruments_tests_helper.rs"
        )));
    }

    /// **An instrument that has CHILDREN makes only itself stale.**
    ///
    /// `value_ranking.rs` became `value_ranking.rs` + `value_ranking/` on
    /// 2026-09-13. The question "which binary does this file belong to?" only
    /// looked at the PARENT, so `value_ranking/sessions.rs` belonged
    /// to nobody — and made the sixteen other instruments stale. The refusal was
    /// INESCAPABLE: cargo does not relink `duplication` for that edit,
    /// so its binary could not get newer.
    #[test]
    fn a_binary_child_stales_only_its_binary() {
        let bins = Path::new("/r/crates/tokenade-bench/src/bin");
        assert_eq!(
            instrument_owning(bins, &bins.join("mutate.rs")).as_deref(),
            Some("mutate")
        );
        assert_eq!(
            instrument_owning(bins, &bins.join("value_ranking/sessions.rs")).as_deref(),
            Some("value_ranking")
        );
        assert_eq!(
            instrument_owning(bins, &bins.join("value_ranking.rs")).as_deref(),
            Some("value_ranking")
        );
        // Outside `src/bin/`, the question does not arise: it is code that
        // ALL instruments link.
        assert_eq!(
            instrument_owning(bins, Path::new("/r/crates/tokenade-bench/src/lib.rs")),
            None
        );
    }

    #[test]
    fn every_instrument_goes_through_the_stale_check() {
        let bins = Bench::workspace_root().join("crates/tokenade-bench/src/bin");
        let mut checked = 0usize;
        for ent in std::fs::read_dir(&bins).expect("src/bin").flatten() {
            let p = ent.path();
            if p.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            checked += 1;
            let src = std::fs::read_to_string(&p).expect("readable source");
            assert!(
                src.contains("reject_unknown_args("),
                "`{}` does not call the common seam: nothing stops it from \
                 running stale",
                p.display()
            );
        }
        assert!(checked >= 14, "only {checked} instrument(s) found");

        // And the seam does carry the question.
        const SEAM: &str = "pub fn reject_unknown_args(known: &[&str], args: &[String]) {";
        let lib = the_one_file_carrying(SEAM);
        let start = lib.find(SEAM).expect("the seam");
        let end = lib[start..].find("\n}\n").expect("its end") + start;
        assert!(
            lib[start..end].contains("refuse_if_this_instrument_is_stale()"),
            "the seam no longer refuses a stale instrument"
        );
    }

    /// The refusal must NAME both files and the command that repairs, otherwise
    /// it reads as an incident of the bench.
    #[test]
    fn the_refusal_names_both_files_and_the_fix() {
        let note = stale_exe_note(Path::new("/t/debug/tokenade"), Path::new("/r/src/filter.rs"));
        assert!(note.contains("/t/debug/tokenade"), "{note}");
        assert!(note.contains("/r/src/filter.rs"), "{note}");
        assert!(note.contains("cargo build"), "{note}");
    }
}

#[cfg(test)]
mod every_bench_refuses_what_it_does_not_understand {
    /// One more bench will be written, and it will forget. The guard sweeps the SOURCES
    /// rather than trusting discipline: it is the same pattern as
    /// the product's `every_process_launcher_either_guards_its_work_or_says_why_not`,
    /// and it has already caught two benches written this week.
    ///
    /// FOUND while using it: `duplication -- 4` silently ignored, measurement
    /// returned at the default threshold; `testsel --something` taken for a
    /// FILE name and answering "no testable module", which looks like a
    /// result.
    #[test]
    fn a_bench_that_takes_arguments_must_reject_the_ones_it_does_not_know() {
        /// Those that FORWARD their argv to another program: refusing what
        /// they do not know would amount to refusing the flags of the
        /// measured program.
        const EXEMPT: &[(&str, &str)] = &[(
            "probe.rs",
            "forwards its argv to the tokenade CLI: the measured flags are not its own",
        )];

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin");
        let mut missing = Vec::new();
        let mut seen = 0usize;
        let entries = std::fs::read_dir(&dir).expect("src/bin must exist");
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                continue;
            }
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            if EXEMPT.iter().any(|(f, _)| *f == name) {
                continue;
            }
            seen += 1;
            let body = std::fs::read_to_string(&p).unwrap_or_default();
            if !body.contains("reject_unknown_args") {
                missing.push(name);
            }
        }
        // ANCHOR assertion, unkillable on a healthy tree and that is its nature:
        // it does not speak about the benches but about the path that finds them. Without it,
        // a moved `src/bin` would return "zero benches missing", i.e. a green
        // over zero files examined — the same defect from the other end, where a gate
        // that only fails on a VALUE is blind to the loss of a ROW.
        assert!(seen >= 4, "only {seen} benches seen — did the path move?");
        missing.sort();
        assert!(
            missing.is_empty(),
            "these benches SILENTLY accept an unknown argument, so they may \
             measure something other than what they are asked: {missing:?}. Call \
             `reject_unknown_args`, or list them in EXEMPT with the reason."
        );
    }
}

#[cfg(test)]
mod page_corpus_tests {
    use super::{captures_on, Route};

    /// **The pages are IN the bench's corpus.**
    ///
    /// They were not: forty-one real pages, twenty-one
    /// megabytes, and the heaviest family of the usage ranking
    /// (`html-to-markdown`, 40.8 % of the pool) had no instrument.
    /// Same hole as for the FORMAT captures, one door further.
    #[test]
    fn the_corpus_carries_the_pages() {
        let pages = captures_on(Route::Page);
        assert!(
            pages.len() >= 39,
            "only {} page(s) in the corpus",
            pages.len()
        );
        // `web`'s routing reads the extension: a page without `.html` reaches
        // no extractor, and the sweep would measure the pass-through.
        for p in &pages {
            assert!(
                p.file_name.ends_with(".html"),
                "{} does not carry the extension `web` reads",
                p.file_name
            );
            assert!(!p.body.is_empty(), "{} is empty", p.key);
        }
    }

    /// **And the results pages too.**
    ///
    /// Six real pages, 1.1 MB, in the repo since July and walked
    /// by no instrument. It is the ✖ case a third time: a
    /// family stays unranked as long as no route goes through it.
    #[test]
    fn the_corpus_carries_the_saved_results_pages() {
        let serps = captures_on(Route::Serp);
        assert!(serps.len() >= 6, "only {} SERP in the corpus", serps.len());
        for p in &serps {
            assert!(p.file_name.ends_with(".html"), "{}", p.file_name);
            assert!(!p.body.is_empty(), "{} is empty", p.key);
        }
    }

    /// **No fixture directory is left both unwalked and unnamed.**
    ///
    /// The question "does an instrument walk this route?" cost twelve
    /// iterations on `pages/` and two months on `serp/`. It is asked
    /// here, once, for each new directory: either a route
    /// walks it, or it is named as not being a captures corpus.
    #[test]
    fn no_fixture_directory_is_silently_unwalked() {
        /// What lives under `fixtures/` without being a captures corpus (`docs/`:
        /// binary documents for the `.doc`/`.xls` readers, read by their tests).
        const NOT_A_CORPUS: &[&str] = &["a11y", "build", "docs"];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|p| p.join("tokenade-core").join("tests").join("fixtures"))
            .expect("fixtures root");
        let walked: std::collections::BTreeSet<String> = super::captures()
            .iter()
            .filter_map(|c| c.key.split('/').next().map(str::to_string))
            .collect();
        let mut orphans = Vec::new();
        let mut seen = 0usize;
        for ent in std::fs::read_dir(&root).expect("fixtures/ unreadable").flatten() {
            if !ent.path().is_dir() {
                continue;
            }
            let name = ent.file_name().to_string_lossy().into_owned();
            seen += 1;
            if !walked.contains(&name) && !NOT_A_CORPUS.contains(&name.as_str()) {
                orphans.push(name);
            }
        }
        assert!(seen >= 6, "the sweep saw only {seen} directory(ies)");
        assert!(
            orphans.is_empty(),
            "these fixture directories are walked by NO route and are not \
             named as outside the corpus — that is how a family stays unranked: {orphans:?}"
        );
    }
}

#[cfg(test)]
mod generated_doc_tests {
    use super::*;

    #[test]
    fn a_swallowed_continuation_is_named_and_the_file_is_not_written() {
        let dir = std::env::temp_dir().join(format!("tokenade-gendoc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("out.md");
        let err = write_generated_doc(&p, "une phrase coupée de              son verbe\n")
            .expect_err("the damaged document must be refused");
        let msg = err.to_string();
        assert!(msg.contains("continuation"), "{msg}");
        assert!(msg.contains("son verbe"), "the refusal must SHOW the line: {msg}");
        assert!(!p.exists(), "nothing must be written when the rendering is damaged");
        // The window is a SLICE of bytes and these documents may contain
        // accented text: cutting it in the middle of a character would panic in the
        // very guard that must protect the write.
        let accents = format!("{}é              suite\n", "é".repeat(40));
        let err = write_generated_doc(&p, &accents).expect_err("refused too");
        assert!(err.to_string().contains("suite"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_clean_document_is_written_whole() {
        let dir = std::env::temp_dir().join(format!("tokenade-gendoc-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("out.md");
        // Indentation, two markdown end-of-line spaces, a table, and a
        // code block that aligns columns: none of this is the
        // fault looked for.
        // The bound is at THREE, and it is tested from below: a
        // swallowed continuation leaves at least the literal's indentation (eight
        // spaces and more), whereas two spaces in the MIDDLE of a line
        // come out of ordinary typing. Without this line, lowering the bound
        // to two breaks nothing — the mutation showed it, the previous version only
        // carried two spaces at the END of a line, which the rule already excludes.
        let md = "# titre\n\n  une liste\n\ndeux  espaces au milieu\n\nfin de ligne  \n| a | b |\n\n```\ncol      col2\n```\n";
        write_generated_doc(&p, md).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), md);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The guard is only worth something if EVERY markdown writer goes through it.
    /// Three bins write some today; it is the fourth that matters.
    #[test]
    fn no_instrument_writes_markdown_behind_the_guard() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut guilty: Vec<String> = Vec::new();
        for (path, code) in tokenade_core::source_guard::rust_sources(&root) {
            for line in code.lines() {
                let l = line.trim_start();
                if l.starts_with("//") {
                    continue;
                }
                if (l.contains("fs::write(") || l.contains("write!(")) && l.contains(".md") {
                    guilty.push(format!("{}: {l}", path.display()));
                }
            }
        }
        assert!(
            guilty.is_empty(),
            "these markdown writes bypass write_generated_doc:\n{}",
            guilty.join("\n")
        );
    }
}

#[cfg(test)]
mod stages_tests {
    use super::*;

    /// **The report's sentence follows the path TAKEN.**
    ///
    /// `--delivered` was built for the figure of BOTH stages; a later fix
    /// corrected its routing by going through `filter <prog>`, which has
    /// only one, and the module comment kept announcing two stages
    /// for eight iterations. A capability lost in a correct fix, that
    /// nothing reported. The sentence comes from here, not from a comment.
    #[test]
    fn the_banner_says_which_path_was_taken() {
        assert!(Stages::CommandOnly.banner().contains("ONE stage ONLY"));
        assert!(Stages::CommandOnly.banner().contains("does not put a figure on"));
        assert!(Stages::Delivered.banner().contains("BOTH stages"));
        assert_ne!(Stages::CommandOnly.banner(), Stages::Delivered.banner());
        // And the rule holds for EACH variant, not for the two we
        // thought of writing: `PageOnly` arrived after this test and would have
        // gone through it silently.
        assert_eq!(Stages::ALL.len(), 3, "a variant was added without going through here");
        for st in Stages::ALL {
            if st.is_delivered() {
                assert!(st.banner().contains("BOTH stages"), "{}", st.banner());
            } else {
                assert!(st.banner().contains("ONE stage ONLY"), "{}", st.banner());
                assert!(
                    st.banner().contains("does not put a figure on"),
                    "a stage presents itself as quantifiable: {}",
                    st.banner()
                );
                assert!(
                    !st.banner().contains("what the agent receives"),
                    "a stage presents itself as deliverable: {}",
                    st.banner()
                );
            }
        }
        // And the sentence names NO path: two instruments use it and
        // do not take the same one. It used to, and `json_headroom` was
        // titled "(`filter <prog>`)" while it goes through `auto`. The path
        // comes from `Measured::via`, in the instrument's words.
        for s in Stages::ALL.iter().map(|s| s.banner()) {
            assert!(!s.contains("filter"), "the sentence names a path: {s}");
            assert!(!s.contains("auto::"), "the sentence names a path: {s}");
        }
        // And the ONE-stage must not present itself as deliverable: that is the
        // exact confusion that was paid for once.
        assert!(!Stages::CommandOnly.banner().contains("what the agent receives"));
    }

    /// The argv built says which of the two is asked for. A flag that
    /// does not reach the CLI returns the same number under another name.
    /// And the argv does start from `filter <prog>`: the capture is measured UNDER ITS
    /// PROGRAM, never under `cat` — eight captures were reported at 100 %
    /// for that reason.
    #[test]
    fn asking_for_two_stages_passes_the_flag() {
        assert_eq!(argv_for(Stages::Delivered, "pacman -Q"), vec!["filter", "--deliver", "pacman", "-Q"]);
        assert_eq!(argv_for(Stages::CommandOnly, "pacman -Q"), vec!["filter", "pacman", "-Q"]);
    }
}

#[cfg(test)]
mod measured_tests {
    use super::*;

    /// **One does not describe a total one did not measure.** The header and the
    /// numbers come out of the same value, so a stage-1 total cannot
    /// present itself as a delivery.
    #[test]
    fn the_header_cannot_claim_a_path_the_numbers_did_not_take() {
        let one = Measured {
            stages: Stages::CommandOnly,
            items: 16_391,
            unit: "fichier(s) .json",
            before: 728_216_298,
            after: 100_036_475,
            via: "auto::compact_with_path_hint",
        };
        let h = one.header();
        assert!(h.contains("ONE stage ONLY"), "{h}");
        // And it names THIS instrument's path, not another's —
        // in the exact form, otherwise removing the parentheses leaves the
        // substring search satisfied (the mutation showed it).
        assert!(
            h.lines().next().unwrap().ends_with("(auto::compact_with_path_hint)"),
            "{h}"
        );
        assert!(!h.contains("filter <prog>"), "{h}");
        assert!(!h.contains("BOTH stages"), "{h}");
        assert!(h.contains("16 391 fichier(s) .json"), "{h}");
        assert!(h.contains("−86 %"), "{h}");
        let two = Measured { stages: Stages::Delivered, ..one };
        assert!(two.header().contains("BOTH stages"));
        assert!(!two.header().contains("ONE stage ONLY"));
    }

    /// **No instrument spells its own stage claim.** The fault
    /// happened twice in two instruments: the guard bears on the
    /// DIRECTORY, not on a hand-kept list.
    #[test]
    fn no_instrument_spells_a_stage_claim_of_its_own() {
        let bins = Bench::workspace_root().join("crates/tokenade-bench/src/bin");
        // `rust_sources` already refuses an empty sweep, so no count needs
        // re-checking here: a second coverage assertion would be
        // unkillable and saying so is better than dressing it up.
        let guilty = stage_claims(&tokenade_core::source_guard::rust_sources(&bins));
        assert!(
            guilty.is_empty(),
            "these instruments announce a number of stages without going through \
             `Stages::banner` — that is how a stage-1 total was titled \
             \"delivery path\":\n{}",
            guilty.join("\n")
        );
    }

    /// And the census checks itself on a known text: a guard that has stopped
    /// finding what it looks for passes silently.
    #[test]
    fn the_census_finds_a_hand_written_stage_claim() {
        let files = vec![
            (
                std::path::PathBuf::from("bad.rs"),
                "fn main() {\n    println!(\"both stages, real CLI\");\n}\n".to_string(),
            ),
            (
                std::path::PathBuf::from("good.rs"),
                "fn main() {\n    println!(\"{}\", Stages::Delivered.banner());\n}\n".to_string(),
            ),
            (
                std::path::PathBuf::from("commented.rs"),
                "fn main() {\n    // two stages, said in a comment\n}\n".to_string(),
            ),
        ];
        let found = stage_claims(&files);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("bad.rs:2:"), "{found:?}");
    }
}

#[cfg(test)]
fn stage_claims(files: &[(std::path::PathBuf, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (path, code) in files {
        for (n, line) in code.lines().enumerate() {
            let l = line.trim_start();
            if l.starts_with("//") {
                continue;
            }
            if l.contains("two stages")
                || l.contains("both stages")
                || l.contains("BOTH stages")
                || l.contains("ONE stage ONLY")
            {
                out.push(format!("{}:{}: {l}", path.display(), n + 1));
            }
        }
    }
    out
}

#[cfg(test)]
mod build_cache_tests {
    use super::*;

    /// **What can be mechanised does not stay an instruction.**
    ///
    /// MEASURED 2026-09-08: `target/debug/deps` held 4,744,341 entries, and
    /// `cargo` answered `No space left on device` while linking on a system
    /// showing 414 GB free — the ext4 directory index can no longer
    /// insert. Two symptoms followed and neither named the cause:
    /// `TREE BROKEN AFTER RESTORE` on an intact tree, and six runs
    /// killed "for low memory" with `/proc/pressure/memory avg10=0.00`.
    #[test]
    fn a_directory_past_the_ceiling_is_recognised_and_counting_stops_early() {
        let dir = std::env::temp_dir().join(format!("tokenade-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..12 {
            std::fs::write(dir.join(format!("a{i}.o")), b"x").unwrap();
        }
        assert!(build_cache_is_unindexable(&dir, 10), "twelve entries pass the ceiling of ten");
        assert!(!build_cache_is_unindexable(&dir, 12), "twelve is not more than twelve");
        // A missing directory is not a saturated cache.
        assert!(!build_cache_is_unindexable(&dir.join("absent"), 0));
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod build_jobs_tests {
    use super::*;

    /// **One link per three AVAILABLE gigabytes, between one and eight.**
    ///
    /// The static cap in `.cargo/config.toml` assumes a free machine.
    /// MEASURED 2026-09-09: it was not — two foreign `python3` at 2.3
    /// and 1.8 GB, free memory under 8 GB, and ten runs killed in a row
    /// "for low memory".
    #[test]
    fn the_job_count_follows_what_is_actually_free() {
        let n = safe_build_jobs();
        assert!((1..=8).contains(&n), "{n} simultaneous links");
        // The upper bound holds: even on an empty machine, we do not link
        // the whole graph twenty-four times at once.
        assert!(n <= 8);
    }
}

#[cfg(test)]
mod hermetic_env_tests {
    use super::*;

    /// **The bench does not measure the operator's environment.**
    ///
    /// MEASURED 2026-09-09: `windows/tasklist_v_win` returns 7,825 bytes in a
    /// clean environment and **3,085** when `TOKENADE_PRESSURE=0.35` lingers
    /// in the shell — the session wrapper exports it. The bench went through
    /// `systemd-run`, which starts from a fresh environment, and manual runs
    /// did not: two figures a factor of 2.5 apart were opposed without
    /// anything saying which one lied.
    #[test]
    fn a_measurement_does_not_inherit_the_operators_pressure() {
        let b = match Bench::new() {
            Ok(b) => b,
            Err(_) => return, // no binary to measure here
        };
        // Set in THIS process: the child must see none of them.
        for var in tokenade_core::stores::COMPACTION_ENV {
            // SAFETY: test-only environment mutation. CI runs the suite under nextest, one
            // process per test, so no other thread reads the environment while it changes.
            unsafe { std::env::set_var(var, "0.35") };
        }
        let run = b.run_with_stdin(&["filter", "cat"], Some("printenv-probe\n"));
        for var in tokenade_core::stores::COMPACTION_ENV {
            // SAFETY: test-only environment mutation. CI runs the suite under nextest, one
            // process per test, so no other thread reads the environment while it changes.
            unsafe { std::env::remove_var(var) };
        }
        assert!(run.is_ok(), "the bench must be able to launch a measurement");
        // The underlying guard: the list is passed to `env_remove` for EACH
        // variable, and it is the source that says so — an end-to-end test
        // cannot observe the child's environment without making it write it.
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"),
        )
        .expect("the bench reads itself");
        assert!(
            src.contains("for var in tokenade_core::stores::COMPACTION_ENV {\n            c.env_remove(var);"),
            "the bench no longer scrubs the inventory of compaction variables"
        );
    }
}

#[cfg(test)]
mod a_measure_can_be_bounded_to_the_current_version {
    use crate::transcripts::before_since;

    #[test]
    fn only_events_before_the_bound_are_left_out() {
        assert!(before_since("2026-10-02T23:59:59Z", Some("2026-10-03")));
        assert!(!before_since("2026-10-03T00:00:01Z", Some("2026-10-03")));
        assert!(!before_since("2026-10-01T00:00:00Z", None));
        assert!(!before_since("", Some("2026-10-03")), "an undated event is kept");
    }
}

#[cfg(test)]
mod a_resume_mask_is_not_a_delivered_fold {
    use crate::transcripts::{carries_fold, is_resume_mask};

    #[test]
    fn only_a_bare_resume_mask_is_set_apart() {
        let mask = "[tokenade:elided 440 tokens on cold resume — recover: tokenade expand-ref c5d941e40953]";
        assert!(is_resume_mask(mask));
        assert!(!carries_fold(mask));
        let fold = "a\n[tokenade:auto:json 592 tokens folded away — `tokenade expand-ref 1c90b5e37bd1`]";
        assert!(!is_resume_mask(fold));
        assert!(carries_fold(fold));
    }
}
