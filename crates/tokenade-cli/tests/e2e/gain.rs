//! End-to-end tests driving `tokenade gain`.

use super::*;

/// Upgrade safety: a user who had the PATH shim from an older version must get
/// it ACTIVELY removed on the first post-upgrade interactive command — dir
/// deleted, rc PATH line stripped, the user's own rc content untouched. The
/// shim is out of the product, so removal is now UNCONDITIONAL (see the
/// opted-in counterpart below). `run_copied` runs the binary from OUTSIDE
/// target/ so the post-upgrade refresh isn't build-dir gated; `gain` is a
/// non-quiet command so the refresh actually fires.
#[test]
fn e2e_upgrade_removes_stale_shim() {
    let h = TempHome::new("shim-migrate");
    let home = h.path();
    let shim = home.join(".tokenade").join("shim");
    std::fs::create_dir_all(&shim).unwrap();
    for c in ["git", "node"] {
        std::fs::write(
            shim.join(c),
            format!("#!/bin/sh\n# tokenade shim v1\nexec /old/tokenade shim-exec {c} -- \"$@\"\n"),
        )
        .unwrap();
    }
    let bashrc = home.join(".bashrc");
    std::fs::write(
        &bashrc,
        "# mine\nexport FOO=1\n# tokenade path shim\ncase \":$PATH:\" in *\":$HOME/.tokenade/shim:\"*) ;; *) PATH=\"$HOME/.tokenade/shim:$PATH\"; export PATH ;; esac\n",
    )
    .unwrap();
    let (_o, _e, code) = run_copied_active(&["gain"], home);
    assert_eq!(code, 0, "gain should exit 0");
    assert!(!shim.exists(), "stale shim dir not removed on upgrade");
    let rc = std::fs::read_to_string(&bashrc).unwrap();
    assert!(
        !rc.contains(".tokenade/shim"),
        "shim PATH line not stripped:\n{rc}"
    );
    assert!(rc.contains("export FOO=1"), "user rc content lost:\n{rc}");
}

/// The shim is REMOVED from the product, so a leftover `shim.optin` marker
/// from the opt-in era must NOT keep it alive: the upgrade rips out the shim
/// dir and the rc line anyway. Guards the migration against regressing to the
/// old "honor the opt-in" behaviour, which would strand these users on a shim
/// nothing maintains.
#[test]
fn e2e_upgrade_removes_shim_even_when_opted_in() {
    let h = TempHome::new("shim-keep");
    let home = h.path();
    let shim = home.join(".tokenade").join("shim");
    std::fs::create_dir_all(&shim).unwrap();
    std::fs::write(
        shim.join("git"),
        "#!/bin/sh\n# tokenade shim v1\nexec /old/tokenade shim-exec git -- \"$@\"\n",
    )
    .unwrap();
    let bashrc = home.join(".bashrc");
    std::fs::write(
        &bashrc,
        "# tokenade path shim\ncase \":$PATH:\" in *\":$HOME/.tokenade/shim:\"*) ;; *) PATH=\"$HOME/.tokenade/shim:$PATH\"; export PATH ;; esac\n",
    )
    .unwrap();
    // Stale opt-in from the era when the shim was a supported feature.
    std::fs::write(home.join(".tokenade").join("shim.optin"), "1\n").unwrap();
    let (_o, _e, code) = run_copied_active(&["gain"], home);
    assert_eq!(code, 0, "gain should exit 0");
    assert!(!shim.exists(), "opt-in marker wrongly kept the shim alive");
    let rc = std::fs::read_to_string(&bashrc).unwrap();
    assert!(
        !rc.contains(".tokenade/shim"),
        "shim PATH line not stripped:\n{rc}"
    );
}

/// Field report 2026-07 — the regression that stranded a user. The removal
/// used to sit BEHIND the once-per-upgrade version marker, so a machine whose
/// marker was already consumed for the running version never got the shim
/// ripped out: `~/.tokenade/shim/node` kept hijacking `PATH` and broke a Claude
/// Code plugin hook with no code path left to repair it.
///
/// First `gain` consumes the marker; the shim is planted only afterwards, so
/// the second `gain` can only clean it up if the dismantler is UNGATED.
#[test]
fn e2e_stale_shim_removed_after_version_marker_already_consumed() {
    let h = TempHome::new("shim-marker-consumed");
    let home = h.path();
    // Burn the post-upgrade marker on a clean machine.
    let (_o, _e, code) = run_copied_active(&["gain"], home);
    assert_eq!(code, 0, "first gain should exit 0");

    // Only NOW does the machine acquire a stale shim (upgrade ordering race,
    // updater temp-binary consuming the marker, restored backup…).
    let shim = home.join(".tokenade").join("shim");
    std::fs::create_dir_all(&shim).unwrap();
    std::fs::write(
        shim.join("node"),
        "#!/bin/sh\n# tokenade shim v1\nexec /old/tokenade shim-exec node -- \"$@\"\n",
    )
    .unwrap();
    let bashrc = home.join(".bashrc");
    std::fs::write(
        &bashrc,
        "# mine\nexport FOO=1\n# tokenade path shim\ncase \":$PATH:\" in *\":$HOME/.tokenade/shim:\"*) ;; *) PATH=\"$HOME/.tokenade/shim:$PATH\"; export PATH ;; esac\n",
    )
    .unwrap();

    let (_o, _e, code) = run_copied_active(&["gain"], home);
    assert_eq!(code, 0, "second gain should exit 0");
    assert!(
        !shim.exists(),
        "stale shim survived because the dismantler is still version-gated"
    );
    let rc = std::fs::read_to_string(&bashrc).unwrap();
    assert!(
        !rc.contains(".tokenade/shim"),
        "shim PATH line not stripped on a consumed marker:\n{rc}"
    );
    assert!(rc.contains("export FOO=1"), "user rc content lost:\n{rc}");
}

