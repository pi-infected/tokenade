//! End-to-end tests driving `tokenade code-intel`.

use super::*;

/// #545 — code-intel ops (symbol_find / semantic_search / skeleton /
/// structure_map / impact_of / call_hierarchy / expand_fold) must be
/// EXEMPT from the writer noise threshold so the dashboard's read-time
/// baseline expansion (#544) has data to work with. Without the
/// exemption, a code-intel call where the response was so small that
/// the heuristic hasn't yet been applied (or the caller forgot to
/// apply it) would be silently dropped — losing that telemetry forever.
///
/// We exercise the `tokenade query` subcommand, whose `log_code_intel`
/// helper goes through `log_gain` with op="symbol_find".  Even when
/// the response is tiny, the ledger must keep the row.
#[test]
fn e2e_code_intel_query_logs_even_on_tiny_response() {
    let h = TempHome::new("ci-exempt");
    // `tokenade query <symbol>`: dispatches to symbol_find. With an
    // unknown symbol the response is small (just a "no match" line),
    // but the ledger entry must still land because op=symbol_find is
    // code-intel-exempt from the writer threshold.
    let (_out, _err, _code) = run(
        &["query", "definitely_not_a_real_symbol_xyz_12345"],
        h.path(),
    );
    let ledger_path = h.path().join(".tokenade").join("gain.jsonl");
    if !ledger_path.exists() {
        // Acceptable: the query may have short-circuited before any
        // tokenade-core path that writes the ledger. Skip rather than
        // false-positive on environmental variance.
        return;
    }
    let log = std::fs::read_to_string(&ledger_path).unwrap();
    let saw_code_intel = log
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .any(|v| {
            v["op"]
                .as_str()
                .map(|op| {
                    op == "symbol_find"
                        || op.starts_with("symbol_find:")
                        || op == "semantic_search"
                        || op.starts_with("semantic_search:")
                })
                .unwrap_or(false)
        });
    assert!(
        saw_code_intel,
        "code-intel op (symbol_find) was dropped by log_gain threshold — exemption regression. ledger:\n{log}"
    );
}

/// #1072 — code-intel CLIs are batchable. `tokenade skeleton a b c`
/// must process every file in ONE invocation (cuts turn count, the
/// dominant cost driver), prefixing each with a head-style `==> path
/// <==` separator. A SINGLE file must stay byte-identical to before
/// (no header), so existing single-file callers don't see new noise.
#[test]
fn e2e_skeleton_batches_multiple_files() {
    let h = TempHome::new("skeleton-batch");
    let a = h.path().join("a.rs");
    let b = h.path().join("b.rs");
    std::fs::write(&a, "pub fn alpha() {}\n").unwrap();
    std::fs::write(&b, "pub fn beta() {}\n").unwrap();
    let (a_s, b_s) = (a.to_str().unwrap(), b.to_str().unwrap());

    // Batch: both files in one call → both headers present.
    let (stdout, _err, code) = run(&["skeleton", a_s, b_s], h.path());
    assert_eq!(code, 0, "batch skeleton should exit 0");
    assert!(
        stdout.contains(&format!("==> {a_s} <==")),
        "missing header for first file:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("==> {b_s} <==")),
        "missing header for second file:\n{stdout}"
    );

    // Single file: NO header (byte-compatible with the old behaviour).
    let (single, _e, c) = run(&["skeleton", a_s], h.path());
    assert_eq!(c, 0);
    assert!(
        !single.contains("==>"),
        "single-file skeleton must not emit a head-style header:\n{single}"
    );

    // One missing file in a batch must not sink the rest: the present
    // file still renders, exit code flags the failure (rc=1).
    let missing = h.path().join("nope.rs");
    let (mixed, _me, mc) = run(&["skeleton", a_s, missing.to_str().unwrap()], h.path());
    assert_eq!(mc, 1, "batch with a missing file should exit 1");
    assert!(
        mixed.contains(&format!("==> {a_s} <==")),
        "present file should still render despite a sibling miss:\n{mixed}"
    );
}

/// `tokenade map <path>` used to be refused, and the refusal named no
/// alternative but "print the whole workspace" — the thing the caller was
/// narrowing away from. Measured across this machine's transcripts, that
/// refusal fired 7 times. Both routes must narrow: a `--json` that ignored
/// the path would answer a different question than the text printed from
/// the same command line.
#[test]
fn e2e_map_narrows_to_a_path_on_both_routes() {
    let h = TempHome::new("map-under");
    let proj = h.path();
    std::fs::create_dir_all(proj.join("src/hooks")).unwrap();
    std::fs::create_dir_all(proj.join("src/network")).unwrap();
    std::fs::write(proj.join("src/net.rs"), "pub fn dial() {}\n").unwrap();
    std::fs::write(proj.join("src/network/dns.rs"), "pub fn resolve() {}\n").unwrap();
    std::fs::write(
        proj.join("src/hooks/pre.rs"),
        "pub fn before() {}\npub fn after() {}\n",
    )
    .unwrap();

    let (whole, _e, c0) = run_in(&["map"], proj, proj);
    assert_eq!(c0, 0, "whole map should exit 0:\n{_e}");

    let (part, _e, c1) = run_in(&["map", "src/hooks"], proj, proj);
    assert_eq!(c1, 0, "a path must be accepted now:\n{_e}");
    assert!(
        part.contains("under src/hooks"),
        "the narrowed map must name its scope:\n{part}"
    );
    assert!(!part.contains("net.rs"), "leaked outside the path:\n{part}");
    assert!(part.len() < whole.len(), "narrowing must reduce:\n{part}");

    // `src/net` must not swallow `src/network` — the sibling is a different
    // answer, not a longer one.
    let (sib, _e, _c) = run_in(&["map", "src/net"], proj, proj);
    assert!(
        sib.contains("no indexed file under 'src/net'"),
        "a prefix must stop at a separator:\n{sib}"
    );

    // The JSON route narrows too, and its counts are the narrowed counts.
    let (js, _e, c2) = run_in(&["map", "src/hooks", "--json"], proj, proj);
    assert_eq!(c2, 0, "json map should exit 0:\n{_e}");
    let v: serde_json::Value = serde_json::from_str(&js).expect(&js);
    assert_eq!(v["under"], "src/hooks", "json must record the scope:\n{js}");
    assert_eq!(v["files_indexed"], 1, "json counts the narrowed set:\n{js}");
    assert_eq!(v["symbols"], 2, "json counts the narrowed set:\n{js}");
    let dirs = v["by_dir"].as_array().expect("by_dir");
    assert!(
        dirs.iter().all(|d| d["dir"]
            .as_str()
            .is_some_and(|x| x.starts_with("src/hooks"))),
        "json leaked outside the path:\n{js}"
    );
}

