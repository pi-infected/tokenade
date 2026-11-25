//! Tests for `wrap`, split out of the flat `tests.rs`.

use super::*;

#[test]
fn a_footer_that_only_names_the_recovery_command_is_not_a_pointer() {
    // Several fold footers say "recover with `tokenade expand-ref <hash>`
    // (hash printed in this output's banner)". When no banner was emitted that
    // sentence points at nothing — and matching the bare word made this the one
    // case that got skipped, so the unkept promise stayed unkept.

    // This test's subject resolves the stash directory from the
    // ENVIRONMENT, and it said so nowhere: it took only `ENV_RW_LOCK`, so a
    // parallel test moving `HOME` or `TOKENADE_STASH_DIR` left it with nowhere
    // to write and no hash to append. `ENV_LOCK` outermost, `ENV_RW_LOCK` inside.
    let _env = tokenade_core::platform::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _rw = super::env_guard();
    let hint = tokenade_core::disclose::SourceHint::Shell;
    let shown = "[tokenade folded 1098 line(s) total — recover the verbatim bytes \
                 with `tokenade expand-ref <hash>` (hash printed in this output's banner)]"
        .to_string();
    let out =
        crate::wrap::attach_drop_recovery(shown.clone(), "raw bytes here", 40_000, 100_000, &hint, "auto:grep");
    assert!(out.len() > shown.len(), "a real hash was appended:\n{out}");
    assert!(
        out.split("expand-ref ").nth(2).is_some(),
        "the generic mention is still there, plus a live one:\n{out}"
    );
}

/// A pointer already in the view only counts if it leads to THESE bytes.
///
/// `maybe_stash` stashes what is about to be SHOWN; `attach_drop_recovery`
/// stashes the RAW. When the fold is deep enough that the remainder is still
/// worth stashing, the first fires and used to silence the second — so the
/// only hash the agent got named a stash the fold had already emptied.
/// Measured end to end: a 252-line output came back as a 3-line stash, and
/// grepping the ref for a string that WAS printed answered
/// `0 matching line(s) of 4 total`.
#[test]
fn a_live_pointer_over_the_shown_bytes_does_not_silence_the_raw_one() {
    let _env = tokenade_core::platform::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _rw = super::env_guard();
    let raw = (0..4_000)
        .map(|i| format!("src/mod{i}/file.rs:{i}:    let hit = {i};\n"))
        .collect::<String>();
    let hint = tokenade_core::disclose::SourceHint::Shell;

    // A view that already carries a FOLLOWABLE hash — but over other bytes.
    let other = tokenade_core::disclose::stash_for_reference("some other text entirely", &hint)
        .expect("stash")
        .0;
    let shown = format!("folded view [stashed: … `tokenade expand-ref {other}` …]");

    let out = crate::wrap::attach_drop_recovery(shown.clone(), &raw, 40_000, 100_000, &hint, "auto:grep");
    assert!(
        out.len() > shown.len(),
        "the raw bytes were left unrecoverable behind someone else's hash:\n{out}"
    );
    assert!(
        out.contains("COMPLETE output"),
        "the second pointer must say what makes it different:\n{out}"
    );

    // …and the hash it names must actually hold the raw.
    let hash = out
        .rsplit("expand-ref ")
        .next()
        .and_then(|t| t.split(|c: char| !c.is_ascii_hexdigit()).next())
        .expect("hash")
        .to_string();
    assert_ne!(hash, other, "it re-offered the view's own hash");
    let dir = tokenade_core::disclose::stash_dir();
    let held = std::fs::read_dir(&dir)
        .expect("stash dir")
        .flatten()
        .find(|e| e.file_name().to_string_lossy().starts_with(&hash))
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .unwrap_or_default();
    assert!(
        held.contains("src/mod3999/file.rs"),
        "the stash behind the new hash is not the raw output"
    );
}

#[test]
fn a_small_fold_gets_no_recovery_pointer() {
    // Below the bar the agent can just re-run; a disk write and 40 tokens of
    // banner would cost more than the recovery is worth. 200 dropped tokens
    // out of 100 000 is under the absolute floor AND a sliver of the output,
    // so neither half of the rule fires — the share matters too.
    let hint = tokenade_core::disclose::SourceHint::Shell;
    let out = crate::wrap::attach_drop_recovery("shown".to_string(), "raw", 200, 100_000, &hint, "auto:grep");
    assert_eq!(out, "shown");
}