/// A rollup row stands for N original calls, and the `N×` breakdown has to say
/// N. It said 1 — the same argument the running total already makes
/// (`count += row.calls`) had not been applied eleven lines below it, so one
/// screen carried `operations: 72064` above a column that summed to 1041.
#[test]
fn e2e_the_by_op_breakdown_counts_calls_not_rollup_rows() {
    let h = TempHome::new("gain-rollup-calls");
    let tk = h.path().join(".tokenade");
    std::fs::create_dir_all(&tk).unwrap();
    // One ordinary row and one rollup standing for 500 calls, same op.
    std::fs::write(
        tk.join("gain.jsonl"),
        "{\"op\":\"widget\",\"label\":\"w\",\"before\":1000,\"after\":100,\"saved\":900,\"source\":\"hook\",\"agent\":\"t\",\"project\":\"p\",\"ts\":1788000000}\n{\"op\":\"widget\",\"label\":\"w\",\"before\":9000,\"after\":900,\"saved\":8100,\"source\":\"hook\",\"agent\":\"t\",\"project\":\"p\",\"ts\":1788000001,\"rollup\":true,\"rollup_calls\":500}\n",
    )
    .unwrap();
    let (stdout, _err, code) = run(&["gain", "--json"], h.path());
    assert_eq!(code, 0, "exit 0 expected — stdout: {stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let ops = v["operations"].as_u64().expect("operations");
    let by: u64 = v["by_op"]
        .as_array()
        .expect("by_op")
        .iter()
        .map(|o| o["count"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(
        ops, 501,
        "the total already counted the rollup's calls: {stdout}"
    );
    assert_eq!(
        by, 501,
        "the breakdown must agree with the total it sits under, not count rows: {stdout}"
    );
    // `by_source` is the same accumulator with the same bug and needs its own
    // assertion: fixing one and not the other leaves the screen contradicting
    // itself in a different column.
    let by_src: u64 = v["by_source"]
        .as_array()
        .expect("by_source")
        .iter()
        .map(|o| o["count"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(
        by_src, 501,
        "the per-source breakdown counts rows, not calls: {stdout}"
    );
    // The redirection has to have taken, or this measured the real ledger.
    assert!(
        tk.join("gain.jsonl").exists(),
        "HOME was not redirected: the probe read the real store"
    );
}

#[test]
fn e2e_dashboard_renders_with_no_data() {
    let h = TempHome::new("dash");
    let (stdout, _err, code) = run(&["dashboard"], h.path());
    assert_eq!(code, 0);
    // Logo or subtitle must be visible.
    assert!(stdout.contains("tokenade"));
}

// #510 lock-in: `dashboard --html <path>` honors the explicit path arg
// (was previously dropped; output landed at the hardcoded default).
// Also covers the new mkdir-of-parent behaviour for nested paths.
#[test]
fn e2e_dashboard_html_writes_to_explicit_path_with_mkdir() {
    let h = TempHome::new("dash-html");
    let nested = h.path().join("reports").join("nested").join("out.html");
    let nested_str = nested.to_str().unwrap().to_string();
    let (stdout, _err, code) = run(&["dashboard", "--html", &nested_str], h.path());
    assert_eq!(code, 0, "exit 0 expected — stdout: {stdout}");
    // The success message echoes the resolved path.
    assert!(
        stdout.contains(&nested_str),
        "stdout should include resolved path; got:\n{stdout}"
    );
    // The file was actually written (mkdir -p the parent worked).
    assert!(
        nested.is_file(),
        "expected HTML at {} after dashboard --html",
        nested.display()
    );
    let body = std::fs::read_to_string(&nested).unwrap();
    // Spot-check: the rendered HTML must carry the footer that tells the
    // reader how to refresh it. The command it names is `tokenade dashboard`
    // now that the bare command IS the page — it used to name `--html`, back
    // when the bare command printed a terminal report instead.
    assert!(
        body.contains("tokenade dashboard"),
        "rendered HTML missing footer marker"
    );

    // (Default-path branch is exercised by other e2e tests + the smoke test in
    // iteration #510. It used to say `run()` sets no cwd, which stopped being
    // true when `e2e_env` was given a `current_dir`: the default path resolves
    // relative to cwd, which is now the temp HOME rather than the crate dir.)
}

// Headless gate (2026-06-10 ask): a GUI-less session (SSH, or a Unix host
// with no X11/Wayland display) must not generate the web dashboard at all —
// the terminal report is the whole dashboard there. `--html` stays the
// explicit export for those machines.
#[test]
fn e2e_dashboard_default_skips_html_when_headless() {
    let h = TempHome::new("dash-headless");
    let bin = binary();
    // SSH_CONNECTION forces is_headless() on every platform, so the test
    // behaves the same on a GUI dev box and in CI.
    let ssh = ("SSH_CONNECTION", "10.0.0.1 51000 10.0.0.2 22");
    let out = Command::new(&bin)
        .args(["dashboard"])
        .env_remove("TOKENADE_DEBUG_LOG")
        .env("HOME", h.path())
        .current_dir(h.path())
        .env("USERPROFILE", h.path())
        .env("TOKENADE_HOOK_DISABLED", "1")
        .env(ssh.0, ssh.1)
        .output()
        .expect("spawn tokenade binary");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}");
    assert!(
        stdout.contains("headless session"),
        "expected the headless skip notice; got:\n{stdout}"
    );
    assert!(
        !h.path().join(".tokenade").join("dashboard.html").exists(),
        "headless default run must not write dashboard.html"
    );

    // Explicit export still works headless — the user asked for the file.
    let target = h.path().join("export.html");
    let target_str = target.to_str().unwrap().to_string();
    let out = Command::new(&bin)
        .args(["dashboard", "--html", &target_str])
        .env_remove("TOKENADE_DEBUG_LOG")
        .env("HOME", h.path())
        .current_dir(h.path())
        .env("USERPROFILE", h.path())
        .env("TOKENADE_HOOK_DISABLED", "1")
        .env(ssh.0, ssh.1)
        .output()
        .expect("spawn tokenade binary");
    assert_eq!(out.status.code(), Some(0));
    assert!(
        target.is_file(),
        "--html must write the file even in a headless session"
    );
}

/// The harness itself must have no browser to open, asserted through DELIVERED
/// output rather than by reading the environment back.
///
/// The test above sets `SSH_CONNECTION` for itself; every other test inherited
/// the developer's session, display and all. That mattered because `login`
/// hands its URL to `xdg-open`, which launches a browser that DETACHES: the
/// `kill()` in `run_gated` reaps the tokenade child and cannot touch a
/// grandchild spawned to outlive it. Measured 2026-08-08 — 92 orphan
/// `chrome_crashpad_handler` processes, the oldest alive 51 hours, 48 temp
/// homes, one batch per suite run, each having popped a real tab at a real
/// device-authorization URL.
///
/// The fix is one line in `e2e_env`. This is what makes deleting that line go
/// red, instead of quietly opening browser tabs again.
#[test]
fn e2e_the_harness_never_has_a_browser_to_open() {
    let h = TempHome::new("no-browser");
    let (stdout, _err, code) = run(&["dashboard"], h.path());
    assert_eq!(code, 0, "dashboard failed:\n{stdout}");
    assert!(
        stdout.contains("headless session"),
        "the harness is not headless: any test reaching `login` will spawn a \
         browser that outlives the run.\n{stdout}"
    );
}

#[test]
fn e2e_healthcheck_runs_clean_on_fresh_home() {
    let h = TempHome::new("health");
    let (stdout, _err, code) = run(&["healthcheck"], h.path());
    // A fresh HOME has no Claude settings → hooks check fails (exit 1)
    // but the binary should still produce a coherent report.
    assert!(stdout.contains("tokenade healthcheck"));
    assert!(stdout.contains("binary at"));
    // Exit code may be 0 or 1 depending on whether ~/.claude exists in
    // the parent env; both are acceptable for a fresh-home invocation.
    assert!(code == 0 || code == 1);
}

/// Lock the #452 contract: `tokenade scrub-secret <substring>`
/// replaces every occurrence in ~/.tokenade/debug.log + gain.jsonl,
/// reports per-file counts, mentions the rotation reminder, and
/// preserves file format (jsonl still parseable).
///
/// Dry-run companion: same scan, no writes.
#[test]
fn e2e_scrub_secret_replaces_substring_in_debug_log_and_gain_jsonl() {
    let h = TempHome::new("scrub-secret");
    let tk = h.path().join(".tokenade");
    std::fs::create_dir_all(&tk).unwrap();
    let debug = tk.join("debug.log");
    let gain = tk.join("gain.jsonl");
    // Seed with a needle that's safe (no real cred shape) and
    // a JSONL entry so we can verify the file is still parseable.
    std::fs::write(
        &debug,
        r#"{"event":"ok","details":{"command":"echo TESTNEEDLE"}}
{"event":"ok","details":{"command":"cat TESTNEEDLE/file"}}
"#,
    )
    .unwrap();
    std::fs::write(
        &gain,
        r#"{"ts":1,"label":"TESTNEEDLE","op":"raw","saved":0}
"#,
    )
    .unwrap();

    // (1) Dry-run: must NOT modify files but must report counts.
    let (out, _err, code) = run(&["scrub-secret", "TESTNEEDLE", "--dry-run"], h.path());
    assert_eq!(code, 0);
    assert!(
        out.contains("◐") && out.contains("would replace"),
        "dry-run output should mention `would replace`: {out}"
    );
    let debug_after_dry = std::fs::read_to_string(&debug).unwrap();
    assert!(
        debug_after_dry.contains("TESTNEEDLE"),
        "dry-run must NOT mutate the file"
    );

    // (2) Real run: writes + reports + rotation reminder.
    // `--yes` since 2026-09-03: a real scrub asks before writing, and there is
    // no terminal here to ask. The refusal path has its own test.
    let (out, _err, code) = run(&["scrub-secret", "TESTNEEDLE", "--yes"], h.path());
    assert_eq!(code, 0);
    assert!(
        out.contains("✓") && out.contains("replaced"),
        "real-run output should mention `replaced`: {out}"
    );
    assert!(
        out.contains("rotate"),
        "must remind user to rotate the underlying secret: {out}"
    );

    // Files must be scrubbed AND still well-formed.
    let debug_after = std::fs::read_to_string(&debug).unwrap();
    let gain_after = std::fs::read_to_string(&gain).unwrap();
    assert!(!debug_after.contains("TESTNEEDLE"));
    assert!(!gain_after.contains("TESTNEEDLE"));
    assert!(debug_after.contains("<redacted>"));
    assert!(gain_after.contains("<redacted>"));
    // jsonl still parseable: every non-empty line is valid JSON.
    for line in debug_after.lines().filter(|l| !l.trim().is_empty()) {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|e| panic!("scrubbed debug.log line not valid JSON: {line}\nerr: {e}"));
    }
    for line in gain_after.lines().filter(|l| !l.trim().is_empty()) {
        serde_json::from_str::<serde_json::Value>(line).unwrap_or_else(|e| {
            panic!("scrubbed gain.jsonl line not valid JSON: {line}\nerr: {e}")
        });
    }
}

#[test]
fn pack_separates_a_typo_from_an_empty_scope() {
    let home = TempHome::new("pack-scope-rc");
    let missing = home.path().join("no-such-thing.rs");
    let (_out, _err, code) = run(&["pack", &missing.to_string_lossy()], home.path());
    assert_ne!(
        code, 0,
        "a path that does not exist must not report success"
    );

    let empty = home.path().join("empty-dir");
    std::fs::create_dir_all(&empty).expect("mkdir");
    let (out, _err, code) = run(&["pack", "--json", &empty.to_string_lossy()], home.path());
    assert_eq!(code, 0, "an empty directory is a result, not an error");
    let v: serde_json::Value =
        serde_json::from_str(out.trim()).expect("--json must answer in JSON, even when empty");
    assert!(
        v.get("included")
            .and_then(|i| i.as_array())
            .is_some_and(|a| a.is_empty()),
        "expected an empty include list, got {out}"
    );

    // And a single FILE is its own scope — the case that started this.
    let f = home.path().join("one.rs");
    std::fs::write(&f, "pub fn one() {}\n").expect("write");
    let (out, _err, code) = run(&["pack", &f.to_string_lossy()], home.path());
    assert_eq!(code, 0);
    assert!(out.contains("one.rs"), "the named file was packed:\n{out}");
}

/// The lifetime figure on the statusline must count records the ledger has
/// already rotated into its rollup. It used to read the live file alone, so it
/// showed a fraction of what `tokenade dashboard` showed for the same install
/// — and the gap widened at every rotation.
#[test]
fn the_statusline_counts_history_already_rotated_into_the_rollup() {
    let h = TempHome::new("statusline-rollup");
    let dir = h.path().join(".tokenade");
    std::fs::create_dir_all(&dir).unwrap();
    let now = tokenade_core::platform::now_unix_secs();
    std::fs::write(
        dir.join("gain.jsonl"),
        format!("{{\"op\":\"git_status\",\"before\":1000,\"after\":100,\"ts\":{now}}}\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("gain.rollup.jsonl"),
        format!(
            "{{\"op\":\"git_status\",\"before\":5000,\"after\":500,\"rollup_calls\":7,\"ts\":{now}}}\n"
        ),
    )
    .unwrap();

    let (out, _e, _c) = run(&["statusline", "--json"], h.path());
    let line = out.lines().find(|l| l.starts_with('{')).unwrap_or("");
    let v: serde_json::Value = serde_json::from_str(line)
        .unwrap_or_else(|_| panic!("statusline --json printed no record: {out:?}"));
    assert_eq!(
        v["tokens_before"].as_u64(),
        Some(6000),
        "rotated history was left out of the statusline total: {v}"
    );
    assert_eq!(v["saved_tokens"].as_u64(), Some(5400), "{v}");
}

/// The measured savings figure has FOUR public surfaces. They used to compute
/// it in four separate loops, and every past realignment was done by hand —
/// which is how they drifted apart again (the statusline was reading a third
/// of the ledger; `--json` was leaving out a cost the terminal subtracted).
/// They now share one aggregator, and this test is what says so out loud: it
/// fails the moment any one of them starts answering a different question.
#[test]
fn every_surface_reports_the_same_measured_savings() {
    let h = TempHome::new("surface-agreement");
    let dir = h.path().join(".tokenade");
    std::fs::create_dir_all(&dir).unwrap();
    let now = tokenade_core::platform::now_unix_secs();
    let later = now + 1;
    // Deliberately mixed: a plain row, a legacy row carrying `filter` instead
    // of `op`, a code-intel row (excluded from the MEASURED figure), a rotated
    // row living only in the rollup — and a DUPLICATE of the first.
    //
    // The duplicate is what makes this test able to tell the surfaces apart.
    // Without it the fixture has no repeated event, so the credit-once rule
    // `gain` applies is indistinguishable from not applying it, and the test
    // passed for years while the surfaces disagreed on real data: measured
    // 2026-09-02, `gain` reported 669 007 667 tokens before and the SIGNED
    // receipt 673 874 356 over the same lifetime ledger.
    let ledger = format!(
        "{{\"op\":\"git_status\",\"before\":4000,\"after\":400,\"project\":\"p\",\"ts\":{now}}}\n\
         {{\"op\":\"git_status\",\"before\":4000,\"after\":400,\"project\":\"p\",\"ts\":{now}}}\n\
         {{\"filter\":\"docker_ps\",\"before\":2000,\"after\":200,\"ts\":{now}}}\n\
         {{\"op\":\"ls_la\",\"before\":2000,\"after\":200,\"ts\":{now}}}\n\
         {{\"filter\":\"docker_ps\",\"before\":3000,\"after\":200,\"ts\":{now}}}\n\
         {{\"op\":\"ps_aux\",\"before\":2000,\"after\":200,\"ts\":{now}}}\n\
         {{\"op\":\"git_status\",\"before\":4000,\"after\":400,\"project\":\"p\",\"ts\":{later}}}\n\
         {{\"op\":\"ls_la\",\"before\":2000,\"after\":500,\"ts\":{now}}}\n\
         {{\"op\":\"semantic_search\",\"before\":900,\"after\":900,\"ts\":{now}}}\n"
    );
    std::fs::write(dir.join("gain.jsonl"), ledger).unwrap();
    std::fs::write(
        dir.join("gain.rollup.jsonl"),
        format!(
            "{{\"op\":\"git_status\",\"before\":10000,\"after\":1000,\"rollup_calls\":5,\"ts\":{now}}}\n"
        ),
    )
    .unwrap();
    // 4000-400 + 2000-200 + 2000-200 + 10000-1000 = 16200 measured; the
    // code-intel row is heuristic and belongs to `estimated_saved`, not here.
    // `ls_la` carries the SAME sizes as `docker_ps` in the same second and is
    // a different operation: the credit-once key has to keep them apart, or
    // deduplication starts eating real work.
    //
    // Every field of the credit-once key needs a pair that OPPOSES it, or a
    // mutation dropping that field survives: `ps_aux` opposes `ls_la` on the
    // op alone (both carry `op`, so the legacy `filter` fallback cannot stand
    // in for it), the later `git_status` opposes the first on the timestamp
    // alone, and the second `ls_la` opposes the first on the output size
    // alone. The second `docker_ps` is the
    // same op in the same second folding to the same size from a LARGER input:
    // the key has to read the input size too.
    const MEASURED: u64 = 25_900;

    let json_of = |args: &[&str]| -> serde_json::Value {
        let (out, err, code) = run(args, h.path());
        // Surfaces differ in shape: some print one compact line, some
        // pretty-print. Take everything from the first brace and read the
        // first complete value out of it.
        let start = out
            .find('{')
            .unwrap_or_else(|| panic!("{args:?} printed no JSON (code {code}): {out:?} {err:?}"));
        serde_json::Deserializer::from_str(&out[start..])
            .into_iter::<serde_json::Value>()
            .next()
            .and_then(|r| r.ok())
            .unwrap_or_else(|| panic!("{args:?}: unparseable JSON in {:?}", &out[start..]))
    };

    let dash = json_of(&["dashboard", "--json"]);
    let gain = json_of(&["gain", "--json"]);
    let status = json_of(&["statusline", "--json"]);

    let dash_saved =
        dash["tokens_before"].as_u64().unwrap_or(0) - dash["tokens_after"].as_u64().unwrap_or(0);
    assert_eq!(dash_saved, MEASURED, "dashboard --json: {dash}");
    assert_eq!(
        gain["saved"].as_u64(),
        Some(MEASURED),
        "gain --json: {gain}"
    );
    assert_eq!(
        status["saved_tokens"].as_u64(),
        Some(MEASURED),
        "statusline --json: {status}"
    );

    // The terminal dashboard prints the same figure in prose. Its hero is
    // rounded, so assert on the exact per-project line it also prints.
    let (text, _e, _c) = run(&["dashboard"], h.path());
    assert!(
        text.contains("25.9k") || text.contains("25,900") || text.contains("25900"),
        "terminal dashboard does not show the shared figure:\n{text}"
    );

    // FIFTH surface, added 2026-08-10 after a user report. `tokenade receipt`
    // is the SIGNED figure — the one a user quotes when they want to know what
    // tokenade actually did — and it was the only public savings surface this
    // guard did not cover. A Codex user reported 359 520 tokens on the receipt
    // and 0 on the dashboard; a guard that omits a surface cannot tell you
    // which of the two is lying.
    let receipt = json_of(&["receipt", "--json"]);
    let body = &receipt["body"];
    assert_eq!(
        body["tokens_saved"].as_u64(),
        Some(MEASURED),
        "receipt --json disagrees with the other four surfaces: {receipt}"
    );
    // The COUNT diverged too, and only the tokens were asserted: on the same
    // install `gain` said 72 064 operations where the receipt certified
    // 72 179 events. Three distinct measured events here — the duplicate is
    // one of them, counted once — plus the rollup standing for five.
    const EVENTS: u64 = 12;
    assert_eq!(
        body["events"].as_u64(),
        Some(EVENTS),
        "the signed event count double-counts a repeated append: {receipt}"
    );
    assert_eq!(
        gain["operations"].as_u64(),
        Some(EVENTS),
        "gain disagrees with the receipt on how many events there were: {gain}"
    );
    assert_eq!(
        dash["operations"].as_u64(),
        Some(EVENTS),
        "dashboard disagrees on the event count: {dash}"
    );
    assert_eq!(
        body["tokens_before"].as_u64().unwrap_or(0) - body["tokens_after"].as_u64().unwrap_or(0),
        MEASURED,
        "the receipt's own before/after do not add up to its saved figure: {receipt}"
    );
}

/// A published token count must say what period it covers.
///
/// The `--recent` header has named its window since it existed, with the
/// reason written at the call site: "a reader who sees 63 % where they saw
/// 88 % yesterday must be able to tell a regression from a different question,
/// and the number alone cannot say which it is". The DEFAULT branch — the one
/// with the widest window, every row ever recorded — named nothing. Measured
/// on the author's own install: 33,250 rows spanning three months, several
/// versions and a rotation, published as a bare `saved: 93.2%` that reads as
/// today's rate. Three sibling projects fixed the same class the same week.
///
/// Both surfaces are asserted, because a machine reader cannot see a header.
#[test]
fn e2e_gain_says_what_period_its_totals_cover() {
    let h = TempHome::new("gain-span");
    let tk = h.path().join(".tokenade");
    std::fs::create_dir_all(&tk).unwrap();
    // Two rows, ten days apart, so the span is a fact and not today's date.
    let first: u64 = 1_780_000_000;
    let last: u64 = first + 10 * 86_400;
    let rows = format!(
        "{{\"op\":\"probe\",\"before\":1000,\"after\":100,\"ts\":{first},\"covered\":true}}\n\
         {{\"op\":\"probe\",\"before\":1000,\"after\":100,\"ts\":{last},\"covered\":true}}\n"
    );
    std::fs::write(tk.join("gain.jsonl"), rows).unwrap();

    let (out, err, code) = run(&["gain"], h.path());
    assert_eq!(code, 0, "gain should exit 0: {out}{err}");
    assert!(
        out.contains("11 days") && out.contains("2026-05-28"),
        "the header must name the span the totals cover:\n{out}"
    );

    let (jout, _e, jcode) = run(&["gain", "--json"], h.path());
    assert_eq!(jcode, 0, "gain --json should exit 0");
    let start = jout.find('{').expect("gain --json printed no JSON");
    let v: serde_json::Value = serde_json::from_str(&jout[start..]).expect("gain --json parses");
    assert_eq!(
        v["span"]["days"].as_u64(),
        Some(11),
        "gain --json must carry the span a consumer cannot read off a header: {v}"
    );
    assert_eq!(v["span"]["first_ts"].as_u64(), Some(first), "{v}");
    assert_eq!(v["span"]["last_ts"].as_u64(), Some(last), "{v}");
}

/// Every outcome of `scrub-secret` names what it did NOT scan.
///
/// The stores it walks are tokenade's own. The largest record of tool output
/// on a machine is the AGENT's transcript, which tokenade writes nothing to
/// and never scrubs — so `no occurrences found`, on a command whose whole
/// subject is a leaked credential, read as "your machine is clean". Same rule
/// as the recovery command's grep: an empty answer must name what it
/// searched, and here it must also name the verb that reaches the rest.
#[test]
fn e2e_scrub_secret_says_what_it_did_not_scan() {
    let h = TempHome::new("scrub-scope");
    let dir = h.path().join(".tokenade");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("debug.log"), "a line with NEEDLE-XYZ in it\n").unwrap();

    // A miss, a dry run and a real scrub: three different sentences, one
    // scope note, because the reassuring reading is available on all three.
    for args in [
        // A run that finds nothing writes nothing, so it never reaches the
        // confirmation and needs no `--yes`; the one that WOULD write does.
        vec!["scrub-secret", "ABSENT-NEEDLE"],
        vec!["scrub-secret", "NEEDLE-XYZ", "--dry-run"],
        vec!["scrub-secret", "NEEDLE-XYZ", "--yes"],
    ] {
        let label = args.join(" ");
        let (out, err, code) = run(&args, h.path());
        assert_eq!(code, 0, "{label}: {err}");
        assert!(
            out.contains("Not scanned:") && out.contains("transcript"),
            "`{label}` must name what it did not scan:\n{out}"
        );
        assert!(
            out.contains("tokenade redact") && out.contains("--in-place"),
            "`{label}` must name the verb that reaches the rest, WITH the flag \
             that edits rather than copying:\n{out}"
        );
    }
    // And the miss must not read as a clean bill of health for the machine.
    let (out, _e, _c) = run(&["scrub-secret", "ABSENT-NEEDLE"], h.path());
    assert!(
        out.contains("no occurrences found in tokenade's own stores"),
        "the miss must bound itself:\n{out}"
    );
}

/// A recovery round is not free, and it is a round WE caused.
///
/// `expand-ref` credits the re-run it spared. It used to book that as
/// `Savings::new(blob_tokens, 0)`: a saving with no delivery on the other
/// side, on every branch, whether or not the branch retrieved anything. Three
/// rules now, driven end to end because the credit is written by the real
/// binary, from a real stash, through the real ledger:
///
///   - a branch that RETRIEVES nothing credits nothing. The size-gated
///     refusal prints a pointer and hands back no bytes, and the tokens it
///     would claim are the ones the fold already claimed under
///     `disclose:stash` — the same bytes twice. Measured on a live ledger
///     2026-09-02: the largest recovery row on the machine, 393 740 tokens,
///     was this branch printing a sixty-token banner.
///   - a `--grep` that matched nothing is the same case.
///   - a branch that DOES retrieve nets what it put in the context window,
///     because that round only exists because we folded.
#[test]
fn e2e_a_recovery_credit_nets_what_the_round_delivered() {
    let h = TempHome::new("recovery-credit");
    let home = h.path();
    let tk = home.join(".tokenade");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut body = String::new();
    for i in 0..6_000 {
        body.push_str(&format!(
            "2024-01-01T00:00:{:02}Z INFO worker {i} processed batch {i}\n",
            i % 60
        ));
    }

    // Each blob is stashed, then expanded once — the credit fires once per
    // hash, so a branch needs its own.
    let stash = |name: &str, text: &str| -> String {
        let f = home.join(name);
        std::fs::write(&f, text).unwrap();
        let cat = format!("cat {}", f.to_str().unwrap());
        let (folded, _e, _c) = run(&["wrap", cat.as_str()], home);
        folded
            .split("expand-ref ")
            .nth(1)
            .and_then(|rest| rest.split(['`', ' ', '\n']).next())
            .map(str::to_string)
            .unwrap_or_else(|| panic!("no expand-ref handle in the banner:\n{folded}"))
    };
    let refused = stash("refused.log", &body);
    let missed = stash("missed.log", &body.replace("worker", "runner"));
    let served = stash("served.log", &body.replace("worker", "actor"));
    let asked = stash("asked.log", &body.replace("worker", "agent"));

    // The credit only fires within 30 min of a compaction digest — that gate
    // is what makes it a POST-COMPACTION recovery rather than any expansion.
    std::fs::create_dir_all(&tk).unwrap();
    std::fs::write(tk.join("compact-digest-ts"), now.to_string()).unwrap();

    let row_for = |hash: &str| -> Option<serde_json::Value> {
        std::fs::read_to_string(tk.join("gain_estimated.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(|v| v["op"] == "digest_recovery" && v["label"] == serde_json::json!(hash))
    };

    let (out, _e, _c) = run(&["expand-ref", refused.as_str()], home);
    assert!(
        out.contains("too large to dump whole"),
        "expected the size-gated refusal, got:\n{out}"
    );
    assert!(
        row_for(&refused).is_none(),
        "the refusal retrieved nothing and credited a recovery anyway: {:?}",
        row_for(&refused)
    );

    let (out, err, _c) = run(
        &["expand-ref", missed.as_str(), "--grep", "no-such-token"],
        home,
    );
    assert!(
        row_for(&missed).is_none(),
        "a grep that matched nothing credited a recovery: {:?}\n{out}",
        row_for(&missed)
    );
    // A miss now shows what IS in there, so the pattern can be fixed in place.
    //
    // MEASURED 2026-09-05 across this machine's transcripts: of 38 `--grep`
    // answers, 22 matched nothing, and in every case sampled the agent's very
    // next call was another `expand-ref` on the SAME hash (`| tail -26`, or
    // `--grep "."`) purely to find out what it held. Six of the 22 held fewer
    // than 60 lines — less than the round trip they forced.
    // On STDERR, deliberately: the sample is orientation, not the grep's
    // result, so `expand-ref --grep X | wc -l` still answers 0.
    assert!(
        err.contains("so the pattern can be refined without another round trip"),
        "a zero-match grep gave the caller nothing to go on:\nstdout: {out}\nstderr: {err}"
    );
    assert!(
        err.contains("--prompt"),
        "the miss never names the cheaper way to ask:\n{err}"
    );
    assert!(
        out.trim().is_empty(),
        "the sample leaked into the grep's own result:\n{out}"
    );

    let (out, _e, _c) = run(&["expand-ref", served.as_str(), "--grep", "batch 42"], home);
    assert!(out.contains("batch 42"), "the grep served nothing:\n{out}");
    let row = row_for(&served)
        .unwrap_or_else(|| panic!("a grep that retrieved lines booked no recovery credit:\n{out}"));
    let before = row["before"].as_u64().unwrap_or(0);
    let after = row["after"].as_u64().unwrap_or(0);
    assert!(before > 0, "the credit booked no assiette at all: {row}");
    assert!(
        after > 0,
        "the round handed back lines and charged itself nothing — a saving \
         with no delivery on the other side: {row}"
    );
    assert!(
        after < before,
        "a recovery that delivers as much as it spared is not a saving: {row}"
    );

    // A QUESTION is the third delivering branch, and it books the same way.
    let (out, _e, _c) = run(
        &[
            "expand-ref",
            asked.as_str(),
            "--prompt",
            "which agent processed batch 42",
        ],
        home,
    );
    let asked_row =
        row_for(&asked).unwrap_or_else(|| panic!("`--prompt` answered and booked nothing:\n{out}"));
    let asked_after = asked_row["after"].as_u64().unwrap_or(0);
    assert!(
        asked_after > 0,
        "the answer went into the context window and was charged nothing: {asked_row}"
    );
    assert!(
        asked_after < asked_row["before"].as_u64().unwrap_or(0),
        "the answer is not smaller than the blob it came from: {asked_row}"
    );

    // And it reads bounded: one truncated re-run minus what it delivered,
    // with no overflow credit on top — this row IS the dance that credit
    // pays for.
    let cap = tokenade_core::coefficients::delivery_cap_tokens(
        row["agent"].as_str().unwrap_or(""),
        row["source"].as_str().unwrap_or(""),
        "digest_recovery",
    );
    assert!(cap < u64::MAX, "the recovery channel carries no cap");
    assert_eq!(
        tokenade_core::gain::estimated_saved_of_row(&row),
        cap - after,
        "recovery row must read as one truncated re-run minus its delivery: {row}"
    );
}

#[test]
fn e2e_every_surface_that_prints_a_total_names_its_approximated_slice() {
    // `saved:` is a measured total, but part of it credits a delivery CAP
    // rather than exact arithmetic — on a channel where the agent would have
    // refused an oversized payload instead of truncating it. That slice stays
    // in the total by decision, and the decision is only honest if it is said.
    //
    // Three surfaces publish the total and each scans the ledger for itself.
    // Only the signed receipt disclosed the slice; `gain` and `dashboard`
    // printed the total in silence — `gain` while disclosing two SMALLER
    // estimate categories in the very next lines. This asserts they agree,
    // which is what caught it.
    let h = TempHome::new("gain-approximated");
    let tk = h.path().join(".tokenade");
    std::fs::create_dir_all(&tk).unwrap();
    // Row 1: cap-credited channel, `before` well over the 20 000 cap ⇒ the
    // whole 90 000 saving is approximated.
    // Row 2: same channel, `before` UNDER the cap ⇒ delivered whole, exact,
    // and must not be labelled. Without this row a rule that labels every
    // approximate-channel row scores identically.
    // Row 3: an ordinary measured row on another op, never approximated.
    std::fs::write(
        tk.join("gain.jsonl"),
        concat!(
            r#"{"op":"read_disclose","label":"r","before":100000,"after":10000,"saved":90000,"source":"hook","agent":"claude-code","project":"p","ts":1788000000}"#, "\n",
            r#"{"op":"read_disclose","label":"r","before":5000,"after":500,"saved":4500,"source":"hook","agent":"claude-code","project":"p","ts":1788000001}"#, "\n",
            r#"{"op":"widget","label":"w","before":8000,"after":800,"saved":7200,"source":"hook","agent":"claude-code","project":"p","ts":1788000002}"#, "\n",
        ),
    )
    .unwrap();
    assert!(
        tk.join("gain.jsonl").exists(),
        "HOME was not redirected: the probe read the real store"
    );

    let (g_out, _e, g_rc) = run(&["gain", "--json"], h.path());
    assert_eq!(g_rc, 0, "gain --json failed: {g_out}");
    let g: serde_json::Value = serde_json::from_str(&g_out).expect("gain json");
    let (d_out, _e, d_rc) = run(&["dashboard", "--json"], h.path());
    assert_eq!(d_rc, 0, "dashboard --json failed: {d_out}");
    let d: serde_json::Value = serde_json::from_str(&d_out).expect("dashboard json");
    let (r_out, _e, r_rc) = run(&["receipt", "--json"], h.path());
    assert_eq!(r_rc, 0, "receipt --json failed: {r_out}");
    let r: serde_json::Value = serde_json::from_str(&r_out).expect("receipt json");

    // The number itself: only the over-cap row counts.
    let want = 90_000u64;
    assert_eq!(
        g["approximated_saved"].as_u64(),
        Some(want),
        "gain --json: {g_out}"
    );
    assert_eq!(
        d["approximated_saved"].as_u64(),
        Some(want),
        "dashboard --json: {d_out}"
    );
    assert_eq!(
        r["body"]["tokens_saved_approximated"].as_u64(),
        Some(want),
        "receipt: {r_out}"
    );

    // It is a SLICE of the total, not an addition to it — the three surfaces
    // must also still agree on the total they are slicing.
    let total = 90_000u64 + 4_500 + 7_200;
    assert_eq!(g["saved"].as_u64(), Some(total), "gain total: {g_out}");
    assert_eq!(
        d["tokens_saved"].as_u64(),
        Some(total),
        "dashboard total: {d_out}"
    );
    assert_eq!(
        r["body"]["tokens_saved"].as_u64(),
        Some(total),
        "receipt total: {r_out}"
    );

    // And the human-readable surface says it too, not only the machine one.
    let (text, _e, rc) = run(&["gain"], h.path());
    assert_eq!(rc, 0, "gain failed: {text}");
    // Both halves: the number, and the sentence that says what it means. The
    // by-op table also prints 90000, so the digits alone would pass on a
    // surface that says nothing.
    let line = text
        .lines()
        .find(|l| l.starts_with("of which:"))
        .unwrap_or_else(|| panic!("the text surface never names the slice: {text}"));
    assert!(
        line.contains("90000") && line.contains("delivery cap"),
        "the disclosure line names neither the amount nor the reason: {line}"
    );
    // And it does not move the total it slices.
    assert!(
        text.contains(&format!("saved:   {total}")),
        "the total changed when the slice was disclosed: {text}"
    );
}

#[test]
fn e2e_a_machine_loop_cannot_inflate_the_ledger() {
    // The credit key is coarse ON PURPOSE and this pins why, because the
    // reasoning that argues for making it finer is sound and recurs.
    //
    // PO #37 (2026-08-04): a `cat` loop at ~37 identical events per second put
    // 89,245,193 tokens into one day, 28% of that install's lifetime figure,
    // for about twenty seconds of work. Thirty-seven Bash tool calls per
    // second is not something an agent does, and 89 million tokens is four
    // hundred times a 200k context — so those events cannot each be output the
    // agent received. The shell proxy makes every `cat` in a loop its own
    // booking while the agent made ONE tool call.
    //
    // A per-booking id was added on 2026-09-03 and removed the same day: with
    // it, 30 identical `cat` runs booked 1,227,660 tokens against 40,922.
    let h = TempHome::new("gain-loop-inflation");
    let tk = h.path().join(".tokenade");
    std::fs::create_dir_all(&tk).unwrap();
    let mut led = String::new();
    // 37 identical bookings in ONE second: one command, looped.
    for _ in 0..37 {
        led.push_str(
            r#"{"op":"cat","label":"cat big.txt","before":40962,"after":40,"saved":40922,"source":"wrap","agent":"claude-code","sess":"s1","project":"p","ts":1788500000}"#,
        );
        led.push('\n');
    }
    // …and, in the same second, two DIFFERENT commands. These are two real
    // savings: the agent received both outputs. The key must separate them
    // from the loop, which is what the label buys.
    for label in ["grep -rn fn src", "grep -rn pub src"] {
        led.push_str(&format!(
            r#"{{"op":"grep","label":"{label}","before":9000,"after":300,"saved":8700,"source":"wrap","agent":"claude-code","sess":"s1","project":"p","ts":1788500000}}"#
        ));
        led.push('\n');
    }
    std::fs::write(tk.join("gain.jsonl"), &led).unwrap();
    assert!(
        tk.join("gain.jsonl").exists(),
        "HOME was not redirected: the probe read the real store"
    );

    let (out, _e, rc) = run(&["gain", "--json"], h.path());
    assert_eq!(rc, 0, "gain --json failed: {out}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("json");
    // The loop is worth ONE booking; the two distinct commands are worth two.
    assert_eq!(
        v["operations"].as_u64(),
        Some(3),
        "a 37-iteration loop was credited more than once, or the two distinct \
         commands were collapsed into it: {out}"
    );
    assert_eq!(
        v["saved"].as_u64(),
        Some(40922 + 2 * 8700),
        "the loop's counterfactual was booked more than once: {out}"
    );
}

#[test]
fn e2e_distinct_commands_in_one_second_are_each_credited() {
    // Each ledger event is credited once. The key used to be
    // `(ts, op, before, after)`, and both sites carried a comment arguing that
    // two distinct events sharing all four are "indistinguishable from a
    // loop" — while the row carries the `label` that distinguishes them.
    //
    // They collide because they are command VARIANTS: the same data folded by
    // the same compactor gives the same two numbers. Measured on a real
    // ledger: 22 such keys, 47 rows and 1,279,063 tokens never credited. Run
    // 24 distinct commands in parallel and every surface reported ONE.
    let h = TempHome::new("gain-credit-key");
    let tk = h.path().join(".tokenade");
    std::fs::create_dir_all(&tk).unwrap();
    let mut led = String::new();
    // Three variants of one command, same second, identical token counts.
    for label in [
        "kubectl get pods",
        "kubectl get po -A",
        "kubectl get pods --all-namespaces",
    ] {
        led.push_str(&format!(
            r#"{{"op":"kubectl","label":"{label}","before":9198,"after":6674,"saved":2524,"source":"wrap","agent":"claude-code","sess":"s1","project":"p","ts":1788360919}}"#
        ));
        led.push('\n');
    }
    // A DOUBLED APPEND of the first one — every field identical. This is what
    // the credit-once key exists to collapse, and it must still collapse.
    led.push_str(&format!(
        r#"{{"op":"kubectl","label":"kubectl get pods","before":9198,"after":6674,"saved":2524,"source":"wrap","agent":"claude-code","sess":"s1","project":"p","ts":1788360919}}"#
    ));
    led.push('\n');
    // The same command from ANOTHER session in the same second: two agents,
    // two real events. A doubled append cannot differ here — one process
    // writes it and the copy carries its `sess`.
    led.push_str(&format!(
        r#"{{"op":"kubectl","label":"kubectl get pods","before":9198,"after":6674,"saved":2524,"source":"wrap","agent":"claude-code","sess":"s2","project":"p","ts":1788360919}}"#
    ));
    led.push('\n');
    // Every OTHER field of the key needs a row that differs in it alone, or a
    // key that drops that field scores identically and the mutation survives.
    // The same command one second later:
    led.push_str(
        r#"{"op":"kubectl","label":"kubectl get pods","before":9198,"after":6674,"saved":2524,"source":"wrap","agent":"claude-code","sess":"s1","project":"p","ts":1788360920}"#,
    );
    led.push('\n');
    // …and one whose input was one token bigger.
    led.push_str(
        r#"{"op":"kubectl","label":"kubectl get pods","before":9199,"after":6674,"saved":2525,"source":"wrap","agent":"claude-code","sess":"s1","project":"p","ts":1788360919}"#,
    );
    led.push('\n');
    // The same event in the LEGACY spelling: `filter` where a modern row says
    // `op`, everything else identical. It must collapse with its modern twin —
    // one event written by two versions of the tool, not two commands. Two of
    // the three scanners fell back to `filter` and the third keyed the empty
    // string, so this row had two identities depending on who was asking.
    led.push_str(
        r#"{"filter":"kubectl","label":"kubectl get pods","before":9198,"after":6674,"saved":2524,"source":"wrap","agent":"claude-code","sess":"s1","project":"p","ts":1788360919}"#,
    );
    led.push('\n');
    std::fs::write(tk.join("gain.jsonl"), &led).unwrap();
    assert!(
        tk.join("gain.jsonl").exists(),
        "HOME was not redirected: the probe read the real store"
    );

    // 3 variants + 1 cross-session + 1 a second later + 1 a token bigger = 6
    // events; the doubled append is not one.
    let want_ops = 6u64;
    let want_saved = 5 * (9198u64 - 6674) + (9199 - 6674);

    let (g_out, _e, rc) = run(&["gain", "--json"], h.path());
    assert_eq!(rc, 0, "gain --json failed: {g_out}");
    let g: serde_json::Value = serde_json::from_str(&g_out).expect("gain json");
    assert_eq!(
        g["operations"].as_u64(),
        Some(want_ops),
        "gain collapsed distinct commands, or counted the doubled append: {g_out}"
    );
    assert_eq!(g["saved"].as_u64(), Some(want_saved), "gain saved: {g_out}");

    // Every surface scans the ledger for itself, so each needs its own
    // assertion — one of the three keyed `op` differently from the others.
    let (d_out, _e, rc) = run(&["dashboard", "--json"], h.path());
    assert_eq!(rc, 0, "dashboard --json failed: {d_out}");
    let d: serde_json::Value = serde_json::from_str(&d_out).expect("dashboard json");
    assert_eq!(
        d["operations"].as_u64(),
        Some(want_ops),
        "dashboard: {d_out}"
    );
    assert_eq!(
        d["tokens_saved"].as_u64(),
        Some(want_saved),
        "dashboard saved: {d_out}"
    );

    let (r_out, _e, rc) = run(&["receipt", "--json"], h.path());
    assert_eq!(rc, 0, "receipt --json failed: {r_out}");
    let r: serde_json::Value = serde_json::from_str(&r_out).expect("receipt json");
    assert_eq!(
        r["body"]["events"].as_u64(),
        Some(want_ops),
        "the signed receipt certified the collapsed count: {r_out}"
    );
    assert_eq!(
        r["body"]["tokens_saved"].as_u64(),
        Some(want_saved),
        "receipt saved: {r_out}"
    );
}

#[test]
fn e2e_gain_prints_no_metered_figure() {
    // 2.0.2: the paid service that metered savings is gone, but `gain` still
    // printed a `metered:` line (and `*_as_metered` JSON fields) about "the
    // server's ingest". This ledger is the one that used to trigger it: three
    // events the coarser server key would have merged into one.
    let h = TempHome::new("gain-no-metered");
    let tk = h.path().join(".tokenade");
    std::fs::create_dir_all(&tk).unwrap();
    let mut led = String::new();
    for label in [
        "kubectl get pods",
        "kubectl get po -A",
        "kubectl get pods --all-namespaces",
    ] {
        led.push_str(&format!(
            r#"{{"op":"kubectl","label":"{label}","before":9198,"after":6674,"saved":2524,"source":"wrap","agent":"claude-code","sess":"s1","project":"p","ts":1788360919}}"#
        ));
        led.push('\n');
    }
    std::fs::write(tk.join("gain.jsonl"), &led).unwrap();
    let (out, _e, rc) = run(&["gain", "--json"], h.path());
    assert_eq!(rc, 0, "gain --json failed: {out}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("json");
    assert_eq!(v["operations"].as_u64(), Some(3), "local events: {out}");
    assert_eq!(v["saved"].as_u64(), Some(3 * 2524), "local saved: {out}");
    assert!(
        v.get("operations_as_metered").is_none() && v.get("saved_as_metered").is_none(),
        "gain --json still reports a metered figure: {out}"
    );
    let (text, _e, rc) = run(&["gain"], h.path());
    assert_eq!(rc, 0, "gain failed: {text}");
    assert!(
        !text.to_lowercase().contains("metered") && !text.contains("server"),
        "gain still talks about server metering: {text}"
    );
}

/// `--days` is refused on the surfaces that are lifetime by design, and still
/// works on the one that windows.
///
/// `cmd_dashboard_json` takes no window and `cmd_dashboard_web` takes none
/// either, so both accepted `--days` and answered with the whole history.
/// Measured 2026-09-04 on a ledger holding rows 60 days old, the text
/// dashboard answered 5 ops at `--days 7` and 8 at `--days 90` while
/// `--json --days 7` and `--days 90` returned the same 8 — the divergence the
/// JSON renderer's own doc claimed it had closed.
#[test]
fn e2e_dashboard_days_is_refused_where_it_cannot_apply() {
    let home = TempHome::new("dash-days");
    let dir = home.path().join(".tokenade");
    std::fs::create_dir_all(&dir).unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut ledger = String::new();
    for (i, age_days) in [0u64, 0, 0, 60, 60].iter().enumerate() {
        let ts = now - age_days * 86_400;
        ledger.push_str(&format!(
            "{{\"ts\":{ts},\"op\":\"shell\",\"label\":\"probe{i}\",\"source\":\"wrap\",\
             \"before\":10000,\"after\":1000,\"saved\":9000}}\n"
        ));
    }
    std::fs::write(dir.join("gain.jsonl"), ledger).unwrap();

    for args in [
        vec!["dashboard", "--json", "--days", "7"],
        vec!["dashboard", "--html", "/dev/null", "--days", "7"],
    ] {
        let (_, err, code) = run(&args, home.path());
        assert_eq!(code, 2, "{args:?} must be refused:\n{err}");
        assert!(
            err.contains("does not apply") && err.contains("LIFETIME"),
            "the refusal must say why:\n{err}"
        );
    }

    // Without the flag, the lifetime surface still answers.
    let (out, err, code) = run(&["dashboard", "--json"], home.path());
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("\"scope\": \"lifetime\""), "{out:.200}");

    // The default run is the web page, which carries its own period selector,
    // so `--days` is refused there too rather than parsed and ignored.
    let (_, err, code) = run(&["dashboard", "--days", "7"], home.path());
    assert_eq!(code, 2, "the page path must refuse --days:\n{err}");
    assert!(
        err.contains("--text") && err.contains("period selector"),
        "the refusal must name what does window:\n{err}"
    );

    // And the surface that DOES window still does.
    let (narrow, _, c1) = run(&["dashboard", "--text", "--days", "7"], home.path());
    let (wide, _, c2) = run(&["dashboard", "--text", "--days", "90"], home.path());
    assert_eq!((c1, c2), (0, 0));
    assert_ne!(
        narrow, wide,
        "the text dashboard must still answer differently for two windows"
    );
}

/// The by-op table includes the estimated rows; `operations:` and `saved:` do
/// not — and a reader who sums the table must be told, with the numbers that
/// close both gaps.
///
/// Code-intel rows (`skeleton`, `query`, `symbol_find`, …) are accumulated
/// into `estimated_saved` and never reach `operations`/`saved`, while the
/// per-op and per-source breakdown is filled for EVERY row. `estimated_saved`
/// let a reader reconcile the tokens; nothing reconciled the call count.
/// MEASURED on this install: `operations: 72679` with an `N×` column summing
/// to 73419 — the same word 740 apart in one screen, which is the very
/// contradiction the neighbouring comment describes and had only half fixed.
#[test]
fn e2e_the_estimated_rows_reconcile_both_totals() {
    let h = TempHome::new("gain-estimated-reconcile");
    let tk = h.path().join(".tokenade");
    std::fs::create_dir_all(&tk).unwrap();
    // One measured row (2 calls) and one code-intel row (3 calls). The second
    // lands in the estimated bucket, so the headline counts 2 and the table 5.
    std::fs::write(
        tk.join("gain.jsonl"),
        "{\"op\":\"widget\",\"label\":\"w\",\"before\":1000,\"after\":100,\"saved\":900,\
         \"source\":\"hook\",\"agent\":\"t\",\"project\":\"p\",\"ts\":1788000000,\
         \"rollup\":true,\"rollup_calls\":2}\n\
         {\"op\":\"skeleton\",\"label\":\"s\",\"before\":8000,\"after\":800,\"saved\":7200,\
         \"source\":\"cli\",\"agent\":\"t\",\"project\":\"p\",\"ts\":1788000001,\
         \"rollup\":true,\"rollup_calls\":3}\n",
    )
    .unwrap();

    let (stdout, _err, code) = run(&["gain", "--json"], h.path());
    assert_eq!(code, 0, "exit 0 expected — stdout: {stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let sum = |k: &str, field: &str| -> u64 {
        v[k].as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|o| o[field].as_u64().unwrap_or(0))
            .sum()
    };
    let ops = v["operations"].as_u64().expect("operations");
    let saved = v["saved"].as_u64().expect("saved");
    let est_calls = v["estimated_calls"].as_u64().expect("estimated_calls");
    let est_saved = v["estimated_saved"].as_u64().expect("estimated_saved");

    // Anti-vacuity: the fixture must really put rows on both sides, or the
    // reconciliation below is 0 == 0 and proves nothing.
    assert!(
        est_calls > 0 && ops > 0,
        "fixture puts nothing in the estimated bucket ({est_calls}) or the \
         measured one ({ops}): {stdout}"
    );

    for key in ["by_op", "by_source"] {
        assert_eq!(
            sum(key, "count"),
            ops + est_calls,
            "{key} count must reconcile as operations + estimated_calls: {stdout}"
        );
        assert_eq!(
            sum(key, "saved"),
            saved + est_saved,
            "{key} saved must reconcile as saved + estimated_saved: {stdout}"
        );
    }

    // `dashboard --json` is the SIBLING surface, built by a different scanner
    // over the same ledger, and it had the same gap: it published
    // `estimated_tokens_saved` (closing the token half) and nothing for the
    // calls. A fix proven on `gain` alone is not proven here.
    let (dash, _e, dc) = run(&["dashboard", "--json"], h.path());
    assert_eq!(dc, 0, "dashboard failed: {dash}");
    let dv: serde_json::Value = serde_json::from_str(&dash).expect("dashboard json");
    let d_ops = dv["operations"].as_u64().expect("operations");
    let d_est = dv["estimated_calls"]
        .as_u64()
        .expect("dashboard estimated_calls");
    let d_n: u64 = dv["per_op"]
        .as_array()
        .expect("per_op")
        .iter()
        .map(|o| o["n"].as_u64().unwrap_or(0))
        .sum();
    assert!(d_est > 0, "dashboard fixture has no estimated rows: {dash}");
    assert_eq!(
        d_n,
        d_ops + d_est,
        "dashboard per_op `n` must reconcile as operations + estimated_calls: {dash}"
    );

    // And the human output must SAY it, with the number — prose is what a
    // person reads, and the JSON field is what a program reads.
    let (text, _e, c) = run(&["gain"], h.path());
    assert_eq!(c, 0, "gain failed: {text}");
    let line = text
        .lines()
        .find(|l| l.contains("excluded from"))
        .unwrap_or_else(|| panic!("no exclusion line:\n{text}"));
    assert!(
        line.contains("operations:") && line.contains(&est_calls.to_string()),
        "the exclusion line must name `operations:` and the call count that \
         closes the gap: {line:?}"
    );
}

/// `by_op` used to publish a `percent` without saying WHAT it compares to.
///
/// A code-intel op has no input: `structure_map` builds a map of the repo, and
/// its "baseline" is the published ratio of `code_intel_heuristic` (57 %). Its
/// `percent` therefore IS that ratio, and its residue is the whole answer. The
/// terminal table has always partitioned these ops; this JSON did not say so,
/// and it is the one automated consumers read — including the value ranking
/// that picks each iteration's target, which listed `structure_map` at −56 %
/// among fourteen measured rates.
#[test]
fn e2e_by_op_says_whether_its_baseline_was_measured_or_borrowed() {
    let h = TempHome::new("gain-borrowed-baseline");
    let tk = h.path().join(".tokenade");
    std::fs::create_dir_all(&tk).unwrap();
    // Three rows: an ordinary measured op, a code-intel op whose baseline is
    // borrowed (`ci_heuristic` stamp), and a code-intel op whose baseline is
    // a real input read — `skeleton` tokenises the file it is handed. The
    // three must be told apart.
    std::fs::write(
        tk.join("gain.jsonl"),
        concat!(
            "{\"op\":\"widget\",\"label\":\"w\",\"before\":1000,\"after\":100,\"saved\":900,\"source\":\"hook\",\"agent\":\"t\",\"project\":\"p\",\"ts\":1788000000}\n",
            "{\"op\":\"structure_map\",\"label\":\"map\",\"class\":\"ci_heuristic\",\"before\":2326,\"after\":1000,\"saved\":1326,\"source\":\"cli\",\"agent\":\"t\",\"project\":\"p\",\"ts\":1788000001}\n",
            "{\"op\":\"skeleton\",\"label\":\"sk\",\"before\":9000,\"after\":400,\"saved\":8600,\"source\":\"cli\",\"agent\":\"t\",\"project\":\"p\",\"ts\":1788000002}\n",
            // And a MIXED family, which is the real case: the `ci_heuristic`
            // stamp arrived after part of the history, so the same op carries
            // stamped rows and older rows that are not. Without it,
            // "borrowed" and "mixed" cannot be told apart.
            "{\"op\":\"symbol_find\",\"label\":\"sf\",\"class\":\"ci_heuristic\",\"before\":2326,\"after\":1000,\"saved\":1326,\"source\":\"cli\",\"agent\":\"t\",\"project\":\"p\",\"ts\":1788000003}\n",
            "{\"op\":\"symbol_find\",\"label\":\"sf\",\"before\":5000,\"after\":1000,\"saved\":4000,\"source\":\"cli\",\"agent\":\"t\",\"project\":\"p\",\"ts\":1788000004}\n"
        ),
    )
    .unwrap();
    let (stdout, _err, code) = run(&["gain", "--json"], h.path());
    assert_eq!(code, 0, "exit 0 expected — stdout: {stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let ops = v["by_op"].as_array().expect("by_op");
    let of = |name: &str| -> serde_json::Value {
        ops.iter()
            .find(|o| o["op"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("{name} absent de by_op: {stdout}"))
            .clone()
    };
    assert_eq!(
        of("structure_map")["baseline"],
        serde_json::json!("borrowed"),
        "a baseline stamped ci_heuristic must call itself borrowed: {stdout}"
    );
    assert_eq!(
        of("structure_map")["borrowed_saved"],
        serde_json::json!(1326),
        "and say HOW MANY, otherwise a consumer cannot weigh a mixed family: {stdout}"
    );
    // The distinction is the point: discarding `skeleton` with the others
    // would disown the MEASURED half of the bucket, which is the larger one.
    assert_eq!(
        of("skeleton")["baseline"],
        serde_json::json!("measured"),
        "skeleton tokenises the file it is handed: {stdout}"
    );
    assert_eq!(of("skeleton")["borrowed_saved"], serde_json::json!(0));
    assert_eq!(of("widget")["baseline"], serde_json::json!("measured"));
    // Mixed: one part borrowed, one part measured. Lumping both into
    // `borrowed` would make the half that has a real input disappear from the
    // table, and the reverse would pass off a half-fabricated rate as a
    // measurement.
    assert_eq!(
        of("symbol_find")["baseline"],
        serde_json::json!("mixed"),
        "a two-halved family must call itself mixed: {stdout}"
    );
    assert_eq!(
        of("symbol_find")["borrowed_saved"],
        serde_json::json!(1326),
        "and count ONLY the borrowed half: {stdout}"
    );
}

/// A baseline COMPUTED from what was delivered is stamped at write time,
/// whatever constant produced it.
///
/// The first fix only covered code-intelligence ops. `coefficients`
/// fabricates a baseline in three other places — a web search's native
/// multiplier, a reader's turn credit, a binary's — and `web-search` came out
/// of the ranking at −67 %, i.e. exactly 1 − 1/3, among measured rates. The
/// stamp is set by `record`, from the flag the `Savings` carries: none of the
/// thirteen callers has to think about it.
#[test]
fn e2e_a_counterfactual_baseline_is_stamped_at_write_time() {
    let h = TempHome::new("gain-counterfactual-stamp");
    let tk = h.path().join(".tokenade");
    std::fs::create_dir_all(&tk).unwrap();
    // Two rows of the SAME op, written by both forms: a read baseline and a
    // fabricated one. The stamp must distinguish rows, not classify the family.
    std::fs::write(
        tk.join("gain.jsonl"),
        concat!(
            "{\"op\":\"feed\",\"label\":\"f\",\"before\":9000,\"after\":400,\"saved\":8600,\"source\":\"hook\",\"agent\":\"t\",\"project\":\"p\",\"ts\":1788000000}\n",
            "{\"op\":\"feed\",\"label\":\"f\",\"class\":\"counterfactual\",\"before\":1300,\"after\":1000,\"saved\":300,\"source\":\"hook\",\"agent\":\"t\",\"project\":\"p\",\"ts\":1788000001}\n",
            // And a row OLDER than the stamp, on an op whose only writer is
            // `web_search_credit_savings`: the op is the only evidence left,
            // and a hundred and four days of ledger depend on it.
            "{\"op\":\"web-search\",\"label\":\"s\",\"before\":3000,\"after\":1000,\"saved\":2000,\"source\":\"cli\",\"agent\":\"t\",\"project\":\"p\",\"ts\":1788000002}\n"
        ),
    )
    .unwrap();
    let (stdout, _err, code) = run(&["gain", "--json"], h.path());
    assert_eq!(code, 0, "exit 0 expected — stdout: {stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let ops = v["by_op"].as_array().expect("by_op");
    let of = |name: &str| -> serde_json::Value {
        ops.iter()
            .find(|o| o["op"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("{name} absent de by_op: {stdout}"))
            .clone()
    };
    assert_eq!(
        of("feed")["baseline"],
        serde_json::json!("mixed"),
        "a family written by both paths is mixed: {stdout}"
    );
    assert_eq!(
        of("feed")["borrowed_saved"],
        serde_json::json!(300),
        "and counts ONLY the stamped row: {stdout}"
    );
    assert_eq!(
        of("web-search")["baseline"],
        serde_json::json!("borrowed"),
        "the op is enough where the stamp did not exist yet: {stdout}"
    );
}

/// `net` keeps the naive `before − after` of `tokens_saved`; `net_realistic`
/// bounds a huge output by what the harness would have delivered — the text
/// block's realistic line, now in the JSON too.
#[test]
fn e2e_dashboard_json_has_a_realistic_net() {
    let h = TempHome::new("net-realistic");
    let dir = h.path().join(".tokenade");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("gain.jsonl"),
        r#"{"ts":1789000000,"source":"wrap","agent":"claude-code","op":"cat","cmd":"cat","before":1000000,"after":500,"saved":999500}
"#,
    )
    .unwrap();
    let (d_out, _e, rc) = run(&["dashboard", "--json"], h.path());
    assert_eq!(rc, 0, "{d_out}");
    let d: serde_json::Value = serde_json::from_str(&d_out).expect("dashboard json");
    let b = &d["estimated_breakdown"];
    let (net, real) = (
        b["net"].as_u64().unwrap(),
        b["net_realistic"].as_u64().unwrap(),
    );
    assert_eq!(net, 999_500, "{b}");
    assert!(
        real < net / 2,
        "a 1 M-token output was not bounded by delivery: {b}"
    );
}

/// A command output folded by `wrap` is banked for the session's compounding
/// credit, as a folded Read always was: it leaves the context it would have
/// been re-read from.
#[cfg(unix)]
#[test]
fn e2e_a_folded_command_is_banked_for_its_session() {
    let h = TempHome::new("wrap-banks");
    let sessions = h.path().join(".tokenade").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("abc123-turn"), "3").unwrap();
    let (out, _e, rc) = run_with_env(
        &["wrap", "seq 1 20000"],
        h.path(),
        &[("TOKENADE_SESSION", "abc-123")],
    );
    assert_eq!(rc, 0, "{out}");
    let log =
        std::fs::read_to_string(sessions.join("abc123-compactions.jsonl")).unwrap_or_default();
    let v: serde_json::Value =
        serde_json::from_str(log.lines().next().expect("no banked fold")).unwrap();
    assert_eq!(v["turn"].as_u64(), Some(3), "{log}");
    assert!(v["delta"].as_u64().unwrap_or(0) > 1_000, "{log}");
}

/// With the session's transcript readable, a fold earns one re-read per real
/// API call after the one that sends it — counted off the transcript, not per
/// user turn.
#[cfg(unix)]
#[test]
fn e2e_a_fold_is_credited_per_real_call_after_it() {
    let h = TempHome::new("rereads-exact");
    let sessions = h.path().join(".tokenade").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let t0 = 1_791_194_400u64; // 2026-10-05T10:00:00Z
    std::fs::write(
        sessions.join("s1-compactions.jsonl"),
        format!("{{\"turn\":1,\"delta\":1000,\"ts\":{t0}}}\n"),
    )
    .unwrap();
    std::fs::write(sessions.join("s1-comprec-exact-v2"), "0").unwrap();
    let call = |s: u64, mid: &str| {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-10-05T10:00:{s:02}Z","message":{{"id":"{mid}","model":"claude-opus-5-5","usage":{{"input_tokens":5,"cache_read_input_tokens":20000}}}}}}"#
        )
    };
    let transcript = h.path().join("t.jsonl");
    std::fs::write(
        &transcript,
        [call(10, "a"), call(20, "b"), call(30, "c")].join("\n") + "\n",
    )
    .unwrap();
    let stdin = serde_json::json!({"session_id": "s1", "transcript_path": transcript, "hook_event_name": "UserPromptSubmit", "prompt": "go"}).to_string();
    let (out, err, rc) = run_hook_as(
        Some("claude-code"),
        &["hookwrap-userprompt"],
        h.path(),
        &stdin,
    );
    assert_eq!(rc, 0, "{out}{err}");
    let est = std::fs::read_to_string(h.path().join(".tokenade").join("gain_estimated.jsonl"))
        .unwrap_or_default();
    let row = est
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v["op"] == "compounding")
        .unwrap_or_else(|| panic!("no compounding row:\n{est}"));
    assert_eq!(row["saved"].as_u64(), Some(2_000), "{row}");
}

/// A recovery costs more than the text it prints: that text stays in the
/// context and is re-read until the next compaction (logged as injected, for
/// the re-bill), and the call itself re-read the whole conversation (its
/// context at fire, from the session's transcript).
#[test]
fn e2e_a_recovery_books_its_turn_and_its_persistence() {
    let h = TempHome::new("recovery-turn");
    let home = h.path();
    let body: String = (0..6_000)
        .map(|i| {
            format!(
                "2024-01-01T00:00:{:02}Z INFO worker {i} processed batch {i}\n",
                i % 60
            )
        })
        .collect();
    let f = home.join("big.log");
    std::fs::write(&f, &body).unwrap();
    let cat = format!("cat {}", f.to_str().unwrap());
    let (folded, _e, _c) = run(&["wrap", cat.as_str()], home);
    let hash = folded
        .split("expand-ref ")
        .nth(1)
        .and_then(|r| r.split(['`', ' ', '\n']).next())
        .unwrap()
        .to_string();
    let sid = "0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0";
    let proj = home.join(".claude").join("projects").join("p");
    std::fs::create_dir_all(&proj).unwrap();
    let fire = serde_json::json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "input": {"command": format!("tokenade expand-ref {hash} --grep 'batch 7'")}}],
        "usage": {"input_tokens": 3, "cache_read_input_tokens": 49_997, "cache_creation_input_tokens": 0}}});
    std::fs::write(proj.join(format!("{sid}.jsonl")), format!("{fire}\n")).unwrap();
    let (out, err, rc) = run_with_env(
        &["expand-ref", hash.as_str(), "--grep", "batch 7"],
        home,
        &[("TOKENADE_SESSION", "s9"), ("CLAUDE_CODE_SESSION_ID", sid)],
    );
    assert_eq!(rc, 0, "{out}{err}");
    let est = std::fs::read_to_string(home.join(".tokenade").join("gain_estimated.jsonl"))
        .unwrap_or_default();
    let rows: Vec<serde_json::Value> = est
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let cost = rows
        .iter()
        .find(|v| v["op"] == "recovery_cost")
        .unwrap_or_else(|| panic!("no recovery_cost:\n{est}"));
    let turn = rows
        .iter()
        .find(|v| v["op"] == "recovery_turn")
        .unwrap_or_else(|| panic!("no recovery_turn:\n{est}"));
    assert_eq!(turn["saved"].as_u64(), Some(50_000), "{turn}");
    assert_eq!(
        (turn["class"].as_str(), turn["io"].as_str()),
        (Some("overhead"), Some("cache")),
        "{turn}"
    );
    let log = std::fs::read_to_string(home.join(".tokenade").join("sessions").join("s9-ovh-ts"))
        .unwrap_or_default();
    let logged: u64 = log
        .lines()
        .filter_map(|l| l.split_once(' ')?.1.parse::<u64>().ok())
        .sum();
    assert_eq!(
        Some(logged),
        cost["saved"].as_u64(),
        "persistence not logged:\n{log}"
    );
    // Fired alongside another tool call, the recovery added no call: no turn.
    let other = serde_json::json!({"type": "assistant", "message": {"id": "m2", "content": [{"type": "tool_use", "input": {"command": "git status"}}],
        "usage": {"input_tokens": 3, "cache_read_input_tokens": 49_997}}});
    let mut fire2 = fire.clone();
    fire2["message"]["id"] = serde_json::json!("m2");
    std::fs::write(
        proj.join(format!("{sid}.jsonl")),
        format!("{other}\n{fire2}\n"),
    )
    .unwrap();
    let (out, err, rc) = run_with_env(
        &["expand-ref", hash.as_str(), "--grep", "batch 8"],
        home,
        &[("TOKENADE_SESSION", "s9"), ("CLAUDE_CODE_SESSION_ID", sid)],
    );
    assert_eq!(rc, 0, "{out}{err}");
    let est = std::fs::read_to_string(home.join(".tokenade").join("gain_estimated.jsonl"))
        .unwrap_or_default();
    assert_eq!(est.matches("\"recovery_turn\"").count(), 1, "{est}");
    assert_eq!(est.matches("\"recovery_cost\"").count(), 2, "{est}");
}