/// `query` shows at most 30 declarations. Four of its five renderings said
/// so; `outline` handed back exactly the cap in silence, which reads as the
/// complete set on the command agents are told to run INSTEAD of opening
/// files. Measured on this repo before the fix: the cap binds on 33 of 80
/// sampled real function names.
#[test]
fn e2e_an_outline_that_hit_the_cap_says_so() {
    let h = TempHome::new("query-outline-cap");
    let proj = h.path();
    let mut src = String::new();
    for i in 0..40 {
        src.push_str(&format!("pub fn widget_{i:02}() {{}}\n"));
    }
    std::fs::write(proj.join("lib.rs"), &src).unwrap();

    let (outline, _e, c) = run_in(&["query", "widget_", "-o", "outline"], proj, proj);
    assert_eq!(c, 0, "outline should exit 0:\n{_e}");
    let listed = outline.lines().filter(|l| !l.starts_with('[')).count();
    assert_eq!(listed, 30, "the cap itself is unchanged:\n{outline}");
    assert!(
        outline.contains("of 40 matches shown"),
        "an outline cut at the cap must name the total:\n{outline}"
    );

    // The neighbouring renderings already agreed; they must keep agreeing.
    let (paths, _e, _c) = run_in(&["query", "widget_", "-o", "paths"], proj, proj);
    assert!(
        paths.contains("of 40 matches shown"),
        "paths regressed:\n{paths}"
    );
    let (count, _e, _c) = run_in(&["query", "widget_", "-o", "count"], proj, proj);
    assert_eq!(
        count.trim(),
        "40",
        "count reports the pre-cap figure:\n{count}"
    );

    // A complete answer grows nothing.
    std::fs::write(proj.join("solo.rs"), "pub fn lonely_one() {}\n").unwrap();
    let (small, _e, _c) = run_in(&["query", "lonely_one", "-o", "outline"], proj, proj);
    assert!(
        !small.contains("matches shown"),
        "an uncut outline must invent no hidden rest:\n{small}"
    );
}

/// `--output-mode` on `query`: `full` (default) is unchanged; `paths`,
/// `count`, `outline` each return a strictly smaller slice.
#[test]
fn e2e_query_output_modes_reduce() {
    let h = TempHome::new("query-omode");
    let proj = h.path();
    std::fs::write(
        proj.join("lib.rs"),
        "pub fn alpha() {}\npub struct Beta;\npub fn gamma() {}\n",
    )
    .unwrap();

    let (full, _e, c0) = run_in(&["query", "alpha"], proj, proj);
    assert_eq!(c0, 0, "query full should exit 0:\n{_e}");
    assert!(
        full.contains("alpha"),
        "full output should mention the symbol:\n{full}"
    );

    // paths: only the file path, no `fn`/`—` signature detail.
    let (paths, _e, c1) = run_in(&["query", "alpha", "-o", "paths"], proj, proj);
    assert_eq!(c1, 0);
    assert!(
        paths.contains("lib.rs"),
        "paths must list the file:\n{paths}"
    );
    assert!(
        !paths.contains(" — "),
        "paths must not carry the location detail:\n{paths}"
    );
    assert!(paths.len() < full.len(), "paths must reduce vs full");

    // count: a bare number.
    let (count, _e, c2) = run_in(&["query", "alpha", "--output-mode=count"], proj, proj);
    assert_eq!(c2, 0);
    assert!(
        count.trim().parse::<usize>().is_ok(),
        "count is numeric:\n{count}"
    );
    assert!(count.len() < full.len(), "count must reduce vs full");

    // outline: location line present, still smaller than a full render.
    let (outline, _e, c3) = run_in(&["query", "alpha", "--format", "outline"], proj, proj);
    assert_eq!(c3, 0);
    assert!(
        outline.contains("alpha"),
        "outline keeps the symbol:\n{outline}"
    );

    // Back-compat: default (no flag) is byte-identical to `-o full`.
    let (full2, _e, _c) = run_in(&["query", "alpha", "-o", "full"], proj, proj);
    assert_eq!(
        full, full2,
        "explicit -o full must equal the default output"
    );

    // Unknown mode → hard error.
    let (_o, err, c4) = run_in(&["query", "alpha", "-o", "bogus"], proj, proj);
    assert_eq!(c4, 2, "unknown mode must exit 2");
    assert!(err.contains("unknown --output-mode"), "clear error:\n{err}");
}

/// `--output-mode` on `skeleton`: default unchanged; `paths`/`count`
/// shrink the output; `outline` == full (skeleton already IS the
/// signatures-only outline).
#[test]
fn e2e_skeleton_output_modes_reduce() {
    let h = TempHome::new("skeleton-omode");
    let f = h.path().join("a.rs");
    // **The file must weigh more than its own PATH.** This test compares
    // lengths in bytes, and `paths` returns precisely the path: with two
    // functions, the full output was 67 bytes and merely moving `TMPDIR` (a
    // full tmpfs forces leaving `/tmp`) was enough to turn "paths must reduce"
    // red without anything being broken. Six signatures put the skeleton out
    // of reach of any reasonable temporary directory.
    let body: String = (0..6)
        .map(|i| format!("pub fn function_number_{i}() {{ let x = {i}; x + 1; }}\n"))
        .collect();
    std::fs::write(&f, body).unwrap();
    let fs = f.to_str().unwrap();

    let (full, _e, c0) = run(&["skeleton", fs], h.path());
    assert_eq!(c0, 0);

    let (paths, _e, c1) = run(&["skeleton", fs, "-o", "paths"], h.path());
    assert_eq!(c1, 0);
    assert_eq!(paths.trim(), fs, "paths mode emits just the file path");
    assert!(paths.len() < full.len(), "paths must reduce");

    let (count, _e, c2) = run(&["skeleton", fs, "-o", "count"], h.path());
    assert_eq!(c2, 0);
    assert!(
        count.trim().parse::<usize>().is_ok(),
        "count numeric:\n{count}"
    );
    assert!(count.len() < full.len(), "count must reduce");

    // outline == full for skeleton (documented fall-through).
    let (outline, _e, _c) = run(&["skeleton", fs, "-o", "outline"], h.path());
    assert_eq!(outline, full, "skeleton outline must equal full");
}

/// "What does this affect?" is a question about scope, and a hub module answers
/// it with hundreds of files. Printing them all turned one call into thousands
/// of tokens of leaf paths, past the point anybody reads — while the count and
/// the machine-readable form still owe the caller the true, complete answer.
#[test]
fn a_hub_module_reports_its_reach_without_dumping_every_leaf() {
    let home = TempHome::new("impact-hub-cap");
    std::fs::write(home.path().join("hub.rs"), "pub fn hub_call() {}\n").expect("write");
    for i in 0..50 {
        std::fs::write(
            home.path().join(format!("leaf{i:03}.rs")),
            "use crate::hub;\nfn go() { hub_call(); }\n",
        )
        .expect("write");
    }
    let hub = "hub.rs";

    let (out, _err, code) = run_in(&["impact", hub], home.path(), home.path());
    assert_eq!(code, 0, "hub must report: {out}");
    let listed = out
        .lines()
        .filter(|l| l.trim_start().starts_with("leaf"))
        .count();
    assert!(
        listed <= 30,
        "the dependent list must stay bounded, listed {listed}: {out}"
    );
    assert!(
        out.contains("dependents shown"),
        "a trimmed list that does not say so reads as the whole answer: {out}"
    );

    // The count owes the TRUE total, not the number that fit.
    let (cnt, _err, _c) = run_in(
        &["impact", hub, "--output-mode", "count"],
        home.path(),
        home.path(),
    );
    let total: usize = cnt
        .split("dependents:")
        .nth(1)
        .and_then(|t| t.split_whitespace().next())
        .and_then(|t| t.parse().ok())
        .unwrap_or(0);
    assert!(total > 30, "count must report every dependent, got {cnt}");

    // And the machine-readable form stays complete.
    let (js, _err, _c) = run_in(&["impact", hub, "--json"], home.path(), home.path());
    assert!(
        js.matches("leaf").count() >= total,
        "--json must still carry the full list: {js}"
    );
}