#[test]
fn a_big_fold_gets_a_recovery_pointer_that_resolves() {
    // This test's subject resolves the stash directory from the
    // ENVIRONMENT, and it said so nowhere: it took only `ENV_RW_LOCK`, so a
    // parallel test moving `HOME` or `TOKENADE_STASH_DIR` left it with nowhere
    // to write and no hash to append. `ENV_LOCK` outermost, `ENV_RW_LOCK` inside.
    let _env = tokenade_core::platform::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _rw = super::env_guard();
    let raw = (0..4_000)
        .map(|i| format!("src/mod{i}/file.rs:{i}:    let hit = {i};\n"))
        .collect::<String>();
    let hint = tokenade_core::disclose::SourceHint::Shell;
    let out =
        crate::wrap::attach_drop_recovery("folded view".to_string(), &raw, 40_000, 100_000, &hint, "auto:grep");
    let hash = out
        .split("expand-ref ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("")
        .trim_end_matches('`')
        .to_string();
    assert_eq!(hash.len(), 12, "a pointer was emitted: {out}");
    // The promise is the ROUND TRIP, not the banner: the stashed bytes must be
    // the bytes that were folded away.
    let (back, _) = tokenade_core::disclose::read_stash_by_hash(&hash).expect("stash resolves");
    assert!(
        back.contains("src/mod3999/file.rs"),
        "the dropped tail came back"
    );
}

/// The old contract was "a view carrying ANY followable hash is left alone",
/// and this test pinned it with a hash typed by hand. That is the defect
/// since fixed: the hash sitting in the view is `maybe_stash`'s, over what was
/// SHOWN, so leaving the view alone left the raw bytes unrecoverable and a
/// `--grep` over the only hash answered `0 matching` for a line that WAS
/// printed.
///
/// The half that survives is the one this test now pins: a pointer over
/// exactly these bytes still earns no second one. The invented hash moves to
/// the case it really describes — a hash that leads somewhere else — where a
/// pointer to the raw bytes must now be added.
#[test]
fn a_view_whose_pointer_leads_elsewhere_still_earns_one_to_the_raw_bytes() {
    let _env = tokenade_core::platform::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _rw = super::env_guard();
    let hint = tokenade_core::disclose::SourceHint::Shell;
    let raw = (0..4_000)
        .map(|i| format!("out/chunk{i}.txt:{i}: value {i}\n"))
        .collect::<String>();

    let shown = "view [stashed: … `tokenade expand-ref 0123456789ab`]".to_string();
    let out = crate::wrap::attach_drop_recovery(shown.clone(), &raw, 90_000, 100_000, &hint, "auto:grep");
    assert_ne!(
        out, shown,
        "a hash over other bytes left the raw output unrecoverable"
    );
    assert!(out.contains("COMPLETE output"), "{out}");

    // Same bytes → nothing added.
    let hash = tokenade_core::disclose::stash_for_reference(&raw, &hint)
        .expect("stash")
        .0;
    let covered = format!("view [stashed: … `tokenade expand-ref {hash}`]");
    assert_eq!(
        crate::wrap::attach_drop_recovery(covered.clone(), &raw, 90_000, 100_000, &hint, "auto:grep"),
        covered
    );
}

/// #1031 — pre-bypass refusal classifier. All assertions live in
/// one #[test] so they share the env-var state safely (parallel
/// cargo test runs would otherwise race on TOKENADE_FORCE_BYPASS).
/// Guards a `Mutex<()>` against any other test in this module that
/// might come to mutate the same vars in the future.
///
/// Covers: safe-class hits, non-safe-class misses, basename
/// resolution for absolute paths, case-insensitive matching,
/// TOKENADE_FORCE_BYPASS override (and is_truthy semantics —
/// "0"/"false" must NOT count as truthy), legacy STS_FORCE_BYPASS
/// alias.
#[test]
fn raw_reads_a_quoted_command_line_as_a_shell_line() {
    let a = |v: &[&str]| -> (Option<String>, String) {
        let owned: Vec<String> = v.iter().map(|s| (*s).to_string()).collect();
        crate::wrap::raw_invocation(&owned)
    };

    // The shape the scaffolded rules teach — and the one that used to fail
    // with `failed to run 'grep -r x .'`, naming the whole line as a program.
    let (line, prog) = a(&["grep -r x ."]);
    assert_eq!(line.as_deref(), Some("grep -r x ."));
    assert_eq!(
        prog, "grep",
        "the refusal must judge the command, not the line"
    );

    // A pipeline is a command line, not a program. Argv could never run it.
    let (line, prog) = a(&["echo hi | tr a-z A-Z"]);
    assert_eq!(line.as_deref(), Some("echo hi | tr a-z A-Z"));
    assert_eq!(prog, "echo");

    // Argv form is untouched.
    let (line, prog) = a(&["grep", "-r", "x", "."]);
    assert_eq!(line, None);
    assert_eq!(prog, "grep");

    // A single bare program is argv, not a shell line.
    let (line, prog) = a(&["ls"]);
    assert_eq!(line, None);
    assert_eq!(prog, "ls");

    // A real path that happens to contain a space stays a program. Built from
    // a file that exists so the check is the one the code makes, not a guess.
    let dir = std::env::temp_dir().join(format!("tok-raw space-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("prog");
    std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
    let p = exe.to_string_lossy().to_string();
    assert!(
        p.contains(' '),
        "the fixture must contain a space to be the test"
    );
    let (line, prog) = a(&[&p]);
    assert_eq!(line, None, "an existing path wins over the shell reading");
    assert_eq!(prog, p);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn safe_class_bypass_refusal_classifier() {
    // #297 — THE guard, not a private mutex, and it restores on drop so the
    // "immunise against the prior test's leftover state" reset below is no
    // longer load-bearing.
    let mut env = tokenade_core::platform::ScopedEnv::new();
    env.unset("TOKENADE_FORCE_BYPASS");
    env.unset("STS_FORCE_BYPASS");

    // -- Membership: every safe-class command triggers refusal.
    for cmd in &[
        "grep", "egrep", "fgrep", "rg", "ag", "ack", "find", "ls", "wc", "head", "tail", "awk",
        "gawk", "mawk", "sed", "cat",
    ] {
        let msg = super::safe_class_bypass_refusal(cmd)
            .unwrap_or_else(|| panic!("expected refusal for `{cmd}`"));
        assert!(
            msg.contains("expand-ref"),
            "refusal must point at expand-ref (cmd={cmd}, msg={msg})"
        );
        assert!(
            msg.contains(cmd),
            "refusal must name the command (cmd={cmd}, msg={msg})"
        );
        assert!(
            msg.contains("TOKENADE_FORCE_BYPASS"),
            "refusal must surface the override env var (cmd={cmd})"
        );
    }

    // -- The refusal must not claim a losslessness the fold does not have.
    //
    // It said "its compactor is lossless at line granularity" and offered
    // `expand-ref` for "the verbatim bytes". Measured on 1 200 lines /
    // 80 370 bytes: grep folds to 2 lines, cat/sed/awk to 154, and on a
    // 25 933-byte input `expand-ref --raw` returned 16 179 bytes with the
    // dedup collapse already in them. A refusal is the one message that must
    // not overstate — it is the door it closes.
    for cmd in &["grep", "cat", "sed"] {
        let msg = super::safe_class_bypass_refusal(cmd).expect("refusal");
        assert!(
            !msg.contains("lossless"),
            "the refusal claims losslessness again (cmd={cmd}): {msg}"
        );
        assert!(
            !msg.contains("verbatim bytes? Run the command"),
            "the refusal still promises verbatim bytes from expand-ref (cmd={cmd}): {msg}"
        );
        assert!(
            msg.contains("what the FOLD produced"),
            "the refusal must say what the stash actually holds (cmd={cmd}): {msg}"
        );
        assert!(
            msg.contains("exact original bytes"),
            "the refusal must name a route that does return them (cmd={cmd}): {msg}"
        );
    }

    // -- Non-membership: lossy compactors stay bypass-able.
    for cmd in &[
        "curl", "kubectl", "git", "ssh", "docker", "jq", "cargo", "npm", "make",
    ] {
        assert!(
            super::safe_class_bypass_refusal(cmd).is_none(),
            "must NOT refuse `{cmd}` (not in safe class)"
        );
    }

    // -- Basename resolution: absolute paths match on basename.
    assert!(super::safe_class_bypass_refusal("/usr/bin/grep").is_some());
    assert!(super::safe_class_bypass_refusal("/opt/homebrew/bin/rg").is_some());
    // .exe suffix stripped. (Windows-style backslash paths can't
    // be tested portably here — `Path::file_name` on Unix treats
    // the whole `C:\…\sed.exe` as one component and lowercase /
    // trim leaves "c:\\tools\\sed" → no match. Documented limit;
    // Windows builds will parse correctly.)
    assert!(super::safe_class_bypass_refusal("grep.exe").is_some());
    assert!(super::safe_class_bypass_refusal("sed.exe").is_some());

    // -- Case-insensitive: agent might type `tokenade raw GREP`.
    assert!(super::safe_class_bypass_refusal("GREP").is_some());
    assert!(super::safe_class_bypass_refusal("Find").is_some());

    // -- TOKENADE_FORCE_BYPASS=truthy → refusal suppressed.
    env.set("TOKENADE_FORCE_BYPASS", "1");
    assert!(
        super::safe_class_bypass_refusal("grep").is_none(),
        "force-bypass=1 must suppress refusal"
    );
    assert!(super::safe_class_bypass_refusal("find").is_none());

    // -- is_truthy semantics: "0" / "false" / "no" do NOT count.
    env.set("TOKENADE_FORCE_BYPASS", "0");
    assert!(
        super::safe_class_bypass_refusal("grep").is_some(),
        "0 must NOT count as force-bypass"
    );
    env.set("TOKENADE_FORCE_BYPASS", "false");
    assert!(
        super::safe_class_bypass_refusal("grep").is_some(),
        "false must NOT count as force-bypass"
    );
    env.unset("TOKENADE_FORCE_BYPASS");

    // -- Legacy STS_FORCE_BYPASS alias.
    env.set("STS_FORCE_BYPASS", "true");
    assert!(
        super::safe_class_bypass_refusal("grep").is_none(),
        "legacy STS_FORCE_BYPASS=true must override"
    );
    env.unset("STS_FORCE_BYPASS");
}

// #856 — bypass-nudge step logic. The pure function does counter
// accounting + threshold detection; the I/O wrapper handles
// persistence. Pin the state machine so changes to the window or
// threshold semantics show up as test diffs.
#[test]
fn bypass_nudge_starts_at_one_with_no_prior_state() {
    let ((ws, c), nudge) = super::bypass_nudge_step(1_000_000, None, 10);
    assert_eq!(
        (ws, c),
        (1_000_000, 1),
        "first bypass establishes window + count=1"
    );
    assert!(!nudge, "single bypass must not nudge below threshold");
}

#[test]
fn bypass_nudge_increments_within_window() {
    // 1h after the window start, count goes 5 -> 6, still below threshold.
    let now = 1_000_000 + 3_600;
    let ((ws, c), nudge) = super::bypass_nudge_step(now, Some((1_000_000, 5)), 10);
    assert_eq!(ws, 1_000_000, "window preserved while within 24h");
    assert_eq!(c, 6);
    assert!(!nudge);
}

#[test]
fn bypass_nudge_rolls_window_after_24h() {
    // 25h later: reset to (now, 1). Previous count of 99 is discarded.
    let now = 1_000_000 + 25 * 3_600;
    let ((ws, c), nudge) = super::bypass_nudge_step(now, Some((1_000_000, 99)), 10);
    assert_eq!((ws, c), (now, 1), "window reset after >24h");
    assert!(!nudge, "fresh window starts below threshold");
}

// #856 — e2e test of the file-I/O wrapper. Uses a tempdir HOME +
// synthetic clock so the test doesn't mutate process env or
// depend on wall-clock time. Pins (a) file format `<ws> <count>`,
// (b) the counter file is created on first bump, (c) crossing
// the threshold flips the return from None to Some, (d) past
// threshold keeps nudging.
#[test]
fn a_bypass_in_any_segment_is_a_bypass() {
    let seg = |c: &str| super::bypassed_segment(c).is_some();
    assert!(seg("TOKENADE_HOOK_DISABLED=1 cargo test"), "leading prefix");
    assert!(
        seg("cd /repo\nTOKENADE_HOOK_DISABLED=1 cargo test"),
        "second line"
    );
    assert!(seg("ls && TOKENADE_HOOK_DISABLED=1 cat f"), "after &&");
    // NOT a pipe stage. The wrap path wraps a pipeline as ONE unit, so a
    // prefix on a later stage is not honoured there either — counting it
    // would report a bypass that did not happen. The counter must agree
    // with what the wrap path actually did, which is the whole point.
    assert!(
        !seg("a | TOKENADE_HOOK_DISABLED=1 grep x"),
        "a pipe is one unit"
    );
    assert!(seg("STS_HOOK_DISABLED=1 ls; echo done"), "legacy alias");
    // The value still has to mean yes, and the var still has to be ours.
    assert!(
        !seg("cd /repo\nTOKENADE_HOOK_DISABLED=0 cargo test"),
        "falsy"
    );
    assert!(!seg("cd /repo\nSOMETHING_ELSE=1 cargo test"), "other var");
    assert!(!seg("cd /repo\ncargo test"), "no bypass anywhere");
    // Naming it is not requesting it.
    assert!(!seg("grep TOKENADE_HOOK_DISABLED=1 notes.txt"));
}

#[test]
fn bump_bypass_counter_at_creates_file_and_crosses_threshold() {
    let tmp = std::env::temp_dir().join(format!(
        "tokenade-nudge-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let now = 1_000_000_u64;

    // First bump: no file → count=1, below threshold → None.
    assert_eq!(super::bump_bypass_counter_at(&tmp, now), None);
    let after_first = std::fs::read_to_string(tmp.join(".tokenade/bypass-counter")).unwrap();
    assert_eq!(after_first.trim(), format!("{now} 1"));

    // Bumps 2..=9: still None, file count reflects.
    for i in 2..=9 {
        assert_eq!(
            super::bump_bypass_counter_at(&tmp, now + i),
            None,
            "bump {i}"
        );
    }
    // Bump 10: at threshold → Some(10).
    assert_eq!(super::bump_bypass_counter_at(&tmp, now + 10), Some(10));
    // Bump 11: past threshold → still Some, count climbs.
    assert_eq!(super::bump_bypass_counter_at(&tmp, now + 11), Some(11));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn format_bypass_count_check_fresh_window_renders_lt_1h() {
    let s = super::format_bypass_count_check(3, 600);
    assert!(s.contains("in window of <1h"), "got: {s}");
    assert!(
        !s.contains(" — "),
        "no suffix when below threshold + not opted out: {s}"
    );
}

#[test]
fn format_bypass_count_check_hourly_window_renders_nh() {
    let s = super::format_bypass_count_check(5, 5 * 3600);
    assert!(s.contains("in window of 5h"), "got: {s}");
}

#[test]
fn format_bypass_count_check_past_threshold_renders_nudge_note() {
    let s = super::format_bypass_count_check(15, 3600);
    assert!(s.contains("past threshold"), "got: {s}");
    assert!(s.contains("nudge is firing"), "got: {s}");
}

#[test]
fn format_bypass_count_check_expired_window_renders_expired_note() {
    // age > 24h: the count is stale (next bump resets the window),
    // so "in window of 168h" + "past threshold" would both lie.
    // Show a dedicated expired-note and DROP the threshold suffix
    // even when count is well past 10.
    let s = super::format_bypass_count_check(42, 7 * 86_400);
    assert!(s.contains("expired"), "got: {s}");
    assert!(s.contains("next bypass resets"), "got: {s}");
    // The note is its own sentence. Slotting it into `in window of {…}`
    // produced "in window of expired (24h window passed; …)" on a real
    // install — a health line the reader has to re-parse.
    assert!(
        !s.contains("in window of"),
        "expired must not reuse the age slot: {s}"
    );
    assert!(
        !s.contains("past threshold"),
        "expired hides threshold note: {s}"
    );
    assert!(!s.contains("168h"), "expired hides raw hour count: {s}");
}

// #856 — lock-in for `prune_stale_bypass_counter`. The 3 branches
// are (a) no file → noop, (b) fresh window → preserve, (c) stale
// window → remove. Same tempdir+synthetic-clock pattern as the
// bump_bypass test above; no env or wall-clock mutation.
#[test]
fn prune_stale_bypass_counter_handles_all_three_cases() {
    let tmp = std::env::temp_dir().join(format!(
        "tokenade-prune-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(tmp.join(".tokenade")).unwrap();
    let path = tmp.join(".tokenade/bypass-counter");
    let now = 1_000_000_u64;

    // (a) No file: returns Ok(false), no panic.
    assert!(!super::prune_stale_bypass_counter(&tmp, now).unwrap());

    // (b) Fresh window (1h old, still within 24h): preserve.
    std::fs::write(&path, format!("{} 5\n", now - 3600)).unwrap();
    assert!(!super::prune_stale_bypass_counter(&tmp, now).unwrap());
    assert!(path.exists(), "fresh window must NOT be removed");

    // (c) Stale window (25h old, past 24h): remove.
    std::fs::write(&path, format!("{} 42\n", now - 25 * 3600)).unwrap();
    assert!(super::prune_stale_bypass_counter(&tmp, now).unwrap());
    assert!(!path.exists(), "stale window must be removed");

    // Idempotency: after removal, another prune call is again Ok(false).
    assert!(!super::prune_stale_bypass_counter(&tmp, now).unwrap());

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn bypass_nudge_fires_at_and_past_threshold() {
    // From 9 -> 10 should nudge (>= threshold). And 10 -> 11 also nudges.
    let ((_, c1), nudge1) = super::bypass_nudge_step(1_000_001, Some((1_000_000, 9)), 10);
    assert_eq!(c1, 10);
    assert!(nudge1, "crossing threshold must nudge");
    let ((_, c2), nudge2) = super::bypass_nudge_step(1_000_002, Some((1_000_000, 10)), 10);
    assert_eq!(c2, 11);
    assert!(
        nudge2,
        "past threshold must keep nudging — agent should keep noticing"
    );
}

/// #977 — lock-in for the bash one-shot env-prefix bypass parser.
/// Pre-fix: `TOKENADE_HOOK_DISABLED=1 <cmd>` had no effect because
/// the env var was set for the child only, never reaching the
/// hook process. The parser is the only way to honor the docs'
/// "one-shot escape" claim — its grammar must stay exact.
/// A bypass whose command IS tokenade bypassed nothing.
///
/// `TOKENADE_HOOK_DISABLED=1 tokenade wrap '<cmd>'` turns the HOOK off because
/// tokenade is being run directly; the output is compacted either way. Both
/// call sites already ended with `if cmd.starts_with("tokenade ") { return 0 }`
/// ("don't double-wrap") — the nudge tick just ran first and returned before
/// reaching it, so the identical command counted as a bypass WITH the env
/// prefix and not without. Measured 2026-08-04: 19 of 19 bypasses in a 13 h
/// window were `TOKENADE_HOOK_DISABLED=1 …/tokenade wrap …`, which fired the
/// over-bypass nudge at an agent doing the right thing.
#[test]
fn a_bypass_that_runs_tokenade_is_not_a_bypass() {
    use crate::wrap::invokes_tokenade_itself;
    // The env prefix is still a bypass REQUEST — the detector stays honest,
    // because skipping hook processing for these is correct.
    assert!(cmdline_requests_hook_bypass(
        "TOKENADE_HOOK_DISABLED=1 tokenade wrap 'git status'"
    ));
    // ...but it must not be COUNTED as one.
    for cmd in [
        "TOKENADE_HOOK_DISABLED=1 tokenade wrap 'git status'",
        "TOKENADE_HOOK_DISABLED=1 ./target/release/tokenade wrap 'git log'",
        "TOKENADE_HOOK_DISABLED=1 /home/u/.local/bin/tokenade expand-ref abc123abc123",
        "LANG=C TOKENADE_HOOK_DISABLED=1 tokenade gain",
        "tokenade wrap 'ls'",
    ] {
        assert!(
            invokes_tokenade_itself(cmd),
            "counted as a real bypass: {cmd}"
        );
    }
    // A genuine bypass — tokenade never runs, the output is never folded.
    for cmd in [
        "TOKENADE_HOOK_DISABLED=1 cat big.log",
        "TOKENADE_HOOK_DISABLED=1 cargo test",
        "LANG=C TOKENADE_HOOK_DISABLED=1 grep -rn foo .",
        // Names that merely START with the same letters are not tokenade.
        "TOKENADE_HOOK_DISABLED=1 tokenade-helper run",
    ] {
        assert!(
            !invokes_tokenade_itself(cmd),
            "a real bypass stopped being counted: {cmd}"
        );
    }
}

#[test]
fn cmdline_requests_hook_bypass_matches_bash_env_prefix_grammar() {
    // Truthy values (case-insensitive) flag bypass.
    assert!(cmdline_requests_hook_bypass(
        "TOKENADE_HOOK_DISABLED=1 cat foo"
    ));
    assert!(cmdline_requests_hook_bypass(
        "TOKENADE_HOOK_DISABLED=true cat foo"
    ));
    assert!(cmdline_requests_hook_bypass(
        "TOKENADE_HOOK_DISABLED=YES cat foo"
    ));
    // Legacy STS alias still honored — mirrors env_compat::is_truthy.
    assert!(cmdline_requests_hook_bypass("STS_HOOK_DISABLED=1 cat foo"));
    // Multiple env prefixes — bypass var anywhere in the prefix
    // chain still flags. Bash semantics: every KEY=VAL token
    // before the first non-env token IS an env-prefix.
    assert!(cmdline_requests_hook_bypass(
        "LANG=C TOKENADE_HOOK_DISABLED=1 cat foo"
    ));
    assert!(cmdline_requests_hook_bypass(
        "TOKENADE_HOOK_DISABLED=1 LANG=C cat foo"
    ));

    // Falsy / empty values are no-ops.
    assert!(!cmdline_requests_hook_bypass(
        "TOKENADE_HOOK_DISABLED=0 cat foo"
    ));
    assert!(!cmdline_requests_hook_bypass(
        "TOKENADE_HOOK_DISABLED=false cat foo"
    ));
    assert!(!cmdline_requests_hook_bypass(
        "TOKENADE_HOOK_DISABLED= cat foo"
    ));

    // No env-prefix at all — never flag.
    assert!(!cmdline_requests_hook_bypass("cat foo"));
    assert!(!cmdline_requests_hook_bypass(""));
    // Var-name in argument position (not env-prefix): the first
    // token `echo` breaks the scan, the literal in argv is ignored.
    assert!(!cmdline_requests_hook_bypass(
        "echo TOKENADE_HOOK_DISABLED=1"
    ));
    // Quoted prefix is NOT a bash env-prefix — it's argv. Our
    // split_whitespace keeps the quotes attached, so the key
    // `\"TOKENADE_HOOK_DISABLED` no longer matches.
    assert!(!cmdline_requests_hook_bypass(
        "\"TOKENADE_HOOK_DISABLED=1\" cat foo"
    ));
    // Compound chain: the env-prefix is mid-chain, not leading.
    // First token `cd` breaks the scan → no bypass. Conservative
    // and matches the bash grammar (env-prefix only applies to
    // the immediately following command).
    assert!(!cmdline_requests_hook_bypass(
        "cd foo && TOKENADE_HOOK_DISABLED=1 cat bar"
    ));
}

/// The stash banner for a COMMAND must not send an exact-value lookup back
/// through a command tokenade refuses to run.
///
/// It used to read `EXACT value/line → grep the original source (re-run via
/// `tokenade raw <cmd>` if needed)`. But `tokenade raw` refuses the whole
/// safe class — `grep`, `cat`, `sed`, `wc`, `ls`, `find`, `head`, `tail`,
/// `awk` — with "refusing bypass for `X`", and points back at `expand-ref`.
/// Those are exactly the programs whose output gets stashed, so the advice
/// spent a turn to be told to come back where it started. `--grep` answers
/// the same question without re-executing anything, and a miss on
/// it names what it searched rather than reading as "never printed".
#[test]
fn the_command_stash_banner_does_not_point_at_a_refused_rerun() {
    use tokenade_core::disclose::SourceHint;
    // The two facts in one test: what the banner says, and what `raw` does
    // with the advice it used to give. The programs are named HERE rather than
    // read from `SAFE_CLASS_FOR_BYPASS_REFUSAL`, because asking that list
    // whether it contains its own members is a tautology — a mutation that
    // emptied it down to two entries passed. These are the shapes whose output
    // actually reaches a stash.
    for prog in [
        "grep", "cat", "sed", "awk", "find", "ls", "wc", "head", "tail", "rg",
    ] {
        assert!(
            crate::safe_class_bypass_refusal(prog).is_some(),
            "`tokenade raw {prog}` must refuse — the banner rule below depends on it"
        );
    }
    let dir = std::env::temp_dir().join(format!(
        "tk-banner-{}",
        tokenade_core::platform::now_unix_secs()
    ));
    let _ = std::fs::create_dir_all(&dir);
    // Env vars are process-wide and this binary's tests run in parallel:
    // take the shared lock rather than racing whoever else reads the stash
    // directory.
    let _env = tokenade_core::platform::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let prev = std::env::var_os("TOKENADE_STASH_DIR");
    // SAFETY: env vars are process-wide; ENV_LOCK is held above.
    unsafe { std::env::set_var("TOKENADE_STASH_DIR", &dir) };
    let big = "line of ordinary output\n".repeat(4000);
    let out = tokenade_core::disclose::maybe_stash(big, SourceHint::Shell);
    match prev {
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        Some(p) => unsafe { std::env::set_var("TOKENADE_STASH_DIR", p) },
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        None => unsafe { std::env::remove_var("TOKENADE_STASH_DIR") },
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.contains("[stashed:"),
        "expected a stash banner, got:\n{out}"
    );
    assert!(
        !out.contains("tokenade raw"),
        "the banner must not recommend a command `raw` refuses, got:\n{out}"
    );
    assert!(
        out.contains("--grep"),
        "the banner must name the verb that answers without re-running, got:\n{out}"
    );
}

/// Which spellings of a recovery command the fold must step aside for.
///
/// The bare `tokenade expand-ref …` is not the interesting case — the
/// PreToolUse hook never wraps it. What reaches `wrap` is the spelling with
/// something in front, and all three launchers below were measured being
/// rewritten into a wrap on 2026-09-03.
#[test]
fn every_spelling_of_the_recovery_command_is_recognised() {
    for cmd in [
        "tokenade expand-ref a663c68674b4",
        "timeout 60 tokenade expand-ref a663c68674b4 --raw",
        "env FOO=1 tokenade expand-ref a663c68674b4",
        "nice /home/u/.local/bin/tokenade expand-ref a663c68674b4 --grep err",
        // The three names main.rs dispatches to `cmd_expand_ref`. Recognising
        // two of three would leave a recovery path folded and nothing would say
        // which one.
        "tokenade expand_ref a663c68674b4",
        "tokenade xref a663c68674b4",
        // Flags before the verb must not hide it.
        "tokenade --json expand-ref a663c68674b4",
    ] {
        assert!(
            crate::wrap::recovers_folded_bytes(cmd),
            "not recognised as a recovery command: {cmd}"
        );
    }
    for cmd in [
        "cat payload.txt",
        "tokenade gain",
        "tokenade wrap 'git log'",
        // Names the verb without invoking tokenade at all.
        "grep expand-ref notes.md",
        "git log --grep expand-ref",
    ] {
        assert!(
            !crate::wrap::recovers_folded_bytes(cmd),
            "an ordinary command must still be compacted: {cmd}"
        );
    }
}

/// A file read that folds a lot names the file as the way back — and nothing
/// else, because nothing else is true here.
///
/// No stash is written for a file (the path holds every byte), so there is no
/// hash to hand out and the line must not promise one. Until 2026-09-04 this
/// route said nothing at all: 4.6 MB of log came back as 154 bytes with no
/// next step, while the installed guidance sends the agent looking for a hash
/// in a banner that is never emitted here.
#[test]
fn a_file_read_that_folds_names_the_file_and_promises_no_hash() {
    let out = crate::wrap::nudges::attach_path_recovery(
        "one shaped line".to_string(),
        "/var/log/build.log",
        40_000,
        100_000,
        "auto:log",
    );
    assert!(out.starts_with("one shaped line\n"), "{out}");
    assert!(
        out.contains("/var/log/build.log"),
        "the way back is the path the agent typed: {out}"
    );
    assert!(
        !out.contains("expand-ref"),
        "nothing was stashed, so no hash may be promised: {out}"
    );
    assert!(out.contains("40000 tokens folded away"), "{out}");
    // **The banner NAMES the compactor.** Measured on 758 real transcripts:
    // 99.9 % of the 3 596 folds came out under a generic label, so "which fold
    // is sending the agent to fetch its bytes" had no answer from our own
    // output. The name is the ledger's, not a second vocabulary.
    assert!(
        out.starts_with("one shaped line\n[tokenade:auto:log "),
        "the banner does not name the compactor: {out}"
    );

    // Same bar as the stash route: a sliver dropped out of a large output
    // earns no line at all.
    assert_eq!(
        crate::wrap::nudges::attach_path_recovery("shown".to_string(), "/f", 200, 100_000, "auto:grep"),
        "shown"
    );
}

/// **The banner names the compactor, and an empty name gives the historical form.**
///
/// Measured on 758 real transcripts: 99.9 % of the 3 596 folds came out under a
/// generic label, so "which fold is sending the agent to fetch its bytes" had
/// no answer from our own output. The name is the ledger's (`auto:grep`,
/// `auto:html`, `read_dedup`…), not a second vocabulary: two names for the same
/// thing and the two measurements would stop cross-referencing.
///
/// The second case is the edge: a caller without a name must give
/// `[tokenade: N`, the exact form every transcript carries — never a bare colon
/// nor a name glued to the count.
#[test]
fn the_fold_banner_names_the_compactor() {
    let hint = tokenade_core::disclose::SourceHint::Shell;
    let raw = "x".repeat(40_000);
    for (name, expected) in [("auto:grep", "[tokenade:auto:grep 40000 "), ("", "[tokenade: 40000 ")] {
        let out = crate::wrap::attach_drop_recovery(
            "vue".to_string(),
            &raw,
            40_000,
            100_000,
            &hint,
            name,
        );
        assert!(
            out.contains(expected),
            "name={name:?}: the banner should carry \"{expected}\":\n{out}"
        );
    }
}

/// **The repeated sentence folds, and NO value disappears.**
///
/// `template_fold` had existed for months with a single caller, `npm.rs`, while
/// 57 % of the corpus residue was made of repeated sentences.
#[test]
fn a_repeated_sentence_is_said_once_and_every_value_survives() {
    let view: String = (0..12)
        .map(|i| {
            format!("/var/lib/snapd/snaps/pkg{i}_5.snap on /var/lib/snapd/snap/pkg{i}/5 type squashfs (ro,nodev,relatime,errors=continue)\n")
        })
        .collect();
    let folded = crate::wrap::fold_templates_unless_it_disarms_the_stash(view.clone());
    assert!(folded.len() < view.len(), "nothing was folded");
    assert_eq!(folded.matches("type squashfs (ro,nodev,relatime,errors=continue)").count(), 1);
    // Every value is read back, in order: that is what separates this fold
    // from post-pass #4, which keeps a count and discards the values. The
    // common root is stated ONCE in the template — same rule as the path
    // listing, where a full path is rebuilt by gluing the two halves back
    // together.
    assert!(
        folded.contains("/var/lib/snapd/snaps/{}.snap on /var/lib/snapd/snap/{} type"),
        "the common root was not hoisted:\n{folded}"
    );
    for i in 0..12 {
        assert!(
            folded.contains(&format!("pkg{i}_5 · pkg{i}/5")),
            "value {i} disappeared:\n{folded}"
        );
    }

    // And the fold only PAYS if it gives back at least a tenth: three
    // sentences whose value is almost the whole line give back nothing, and
    // changing the shape for nothing is a cost bytes do not measure.
    // The value is six times the template: reading it back three times costs
    // almost what the three sentences cost.
    let barely: String = (0..3)
        .map(|i| {
            let deep: String = (0..10).map(|d| format!("/segment{i}{d}_of_a_very_long_path")).collect();
            format!("{deep}/leaf{i} checked and consistent with the manifest\n")
        })
        .collect();
    // The fixture REACHES the rule: the fold forms, and it is the
    // profitability gate — not some neighbouring bound — that refuses it.
    assert_ne!(
        tokenade_core::template_fold::fold_templated_runs(&barely),
        barely,
        "the fixture does not reach the fold it tests"
    );
    assert_eq!(
        crate::wrap::fold_templates_unless_it_disarms_the_stash(barely.clone()),
        barely,
        "a fold that does not give back a tenth was delivered anyway"
    );
}

/// **And it must not be what disarms the next stage** — the rule shared with
/// the marker fold.
///
/// MEASURED 2026-09-08: placed at the FIRST stage, this fold takes the
/// delivered corpus from 237 180 to 238 680 bytes. It gains 2 179 on
/// `tasklist`, 2 370 on `mount` and 1 421 on `ps aux`, and loses 10 281 on
/// `ls -la /usr/bin` alone, whose shortened view drops under the bar and stops
/// being summarised.
#[test]
fn a_template_fold_that_would_drop_the_view_under_the_stash_bar_is_refused() {
    let threshold = tokenade_core::disclose::threshold_tokens();
    // The part that VARIES must be a recognised value — a path here. A first
    // version varied `libpkg{i}`, which nothing recognises as a value: each
    // line had its own template and the fold did not fire. A fixture that
    // does not reach the rule it tests is green for nothing.
    let build = |n: usize| -> String {
        (0..n)
            .map(|i| format!("Setting up /var/lib/pkg/libpkg{i}.deb over the wire and into the tree ...\n"))
            .collect()
    };
    // A view JUST above the threshold, which the fold would push below it.
    let mut n = 1usize;
    let borderline = loop {
        let v = build(n);
        // The SAME fold as the function under test, and with a MARGIN: sought
        // at exact equality, the bar is crossed the other way as soon as the
        // legend adds two hundred bytes, and the fixture lands on the other
        // case than the one it tests.
        let folded = tokenade_core::template_fold::fold_if_it_pays(&v);
        if tokenade_core::tokenizer::estimate(&v) >= threshold
            && tokenade_core::tokenizer::estimate(&folded) * 10 < threshold * 8
        {
            break v;
        }
        n += 1;
        assert!(n < 20_000, "no size puts the view astride the bar");
    };
    assert_eq!(
        crate::wrap::fold_templates_unless_it_disarms_the_stash(borderline.clone()),
        borderline,
        "the fold disarmed the stash"
    );
    // Well above the bar AFTER the fold: there it applies.
    let big = build(n * 20);
    let folded = crate::wrap::fold_templates_unless_it_disarms_the_stash(big.clone());
    assert!(folded.len() < big.len(), "the fold should have applied");
    assert!(tokenade_core::tokenizer::estimate(&folded) >= threshold);
}

/// **The fold must never be what disarms the next stage.**
///
/// Regular sampling pays for its marker at every gap, and saying it once
/// gives back a lot: on `pacman -Q`, what the agent receives goes from 8 726
/// to 5 136 bytes. But the first stage cannot know what the second will do:
/// placed there, this same fold takes the 265 captures from 240 776 to
/// 247 330 delivered bytes, and the whole loss sits in `ls -la /usr/bin`,
/// which goes from 3 585 to 13 729 because the shortened view is no longer
/// summarised.
#[test]
fn a_uniform_fold_that_would_drop_the_view_under_the_stash_bar_is_refused() {
    use tokenade_core::elision::marker;
    let threshold = tokenade_core::disclose::threshold_tokens();
    // A view JUST above the threshold, whose markers weigh enough to push it
    // below. That is the case measured on `ls -la /usr/bin`.
    let build = |content_lines: usize, markers: usize| {
        let mut s = String::new();
        for i in 0..content_lines {
            s.push_str(&format!("-rw-r--r-- 1 root root {i} 2026-09-08 file-name-{i}\n"));
        }
        for _ in 0..markers {
            s.push_str(&marker(20));
            s.push('\n');
        }
        s
    };
    // What this process's passes would really have folded: 400 gaps of 20
    // lines. The fold can only claim that much.
    const CEILING: usize = 400 * 20;
    // Enough markers for their removal to cross the bar.
    let mut content = 1usize;
    let borderline = loop {
        let v = build(content, 400);
        let folded =
            tokenade_core::elision::fold_uniform_markers(&v, CEILING).expect("one gap dominates");
        // With a MARGIN, for the same reason: at exact equality, one more line
        // makes the fixture land on the other case.
        if tokenade_core::tokenizer::estimate(&v) >= threshold
            && tokenade_core::tokenizer::estimate(&folded) * 10 < threshold * 8
        {
            break v;
        }
        content += 1;
        assert!(content < 4_000, "no size puts the view astride the bar");
    };
    let kept = crate::wrap::fold_uniform_unless_it_disarms_the_stash(borderline.clone(), CEILING);
    // The message carries the three numbers: without them, this test cost six
    // reruns to establish that the tree carried a mutation left over from a
    // killed run.
    assert!(
        kept == borderline,
        "the fold disarmed the stash — threshold {threshold}, before {}, after {}",
        tokenade_core::tokenizer::estimate(&borderline),
        tokenade_core::tokenizer::estimate(&kept)
    );

    // And the same view, well above the bar AFTER the fold: there it applies,
    // and that is the whole point.
    let big = build(6_000, 400);
    let folded = crate::wrap::fold_uniform_unless_it_disarms_the_stash(big.clone(), CEILING);
    assert_ne!(folded, big, "the fold should have applied");
    assert!(folded.len() < big.len());
    assert_eq!(folded.matches(&marker(20)).count(), 0);
    // What the view REPRESENTS stays readable for the next stage: without
    // that, it loses its summary and its recovery pointer.
    assert_eq!(
        tokenade_core::elision::weight_of(folded.lines().next().unwrap()),
        Some(8_000)
    );

    // A view already under the bar loses nothing by being folded: the rule
    // forbids MAKING it cross the line, not being below it.
    let small = build(2, 20);
    assert!(
        tokenade_core::tokenizer::estimate(&small) < threshold,
        "the low fixture must be under the bar"
    );
    let folded_small = crate::wrap::fold_uniform_unless_it_disarms_the_stash(small.clone(), 20 * 20);
    assert_ne!(folded_small, small, "a view already low must be folded");
}

/// A cut value is a loss whatever its weight: `cat messages.json` returned 30
/// of its 63 texts truncated (`…[+153 chars]`) for ~1 100 tokens removed,
/// under both thresholds — and with no way back.
#[test]
fn a_cut_value_earns_the_way_back_under_the_token_gate() {
    let cut = r#"{"body":"Hi there, your point that serving LLMs…[+153 chars]","platform":"bluesky"}"#;
    let out = crate::wrap::nudges::attach_path_recovery(cut.to_string(), "/tmp/lot/dm.json", 200, 100_000, "auto:json");
    assert!(out.contains("/tmp/lot/dm.json"), "{out}");
    let whole = r#"{"body":"Hi there","platform":"bluesky"}"#;
    assert_eq!(
        crate::wrap::nudges::attach_path_recovery(whole.to_string(), "/tmp/lot/dm.json", 200, 100_000, "auto:json"),
        whole,
        "nothing cut, small drop: no line"
    );
    assert!(!crate::wrap::nudges::cuts_values("… [+3 more] …"));
    assert!(!crate::wrap::nudges::cuts_values("…[+ chars]"));
}

/// `env` used to cut a long value as `…[+N total]`: a shape `cuts_values` does
/// not read, so a 300-character `PATH` came back truncated with no way back,
/// under the token threshold. The compactor now writes the common shape, and
/// that is the one the gate recognises.
#[test]
fn a_value_cut_by_env_is_seen_as_a_cut() {
    let path = (0..16).map(|i| format!("/opt/tool{i}/bin")).collect::<Vec<_>>().join(":");
    let dump = format!("A_ONE=1\nB_TWO=2\nLONGPATH={path}\nC_THREE=3\n");
    let out = tokenade_core::env::compact(&dump);
    assert_ne!(out, dump, "the value must have been cut");
    assert!(crate::wrap::nudges::cuts_values(&out), "{out}");
}

/// A deliberate read of ONE source file is stashed at a Read's threshold, not
/// at the incidental one that session pressure lowers: 173 lines of a `.ts`
/// file (1 849 tokens) came back as head+tail, then `expand-ref --raw`.
#[test]
fn a_single_source_file_read_is_stashed_like_a_read() {
    let code = std::path::Path::new("src/connectors/access-boundary.service.ts");
    let log = std::path::Path::new("logs/deploy.log");
    let read = tokenade_core::disclose::read_threshold_tokens();
    assert_eq!(crate::wrap::second_stage::stash_threshold_for(Some(code)), read);
    assert_eq!(crate::wrap::second_stage::stash_threshold_for(Some(log)), tokenade_core::disclose::threshold_tokens());
    assert_eq!(crate::wrap::second_stage::stash_threshold_for(None), tokenade_core::disclose::threshold_tokens());
    assert!(read > tokenade_core::disclose::threshold_tokens());
    // Documents and configuration are read on purpose too (measured: `.md`
    // 9/27, `.txt` 7/10, `.yml` and `.toml` 2/2 fetched back); a `.log` 0/2.
    for doc in ["docs/actions-manquantes.md", "scratchpad/extract.txt", "compose/production/donnees.yml", "platforms/reddit.toml"] {
        assert_eq!(crate::wrap::second_stage::stash_threshold_for(Some(std::path::Path::new(doc))), read, "{doc}");
    }
    assert_eq!(crate::wrap::second_stage::stash_threshold_for(Some(std::path::Path::new("run.output"))), tokenade_core::disclose::threshold_tokens());
}