/// A resume trim is banked for the re-read credit DATED, like every fold:
/// the exact count skips an undated line, and this path wrote its own.
#[test]
fn e2e_a_resume_trim_is_banked_with_its_time() {
    let h = TempHome::new("resume-ts");
    let transcript = h.path().join("t.jsonl");
    let masked = serde_json::json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "x",
        "content": "[tokenade:elided 5000 tokens on cold resume — recover: tokenade expand-ref abcdef012345]"}]}});
    std::fs::write(&transcript, format!("{masked}\n")).unwrap();
    let stdin = serde_json::json!({"session_id": "s7", "transcript_path": transcript, "hook_event_name": "SessionStart", "source": "resume"}).to_string();
    let (out, err, rc) = run_hook_as(
        Some("claude-code"),
        &["hookwrap-sessionstart"],
        h.path(),
        &stdin,
    );
    assert_eq!(rc, 0, "{out}{err}");
    let log = std::fs::read_to_string(
        h.path()
            .join(".tokenade")
            .join("sessions")
            .join("s7-compactions.jsonl"),
    )
    .unwrap_or_default();
    let line: serde_json::Value = log
        .lines()
        .find_map(|l| serde_json::from_str(l).ok())
        .unwrap_or_else(|| panic!("nothing banked:\n{log}\n{out}{err}"));
    assert!(
        line["ts"].as_u64().is_some_and(|t| t > 1_700_000_000),
        "{line}"
    );
    assert!(line["delta"].as_u64().is_some_and(|d| d > 0), "{line}");
}