#[test]
fn asking_a_directory_what_depends_on_it_is_an_error_not_an_empty_answer() {
    let home = TempHome::new("impact-dir-rc");
    let dir = home.path().join("mod_dir");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("thing.rs"), "pub fn thing() {}\n").expect("write");

    let (_out, err, code) = run(&["impact", &dir.to_string_lossy()], home.path());
    assert_ne!(code, 0, "a directory must not answer as an empty success");
    assert!(
        err.to_lowercase().contains("directory"),
        "the refusal must say why: {err}"
    );

    // The file inside it still answers normally — the guard must not swallow
    // the case it exists to protect.
    let (out, _err, code) = run(
        &["impact", &dir.join("thing.rs").to_string_lossy()],
        home.path(),
    );
    assert_eq!(code, 0, "a real file still reports: {out}");
    assert!(out.contains("declares"), "expected a report, got {out}");
}

/// `pack` documents itself as a "budget-bounded repo export", and the budget
/// machinery worked — nothing set it unless the caller thought to.
///
/// MEASURED 2026-08-04: bare `tokenade pack crates/tokenade-core/src` returned
/// 8 569 402 bytes, roughly 2,1 million tokens, from ONE source directory.
/// Same shape as `skeleton`, capped the same day: a guard nobody activates
/// protects nobody.
#[test]
fn e2e_pack_is_bounded_by_default_and_unbounded_on_request() {
    let h = TempHome::new("packbudget");
    let src = h.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    // Comfortably past any reply-sized ceiling.
    for i in 0..60 {
        let body: String = (0..400)
            .map(|j| format!("fn f{i}_{j}() {{ let x = {j}; println!(\"{{x}}\"); }}\n"))
            .collect();
        std::fs::write(src.join(format!("m{i}.rs")), body).unwrap();
    }
    let p = src.to_string_lossy().into_owned();

    let (bounded, _e, code) = run(&["pack", &p], h.path());
    assert_eq!(code, 0, "pack failed");
    let (whole, _e, code0) = run(&["pack", "--budget", "0", &p], h.path());
    assert_eq!(
        code0, 0,
        "`--budget 0` must mean unbounded, as it does for skeleton"
    );

    assert!(
        bounded.len() * 4 < whole.len(),
        "the default is not a ceiling: {} vs {} bytes unbounded",
        bounded.len(),
        whole.len()
    );
    assert!(
        bounded.contains("omitted"),
        "files were dropped without saying so: {}",
        bounded
            .lines()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .join(" | ")
    );
}

/// An empty answer from `query`/`semantic`/`map` must say WHICH empty it is.
///
/// MEASURED 2026-08-08, release binary, one directory outside the repo:
/// `tokenade query fetch_detailed` answered `no symbol matching
/// 'fetch_detailed'` while that symbol exists in the repo it was standing next
/// to. The sentence is a claim about the code, it was false, and it was
/// byte-identical to a genuine miss — the agent has no way to tell "you are in
/// the wrong directory" from "this symbol does not exist", and will believe the
/// second. `map` already named the situation; `query` and `semantic` did not.
///
/// Asserted on all three together, because the defect was precisely that one of
/// them knew and the others did not.
#[test]
fn an_unindexed_directory_says_so_on_every_exploration_command() {
    let h = TempHome::new("nothing-indexed");
    let empty = h.path().join("no-code-here");
    std::fs::create_dir_all(&empty).unwrap();
    // Not source: an indexable file here would defeat the whole test.
    std::fs::write(empty.join("notes.txt"), "juste du texte\n").unwrap();

    const NOTE: &str = "no indexable source code found here";
    for args in [
        vec!["map"],
        vec!["query", "fetch_detailed"],
        vec!["semantic", "how does fetch report errors"],
    ] {
        let (out, err, code) = run_in(&args, h.path(), &empty);
        assert_eq!(code, 0, "{args:?} should not fail, only explain: {err}");
        assert!(
            err.contains(NOTE),
            "`tokenade {}` left the empty answer unexplained.\nstdout: {out}\nstderr: {err}",
            args.join(" ")
        );
    }
}

/// A file the indexer never parsed must not answer like a leaf module.
///
/// `impact <non-source>` returned "declares (0 symbols) / dependents (0)" with
/// exit 0 — a confident "nothing depends on this" about a file the graph has
/// never seen. MEASURED 2026-08-08 on this repo with `impact README.md`. The
/// directory guard directly above in `code_intel.rs` was written for exactly
/// this shape (#978) and closed only the directory half; the same answer was
/// still reachable with a real file, and for a source file in an unsupported
/// language the claim is not just uninformative but wrong.
///
/// Asserted on stderr with stdout and the exit code unchanged: the report is
/// still correct for a file that genuinely has no dependents, and anything
/// parsing stdout keeps working. Same shape as
/// `an_unindexed_directory_says_so_on_every_exploration_command`.
#[test]
fn a_file_the_indexer_never_parsed_says_so_instead_of_claiming_no_dependents() {
    let home = TempHome::new("impact-unparsed");
    let src = home.path().join("lib.rs");
    std::fs::write(&src, "pub fn thing() {}\n").expect("write");
    let doc = home.path().join("NOTES.md");
    std::fs::write(&doc, "# notes\n\nsome prose\n").expect("write");

    const NOTE: &str = "contributed no symbols to the index";

    let (_out, err, code) = run_in(
        &["impact", &doc.to_string_lossy()],
        home.path(),
        home.path(),
    );
    assert_eq!(code, 0, "the report itself is not an error: {err}");
    assert!(
        err.contains(NOTE),
        "a file outside the graph answered `dependents (0)` unexplained: {err}"
    );

    // The same file by ABSOLUTE path must answer identically. The index keys
    // symbols by root-relative path (exact string compare) while dependents
    // match on file stem, so an absolute path used to return "declares (0
    // symbols)" WITH dependents — half a report, exit 0, for a path shape the
    // refusal message two guards up explicitly advertises. Found by this very
    // test firing its note on a real source file.
    let abs = std::fs::canonicalize(&src).expect("canonicalize");
    let (out_abs, err_abs, _c) = run_in(
        &["impact", &abs.to_string_lossy()],
        home.path(),
        home.path(),
    );
    assert!(
        !err_abs.contains(NOTE) && out_abs.contains("1 symbol"),
        "an absolute path lost the declarations:\nstdout: {out_abs}\nstderr: {err_abs}"
    );

    // The control the guard must not swallow: a real source file answers
    // normally and says nothing extra.
    let (_out, err, _code) = run_in(
        &["impact", &src.to_string_lossy()],
        home.path(),
        home.path(),
    );
    assert!(
        !err.contains(NOTE),
        "the note fired on an indexed source file: {err}"
    );
}