/// The brief injected into a subagent lives in the subagent's context: its
/// re-reads are charged for a subagent's calls (mean 65 after its first), not logged as the
/// parent's injection to be re-billed until the parent compacts.
#[test]
fn e2e_a_subagent_brief_is_charged_for_the_subagent_s_calls() {
    let h = TempHome::new("subagent-brief");
    let (out, err, rc) = run_with_stdin(
        &["subagent-brief", "--type", "Explore"],
        h.path(),
        "find the bug",
    );
    assert_eq!(rc, 0, "{out}{err}");
    let est = std::fs::read_to_string(h.path().join(".tokenade").join("gain_estimated.jsonl"))
        .unwrap_or_default();
    let rows: Vec<serde_json::Value> = est
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let injected = rows
        .iter()
        .find(|v| v["op"] == "injected_context")
        .unwrap_or_else(|| panic!("no injection row:\n{est}"));
    let rebill = rows
        .iter()
        .find(|v| v["op"] == "overhead_rebill")
        .unwrap_or_else(|| panic!("no re-bill row:\n{est}"));
    assert_eq!(
        rebill["saved"].as_u64(),
        injected["saved"]
            .as_u64()
            .map(|n| n * tokenade_core::gain::SUBAGENT_BRIEF_REREADS),
        "{est}"
    );
    assert_eq!(rebill["label"], "proxy", "{rebill}");
    let logs = std::fs::read_dir(h.path().join(".tokenade").join("sessions"))
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with("-ovh-ts"))
                .count()
        })
        .unwrap_or(0);
    assert_eq!(logs, 0);
}

/// A web page is credited on what the agent receives: a page put behind a
/// stash pointer is not credited as if it had been delivered whole.
#[test]
fn e2e_a_stashed_web_page_is_credited_on_what_was_delivered() {
    let h = TempHome::new("web-delivered");
    let body: String = (0..400).map(|i| format!("<h2>Section {i}</h2><p>Paragraph {i} explains a different detail at length, with words.</p>")).collect();
    let page = h.path().join("big.html");
    std::fs::write(
        &page,
        format!("<html><head><title>Big</title></head><body>{body}</body></html>"),
    )
    .unwrap();
    let (out, err, rc) = run_hook_as(
        Some("claude-code"),
        &["web", page.to_str().unwrap()],
        h.path(),
        "",
    );
    assert_eq!(rc, 0, "{out}{err}");
    let ledger =
        std::fs::read_to_string(h.path().join(".tokenade").join("gain.jsonl")).unwrap_or_default();
    let row: serde_json::Value = ledger
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .find(|v: &serde_json::Value| v["op"] == "html-to-markdown")
        .unwrap_or_else(|| panic!("no page row:\n{ledger}\n{out}{err}"));
    let after = row["after"].as_u64().unwrap();
    let delivered = tokenade_core::tokenizer::estimate(&out) as u64;
    assert!(
        after <= delivered + 20 && after + 20 >= delivered,
        "booked {after}, delivered {delivered}\n{row}"
    );
    assert_eq!(row["before"].as_u64(), Some(after * 3), "{row}");
}