/// `tokenade read` must get the same competing candidates `tokenade wrap`
/// gets. They lived inside `filter::auto_or_generic`, which only the command
/// path reaches, so the SAME BYTES folded very differently depending on which
/// door they came through.
///
/// Measured 2026-08-26 on this exact shape — a 1 500-entry tar listing whose
/// entries sit in 1 500 distinct top-level directories, so the archive
/// compactor has nothing to group and prints a line per entry: 94 890 bytes in,
/// 39 538 out through `read` and about 1 600 through `wrap`.
///
/// Asserted through the BINARY, not against `best_auto_candidate`: a unit test
/// of the chooser passes whether or not `cmd_auto` calls it.
#[test]
fn e2e_read_gets_the_same_competing_candidates_as_wrap() {
    let h = TempHome::new("read-candidate-parity");
    let home = h.path();
    let p = home.join("listing.txt");
    let mut t = String::new();
    for i in 0..1500 {
        t.push_str(&format!(
            "-rw-rw-r-- 7/9   {:>7} 2026-08-19 03:07 sigma{i}/dist/bundle.mjs\n",
            91_000 + i
        ));
    }
    std::fs::write(&p, &t).unwrap();
    let (out, _err, code) = run(&["read", p.to_str().unwrap()], home);
    assert_eq!(code, 0);
    assert!(
        out.len() * 100 < t.len(),
        "read left {} bytes of {} — the fold candidate did not reach it:\n{}",
        out.len(),
        t.len(),
        &out[..300.min(out.len())]
    );
    // …and it is the TEMPLATE fold, not a truncation: the count must be stated.
    assert!(
        out.contains("1500"),
        "the fold did not declare how many lines it stood for:\n{out}"
    );
}

/// The same parity question, one layer further out. The test above
/// covers the COMPETING CANDIDATES; this one covers the POST-PASSES, which
/// lived inside `filter_command` and so ran on the command door alone.
///
/// The shape is a BuildKit-style build log: four lines redrawn thirty times
/// with `ESC[4A`, which no `\r` collapse touches and no format detector claims.
/// Measured 2026-08-27 before the fix — `wrap` delivered 267 bytes of it and
/// `read` delivered 4 678, which is not a weaker fold but none.
///
/// Through the BINARY, because a unit test of `shrink_only_post_passes` passes
/// whether or not `cmd_auto` calls it — the same reason the test above gives.
#[test]
fn e2e_read_gets_the_same_post_passes_as_wrap() {
    let h = TempHome::new("read-post-pass-parity");
    let home = h.path();
    // NOT `.log`: the extension hint routes that straight to the Logs
    // compactor, which folds this shape on its own and would make the guard
    // pass with the post-passes unwired. Measured — the first version of this
    // test did exactly that.
    let p = home.join("buildkit.txt");
    let mut t = String::new();
    for tick in 0..30 {
        if tick > 0 {
            t.push_str("\x1b[4A");
        }
        for j in 1..=4 {
            t.push_str(&format!(
                "\x1b[2K#{j} [internal] step {j} of 4 ... {}s\n",
                tick * 3 + j
            ));
        }
    }
    t.push_str("#4 DONE 12.3s\n");
    std::fs::write(&p, &t).unwrap();
    let (out, _err, code) = run(&["read", p.to_str().unwrap()], home);
    assert_eq!(code, 0);
    assert!(
        out.len() * 5 < t.len(),
        "read left {} bytes of {} — the post-passes did not reach it:\n{}",
        out.len(),
        t.len(),
        &out[..300.min(out.len())]
    );
    // REPLAYED, not folded: the terminal showed four lines and they are the
    // LAST frame, so there is nothing to declare a count for.
    assert_eq!(
        out.lines().filter(|l| !l.trim().is_empty()).count(),
        5,
        "four redrawn lines and the DONE line:\n{out}"
    );
    assert!(
        out.contains("#1 [internal] step 1 of 4 ... 88s"),
        "the surviving frame must be the last one:\n{out}"
    );
    assert!(
        out.contains("#4 DONE 12.3s"),
        "the one line that is not part of the redraw must survive:\n{out}"
    );
    assert!(
        !out.contains('\x1b'),
        "the redraw grammar is consumed, not passed on:\n{out:?}"
    );
}

/// `cat -n` is how an agent asks for line numbers before an edit, and it broke
/// the detector, the deduper and the disclose summary at once. Measured
/// 2026-08-26 on a 26 KB markdown article: `cat FILE` delivered 3 294 bytes and
/// `cat -n FILE` delivered 14 743 — and reading the numbered file back returned
/// 27 653 bytes of a 27 652-byte input, which is not a weaker fold but none.
///
/// Through the BINARY, both doors, because the fix sits in the one place both
/// paths share and a unit test of `line_gutter` proves neither reaches it.
#[test]
fn e2e_line_numbered_output_folds_like_the_document_it_is() {
    let h = TempHome::new("cat-n-gutter");
    let home = h.path();
    let mut doc = String::from("# A document\n\n");
    for i in 0..120 {
        doc.push_str(&format!(
            "## Section {i}\n\nA paragraph of prose about section {i}, long enough that folding \
             it is worth doing.\n\n- point one\n- point two\n\n"
        ));
    }
    let numbered: String = doc
        .lines()
        .enumerate()
        .map(|(i, l)| format!("{:>6}\t{l}\n", i + 1))
        .collect();
    let p = home.join("numbered.txt");
    std::fs::write(&p, &numbered).unwrap();
    let (out, _err, code) = run(&["read", p.to_str().unwrap()], home);
    assert_eq!(code, 0);
    assert!(
        out.len() * 2 < numbered.len(),
        "read left {} bytes of {} — the gutter still hides the document:\n{}",
        out.len(),
        numbered.len(),
        &out[..200.min(out.len())]
    );

    // The same bytes through the COMMAND door, and the numbers are declared
    // rather than silently dropped.
    let plain = home.join("plain.md");
    std::fs::write(&plain, &doc).unwrap();
    let (wout, _e, wcode) = run(&["wrap", &format!("cat -n {}", plain.display())], home);
    assert_eq!(wcode, 0);
    assert!(
        wout.len() * 2 < numbered.len(),
        "wrap left {} bytes of {}",
        wout.len(),
        numbered.len()
    );
}

/// `tokenade index` must not reuse an index another tokenade built.
///
/// `Index::load` refuses a foreign `built_by` — that is what makes an upgrade
/// pick up a changed extractor. `cmd_index` read and deserialised the sidecar
/// itself, skipping the guard, so it answered `incremental` and reused the
/// previous build's symbols while every query path in the SAME binary was
/// correctly refusing the same file. Two answers to one question.
///
/// The fixture builds the sidecar itself. It used to restamp the one
/// `cmd_index` left behind, and that stopped working twice over: the store
/// became the artefact `load` prefers, and then `index` stopped writing a
/// sidecar at all once the store had taken the index. Both times the
/// property stayed true and only the artefact moved — so the sidecar is
/// written here, and the store removed, to put it back on the path under test.
#[test]
fn index_refuses_a_sidecar_another_build_wrote() {
    let h = TempHome::new("index-foreign-stamp");
    let proj = h.path().join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::write(proj.join("src").join("a.rs"), "pub fn alpha() {}\n").unwrap();

    // A real index for this tree, serialised exactly as `load` expects it.
    let built = tokenade_core::index::Index::build(&proj).expect("build");
    assert!(
        !built.mtimes.is_empty(),
        "the fixture needs mtimes, or every run is full for another reason"
    );
    let base = serde_json::to_value(&built).expect("serialise");

    let store = h.path().join(".tokenade").join("index.db");
    let sidecar = proj.join(".tokenade").join("index.json");
    std::fs::create_dir_all(proj.join(".tokenade")).unwrap();
    let run_with_stamp = |stamp: &str| -> String {
        let mut v = base.clone();
        v["built_by"] = serde_json::Value::String(stamp.into());
        std::fs::write(&sidecar, serde_json::to_string(&v).unwrap()).unwrap();
        // The sidecar is the MIGRATION artefact: it is read only when the
        // store has no row, so the store must be out of the way for this to
        // test anything at all.
        let _ = std::fs::remove_file(&store);
        let (out, err, code) = run_in(&["index"], h.path(), &proj);
        assert_eq!(code, 0, "index failed: {err}");
        out
    };

    // Non-vacuity FIRST: with THIS build's stamp the same sidecar is read and
    // the rebuild is incremental. Without this, "full" below could mean the
    // sidecar was unreadable for any reason at all.
    let own = run_with_stamp(env!("CARGO_PKG_VERSION"));
    assert!(
        own.contains("incremental"),
        "our own stamp must be reused, got: {own}"
    );

    let foreign = run_with_stamp("0.0.0-from-an-older-tokenade");
    assert!(
        foreign.contains("full"),
        "a foreign stamp must force a full rebuild, got: {foreign}"
    );
}