/// A subagent that loads CLAUDE.md carries tokenade's block on each of its
/// calls; Explore opts out of CLAUDE.md and pays nothing for it.
#[test]
fn e2e_a_subagent_that_loads_the_rules_file_is_charged_for_it() {
    let h = TempHome::new("subagent-rules");
    let claude = h.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    let block = format!(
        "{}\n{}\n<!-- /tokenade-scaffold -->\n",
        tokenade_core::project_scaffold::SCAFFOLD_MARKER,
        "Use tokenade map. ".repeat(40)
    );
    std::fs::write(claude.join("CLAUDE.md"), block).unwrap();
    let rows = |h: &TempHome| -> Vec<serde_json::Value> {
        let est = std::fs::read_to_string(h.path().join(".tokenade").join("gain_estimated.jsonl"))
            .unwrap_or_default();
        est.lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    };
    let (out, err, rc) = run_with_stdin(
        &["subagent-brief", "--type", "Explore"],
        h.path(),
        "find the bug",
    );
    assert_eq!(rc, 0, "{out}{err}");
    assert!(
        !rows(&h).iter().any(|v| v["label"] == "subagent-rules"),
        "Explore does not load CLAUDE.md"
    );
    // A custom agent that opts out in its frontmatter.
    std::fs::create_dir_all(claude.join("agents")).unwrap();
    std::fs::write(
        claude.join("agents").join("quiet.md"),
        "---\nname: quiet\nomitClaudeMd: true\n---\nYou are quiet.\n",
    )
    .unwrap();
    let (out, err, rc) = run_with_stdin(
        &["subagent-brief", "--type", "quiet"],
        h.path(),
        "find the bug",
    );
    assert_eq!(rc, 0, "{out}{err}");
    assert!(
        !rows(&h).iter().any(|v| v["label"] == "subagent-rules"),
        "quiet opts out of CLAUDE.md"
    );
    let (out, err, rc) = run_with_stdin(
        &["subagent-brief", "--type", "general-purpose"],
        h.path(),
        "find the bug",
    );
    assert_eq!(rc, 0, "{out}{err}");
    let all = rows(&h);
    let sent = all
        .iter()
        .find(|v| v["label"] == "subagent-rules")
        .unwrap_or_else(|| panic!("no rules row: {all:?}"));
    let n = sent["saved"].as_u64().unwrap();
    assert!(n > 100, "{sent}");
    assert!(
        all.iter().any(|v| v["op"] == "overhead_rebill"
            && v["saved"].as_u64() == Some(n * tokenade_core::gain::SUBAGENT_RULES_REREADS)),
        "{all:?}"
    );
}