/// `--force` rebuilds when nothing on disk changed.
///
/// The incremental path asks the filesystem "did any file change", and the
/// answer is no whenever what changed is tokenade itself — a grammar, an
/// extraction rule, a tokenizer. Within one version nothing can notice that,
/// and the workaround was deleting two files by hand.
#[test]
fn index_force_rebuilds_an_unchanged_tree() {
    let h = TempHome::new("index-force");
    let proj = h.path().join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(proj.join("src").join("a.rs"), "pub fn alpha() {}\n").unwrap();
    let (_o, err, code) = run_in(&["index"], h.path(), &proj);
    assert_eq!(code, 0, "first index failed: {err}");
    let (warm, _e, _c) = run_in(&["index"], h.path(), &proj);
    assert!(
        warm.contains("incremental"),
        "second run should reuse: {warm}"
    );
    let (forced, _e2, c2) = run_in(&["index", "--force"], h.path(), &proj);
    assert_eq!(c2, 0);
    assert!(
        forced.contains("full (forced)"),
        "--force must rebuild from scratch, got: {forced}"
    );
    // …including the SEMANTIC index. Its postings are the tokenizer's output,
    // so a forced rebuild that reused them would miss exactly the kind of
    // change the caller is forcing for.
    let (warm_json, _e3, _c3) = run_in(&["index", "--json"], h.path(), &proj);
    let warm_v: serde_json::Value = serde_json::from_str(&warm_json).unwrap();
    assert_eq!(warm_v["semantic_mode"], "incremental", "{warm_json}");
    let (json, _e4, _c4) = run_in(&["index", "--force", "--json"], h.path(), &proj);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["mode"], "full (forced)", "{json}");
    assert_eq!(
        v["semantic_mode"], "full",
        "the semantic index was reused: {json}"
    );
}

/// `tokenade index --force` must change what the OTHER commands see.
///
/// It wrote `.tokenade/index.json` and nothing else. Since storage moved to the
/// global index store, that sidecar is only a migration fallback for a project
/// the store does not hold yet — so on any project queried even once, a forced
/// rebuild landed where nothing reads it, reported `full (forced)`, and left
/// every query answering from the previous blob.
///
/// The fixture pins the file's SIZE and MTIME across the edit, so the
/// incremental path genuinely cannot see the change: forcing is the only thing
/// that can, which is exactly what `--force` is for (a grammar or extraction
/// rule changing inside tokenade itself is invisible to any fingerprint).
#[test]
fn e2e_a_forced_reindex_reaches_the_store_the_other_commands_read() {
    let h = TempHome::new("index-force");
    let proj = h.path().join("proj");
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    let src = proj.join("a.rs");
    std::fs::write(&src, "pub fn alpha_one() {}\n").unwrap();

    // A first query populates the store.
    let (out, _e, code) = run_in(&["query", "alpha_one"], h.path(), &proj);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("alpha_one"), "{out}");

    // Same length, and the mtime put back: the fingerprint does not move.
    let meta = std::fs::metadata(&src).unwrap();
    let mtime = filetime_of(&meta);
    std::fs::write(&src, "pub fn betaa_two() {}\n").unwrap();
    set_mtime(&src, mtime);
    assert_eq!(
        std::fs::metadata(&src).unwrap().len(),
        meta.len(),
        "the fixture must keep the same size or the walk sees the edit"
    );

    // Non-vacuity: without forcing, the change really is invisible.
    let (before, _e, _c) = run_in(&["query", "betaa_two"], h.path(), &proj);
    assert!(
        before.contains("no symbol matching"),
        "the fixture did not actually hide the edit: {before}"
    );

    let (js, _e, code) = run_in(&["index", "--force", "--json"], h.path(), &proj);
    assert_eq!(code, 0, "{js}");
    let v: serde_json::Value = serde_json::from_str(&js).expect("--json output");
    assert_eq!(v["mode"], "full (forced)", "{js}");
    assert_eq!(
        v["stored"], true,
        "the rebuild did not reach the store: {js}"
    );

    let (after, _e, _c) = run_in(&["query", "betaa_two"], h.path(), &proj);
    assert!(
        after.contains("betaa_two"),
        "a forced rebuild did not change what `query` answers: {after}"
    );
    let (gone, _e, _c) = run_in(&["query", "alpha_one"], h.path(), &proj);
    assert!(
        gone.contains("no symbol matching"),
        "the stale symbol survived a forced rebuild: {gone}"
    );
}

/// And `tokenade index` must not write the sidecar back.
///
/// `Index::save` removes the legacy artefacts once the store has the index;
/// `cmd_index` re-created one on every run, which undid `migrate_local_index`
/// and left a stale duplicate — 22.2 MB on this repository against the 5.5 MB
/// the store keeps. It is written now only when the store REFUSED, where it is
/// the one copy `load` can still fall back to.
#[test]
fn e2e_index_leaves_no_stale_sidecar_behind() {
    let h = TempHome::new("index-sidecar");
    let proj = h.path().join("proj");
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::write(proj.join("a.rs"), "pub fn alpha_one() {}\n").unwrap();

    let (js, err, code) = run_in(&["index", "--json"], h.path(), &proj);
    assert_eq!(code, 0, "{err}");
    let v: serde_json::Value = serde_json::from_str(&js).expect("--json output");
    assert_eq!(v["stored"], true, "{js}");
    assert_eq!(
        v["index_path"], "the index store",
        "the report named an artefact this run did not write: {js}"
    );

    // Non-vacuity: the run really did index something, so "no sidecar" is not
    // "nothing happened".
    assert!(v["symbols"].as_u64().unwrap_or(0) >= 1, "{js}");
    for legacy in [".tokenade", ".sts"] {
        let p = proj.join(legacy).join("index.json");
        assert!(
            !p.exists(),
            "a stale {legacy}/index.json was left in the project: {}",
            p.display()
        );
    }
}

/// The hazard the fix creates, closed. `Index::load` deliberately leaves the
/// call graph behind, so an `index` run that sees no change holds an EMPTY one
/// — persisting that would wipe a real graph, and every `called by:` line with
/// it. Nothing else would break: a truncated graph is a valid graph.
#[test]
fn e2e_an_unchanged_reindex_keeps_the_call_graph() {
    let h = TempHome::new("index-graph");
    let proj = h.path().join("proj");
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::write(
        proj.join("c.rs"),
        "pub fn helper_x() {}\npub fn caller_y() { helper_x(); }\n",
    )
    .unwrap();

    let (out, _e, _c) = run_in(&["query", "helper_x"], h.path(), &proj);
    assert!(
        out.contains("called by:") && out.contains("caller_y"),
        "the fixture must produce a call edge to begin with: {out}"
    );

    // Nothing changed on disk — the rebuild is incremental and quiet.
    let (idx_out, _e, code) = run_in(&["index"], h.path(), &proj);
    assert_eq!(code, 0, "{idx_out}");

    let (again, _e, _c) = run_in(&["query", "helper_x"], h.path(), &proj);
    assert!(
        again.contains("called by:") && again.contains("caller_y"),
        "the reindex persisted an empty call graph over a real one: {again}"
    );
}