/// A fold made while a subagent runs is not banked for the parent's re-reads
/// (it never sat in the parent's context): credited at once, for a
/// subagent's calls.
#[test]
fn e2e_a_fold_inside_a_subagent_is_credited_for_the_subagent_only() {
    let h = TempHome::new("subagent-fold");
    let home = h.path();
    let body: String = (0..6_000)
        .map(|i| {
            format!(
                "2024-01-01T00:00:{:02}Z INFO worker {i} processed batch {i}\n",
                i % 60
            )
        })
        .collect();
    let f = home.join("big.log");
    std::fs::write(&f, &body).unwrap();
    let pending = home.join(".tokenade").join("read-pending");
    std::fs::create_dir_all(&pending).unwrap();
    std::fs::write(pending.join("sidechain-s4"), "1:9999999999").unwrap();
    let cat = format!("cat {}", f.to_str().unwrap());
    let (out, err, rc) = run_with_env(&["wrap", cat.as_str()], home, &[("TOKENADE_SESSION", "s4")]);
    assert_eq!(rc, 0, "{err}");
    assert!(out.contains("expand-ref"), "not folded:\n{out}");
    let sessions = home.join(".tokenade").join("sessions");
    assert!(
        !sessions.join("s4-compactions.jsonl").exists(),
        "banked for the parent"
    );
    let est = std::fs::read_to_string(home.join(".tokenade").join("gain_estimated.jsonl"))
        .unwrap_or_default();
    let row: serde_json::Value = est
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .find(|v: &serde_json::Value| v["op"] == "compounding")
        .unwrap_or_else(|| panic!("no credit:\n{est}"));
    assert_eq!(row["label"], "proxy", "{row}");
    assert!(
        row["saved"]
            .as_u64()
            .is_some_and(|n| n > 0 && n % tokenade_core::gain::SUBAGENT_EVENT_REREADS == 0),
        "{row}"
    );
}

/// What tokenade adds to the context while a subagent runs is the subagent's:
/// re-billed for a subagent's calls, not logged for the parent's.
#[test]
fn e2e_text_added_inside_a_subagent_is_billed_for_the_subagent() {
    let h = TempHome::new("subagent-inject");
    let home = h.path();
    let body: String = (0..6_000)
        .map(|i| {
            format!(
                "2024-01-01T00:00:{:02}Z INFO worker {i} processed batch {i}\n",
                i % 60
            )
        })
        .collect();
    let f = home.join("big.log");
    std::fs::write(&f, &body).unwrap();
    let cat = format!("cat {}", f.to_str().unwrap());
    let (folded, _e, _c) = run(&["wrap", cat.as_str()], home);
    let hash = folded
        .split("expand-ref ")
        .nth(1)
        .and_then(|r| r.split(['`', ' ', '\n']).next())
        .unwrap()
        .to_string();
    let pending = home.join(".tokenade").join("read-pending");
    std::fs::create_dir_all(&pending).unwrap();
    std::fs::write(pending.join("sidechain-s6"), "1:9999999999").unwrap();
    let (out, err, rc) = run_with_env(
        &["expand-ref", hash.as_str(), "--grep", "batch 7"],
        home,
        &[("TOKENADE_SESSION", "s6")],
    );
    assert_eq!(rc, 0, "{out}{err}");
    assert!(
        !home
            .join(".tokenade")
            .join("sessions")
            .join("s6-ovh-ts")
            .exists(),
        "logged for the parent"
    );
    let est = std::fs::read_to_string(home.join(".tokenade").join("gain_estimated.jsonl"))
        .unwrap_or_default();
    let rows: Vec<serde_json::Value> = est
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let cost = rows
        .iter()
        .find(|v| v["op"] == "recovery_cost")
        .unwrap_or_else(|| panic!("{est}"));
    let rebill = rows
        .iter()
        .find(|v| v["op"] == "overhead_rebill")
        .unwrap_or_else(|| panic!("no re-bill:\n{est}"));
    assert_eq!(
        rebill["saved"].as_u64(),
        cost["saved"]
            .as_u64()
            .map(|n| n * tokenade_core::gain::SUBAGENT_EVENT_REREADS),
        "{est}"
    );
    assert_eq!(rebill["label"], "proxy");
}

/// An agent with no prompt hook and no readable timeline: each tool call is an
/// API call that re-reads what tokenade put in the context. The tokenade block
/// of its rules file is charged once per session, then 0.69 times per call.
#[test]
fn e2e_each_tool_call_charges_the_injected_context_once() {
    let h = TempHome::new("tool-tick");
    let home = h.path();
    let rules = home.join(".factory").join("AGENTS.md");
    std::fs::create_dir_all(rules.parent().unwrap()).unwrap();
    std::fs::write(
        &rules,
        format!(
            "# mine\n<!-- tokenade-scaffold -->\n{}\n<!-- /tokenade-scaffold -->\n",
            "Use tokenade map. ".repeat(60)
        ),
    )
    .unwrap();
    for _ in 0..3 {
        let (out, err, rc) = run(
            &["session-tick", "--session", "t1", "--agent", "droid"],
            home,
        );
        assert_eq!(rc, 0, "{out}{err}");
    }
    let est = std::fs::read_to_string(home.join(".tokenade").join("gain_estimated.jsonl"))
        .unwrap_or_default();
    let rows: Vec<serde_json::Value> = est
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let scaffold: Vec<_> = rows
        .iter()
        .filter(|v| v["op"] == "scaffold_overhead")
        .collect();
    assert_eq!(scaffold.len(), 1, "once per session:\n{est}");
    let s = scaffold[0]["saved"].as_u64().unwrap();
    assert!(s > 100, "{est}");
    let rebilled: u64 = rows
        .iter()
        .filter(|v| v["op"] == "overhead_rebill")
        .map(|v| v["saved"].as_u64().unwrap())
        .sum();
    let per_call = (s as f64 * 0.69).round() as u64;
    assert_eq!(rebilled, 3 * per_call, "0.69 re-read per call:\n{est}");
    assert!(
        rows.iter()
            .filter(|v| v["op"] == "overhead_rebill")
            .all(|v| v["label"] == "proxy" && v["agent"] == "droid"),
        "{est}"
    );
}

/// An agent with no prompt hook ran no credit at all for its folds. Its tool
/// calls are now its clock: 0.69 API call each (the median measured on
/// Claude Code), so ten calls after a fold credit six re-reads of it.
#[test]
fn e2e_a_fold_is_credited_on_the_tool_call_clock_without_a_prompt_hook() {
    let h = TempHome::new("tick-credit");
    let home = h.path();
    let body: String = (0..6_000)
        .map(|i| {
            format!(
                "2024-01-01T00:00:{:02}Z INFO worker {i} processed batch {i}\n",
                i % 60
            )
        })
        .collect();
    let f = home.join("big.log");
    std::fs::write(&f, &body).unwrap();
    let cat = format!("cat {}", f.to_str().unwrap());
    let env = [("TOKENADE_SESSION", "oc1"), ("TOKENADE_AGENT", "opencode")];
    let (out, err, rc) = run_with_env(&["wrap", cat.as_str()], home, &env);
    assert_eq!(rc, 0, "{err}");
    assert!(out.contains("expand-ref"), "not folded:\n{out}");
    let log = std::fs::read_to_string(
        home.join(".tokenade")
            .join("sessions")
            .join("oc1-compactions.jsonl"),
    )
    .unwrap();
    let banked: u64 = log
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v["delta"].as_u64())
        .sum();
    assert!(banked > 0, "{log}");
    for _ in 0..10 {
        let (o, e, rc) = run(
            &["session-tick", "--session", "oc1", "--agent", "opencode"],
            home,
        );
        assert_eq!(rc, 0, "{o}{e}");
    }
    let est = std::fs::read_to_string(home.join(".tokenade").join("gain_estimated.jsonl"))
        .unwrap_or_default();
    let credited: u64 = est
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["op"] == "compounding" && v["label"] == "proxy")
        .map(|v| v["saved"].as_u64().unwrap())
        .sum();
    assert_eq!(
        credited,
        6 * banked,
        "floor(10 × 0.69) = 6 re-reads:\n{est}"
    );
}

/// A command the agent types as `tokenade wrap …` is never rewritten by the
/// hook, so it has no `TOKENADE_SESSION`: its row went to the ledger with no
/// session and its fold was never banked for the re-reads it saves. Claude
/// Code's own `CLAUDE_CODE_SESSION_ID` names the session instead.
#[test]
fn e2e_a_typed_wrap_is_booked_under_the_agent_s_session() {
    let h = TempHome::new("typed-wrap-sess");
    let (out, err, rc) = run_with_env(
        &["wrap", "seq 1 4000"],
        h.path(),
        &[("CLAUDE_CODE_SESSION_ID", "ab12-cd34")],
    );
    assert_eq!(rc, 0, "{out}{err}");
    let ledger =
        std::fs::read_to_string(h.path().join(".tokenade").join("gain.jsonl")).unwrap_or_default();
    assert!(ledger.contains("\"sess\":\"ab12cd34\""), "{ledger}");
    let log = std::fs::read_to_string(
        h.path()
            .join(".tokenade")
            .join("sessions")
            .join("ab12cd34-compactions.jsonl"),
    )
    .unwrap_or_default();
    assert!(log.contains("\"delta\":"), "fold not banked:\n{log}");
}

/// Every measured fold is banked for the re-reads it saves, at the ledger
/// write — not only the two paths (`wrap`, the Read hook) that used to do it
/// by hand. `tokenade read` of a big file was folded and never banked.
#[test]
fn e2e_a_cli_fold_is_banked_at_the_ledger_write() {
    let h = TempHome::new("cli-fold-bank");
    let doc = h.path().join("big.json");
    let rows: Vec<String> = (0..3000)
        .map(|i| format!("{{\"id\":{i},\"name\":\"item {i}\",\"tags\":[\"a\",\"b\"],\"ok\":true}}"))
        .collect();
    std::fs::write(&doc, format!("[{}]", rows.join(","))).unwrap();
    let (out, err, rc) = run_with_env(
        &["read", doc.to_str().unwrap()],
        h.path(),
        &[("CLAUDE_CODE_SESSION_ID", "cf-01")],
    );
    assert_eq!(rc, 0, "{out}{err}");
    let ledger =
        std::fs::read_to_string(h.path().join(".tokenade").join("gain.jsonl")).unwrap_or_default();
    assert!(
        ledger.contains("\"sess\":\"cf01\""),
        "no folded row:\n{ledger}"
    );
    let log = std::fs::read_to_string(
        h.path()
            .join(".tokenade")
            .join("sessions")
            .join("cf01-compactions.jsonl"),
    )
    .unwrap_or_default();
    let banked: u64 = log
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()?["delta"].as_u64())
        .sum();
    assert!(banked > 1000, "fold not banked:\n{log}\n{ledger}");
}

/// A compaction on a Gemini-family host or reported by a plugin bridge, and a
/// `/clear` that keeps the session id, empty the context: what tokenade injected before stops being charged,
/// from the base the reset sets.
#[test]
fn e2e_a_compaction_or_clear_on_any_host_resets_the_charged_context() {
    for (args, event, extra) in [
        (
            vec!["gemini-lifecycle", "--agent", "qwen"],
            "PreCompact",
            serde_json::json!({}),
        ),
        (
            vec!["hookwrap-sessionstart"],
            "SessionStart",
            serde_json::json!({"source": "clear"}),
        ),
        // A plugin bridge (opencode 1.x `session.compacted` / 2.x
        // `session.compaction.ended`, pi `session_compact`).
        (
            vec![
                "session-tick",
                "--session",
                "r1",
                "--agent",
                "opencode",
                "--reset",
            ],
            "bridge",
            serde_json::json!({}),
        ),
    ] {
        let h = TempHome::new("reset-any-host");
        let sessions = h.path().join(".tokenade").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("r1-ovh"), "500\n").unwrap();
        let mut stdin = serde_json::json!({"session_id": "r1", "hook_event_name": event});
        stdin
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let (out, err, rc) = run_hook_as(None, &args, h.path(), &stdin.to_string());
        assert_eq!(rc, 0, "{out}{err}");
        let base = std::fs::read_to_string(sessions.join("r1-ovh-at-compact"))
            .unwrap_or_else(|_| panic!("{event}: no reset\n{out}{err}"));
        assert_eq!(base.trim(), "500", "{event}");
    }
}

/// A bridge agent that names its session file (pi) is charged exactly: each
/// API call recorded in the file after the one that first sent an injection
/// re-reads it, counted off the file, not estimated.
#[test]
fn e2e_a_pi_session_file_is_counted_call_by_call() {
    let h = TempHome::new("pi-exact");
    let home = h.path();
    let sessions = home.join(".tokenade").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    // 400 tokens injected at 2026-09-28T11:53:50Z, before every call below.
    std::fs::write(sessions.join("pi1-ovh-ts"), "1790596430 400\n").unwrap();
    std::fs::write(sessions.join("pi1-ovh"), "400\n").unwrap();
    std::fs::write(sessions.join("pi1-overhead"), "1").unwrap();
    let file = home.join("s.jsonl");
    let call = |s: u32| {
        format!(
            r#"{{"type":"message","timestamp":"2026-09-28T11:54:{s:02}.000Z","message":{{"role":"assistant","model":"gpt-5.5","usage":{{"input":50,"cacheRead":900,"cacheWrite":0}}}}}}"#
        )
    };
    let tick = || {
        let (o, e, rc) = run(
            &[
                "session-tick",
                "--session",
                "pi1",
                "--agent",
                "pi",
                "--transcript",
                file.to_str().unwrap(),
            ],
            home,
        );
        assert_eq!(rc, 0, "{o}{e}");
    };
    std::fs::write(&file, [call(1), call(2)].join("\n") + "\n").unwrap();
    tick(); // sets the count's starting point
    std::fs::write(
        &file,
        [call(1), call(2), call(3), call(4)].join("\n") + "\n",
    )
    .unwrap();
    tick();
    let est = std::fs::read_to_string(home.join(".tokenade").join("gain_estimated.jsonl"))
        .unwrap_or_default();
    let rows: Vec<serde_json::Value> = est
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter(|v: &serde_json::Value| v["op"] == "overhead_rebill")
        .collect();
    assert_eq!(rows.len(), 1, "{est}");
    assert_eq!(rows[0]["label"], "exact", "{est}");
    // Two more calls re-read the 400 injected tokens.
    assert_eq!(rows[0]["saved"].as_u64(), Some(800), "{est}");
}

/// A bridge that ticks once per API request (Cline) says so: each tick is a
/// whole call, not the 0.69 call a tool call stands for.
#[test]
fn e2e_a_per_request_tick_is_one_call() {
    let h = TempHome::new("per-call");
    let home = h.path();
    let rules = home.join(".cline").join("rules").join("tokenade.md");
    std::fs::create_dir_all(rules.parent().unwrap()).unwrap();
    std::fs::write(&rules, "Use tokenade map. ".repeat(60)).unwrap();
    for _ in 0..3 {
        let (o, e, rc) = run(
            &[
                "session-tick",
                "--session",
                "c1",
                "--agent",
                "cline",
                "--per-call",
            ],
            home,
        );
        assert_eq!(rc, 0, "{o}{e}");
    }
    let est = std::fs::read_to_string(home.join(".tokenade").join("gain_estimated.jsonl"))
        .unwrap_or_default();
    let rows: Vec<serde_json::Value> = est
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let s = rows
        .iter()
        .find(|v| v["op"] == "scaffold_overhead")
        .unwrap_or_else(|| panic!("{est}"))["saved"]
        .as_u64()
        .unwrap();
    let rebilled: u64 = rows
        .iter()
        .filter(|v| v["op"] == "overhead_rebill")
        .map(|v| v["saved"].as_u64().unwrap())
        .sum();
    assert_eq!(rebilled, 3 * s, "{est}");
}

/// A session already counted exactly is never priced by the proxy on a tick
/// where its record cannot be read: those calls are in the exact count.
#[test]
fn e2e_an_exact_session_never_falls_back_to_the_proxy() {
    let h = TempHome::new("exact-sticky");
    let home = h.path();
    let sessions = home.join(".tokenade").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("ex1-ovh"), "500\n").unwrap();
    std::fs::write(sessions.join("ex1-ovh-ts"), "1790596430 500\n").unwrap();
    std::fs::write(sessions.join("ex1-overhead"), "1").unwrap();
    std::fs::write(sessions.join("ex1-rebill-exact-v2"), "0").unwrap();
    let (o, e, rc) = run(
        &["session-tick", "--session", "ex1", "--agent", "opencode"],
        home,
    );
    assert_eq!(rc, 0, "{o}{e}");
    let est = std::fs::read_to_string(home.join(".tokenade").join("gain_estimated.jsonl"))
        .unwrap_or_default();
    assert!(!est.contains("overhead_rebill"), "{est}");
}