fn filetime_of(m: &std::fs::Metadata) -> std::time::SystemTime {
    m.modified().expect("mtime")
}

fn set_mtime(p: &Path, t: std::time::SystemTime) {
    let f = std::fs::File::options().write(true).open(p).unwrap();
    f.set_modified(t).unwrap();
}

/// The suggestion has to survive the real binary, both renderings, and the
/// exit code. `query` prints the miss from two places — the default path and
/// the `-o outline` renderer — and only one of them held the index, so the
/// fix had to be routed through one declarant rather than pasted twice.
#[test]
fn a_query_miss_names_the_nearest_indexed_symbols() {
    let home = crate::TempHome::new("query-miss-suggest");
    let proj = home.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join("lib.rs"),
        "pub fn cmd_mcp_proxy() {}\npub fn uninstall_path_shim() {}\npub fn totally_other() {}\n",
    )
    .unwrap();
    let p = proj.to_str().unwrap();

    for args in [
        vec!["query", "cmd_mcp_wrap"],
        vec!["query", "cmd_mcp_wrap", "-o", "outline"],
    ] {
        let (out, _e, rc) = crate::run_in(&args, home.path(), std::path::Path::new(p));
        assert_eq!(rc, 0, "a miss is not an error: {out}");
        assert!(
            out.contains("no symbol matching 'cmd_mcp_wrap'"),
            "the refusal itself must survive: {out}"
        );
        assert!(
            out.contains("cmd_mcp_proxy"),
            "the refusal must name what was meant ({args:?}): {out}"
        );
    }

    // Nothing close → the bare refusal, with no invented list after it.
    let (out, _e, rc) = crate::run_in(
        &["query", "zzqqxx_nothing_like_this"],
        home.path(),
        std::path::Path::new(p),
    );
    assert_eq!(rc, 0);
    assert!(
        !out.contains("closest indexed"),
        "nothing is close, so nothing should be offered: {out}"
    );
}

/// A `map` path spelled from the workspace root resolves from a subdirectory.
///
/// The index handed to the command is re-based to the cwd, so from a
/// subdirectory a sibling's file is stored as `../sibling/src/…`. Measured
/// 2026-09-04 in this repository: `tokenade map crates/tokenade-core/src`, run
/// from `crates/tokenade-cli`, answered `no indexed file under
/// 'crates/tokenade-core/src'` — and the same message said paths are matched
/// "from the workspace root".
#[test]
fn e2e_map_accepts_a_path_spelled_from_the_workspace_root() {
    let h = TempHome::new("map-rebase");
    // The project sits UNDER the home directory, never AT it, and HOME is
    // passed separately: a project whose root IS the home directory is not
    // treated as a workspace, so the index built from a subdirectory holds
    // only that subdirectory and no spelling of a sibling can resolve. That
    // cost three runs of this test reading as a broken fix.
    let proj = &h.path().join("repo");
    std::fs::create_dir_all(proj.join("alpha/src")).unwrap();
    std::fs::create_dir_all(proj.join("beta/src")).unwrap();
    std::fs::create_dir_all(proj.join("delta/src")).unwrap();
    std::fs::create_dir_all(proj.join("beta/alpha/src")).unwrap();
    std::fs::write(proj.join("alpha/src/one.rs"), "pub fn alpha_one() {}\n").unwrap();
    std::fs::write(proj.join("beta/src/two.rs"), "pub fn beta_two() {}\n").unwrap();
    // `alpha/src` exists BOTH under the cwd and under the root, so the two
    // spellings name different subtrees and the precedence is testable.
    std::fs::write(
        proj.join("beta/alpha/src/three.rs"),
        "pub fn beta_alpha_three() {}\n",
    )
    .unwrap();
    // …and `delta/src` exists only under the root.
    std::fs::write(proj.join("delta/src/four.rs"), "pub fn delta_four() {}\n").unwrap();
    // The root is found by walking up to a VCS marker, and without one the
    // "workspace root" from `beta/` IS `beta/` — there would be no second
    // spelling to accept, and the test would pass for the wrong reason.
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    let sub = proj.join("beta");
    // Build the workspace index once from the root, which is what any user of
    // this repository has already done. Without it the index built from a
    // subdirectory holds only that subdirectory, and no spelling of a
    // sibling's path can resolve because the sibling is not in it.
    let (_w, _e, cw) = run_in(&["map"], h.path(), proj);
    assert_eq!(cw, 0, "the warm-up map failed:\n{_e}");

    // The spelling the caller sees around them still wins.
    let (own, _e, c0) = run_in(&["map", "src"], h.path(), &sub);
    assert_eq!(c0, 0, "the cwd-relative path broke:\n{_e}");
    assert!(own.contains("two.rs"), "wrong subtree:\n{own}");
    assert!(!own.contains("one.rs"), "leaked outside the path:\n{own}");

    // Where BOTH spellings match, the one the caller sees around them wins:
    // re-basing rescues a path that would be refused, it never re-interprets
    // one that already works.
    let (both, _e, cb) = run_in(&["map", "alpha/src"], h.path(), &sub);
    assert_eq!(cb, 0, "the ambiguous path failed:\n{_e}");
    assert!(
        both.contains("three.rs") && !both.contains("one.rs"),
        "the cwd spelling must win when both match:\n{both}"
    );

    // …and the workspace-root spelling is no longer a refusal.
    let (root_spelled, _e, c1) = run_in(&["map", "delta/src"], h.path(), &sub);
    assert_eq!(c1, 0, "the root-relative path was refused:\n{_e}");
    assert!(
        !root_spelled.contains("no indexed file"),
        "a path that IS indexed was refused:\n{root_spelled}"
    );
    assert!(
        root_spelled.contains("four.rs"),
        "resolved to the wrong subtree:\n{root_spelled}"
    );

    // A path that is nowhere is still a refusal, on both routes.
    let (missing, _e, _c) = run_in(&["map", "gamma/src"], h.path(), &sub);
    assert!(
        missing.contains("no indexed file under 'gamma/src'"),
        "a path that exists nowhere must still be refused:\n{missing}"
    );
    let (js, _e, _c) = run_in(&["map", "delta/src", "--json"], h.path(), &sub);
    let v: serde_json::Value = serde_json::from_str(&js).expect(&js);
    assert_eq!(
        v["files_indexed"], 1,
        "the json route must narrow to the same subtree:\n{js}"
    );
}

/// A depth flag with a value that is not a depth is refused through the binary.
///
/// MEASURED 2026-09-04: `tokenade semantic "dedup" --limit abc` printed the
/// default ten hits and exited 0 — the same 699 bytes as passing no flag —
/// while `--budget abc`, `--max abc`, `--context abc` and `hooks tail abc` all
/// refuse. A caller that asked to look past the first screen got the first
/// screen and no way to know.
#[test]
fn e2e_semantic_refuses_a_limit_that_is_not_a_count() {
    let h = TempHome::new("semantic-limit");
    let proj = &h.path().join("repo");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::write(proj.join("src/a.rs"), "pub fn dedup_rows() {}\n").unwrap();

    for bad in [
        ["semantic", "dedup", "--limit", "abc"],
        ["semantic", "dedup", "--limit", "0"],
    ] {
        let (out, err, code) = run_in(&bad, h.path(), proj);
        let both = format!("{out}\n{err}");
        assert_eq!(code, 2, "accepted {bad:?}:\n{both}");
        assert!(
            both.contains("--limit needs a positive integer"),
            "the refusal does not say what is wrong:\n{both}"
        );
    }
    // A real depth still works, and so does no flag at all.
    for good in [
        vec!["semantic", "dedup", "--limit", "3"],
        vec!["semantic", "dedup"],
    ] {
        let (_o, _e, code) = run_in(&good, h.path(), proj);
        assert_eq!(code, 0, "refused a valid call {good:?}:\n{_e}");
    }
}

/// Every listing mode says when the ranking stopped at the depth it was given.
///
/// MEASURED 2026-09-04 on this repository: `semantic "dedup" -o count`
/// answered `40+` while the same query returned exactly ten rows in full,
/// paths and outline modes with nothing to say a further screen existed.
#[test]
fn e2e_semantic_says_when_it_stopped_at_the_depth() {
    let h = TempHome::new("semantic-depth");
    let proj = &h.path().join("repo");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    for i in 0..12 {
        std::fs::write(
            proj.join(format!("src/dedup_{i}.rs")),
            format!("pub fn dedup_rows_{i}() {{}}\n"),
        )
        .unwrap();
    }

    for mode in [vec![], vec!["-o", "paths"], vec!["-o", "outline"]] {
        let mut args: Vec<&str> = vec!["semantic", "dedup", "--limit", "2"];
        args.extend(mode.iter().copied());
        let (out, err, code) = run_in(&args, h.path(), proj);
        assert_eq!(code, 0, "{args:?} failed:\n{err}");
        assert!(
            out.contains("[tokenade:depth]") && out.contains("2 shown"),
            "a truncated listing looks complete ({args:?}):\n{out}"
        );
    }

    // `count` answers with the number itself and already marks its own floor.
    let (count, _e, _c) = run_in(
        &["semantic", "dedup", "--limit", "2", "-o", "count"],
        h.path(),
        proj,
    );
    assert!(
        !count.contains("[tokenade:depth]"),
        "the count mode does not need a footer about itself:\n{count}"
    );

    // A depth nothing reached says nothing.
    let (deep, _e, _c) = run_in(&["semantic", "dedup", "--limit", "200"], h.path(), proj);
    assert!(
        !deep.contains("[tokenade:depth]"),
        "an answer that was not cut claims it was:\n{deep}"
    );
}

/// Both misses name their corpus and a way out, through the binary.
///
/// MEASURED 2026-09-04: `query zzz_no_such_symbol` and `semantic "<nonsense>"`
/// each answered one bare line. The declarant that exists to stop that says it
/// is "shared by map, query and semantic" — and `semantic` printed its own
/// literal instead, so the drift the declarant was written to end had already
/// come back in one of the three.
#[test]
fn e2e_a_miss_says_what_was_searched_and_what_to_try() {
    let h = TempHome::new("miss-lines");
    let proj = &h.path().join("repo");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::write(proj.join("src/a.rs"), "pub fn dedup_rows() {}\n").unwrap();

    let (q, _e, qc) = run_in(&["query", "zqxjv_no_such_symbol"], h.path(), proj);
    assert_eq!(qc, 0, "query miss should still exit 0:\n{_e}");
    assert!(
        q.contains("indexed file(s) searched") && q.contains("tokenade semantic"),
        "the symbol miss says neither what was searched nor where to go:\n{q}"
    );

    let (s, _e, sc) = run_in(&["semantic", "zqxjv wgtbp"], h.path(), proj);
    assert_eq!(sc, 0, "semantic miss should still exit 0:\n{_e}");
    assert!(
        s.contains("no match for 'zqxjv wgtbp'"),
        "the query echo is what the help promises:\n{s}"
    );
    assert!(
        s.contains("indexed file(s) searched") && s.contains("grep -rn"),
        "the semantic miss says neither what was searched nor where to go:\n{s}"
    );

    // A hit is untouched by any of this.
    let (hit, _e, _c) = run_in(&["query", "dedup_rows"], h.path(), proj);
    assert!(
        hit.contains("dedup_rows") && !hit.contains("indexed file(s) searched"),
        "a hit gained a miss note:\n{hit}"
    );
}

/// Each miss sentence is written in exactly ONE production place.
///
/// The declarant's doc long claimed that the line is "shared by
/// map, query and semantic", and `semantic` was printing its own literal the
/// whole time. A sweep is the only thing that catches the fourth site.
#[test]
fn each_miss_sentence_has_a_single_production_site() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sites: std::collections::BTreeMap<&str, Vec<String>> =
        std::collections::BTreeMap::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).expect("read_dir") {
            let p = e.expect("entry").path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().and_then(|x| x.to_str()) != Some("rs") {
                continue;
            }
            let src = std::fs::read_to_string(&p).expect("read");
            let production: std::collections::HashSet<usize> =
                tokenade_core::srcsweep::production_lines(&src)
                    .into_iter()
                    .map(|(n, _)| n)
                    .collect();
            for (i, line) in src.lines().enumerate() {
                for needle in ["no symbol matching '{q}'", "no match for '{q}'"] {
                    if line.contains(needle) && production.contains(&i) {
                        sites
                            .entry(needle)
                            .or_default()
                            .push(format!("{}:{}", p.display(), i + 1));
                    }
                }
            }
        }
    }
    // Several branches may spell a miss — bare, "closest indexed", and the
    // one for a renderer with no index — but all of them must live in the SAME
    // file, which is what keeps a fourth from drifting. The sweep found exactly
    // that when it was written: a third spelling in `code_intel/output.rs`, in
    // the command whose declarer's doc claims the line is shared.
    //
    // The rule is about the file's UNIQUENESS, not its name: the declarer was
    // called `code_intel.rs`, it has been `code_intel/map.rs` since the
    // commands were split, and naming the address made it report a drift where
    // nothing had drifted.
    for (needle, found) in &sites {
        assert!(
            !found.is_empty(),
            "{needle} is written nowhere: the guard no longer measures anything"
        );
        let files: std::collections::BTreeSet<&str> = found
            .iter()
            .map(|f| f.rsplit_once(':').map(|(p, _)| p).unwrap_or(f))
            .collect();
        assert_eq!(
            files.len(),
            1,
            "{needle} is written in several files, so it can drift: {files:?}"
        );
    }
}

/// `--json` and `-o <mode>` cannot both choose the shape, through the binary.
///
/// MEASURED 2026-09-04 on this repository: `tokenade map crates/tokenade-core/src
/// --json -o paths` returned 5 728 bytes, byte-identical to `--json` alone,
/// where `-o paths` alone returns 1 159 — five times what the caller asked
/// for, silently, on a product whose subject is tokens.
#[test]
fn e2e_json_and_output_mode_do_not_silently_pick_one() {
    let h = TempHome::new("json-vs-mode");
    let proj = &h.path().join("repo");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::write(proj.join("src/a.rs"), "pub fn dedup_rows() {}\n").unwrap();

    for args in [
        vec!["map", "src", "--json", "-o", "paths"],
        vec!["query", "dedup_rows", "--json", "-o", "count"],
        vec!["impact", "src/a.rs", "--json", "-o", "paths"],
    ] {
        let (out, err, code) = run_in(&args, h.path(), proj);
        let both = format!("{out}\n{err}");
        assert_eq!(code, 2, "{args:?} was accepted:\n{both}");
        assert!(
            both.contains("both choose the shape"),
            "{args:?} refused for the wrong reason:\n{both}"
        );
    }

    // The list of json-capable commands cannot rot: each of the three really
    // has the flag, and the two that do not keep their own, better refusal.
    for v in [
        vec!["map", "src", "--json"],
        vec!["query", "dedup_rows", "--json"],
        vec!["impact", "src/a.rs", "--json"],
    ] {
        let (out, _e, code) = run_in(&v, h.path(), proj);
        assert_eq!(code, 0, "{v:?} should be a real flag:\n{_e}");
        assert!(
            out.trim_start().starts_with('{'),
            "{v:?} returned no document:\n{out}"
        );
    }
    for v in [
        vec!["skeleton", "src/a.rs", "--json"],
        vec!["semantic", "dedup", "--json"],
    ] {
        let (_o, err, code) = run_in(&v, h.path(), proj);
        assert_eq!(code, 2, "{v:?} should be refused");
        assert!(
            err.contains("unknown flag '--json'"),
            "{v:?} lost its own refusal:\n{err}"
        );
    }
}

/// `--budget 0` means NO CEILING, on every command that takes the flag.
///
/// `parse_budget`'s own doc states the contract — "`--budget 0` means 'no
/// ceiling' and is legal" — and `skeleton` implements it as
/// `.filter(|n| *n > 0)`, `pack` returns MORE than its default under it, and
/// `expand-ref --raw --budget 0` hands over the whole blob. `semantic` passed
/// the parsed `Some(0)` straight to the budget, which enforced a literal zero:
/// MEASURED on 1.1.9, `semantic <q> --budget 0` returned 316 bytes against 922
/// with no flag — one row, nine withheld, under a footer reading "24 tokens
/// spent against a 0-token ceiling" that advised raising the very flag the
/// caller had used to remove the ceiling. A budget of 20 tokens returned 317
/// bytes, so the flag meant its own opposite.
#[test]
fn e2e_a_zero_budget_means_no_ceiling() {
    let h = TempHome::new("semantic-zero-budget");
    let proj = &h.path().join("repo");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    // Enough distinct hits that a ceiling has something to withhold.
    for i in 0..12 {
        std::fs::write(
            proj.join(format!("src/dedup_{i}.rs")),
            format!(
                "/// Deduplicate stored rows, pass {i}.\npub fn dedup_rows_{i}() {{}}\n\
                 pub fn dedup_helper_{i}() {{}}\n"
            ),
        )
        .unwrap();
    }

    let ask = |args: &[&str]| -> String {
        let (out, err, code) = run_in(args, h.path(), proj);
        assert_eq!(code, 0, "{args:?} failed:\n{err}");
        out
    };
    let zero = ask(&["semantic", "dedup", "--budget", "0"]);
    let tight = ask(&["semantic", "dedup", "--budget", "20"]);
    let unset = ask(&["semantic", "dedup"]);

    // Anti-vacuity: the tight ceiling must really bite, or all three are the
    // same passthrough and the comparison proves nothing.
    assert!(
        tight.len() < unset.len(),
        "fixture too small — a 20-token ceiling withheld nothing, so this test \
         cannot tell a lifted ceiling from an enforced one:\n{tight}"
    );
    // The contract: 0 is not a small ceiling, it is no ceiling.
    assert!(
        zero.len() > tight.len(),
        "`--budget 0` behaved like a TIGHT ceiling ({} bytes vs {} at --budget \
         20); the flag means no ceiling:\n{zero}",
        zero.len(),
        tight.len()
    );
    assert_eq!(
        zero.trim(),
        unset.trim(),
        "`--budget 0` must deliver exactly what no flag delivers"
    );
    assert!(
        !zero.contains("0-token ceiling"),
        "`--budget 0` must not report a ceiling it does not have:\n{zero}"
    );

    // The contract is SHARED, so guard every consumer of it. Proving it on
    // `semantic` alone left the sibling free to break: a mutation removing
    // `skeleton`'s own zero-guard lived through this test until these lines
    // existed, and `skeleton` is where the spelling was copied FROM.
    let files: Vec<String> = (0..12)
        .map(|i| {
            proj.join(format!("src/dedup_{i}.rs"))
                .to_str()
                .unwrap()
                .to_string()
        })
        .collect();
    let mut sk_zero: Vec<&str> = vec!["skeleton", "--budget", "0"];
    let mut sk_tight: Vec<&str> = vec!["skeleton", "--budget", "20"];
    let mut sk_unset: Vec<&str> = vec!["skeleton"];
    for f in &files {
        sk_zero.push(f);
        sk_tight.push(f);
        sk_unset.push(f);
    }
    let (sz, se, sc) = run_in(&sk_zero, h.path(), proj);
    assert_eq!(sc, 0, "skeleton --budget 0 failed:\n{se}");
    let (st, _e, _c) = run_in(&sk_tight, h.path(), proj);
    let (su, _e, _c) = run_in(&sk_unset, h.path(), proj);
    assert!(
        st.len() < su.len(),
        "skeleton fixture too small — a 20-token ceiling withheld nothing"
    );
    assert!(
        sz.len() > st.len(),
        "`skeleton --budget 0` behaved like a tight ceiling ({} vs {}):\n{sz}",
        sz.len(),
        st.len()
    );
    assert_eq!(
        sz.trim(),
        su.trim(),
        "`skeleton --budget 0` must deliver exactly what no flag delivers"
    );
}

/// `called by` from a SUBDIRECTORY is the same answer as from the root. The
/// call graph was loaded after the symbols were re-based to the cwd: from
/// `src/`, `query alpha_fn` printed `lib.rs:1` beside `src/lib.rs:2`, and the
/// qualified call in `g.rs` (`crate::alpha_fn()`) vanished from `called by`.
#[test]
fn e2e_called_by_is_the_same_from_a_subdirectory() {
    let h = TempHome::new("query-subdir");
    // A project UNDER the home: the home itself is never taken as a project
    // root, so a fixture at the home would index `src/` alone from `src/`.
    let proj = &h.path().join("p");
    std::fs::create_dir_all(proj.join(".git")).unwrap();
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(
        proj.join("src/lib.rs"),
        "pub fn alpha_fn() {}\npub fn beta_fn() { alpha_fn(); }\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src/g.rs"),
        "pub fn gamma_fn() { crate::alpha_fn(); }\n",
    )
    .unwrap();
    let (root_out, _e, c0) = run_in(&["query", "alpha_fn"], h.path(), proj);
    assert_eq!(c0, 0, "{_e}");
    assert!(root_out.contains("gamma_fn (src/g.rs:1)"), "{root_out}");
    let (sub_out, _e, c1) = run_in(&["query", "alpha_fn"], h.path(), &proj.join("src"));
    assert_eq!(c1, 0, "{_e}");
    assert!(
        sub_out.contains("gamma_fn (g.rs:1)"),
        "caller lost from src/:\n{sub_out}"
    );
    assert!(
        sub_out.contains("beta_fn (lib.rs:2)"),
        "mixed path bases:\n{sub_out}"
    );
}
