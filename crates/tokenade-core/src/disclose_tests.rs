// The tests of `crate::disclose`, moved out of the production file.
//
// Mounted by `include!`, not by `mod`: the tokens are spliced INTO
// `disclose`, so every `super::` keeps the meaning it had. Split by
// `srcsweep::production_lines`.

#[cfg(test)]
mod two_accountings_must_not_exceed_the_input {
    use super::*;

    /// A view a compactor already collapsed, then summarised.
    ///
    /// The summary counts lines of the VIEW. When those lines are fold markers
    /// or collapsed runs, that number is not what the reader wants and it does
    /// not compose with the compactor's own banner: measured on a 400-line log,
    /// the compactor said "folded 222", this stage said "elided 260", and
    /// 260 + 222 = 482 came out of 400 lines. Both true, the sum impossible.
    fn collapsed_view(markers: usize, plain: usize, per_marker: usize) -> String {
        // The fixture hand-builds a view that a compactor is PRETENDING to have
        // produced, so it has to declare the folding it stands for. Without
        // this the claim is capped away — which is the whole point of
        // `fold_count::folded_ceiling`: a marker nobody folded is content, and
        // content does not get to say how much of the original is missing.
        crate::fold_count::note_folded(markers * per_marker);
        let mut v: Vec<String> = Vec::new();
        for i in 0..plain {
            v.push(format!("line {i} of the surviving view"));
        }
        for _ in 0..markers {
            v.push(format!("… [{per_marker} lines elided by tokenade] …"));
        }
        v.join("\n")
    }

    /// A line of CONTENT that happens to be shaped like one of our fold
    /// markers is indistinguishable from one we wrote — a saved transcript, a
    /// CI log of a tokenade run, a pasted banner all carry them. Measured
    /// 2026-09-07 before the cap: one such line in a 9 000-line file made the
    /// summary announce "standing for 908969 line(s) of the original output".
    #[test]
    fn content_shaped_like_our_marker_cannot_inflate_the_claim() {
        let mut v: Vec<String> = (0..200).map(|i| format!("line {i} of real content")).collect();
        v[100] = "… [900000 lines elided by tokenade] …".to_string();
        v[101] = "cache hit for shard 4  (×900000)".to_string();
        let text = v.join("\n");
        let out = build_summary(
            &text,
            9_000,
            Path::new("/tmp/x.txt"),
            "txt",
            "abc123",
            &SourceHint::Shell,
        );
        assert!(
            !out.contains("900000") && !out.contains("1800000"),
            "a forged count reached the banner: {out}"
        );
        // The invariant itself, stated where a future reader meets it: the
        // claim never exceeds the view lines cut plus what we really folded.
        let elided = text.lines().count() - SUMMARY_HEAD_LINES - SUMMARY_TAIL_LINES;
        let marker = out
            .lines()
            .find(|l| l.contains("elided"))
            .expect("the summary elided a middle and said nothing");
        if let Some((_, rest)) = marker.split_once("standing for ") {
            let n: usize = rest
                .split_once(" line(s)")
                .and_then(|(n, _)| n.parse().ok())
                .expect("malformed claim");
            assert!(
                n <= elided + crate::fold_count::folded_ceiling(),
                "claim {n} exceeds {elided} cut plus {} folded",
                crate::fold_count::folded_ceiling()
            );
        }
    }

    /// The same forgery with a count near `usize::MAX` used to abort the whole
    /// command — `attempt to add with overflow` inside the sum, output lost.
    #[test]
    fn a_forged_count_near_usize_max_does_not_abort_the_command() {
        for forged in [
            format!("… [{} lines elided by tokenade] …", usize::MAX),
            format!("cache hit  (×{})", usize::MAX),
        ] {
            let mut v: Vec<String> =
                (0..200).map(|i| format!("line {i} of real content")).collect();
            for slot in 100..140 {
                v[slot] = forged.clone();
            }
            let out = build_summary(
                &v.join("\n"),
                9_000,
                Path::new("/tmp/x.txt"),
                "txt",
                "abc123",
                &SourceHint::Shell,
            );
            assert!(out.contains("[disclose:summary"), "no summary for {forged}");
        }
    }

    #[test]
    fn the_summary_says_what_the_elided_view_lines_stand_for() {
        // 40 plain + 60 markers of 5 = 100 view lines standing for 40 + 300.
        let text = collapsed_view(60, 40, 5);
        let total_view = text.lines().count();
        assert_eq!(total_view, 100);
        let out = build_summary(
            &text,
            1234,
            std::path::Path::new("/tmp/x.log"),
            "log",
            "deadbeefcafe",
            &SourceHint::Shell,
        );
        let marker = out
            .lines()
            .find(|l| l.contains("elided"))
            .expect("the summary elided a middle and said nothing");
        let elided_view = total_view - SUMMARY_HEAD_LINES - SUMMARY_TAIL_LINES;
        assert!(
            marker.contains(&format!("elided {elided_view} lines")),
            "marker does not name the view count it cut: {marker:?}"
        );
        // What those elided view lines stand for in the ORIGINAL.
        let represented: usize = text
            .lines()
            .skip(SUMMARY_HEAD_LINES)
            .take(elided_view)
            .map(original_weight)
            .sum();
        assert!(
            represented > elided_view,
            "the fixture is not collapsed — it proves nothing ({represented} vs {elided_view})"
        );
        assert!(
            marker.contains(&format!(
                "standing for {represented} line(s) of the original output"
            )),
            "the marker counts VIEW lines and never says what they stand for, so \
             a reader adding it to the compactor's own banner overcounts the \
             input: {marker:?}"
        );
    }

    /// The rescue hands lines BACK, so they are not gone.
    ///
    /// The middle is scanned for error/failure lines and up to
    /// `SUMMARY_RESCUED_ERROR_LINES` of them are printed under the marker. The
    /// first version of the fix above counted the whole middle and forgot
    /// that: measured on a 620-line test log, 571 lines were really missing
    /// and the marker said 591 — over by exactly the 20 just handed back.
    #[test]
    fn the_lines_the_rescue_hands_back_are_not_counted_as_gone() {
        let mut v: Vec<String> = Vec::new();
        for i in 0..40 {
            v.push(format!("tests/case_{i} PASSED"));
        }
        // Error lines in the middle, where the rescue will find them.
        for i in 0..30 {
            v.push(format!("tests/case_f{i} FAILED"));
        }
        for _ in 0..60 {
            v.push("… [5 lines elided by tokenade] …".to_string());
        }
        // The fixture builds a view that a compactor is SUPPOSED to have
        // produced: it must therefore declare the fold it represents, otherwise
        // `fold_count::folded_ceiling` caps the claim — a marker that
        // nobody folded is content. The test passed without
        // this line ONLY because a neighbour in the same process had
        // already raised the ceiling; one test per process showed it.
        crate::fold_count::note_folded(60 * 5);
        let text = v.join("\n");
        let out = build_summary(
            &text,
            1234,
            std::path::Path::new("/tmp/x.log"),
            "log",
            "deadbeefcafe",
            &SourceHint::Shell,
        );
        let marker = out
            .lines()
            .find(|l| l.contains("elided"))
            .expect("no marker");
        assert!(
            marker.contains("kept below"),
            "the fixture did not trigger the rescue, so it proves nothing: {marker:?}"
        );
        // What the answer hands back, weighed the same way — a marker kept in
        // the TAIL is delivered and still stands for its lines. Counting it as
        // one (or as nothing) is the very confusion this guard is about, and
        // the first draft of this test made it.
        let delivered: usize = out
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !t.starts_with('[') && !t.is_empty() && !t.contains("(elided")
            })
            .map(original_weight)
            .sum();
        // Original magnitude of the whole view.
        let original: usize = text.lines().map(original_weight).sum();
        let missing = original - delivered;
        assert!(
            marker.contains(&format!(
                "standing for {missing} line(s) of the original output"
            )),
            "the marker counts the rescued lines as gone while printing them \
             below itself (really missing: {missing}): {marker:?}"
        );
    }

    /// The rescue names its own shortfall.
    ///
    /// It takes at most `SUMMARY_RESCUED_ERROR_LINES` and the sentence said
    /// "the error/failure lines among them are kept below" — definite article,
    /// no number. Measured 2026-09-06 on 30 DISTINCT failures buried in a
    /// heterogeneous 810-line log: 20 were taken and the sentence claimed all
    /// of them, so an agent counting what it saw would have counted the
    /// failures wrong.
    #[test]
    fn the_rescue_says_how_many_of_the_failures_it_kept() {
        let build = |errors: usize| {
            let mut v: Vec<String> = (0..40).map(|i| format!("step {i} ok")).collect();
            for j in 0..errors {
                v.push(format!("error: task {j} failed with code {j}"));
            }
            v.extend((0..40).map(|i| format!("tail step {i} ok")));
            build_summary(
                &v.join("\n"),
                1234,
                std::path::Path::new("/tmp/x.log"),
                "log",
                "deadbeefcafe",
                &SourceHint::Shell,
            )
        };
        // Under the cap: the plain sentence, because it is then true.
        let few = build(5);
        let m = few.lines().find(|l| l.contains("elided")).expect("no marker");
        assert!(
            m.contains("kept below"),
            "the rescue did not fire on the fixture: {m:?}"
        );
        assert!(
            !m.contains(" of the "),
            "claimed a shortfall where the rescue took them all: {m:?}"
        );
        // Over the cap: the numbers, both of them.
        let many = build(30);
        let m = many.lines().find(|l| l.contains("elided")).expect("no marker");
        assert!(
            m.contains(&format!("{SUMMARY_RESCUED_ERROR_LINES} of the 30 error/failure lines")),
            "the rescue kept {SUMMARY_RESCUED_ERROR_LINES} of 30 failures and did \
             not say so, so a reader counts the failures wrong: {m:?}"
        );
    }

    /// The preview may lose CONTEXT; it may not lose the rescue.
    ///
    /// `clamp_summary_body` keeps the first two thirds and the last third of
    /// the characters, and the rescued failures sit in the middle — the slice
    /// it drops. Measured 2026-09-06 on 25 distinct failures: the marker said
    /// "20 … are kept below" and the answer held FIVE. The sentence was true of
    /// the rescue and false of the answer.
    #[test]
    fn the_preview_clamp_does_not_eat_the_rescued_failures() {
        // Head and tail lines long enough that the old clamp had to cut, and
        // it cut the middle.
        let filler = "x".repeat(160);
        let mut v: Vec<String> = (0..40).map(|i| format!("step {i} ok {filler}")).collect();
        for j in 0..25 {
            v.push(format!("error: task {j} failed with code {j} MARKER{j}END"));
        }
        v.extend((0..40).map(|i| format!("tail step {i} ok {filler}")));
        let out = build_summary(
            &v.join("\n"),
            1234,
            std::path::Path::new("/tmp/x.log"),
            "log",
            "deadbeefcafe",
            &SourceHint::Shell,
        );
        // Anti-vacuity: the body must really have been over the bound, or the
        // clamp never ran and this proves nothing.
        assert!(
            out.contains("preview clamped"),
            "the clamp did not fire on this fixture, so the guard is vacuous"
        );
        let marker = out
            .lines()
            .find(|l| l.contains("error/failure lines among them are kept"))
            .expect("no rescue sentence");
        // The count the RESCUE names, not the elision count that opens the same
        // line — the first draft of this test took the first number it saw and
        // compared 20 delivered lines against "elided 75".
        let claimed: usize = {
            let at = marker
                .find(" of the ")
                .expect("the sentence names no rescue count");
            marker[..at]
                .split_whitespace()
                .next_back()
                .and_then(|w| w.parse::<usize>().ok())
                .expect("the token before `of the` is not a count")
        };
        let delivered = (0..25)
            .filter(|j| out.contains(&format!("MARKER{j}END")))
            .count();
        assert_eq!(
            delivered, claimed,
            "the answer says {claimed} failure lines are kept below and holds \
             {delivered}: the clamp ate what the rescue saved\n{marker}"
        );
        // Honest is not enough: shrinking the rescue to one line and SAYING
        // "1 of the 25" also satisfies the equality above, and that mutation
        // survived until this line existed. These 25 failures are short enough
        // that the whole cap fits the rescue budget, so the cap is what must
        // decide.
        assert_eq!(
            claimed, SUMMARY_RESCUED_ERROR_LINES,
            "25 short failure lines fit the rescue budget, so the cap should \
             decide and {SUMMARY_RESCUED_ERROR_LINES} should be kept — the \
             answer kept {claimed}"
        );
    }

    #[test]
    fn a_view_nothing_collapsed_gets_no_such_clause() {
        // Anti-vacuity in the other direction: the phrase must not appear when
        // there is nothing behind the elided lines but themselves.
        let text = collapsed_view(0, 120, 0);
        let out = build_summary(
            &text,
            1234,
            std::path::Path::new("/tmp/x.log"),
            "log",
            "deadbeefcafe",
            &SourceHint::Shell,
        );
        let marker = out.lines().find(|l| l.contains("elided")).expect("no marker");
        assert!(
            marker.contains("elided 90 lines"),
            "unexpected marker: {marker:?}"
        );
        assert!(
            !marker.contains("standing for"),
            "claimed a collapsed magnitude on a view that collapsed nothing: {marker:?}"
        );
    }

    #[test]
    fn a_banner_line_stands_for_nothing_and_a_run_for_its_whole_count() {
        assert_eq!(original_weight("plain line"), 1);
        assert_eq!(original_weight("… [7 lines elided by tokenade] …"), 7);
        assert_eq!(original_weight("Nov 3 sshd accepted  (×47)"), 47);
        assert_eq!(original_weight("[tokenade folded 5 line(s) total — x]"), 0);
        assert_eq!(original_weight("[disclose:summary kind=log]"), 0);
        // `(×1)` is not a run.
        assert_eq!(original_weight("a line  (×1)"), 1);
    }
}
#[cfg(test)]
mod stash_pointer_line_tests {
    /// Two halves, one per line so a single mutation cannot pass both.
    /// LAST wins: the body a fold serves can itself quote a `[stashed: …]`
    /// line (a page repeating one of our own answers), and the pointer that
    /// belongs to THIS fold is the one appended last. Naming the quoted one
    /// hands the caller a hash that resolves to somebody else's bytes, or to
    /// nothing at all.
    /// And it must be the line's OWN opening: a sentence merely mentioning
    /// the marker mid-line is prose, not a pointer.
    #[test]
    fn takes_the_last_pointer_and_only_at_the_start_of_a_line() {
        let quoted = "[stashed: /tmp/quoted.md \u{b7} `tokenade expand-ref deadbeefdead`]";
        let ours = "[stashed: /tmp/ours.md \u{b7} `tokenade expand-ref 0123456789ab`]";
        assert_eq!(
            super::stash_pointer_line(&format!("{quoted}\nbody\n{ours}")),
            Some(ours)
        );
        assert_eq!(
            super::stash_pointer_line(&format!("we printed {ours} earlier\nbody")),
            None
        );
    }
}
#[cfg(test)]
mod tests {
    use std::fmt::Write;

    /// Over the delivery threshold a diff is delivered as its list of files.
    /// The head-and-tail fallback named one file of 36 on a real `diff -r`.
    #[test]
    fn a_diff_too_big_to_deliver_comes_back_as_its_plan() {
        let (_g, _drop, _stash) = isolated_stash();
        let mut t = String::new();
        for f in 0..40 {
            let _ = writeln!(t, "diff -r old/src/m{f}.rs new/src/m{f}.rs");
            for h in 0..12 {
                let _ = writeln!(t, "{}c{}\n< let a{h} = compute_{f}_{h}(old, {h});\n---\n> let a{h} = compute_{f}_{h}(new, {h}, opts);", h * 9 + 3, h * 9 + 3);
            }
        }
        assert!(crate::tokenizer::estimate(&t) > crate::disclose::threshold_tokens());
        let out = maybe_stash(t, SourceHint::Shell);
        assert!(out.contains("mode=diff-plan"), "{}", &out[..out.len().min(300)]);
        for f in 0..40 {
            assert!(out.contains(&format!("new/src/m{f}.rs  +12 −12")), "m{f} missing");
        }
        // The footer says what the body is: an outline holds no line of the
        // diff, so "omits middle lines" would send a grep to an empty place.
        assert!(out.contains("outline only (no line of the body)"), "{}", &out[out.len().saturating_sub(400)..]);
        assert!(!out.contains("omits middle lines"));
        // A head-and-tail preview keeps its own phrase.
        let log: String = (0..3_000).map(|i| format!("2026-10-02T10:{:02}:{:02} worker-{} step {i} ok\n", i / 60 % 60, i % 60, i % 7)).collect();
        let out = maybe_stash(log, SourceHint::Shell);
        assert!(out.contains("folded summary (omits middle lines)"), "{}", &out[out.len().saturating_sub(400)..]);
    }


    /// The sweep belongs where the stash GROWS, and nowhere else.
    ///
    /// It used to sit at the top of `maybe_stash`, `maybe_stash_bytes` and
    /// `stash_for_reference` — all three BEFORE the size threshold that
    /// declines to stash at all. Every hook is a fresh process, so the
    /// `OnceLock` amortised nothing and a three-byte `echo hi` paid a full walk
    /// of the stash directory: 9.9 ms of the 30 ms `tokenade wrap` spent.
    ///
    /// Behaviour can't catch a regression here (the sweep is a no-op under this
    /// crate's `cfg!(test)`, precisely so its suite stops touching the
    /// developer's real stash), so guard the placement in the source instead:
    /// every call site must be the line that opens a writer.
    #[test]
    fn the_stash_sweep_only_runs_where_the_stash_grows() {
        // Every file of the module, not just this one — and matched on the CALL
        // rather than on one exact spelling of the line.
        //
        // The previous version searched for the literal
        // `"\n    opportunistic_evict();\n"`: four spaces, no path prefix. From
        // a child module the call can only be written `super::…`, so it matched
        // nothing there. MUTATION-TESTED both ways: a third sweep inserted into
        // `eviction.rs` left the old assertion green, and fails this one.
        // A guard that cannot fail is indistinguishable from a guard that passes.
        let files = [
            &crate::source_guard::library_module_family("disclose"),
            include_str!("disclose/binary.rs"),
            include_str!("disclose/eviction.rs"),
            include_str!("disclose/kind.rs"),
            include_str!("disclose/search.rs"),
        ];
        let mut sites: Vec<(usize, String)> = Vec::new();
        for src in files {
            let lines: Vec<&str> = src.lines().collect();
            for (i, l) in lines.iter().enumerate() {
                let t = l.trim();
                // The definition, and this test's own description of the
                // pattern, are not call sites.
                if !t.ends_with("opportunistic_evict();") || t.starts_with("//") {
                    continue;
                }
                let next = lines[i + 1..]
                    .iter()
                    .find(|n| !n.trim().is_empty())
                    .map(|n| n.trim().to_string())
                    .unwrap_or_default();
                sites.push((i + 1, next));
            }
        }
        assert_eq!(
            sites.len(),
            2,
            "expected exactly the two writers (`ensure_stashed`, \
             `ensure_stashed_bytes`) to sweep, found: {sites:?}"
        );
        for (line, next) in sites {
            assert_eq!(
                next, "let dir = stash_dir();",
                "line {line}: a sweep moved off the write path — it must open a \
                 writer, not a read or a threshold check"
            );
        }
    }

    /// The cap stays hard, but it sacrifices the replaceable first. Before
    /// this, a sole copy could be deleted ahead of a blob ten times its size
    /// purely because its mtime was older.
    #[test]
    fn the_size_cap_evicts_replaceable_entries_before_the_last_copy() {
        let base = std::env::temp_dir().join(format!("tokenade-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("stash");
        let keep = base.join("stash-keep");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&keep).unwrap();

        // The sole copy is the OLDEST, so an mtime-only sort would take it.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(9_000);
        let sole = dir.join("aaaaaaaaaaaa.txt");
        std::fs::write(&sole, vec![b'x'; 4_000]).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&sole)
            .unwrap()
            .set_modified(old)
            .unwrap();
        std::fs::write(keep.join("aaaaaaaaaaaa"), b"").unwrap();
        std::fs::write(dir.join("bbbbbbbbbbbb.txt"), vec![b'y'; 4_000]).unwrap();

        // Order the eviction WOULD use, with the same key the cap sorts by.
        let mut names: Vec<(bool, String)> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                let stem = n.split_once('.').map(|(a, _)| a).unwrap_or(&n).to_string();
                (keep.join(&stem).exists(), n)
            })
            .collect();
        names.sort();
        assert!(
            names[0].1.starts_with("bbbb"),
            "the replaceable entry is first in line: {names:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The TTL sweep must not take the last copy of anything.
    #[test]
    fn a_sole_copy_stash_entry_outlives_the_ttl() {
        let dir = std::env::temp_dir().join(format!("tokenade-sole-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let keep = dir.with_file_name("stash-keep");
        let _ = std::fs::remove_dir_all(&keep);

        // Two expired files; one carries a sole-copy marker.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
        for name in ["aaaaaaaaaaaa.txt", "bbbbbbbbbbbb.txt"] {
            let p = dir.join(name);
            std::fs::write(&p, "content").unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(old).unwrap();
        }
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(keep.join("bbbbbbbbbbbb"), b"").unwrap();

        // Drive the same predicate the sweep uses.
        let sole = |stem: &str| keep.join(stem).exists();
        assert!(!sole("aaaaaaaaaaaa"), "an ordinary fold stays evictable");
        assert!(sole("bbbbbbbbbbbb"), "the last copy is protected");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&keep);
    }

    #[test]
    fn a_marker_never_shadows_the_content_it_protects() {
        // The marker lives outside the stash dir on purpose: `read_stash_raw`
        // takes the first file whose stem matches the hash, so a sibling would
        // be served as the content.
        assert_ne!(
            super::keep_marker_dir(),
            super::stash_dir(),
            "markers must not share the directory the resolver scans"
        );
    }


    #[test]
    fn grep_output_and_logs_are_record_streams() {
        let grep: String = (0..40)
            .map(|i| format!("crates/core/src/mod{i}.rs:pub fn thing{i}() {{\n"))
            .collect();
        assert!(super::is_record_stream(&grep));
        let log: String = (0..40)
            .map(|i| format!("worker={i} status=ok latency={i}ms\n"))
            .collect();
        assert!(super::is_record_stream(&log));
    }

    #[test]
    fn prose_and_source_are_not_record_streams() {
        // A question beats a pattern over these, so the banner must not steer
        // the agent to `--grep`.
        let prose = "# Title\n\nA paragraph of ordinary text that runs on.\n".repeat(10);
        assert!(!super::is_record_stream(&prose));
        let source: String = (0..40)
            .map(|i| format!("    let value{i} = compute(i, {i});\n"))
            .collect();
        assert!(!super::is_record_stream(&source));
    }

    #[test]
    fn a_handful_of_lines_is_not_enough_to_call_it_a_stream() {
        assert!(!super::is_record_stream("a:1\nb:2\nc:3\n"));
    }
    use super::*;
    use std::sync::Mutex;

    // ---- SECURITY: sub-threshold passthrough must still redact ----
    // A small MCP/Bash result under the stash threshold used to be returned
    // verbatim (auto::compact does not redact) — a leak. maybe_stash is the
    // single choke point, so redacting the passthrough covers every caller.
    /// Filtering before redacting is a performance fix, and a performance fix
    /// that leaks is a security bug. The emitted lines must be redacted exactly
    /// as if the whole blob had been redacted first — including a line that only
    /// matches BECAUSE of its secret.
    #[test]
    fn grep_stash_filters_before_redacting_but_still_redacts_what_it_emits() {
        let (_g, _cleanup, _dir) = isolated_stash();
        let secret = "sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let body = format!("alpha row\nbeta {secret} row\ngamma row\n");
        let (hash, _p) =
            stash_for_reference(&body, &SourceHint::File("t.md".into())).expect("stash");

        let (hits, _matched, total, toks) = grep_stash_by_hash(&hash, "beta", false).expect("grep");
        assert_eq!(total, 3, "line count wrong");
        assert!(
            toks > 0,
            "blob token estimate missing — recovery credit would be 0"
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, 2, "line number must be 1-based and absolute");
        assert!(
            !hits[0].1.contains(secret),
            "SECRET EMITTED by the filtered path: {}",
            hits[0].1
        );
        // The secret was redacted at WRITE time, so it is not in the file and
        // cannot be matched. Filtering before redacting therefore cannot widen
        // what a needle can reach.
        let (hits, _, _, _) = grep_stash_by_hash(&hash, secret, false).expect("grep");
        assert!(
            hits.is_empty(),
            "a secret should not be greppable: {hits:?}"
        );
        assert!(grep_stash_by_hash("nosuchhash", "x", false).is_none());
    }

    #[test]
    fn maybe_stash_sub_threshold_still_redacts() {
        let secret =
            "AWS_SECRET=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\nAPI_KEY=sk-ant-SECRET123\n";
        // Tiny input → well below any stash threshold → passthrough branch.
        let out = maybe_stash(secret.to_string(), SourceHint::Mcp);
        assert!(
            !out.contains("wJalrXUtnFEMI"),
            "AWS secret leaked on sub-threshold passthrough: {out:?}"
        );
        assert!(
            !out.contains("sk-ant-SECRET123"),
            "anthropic key leaked on sub-threshold passthrough: {out:?}"
        );
        assert!(
            out.contains("<redacted>"),
            "expected redaction marker: {out:?}"
        );
    }

    // ---- #955 prompt_scoped_extract (WebFetch net-negative fix) ----

    #[test]
    fn scoped_extract_keyword_hit_contains_answer_and_context() {
        // nginx-style directive page: the answer "75s" sits one line under
        // the keyword the prompt names. Context window must pull it in.
        let page = "# ngx_http_core_module\n\
                    Some intro paragraph about the module.\n\
                    ## keepalive_timeout\n\
                    Syntax: keepalive_timeout timeout;\n\
                    Default: keepalive_timeout 75s;\n\
                    Sets a timeout during which a keep-alive connection stays open.\n\
                    ## something_else\n\
                    Unrelated directive text here.\n";
        let out = prompt_scoped_extract(page, "default value of keepalive_timeout", 1500)
            .expect("keyword prompt must scope");
        assert!(out.contains("75s"), "answer must survive in excerpt: {out}");
        assert!(out.contains("keepalive_timeout"));
    }

    /// The packing loop admits its FIRST pick regardless of size, so a page
    /// always yields something. Nothing bounded that pick, and on a page whose
    /// units are few and very long the one unit was 11x the budget (MEASURED
    /// 2026-08-01 on `tokenade.net/llms.txt`: budget 500, extract 5 667).
    #[test]
    fn the_first_pick_may_be_admitted_but_not_at_any_size() {
        // Four paragraph-long lines, the answer in the second.
        let long = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(60);
        let text = format!("{long}\nthe keepalive_timeout default is 75s {long}\n{long}\n{long}");
        let budget = 120;
        let out = prompt_scoped_extract(&text, "what is the keepalive_timeout default", budget)
            .expect("a lexical anchor exists");
        let got = crate::tokenizer::estimate(&out);
        assert!(
            got <= budget + 2,
            "extract {got} tokens for a {budget}-token budget:\n{out}"
        );
        // …and it still carries the answer it was asked for.
        assert!(out.contains("keepalive_timeout"), "{out}");
    }

    #[test]
    fn truncate_to_tokens_fits_the_budget_and_marks_the_cut() {
        let text = "alpha beta gamma delta ".repeat(500);
        let cut = truncate_to_tokens(&text, 50);
        assert!(
            crate::tokenizer::estimate(&cut) <= 50,
            "{} tokens, {} chars",
            crate::tokenizer::estimate(&cut),
            cut.chars().count()
        );
        assert!(cut.ends_with('…'));
        // Something that already fits is returned untouched.
        assert_eq!(truncate_to_tokens("short", 50), "short");
    }

    #[test]
    fn scoped_extract_lexical_gap_returns_none() {
        // Adversarial #1: prompt words appear NOWHERE on the page (the page
        // says "keepalive_timeout 75s", the prompt says "idle disconnect").
        // A keyword scoper MUST bail (None) so the caller serves the whole
        // page instead of fabricating a wrong excerpt.
        let page = "## keepalive_timeout\n\
                    Default: keepalive_timeout 75s;\n\
                    Sets how long a keep-alive connection is kept open.\n";
        let out = prompt_scoped_extract(page, "maximum idle disconnect grace period", 1500);
        assert!(out.is_none(), "lexical gap must fall back, got: {out:?}");
    }

    #[test]
    fn scoped_extract_vague_prompt_returns_none() {
        // Adversarial #5: "summarise this page" has no anchor term worth
        // scoping — must fall back rather than emit an arbitrary excerpt.
        let page = "# Title\n\
                    First paragraph of general prose about a topic.\n\
                    Second paragraph continuing the discussion at length.\n\
                    Third paragraph wrapping things up nicely.\n";
        let out = prompt_scoped_extract(page, "please summarise this page", 1500);
        assert!(out.is_none(), "vague prompt must fall back, got: {out:?}");
    }

    #[test]
    fn scoped_extract_decoy_serves_all_candidates() {
        // Adversarial #2: several values sit near the prompt terms; only one
        // is right. The scoper must serve ALL candidate passages (never one
        // hand-picked value) so the model disambiguates.
        let page = "# Planck constant history\n\
                    The 1985 recommended value was 6.626176e-34.\n\
                    An intermediate 2014 value was 6.62606957e-34.\n\
                    The 2019 SI defining value of the Planck constant is 6.62607015e-34.\n\
                    Other unrelated constants follow below here.\n";
        let out =
            prompt_scoped_extract(page, "value of the Planck constant", 1500).expect("must scope");
        // The correct 2019 value must be present …
        assert!(
            out.contains("6.62607015e-34"),
            "correct value missing: {out}"
        );
        // … and the decoys too (all candidates shown, not silently dropped).
        assert!(
            out.contains("6.626176e-34") || out.contains("6.62606957e-34"),
            "decoy candidates should be shown for disambiguation: {out}"
        );
    }

    #[test]
    fn scoped_extract_respects_token_budget() {
        // Budget must actually bound the excerpt on a big matching page.
        // The anchor term appears on only a subset of lines (df ≪ N) so it
        // stays discriminating; filler lines pad the page so budget bites.
        let mut page = String::from("# doc\n");
        for i in 0..500 {
            if i % 12 == 0 {
                page.push_str(&format!(
                    "Row {i}: widget calibration knob controls behaviour.\n"
                ));
            } else {
                page.push_str(&format!(
                    "General prose sentence number {i} on assorted topics.\n"
                ));
            }
        }
        let out = prompt_scoped_extract(&page, "widget calibration", 300).expect("must scope");
        let toks = crate::tokenizer::estimate(&out);
        assert!(
            toks <= 400,
            "excerpt {toks} tok should stay near the 300 budget"
        );
    }

    #[test]
    fn scoped_extract_empty_prompt_returns_none() {
        let page = "line one\nline two\nline three\n";
        assert!(prompt_scoped_extract(page, "", 1500).is_none());
        assert!(prompt_scoped_extract(page, "   ", 1500).is_none());
    }

    // ---- enumeration-aware packing (scattered-list fix) ----

    #[test]
    fn scoped_item_classifier_is_structural() {
        // Items: bullets, ordered, table rows, link-dominated.
        for l in [
            "- [tool](https://x.y) — a thing",
            "* plain star bullet",
            "+ plus bullet",
            "• unicode bullet",
            "  - indented bullet",
            "12. ordered dot entry",
            "3) ordered paren entry",
            "| cell | other cell |",
            "[almost-all-link](https://example.com/path)",
        ] {
            assert!(scoped_line_is_item(l), "should be item: {l:?}");
        }
        // Non-items: prose, headings, rules, emphasis, diff-ish lines.
        for l in [
            "Plain prose sentence about a topic.",
            "# Heading",
            "---",
            "*emphasised* prose continues here at length",
            "-no space after dash",
            "2024 was a year of prose, not a list marker.",
            "see [one link](https://x.y) inside a much longer prose sentence that dominates",
            "",
        ] {
            assert!(!scoped_line_is_item(l), "should NOT be item: {l:?}");
        }
    }

    /// Synthetic 500-entry link list with 12 relevant items scattered
    /// across it: the enumeration path must surface ≥10 within a small
    /// budget, and the OLD pure-window packing must miss most (the
    /// delta is the point — windows burn the budget on ±4 neighbour
    /// entries that scored nothing).
    #[test]
    fn scoped_extract_record_stream_packs_each_matching_line() {
        // A log: every line a record. Ten scattered lines carry the anchor;
        // windows of neighbours used to spend the budget on two of them.
        let mut log = String::new();
        for i in 0..3000 {
            let path = if i % 300 == 7 { 777 } else { i % 700 };
            log.push_str(&format!(
                "2026-10-03T12:00:{:02}.{:03}Z INFO worker-{} request path=/api/v1/items/{path} status=500 dur={}ms\n",
                i % 60, i % 1000, i % 8, i % 900
            ));
        }
        assert!(is_record_stream(&log));
        let out = prompt_scoped_extract(&log, "which requests hit items/777", 600).expect("anchored");
        assert_eq!(out.matches("items/777 ").count(), 10, "{out}");
    }

    #[test]
    fn scoped_extract_enum_dense_packs_scattered_items() {
        let mut page = String::from("# Awesome list\n\n## Entries\n\n");
        let mut needles: Vec<String> = Vec::new();
        for i in 0..500 {
            if i % 41 == 7 && needles.len() < 12 {
                let name = format!("zephyr-telemetry-{i}");
                page.push_str(&format!(
                    "- [{name}](https://github.com/x/{name}) — zephyr telemetry adapter\n"
                ));
                needles.push(name);
            } else {
                page.push_str(&format!(
                    "- [misc-tool-{i}](https://github.com/x/misc-tool-{i}) — generic utility\n"
                ));
            }
        }
        assert_eq!(needles.len(), 12);
        let budget = 300;
        let new = scoped_extract_inner(&page, "zephyr telemetry adapters", budget, true)
            .expect("anchored enum page must scope");
        let old = scoped_extract_inner(&page, "zephyr telemetry adapters", budget, false)
            .expect("old path must also scope");
        let hits = |s: &str| needles.iter().filter(|n| s.contains(n.as_str())).count();
        let (new_hits, old_hits) = (hits(&new), hits(&old));
        assert!(new_hits >= 10, "enum path found {new_hits}/12: {new}");
        assert!(
            old_hits < new_hits && old_hits <= 6,
            "window path should miss scattered items (got {old_hits} vs {new_hits})"
        );
        // Budget still binds.
        let toks = crate::tokenizer::estimate(&new);
        assert!(
            toks <= budget + 100,
            "enum extract {toks} tok blew the {budget} budget"
        );
        // Orientation: nearest heading prefixes the group.
        assert!(new.contains("## Entries"), "heading prefix missing: {new}");
    }

    #[test]
    fn scoped_extract_enum_item_trimmed_and_continuation_merged() {
        // A wrapped item (indented continuation) is one unit, collapsed
        // to a single ≤200-char line.
        let mut page = String::from("# Index\n");
        for i in 0..20 {
            page.push_str(&format!("- [filler-{i}](https://x.y/{i}) — filler entry\n"));
        }
        page.push_str(&format!(
            "- [target-lib](https://x.y/target) — quokka framework binding, {}\n  wrapped continuation line mentioning quokka again\n",
            "very long description ".repeat(20),
        ));
        let out =
            prompt_scoped_extract(&page, "quokka framework binding", 200).expect("must scope");
        let item_line = out
            .lines()
            .find(|l| l.contains("target-lib"))
            .expect("target item must surface");
        assert!(
            item_line.chars().count() <= SCOPED_ITEM_MAX_CHARS + 1,
            "item not trimmed: {} chars",
            item_line.chars().count()
        );
        assert!(
            item_line.ends_with('…'),
            "trimmed item should end with …: {item_line}"
        );
    }

    #[test]
    fn scoped_extract_enum_dense_no_anchor_still_returns_none() {
        // Contract: the anchor gate is unchanged by enum mode — a
        // lexically-blind prompt on a list page must still fall back to
        // whole-page serving (webfetch passthrough), never fabricate.
        let mut page = String::from("# List\n");
        for i in 0..50 {
            page.push_str(&format!(
                "- [entry-{i}](https://x.y/{i}) — common words here\n"
            ));
        }
        let out = prompt_scoped_extract(&page, "yttrium borehole", 1500);
        assert!(
            out.is_none(),
            "no-anchor enum page must return None, got {out:?}"
        );
    }

    #[test]
    fn scoped_extract_prose_page_unaffected_by_enum_mode() {
        // A prose-dominant page (bullets < density gate) must produce
        // byte-identical output with and without enum awareness.
        let mut page = String::from("# Guide\n");
        for i in 0..40 {
            page.push_str(&format!(
                "Paragraph {i} of prose about deployment and rollout strategy at length.\n"
            ));
            if i % 10 == 0 {
                page.push_str(&format!("- occasional bullet {i} aside\n"));
            }
        }
        page.push_str("The canary threshold defaults to 5% of traffic.\n");
        let with_enum = scoped_extract_inner(&page, "canary threshold traffic", 400, true);
        let without = scoped_extract_inner(&page, "canary threshold traffic", 400, false);
        assert_eq!(
            with_enum, without,
            "prose page must be untouched by enum awareness"
        );
        assert!(with_enum.expect("must scope").contains("5%"));
    }

    #[test]
    fn webfetch_scoped_budget_adapts_and_honours_override() {
        // Bounds of clamp(input/16, 500, 1500) — no env set.
        std::env::remove_var("TOKENADE_WEBFETCH_SCOPED_TOKENS");
        assert_eq!(webfetch_scoped_budget_for(0), 500);
        assert_eq!(webfetch_scoped_budget_for(5_000), 500);
        assert_eq!(webfetch_scoped_budget_for(12_000), 750); // just over the gate
        assert_eq!(webfetch_scoped_budget_for(18_000), 1_125);
        assert_eq!(webfetch_scoped_budget_for(24_000), 1_500);
        assert_eq!(webfetch_scoped_budget_for(1_000_000), 1_500); // ceiling
                                                                  // Env override pins the budget exactly (fixed, no adaptivity).
        std::env::set_var("TOKENADE_WEBFETCH_SCOPED_TOKENS", "900");
        assert_eq!(webfetch_scoped_budget_for(1_000_000), 900);
        std::env::remove_var("TOKENADE_WEBFETCH_SCOPED_TOKENS");
    }

    #[test]
    fn head_tail_at_cap_never_panics_on_multibyte() {
        // A fixed byte cap that lands mid-codepoint used to slice a &str and
        // panic — crashing the stash for any large CJK/emoji/Arabic blob.
        let cjk = "令牌优".repeat(200); // 3-byte chars, 1800 bytes
        let emoji = "😀🎉".repeat(200); // 4-byte chars
        for sample in [&cjk, &emoji] {
            for cap in 1..=32 {
                let (head, tail) = head_tail_at_cap(sample, cap); // must not panic
                assert!(sample.starts_with(head), "head not a prefix");
                assert!(sample.ends_with(tail), "tail not a suffix");
                assert!(head.len() <= cap, "head {} > cap {cap}", head.len());
                assert!(tail.len() <= cap, "tail {} > cap {cap}", tail.len());
            }
        }
        // Pure ASCII is unaffected (boundaries everywhere).
        let (h, t) = head_tail_at_cap("hello world foo bar", 5);
        assert_eq!(h, "hello");
        assert_eq!(t, "o bar");
    }

    /// The crate-wide environment lock, not a private one: these tests set
    /// `TOKENADE_STASH_DIR` and `TOKENADE_DISCLOSE_THRESHOLD`, and tests in
    /// other modules change the environment under `ENV_LOCK` — two locks for
    /// one environment serialise nothing (`above_threshold_stashes_and_
    /// summarises` red in a full run 2026-10-03, green alone).
    fn io_lock() -> &'static Mutex<()> {
        &crate::platform::ENV_LOCK
    }

    /// RAII cleanup guard: removes the tmpdir at Drop. Since L121 we
    /// override `TOKENADE_STASH_DIR` (the dedicated knob) instead of
    /// `XDG_CACHE_HOME`, so model downloads no longer land in the
    /// per-test tmpdir — the leak class that caused 53 × 465 MB =
    /// 20 GB in tmpfs is closed.
    struct StashGuard {
        path: PathBuf,
    }

    impl Drop for StashGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
            super::set_test_stash_dir(None);
        }
    }

    fn isolated_stash() -> (std::sync::MutexGuard<'static, ()>, StashGuard, PathBuf) {
        let guard = io_lock().lock().unwrap_or_else(|p| p.into_inner());
        let d = std::env::temp_dir().join(format!(
            "tokenade-disclose-test-{}-{}",
            std::process::id(),
            crate::platform::now_unix_secs()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // For THIS thread only: the process-wide variable redirected every
        // parallel test's stash and then deleted it (see `set_test_stash_dir`).
        super::set_test_stash_dir(Some(d.clone()));
        // stash_dir() now returns `d` verbatim, so the path we hand back
        // matches what the production code resolves at runtime.
        (guard, StashGuard { path: d.clone() }, d)
    }

    /// The stash holds the FULL raw text of a tool result — more than any
    /// other file tokenade owns — and every entry was created 0644 because
    /// `std::fs::write` masks 0666 with the umask. The 0700 directory was the
    /// only lock, and a directory mode does not survive a restore, an rsync or
    /// a tar unpacked with its own permissions.
    #[cfg(unix)]
    #[test]
    fn a_stashed_blob_is_born_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_g, _sg, _d) = isolated_stash();
        let text = "x".repeat(60_000);
        let (_hash, path) = stash_for_reference(&text, &SourceHint::File("t.txt".into()))
            .expect("stash must accept it");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{path:?} was created world-readable");
    }

    /// A write cut short left a partial file under the content's hash, and the
    /// next stash of the same output TRUSTED it: `expand-ref` then handed back
    /// the head of the output as all of it. A blob of the wrong length is
    /// rewritten; the right one is only touched, and no temporary is left.
    #[test]
    fn a_partial_blob_under_the_hash_is_rewritten_not_trusted() {
        let (_g, _sg, d) = isolated_stash();
        let text: String = (0..3_000).map(|i| format!("unique line {i}\n")).collect();
        let (hash, path) = stash_for_reference(&text, &SourceHint::File("t.txt".into())).expect("stash");
        let whole = std::fs::read(&path).unwrap();
        std::fs::write(&path, &whole[..whole.len() / 3]).unwrap();
        let (again, path2) = stash_for_reference(&text, &SourceHint::File("t.txt".into())).expect("stash");
        assert_eq!((again, &path2), (hash, &path));
        assert_eq!(std::fs::read(&path).unwrap(), whole, "the partial blob was trusted");
        let leftovers: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    /// A summary must be bounded in BYTES, not just in lines.
    ///
    /// The regression this locks: a minified document has almost no
    /// newlines, so the line-based fold fell into the "short document"
    /// branch and returned the whole thing verbatim. Measured on a real
    /// 6.8 MB npm registry response, the "folded summary" delivered
    /// ~1.4M tokens while claiming to omit middle lines.
    ///
    /// Uses a ONE-LINE payload deliberately: any line-count assertion
    /// would pass on it while the byte bound is what actually holds.
    #[test]
    fn summary_is_bounded_on_a_single_line_document() {
        let (_g, _sg, _d) = isolated_stash();
        // One line, no newlines at all, far past the stash threshold.
        let huge = format!("{{\"k\":\"{}\"}}", "x".repeat(2_000_000));
        let out = maybe_stash_with_threshold(huge.clone(), SourceHint::web(), 1_000);
        assert!(
            out.len() < huge.len() / 100,
            "summary must fold a single-line document, got {} bytes from {}",
            out.len(),
            huge.len()
        );
        assert!(
            out.chars().count() <= SUMMARY_MAX_BODY_CHARS + 2_000,
            "body must respect SUMMARY_MAX_BODY_CHARS (+ footer), got {} chars",
            out.chars().count()
        );
        // Lines must be bounded too: a total-only bound lets one
        // pathological line eat the whole budget and reads as garbage.
        // The footer is one deliberate long line, so exempt it.
        for l in out.lines().filter(|l| !l.starts_with("[stashed:")) {
            assert!(
                l.chars().count() <= SUMMARY_MAX_LINE_CHARS + 32,
                "line of {} chars exceeds SUMMARY_MAX_LINE_CHARS",
                l.chars().count()
            );
        }
        assert!(out.contains("stashed:"), "recovery pointer must survive");
    }

    /// Do not cut lines of a body that already fits. The
    /// per-line bound exists to stop one pathological line from eating a
    /// CONTESTED budget; when the whole body is under budget there is nothing
    /// to contest and truncating only destroys content.
    #[test]
    fn short_body_keeps_its_long_lines_intact() {
        let one_long_line = "x".repeat(SUMMARY_MAX_LINE_CHARS * 2);
        assert!(one_long_line.chars().count() < SUMMARY_MAX_BODY_CHARS);
        let out = clamp_summary_body(one_long_line.clone());
        assert_eq!(out, one_long_line, "short body must not be line-clamped");
    }

    #[test]
    fn detects_json_object() {
        assert_eq!(detect_kind(r#"{"x":1}"#), "json");
        assert_eq!(detect_kind(r#"[1,2,3]"#), "json");
        assert_eq!(detect_kind(r#"   {"x":1}"#), "json");
    }

    #[test]
    fn detects_html() {
        assert_eq!(detect_kind("<!doctype html><html>foo"), "html");
        assert_eq!(detect_kind("<html><body>x</body></html>"), "html");
    }

    #[test]
    fn detects_xml() {
        assert_eq!(detect_kind("<?xml version=\"1.0\"?><root/>"), "xml");
    }

    #[test]
    fn detects_csv() {
        assert_eq!(detect_kind("a,b,c\n1,2,3\n4,5,6"), "csv");
    }

    /// A secret must not survive the ROUND TRIP, not merely the delivery.
    ///
    /// Delivery being clean is the easy half; the stash is a FILE that stays on
    /// disk, and `expand-ref --raw` is documented as returning "the whole stored
    /// blob". Both were verified by hand on 2026-08-04 (a `ghp_` token through
    /// `read`, the stash file, and `--raw`: masked at all three) — this pins it,
    /// because the failure mode is a NEW delivery path that forgets to redact,
    /// which is exactly the leak this product has already shipped once.
    #[test]
    fn a_secret_survives_neither_the_stash_file_nor_the_raw_recovery() {
        let (_g, _drop, dir) = isolated_stash();
        let secret = "ghp_zzqqwwee1122334455667788990011aabb";
        let body = format!("line one\nconfig token = {secret}\nline three\n");
        let (hash, path) =
            stash_for_reference(&body, &SourceHint::Unknown).expect("stash must be written");
        let on_disk = std::fs::read_to_string(&path).expect("stash file readable");
        assert!(
            !on_disk.contains(secret),
            "the token was written to disk in clear: {}",
            path.display()
        );
        let (recovered, _ext) = read_stash_by_hash(&hash).expect("hash must resolve");
        assert!(
            !recovered.contains(secret),
            "the raw recovery path handed the token back"
        );
        // The surrounding content must still be there — a guard that redacts
        // everything would pass this test and destroy the feature.
        assert!(
            recovered.contains("line one"),
            "content was lost, not redacted"
        );
        assert!(dir.exists());
    }

    #[test]
    fn a_ragged_comma_line_is_not_a_table() {
        // The blob that motivated this (stash `40ba7ca5a9f3`): a line-numbered
        // Rust excerpt whose FIRST line carries 5 commas. Labelled `.csv`, it
        // was read back through the CSV compactor, which cut every line at its
        // first comma and doubled the quotes in string literals.
        let src = "304\t#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\n\
                   305\tenum SlotKind {\n\
                   306\t    Ts,\n\
                   307\t    Num,\n";
        assert_ne!(detect_kind(src), "csv", "rows must agree on their width");
        // Prose with a comma-heavy opening sentence, same shape.
        assert_ne!(
            detect_kind("first, second, third and more\nbut this line has none\n"),
            "csv"
        );
    }

    #[test]
    fn quoted_commas_do_not_break_a_real_table() {
        // Field counts must be compared OUTSIDE quotes, or this legitimate
        // table would look ragged (2 raw commas in the header, 3 in row 1).
        assert_eq!(detect_kind("a,b,c\n\"x,y\",z,w\n1,2,3"), "csv");
    }

    #[test]
    fn detects_pdf_magic() {
        // PDF preamble is ASCII so we can build a valid str.
        let txt = "%PDF-1.7\n%binary parts removed for tests";
        assert_eq!(detect_kind(txt), "pdf");
    }

    #[test]
    fn detects_ascii_binary_magic() {
        // `detect_kind` takes &str, so only ASCII-prefix magic bytes
        // round-trip safely (GIF, ZIP, OggS — non-ASCII like PNG's
        // 0x89 would have been lossy-replaced by any UTF-8 decoder
        // upstream and never reach this point).
        assert_eq!(detect_kind("GIF87a content"), "gif");
        assert_eq!(detect_kind("OggS audio..."), "ogg");
    }

    #[test]
    fn unknown_text_falls_back_to_txt() {
        assert_eq!(detect_kind("just some prose, no markers"), "txt");
    }

    #[test]
    fn looks_like_graphql_positive() {
        assert!(looks_like_graphql(r#"{"data":{"users":[]},"errors":null}"#));
        assert!(looks_like_graphql(r#"{"errors":[{"message":"oops"}]}"#));
    }

    #[test]
    fn looks_like_graphql_negative() {
        assert!(!looks_like_graphql(r#"{"x":1,"y":2}"#));
        assert!(!looks_like_graphql("[1,2,3]"));
        assert!(!looks_like_graphql("plain text"));
    }

    #[test]
    fn detect_kind_bytes_recognises_png() {
        // 8-byte PNG signature.
        let png: [u8; 16] = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0xD, b'I', b'H', b'D', b'R',
        ];
        assert_eq!(detect_kind_bytes(&png), "png");
    }

    #[test]
    fn detect_kind_bytes_recognises_jpeg() {
        let jpeg: [u8; 8] = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F'];
        assert_eq!(detect_kind_bytes(&jpeg), "jpg");
    }

    #[test]
    fn detect_kind_bytes_recognises_wasm_binary() {
        // WASM magic: 0x00 0x61 0x73 0x6D + version 1
        let wasm: [u8; 8] = [0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
        assert_eq!(detect_kind_bytes(&wasm), "wasm");
    }

    #[test]
    fn binary_short_circuit_always_stashes_regardless_of_size() {
        let (_g, _drop, _stash_dir) = isolated_stash();
        // A real PNG header — short enough to be well under the
        // disclose token threshold. The bytes-aware API picks up
        // the magic via infer and routes to the binary path
        // regardless of size. The string-only `maybe_stash` cannot
        // recover from the lossy U+FFFD round-trip 0x89 takes
        // through `String::from_utf8_lossy` — callers with raw
        // bytes MUST use `maybe_stash_bytes` to preserve binary.
        let png: [u8; 16] = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0xD, b'I', b'H', b'D', b'R',
        ];
        let out = maybe_stash_bytes(png.to_vec(), SourceHint::Shell);
        assert!(out.contains("[disclose:binary"), "got: {out}");
        assert!(out.contains("kind=png"), "got: {out}");
        assert!(!out.contains("IHDR\u{0}"), "must not echo raw bytes: {out}");
    }

    #[test]
    fn html_is_not_classified_binary() {
        // `infer` recognises `<!DOCTYPE html>` as text/html. It must
        // NOT be routed to the binary stash — the agent wanted to read
        // it. Regression: stash `.html` files were served as
        // `[disclose:binary kind=html …]`, forcing an extra Read.
        let html = b"<!DOCTYPE html>\n<html><head><title>x</title></head>\
            <body><h1>hello</h1><p>some readable content here</p></body></html>";
        assert!(detect_binary(html).is_none(), "HTML must not be binary");
        // Also covers XML (another infer Text matcher).
        let xml = b"<?xml version=\"1.0\"?>\n<root><item>value</item></root>";
        assert!(detect_binary(xml).is_none(), "XML must not be binary");
    }

    #[test]
    fn binary_metadata_carries_tool_hints() {
        let (_g, _drop, _stash) = isolated_stash();
        let pdf = b"%PDF-1.7\n%bytes\x00\x01\x02ignored";
        let out = maybe_stash_bytes(pdf.to_vec(), SourceHint::Shell);
        assert!(out.contains("kind=pdf"));
        assert!(out.contains("pdftotext"), "PDF hint missing: {out}");
    }

    #[test]
    fn detect_binary_density_heuristic() {
        // Random-looking control bytes — not a known magic but still
        // clearly binary by density.
        let mut v = vec![0u8; 4096];
        for (i, b) in v.iter_mut().enumerate() {
            *b = (i % 32) as u8;
        }
        assert_eq!(detect_binary(&v).as_deref(), Some("bin"));
    }

    /// The user-visible half of the escape-sequence finding: what reaches the
    /// agent for a real coloured command. Before this, a 558-byte `grep -rn
    /// --color=always` result came back as `(Binary content — not shown as
    /// text)` with the bytes in the stash, and the agent's only move was to run
    /// the command again without colour.
    #[test]
    fn a_coloured_grep_result_is_not_stashed_as_binary() {
        let mut grep = String::new();
        for i in 0..40 {
            grep.push_str(&format!(
                "\x1b[32m\x1b[Kcrates/x/{i}.rs\x1b[m\x1b[K\x1b[36m\x1b[K:\x1b[m\x1b[K{i}\
                 \x1b[36m\x1b[K:\x1b[m\x1b[Kpub \x1b[01;31m\x1b[Kfn \x1b[m\x1b[Kwork() {{\n"
            ));
        }
        assert!(
            grep.len() > 64,
            "the density heuristic only runs past 64 bytes"
        );
        let escapes = grep.bytes().filter(|&b| b == 0x1B).count();
        assert!(
            escapes * 100 / grep.len() >= 5,
            "the fixture must be past the 5 % the old heuristic used, or it \
             proves nothing: {}%",
            escapes * 100 / grep.len()
        );
        assert_eq!(
            detect_binary(grep.as_bytes()),
            None,
            "coloured text is text"
        );
    }

    #[test]
    fn detect_binary_skips_text() {
        let text = "Plain prose with newlines\nand normal ASCII. Nothing binary about it.\n";
        assert_eq!(detect_binary(text.as_bytes()), None);
    }

    #[test]
    fn maybe_stash_bytes_text_path_below_threshold_passes_through() {
        let (_g, _drop, _stash) = isolated_stash();
        let small = b"small text".to_vec();
        let out = maybe_stash_bytes(small, SourceHint::Shell);
        assert_eq!(out, "small text");
    }

    // --- #475: stash layer redacts credentials before disk write ---

    /// Build a text payload that crosses the stash token threshold so
    /// the stash actually fires. The threshold is tokenizer-based;
    /// padding with simple ASCII does the trick reliably.
    fn padded_above_threshold(payload: &str) -> String {
        let mut s = String::from(payload);
        s.push('\n');
        // 8000 bytes of padding => well above any plausible threshold.
        s.push_str(&"padding line for threshold inflation.\n".repeat(220));
        s
    }

    #[test]
    fn maybe_stash_redacts_aws_key_before_writing_to_disk() {
        // Regression for #475 — without the fix, an `env`/`cat .env`
        // wrapped via the CLI would land the AKIA key in cleartext
        // inside ~/.cache/tokenade/stash/.
        let (_g, _drop, stash) = isolated_stash();
        let secret = "AKIA1234567890ABCDEF";
        let text = padded_above_threshold(&format!("AWS_ACCESS_KEY_ID={secret}\n"));
        let _summary = maybe_stash(text, SourceHint::Shell);
        // Walk the stash dir; assert NO file contains the secret.
        let mut leaked = false;
        for entry in std::fs::read_dir(&stash).unwrap().flatten() {
            let body = std::fs::read_to_string(entry.path()).unwrap_or_default();
            if body.contains(secret) {
                leaked = true;
                break;
            }
        }
        assert!(!leaked, "AWS key leaked to stash file in cleartext");
    }

    #[test]
    fn maybe_stash_bytes_text_path_redacts_before_disk_write() {
        // Same guarantee on the bytes-input path (used by curl, by
        // commands whose stdout might be binary, etc.).
        let (_g, _drop, stash) = isolated_stash();
        let secret = "ghp_thirty_plus_chars_abcdef0123456789xyz";
        let text = padded_above_threshold(&format!("export GITHUB_TOKEN={secret}\n"));
        let _summary = maybe_stash_bytes(text.into_bytes(), SourceHint::Shell);
        let mut leaked = false;
        for entry in std::fs::read_dir(&stash).unwrap().flatten() {
            let body = std::fs::read_to_string(entry.path()).unwrap_or_default();
            if body.contains(secret) {
                leaked = true;
                break;
            }
        }
        assert!(!leaked, "GitHub PAT leaked to stash file via bytes path");
    }

    #[test]
    fn maybe_stash_bytes_binary_path_unaffected_by_redaction() {
        // PNG header + payload — must NOT be touched by the redact
        // pass. Without this guard, our regex could mangle binary
        // content that happens to match (extremely unlikely but
        // worth pinning).
        let (_g, _drop, stash) = isolated_stash();
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&vec![0xAB; 8192]);
        let png_len = png.len();
        let _summary = maybe_stash_bytes(png.clone(), SourceHint::Shell);
        // Find the .png stash and assert byte-exact preservation.
        let mut matched = false;
        for entry in std::fs::read_dir(&stash).unwrap().flatten() {
            let body = std::fs::read(entry.path()).unwrap_or_default();
            if body.len() == png_len && body == png {
                matched = true;
                break;
            }
        }
        assert!(matched, "binary path corrupted (byte-exact stash missing)");
    }

    #[test]
    fn detect_kind_bytes_falls_back_to_text_for_unknown_bytes() {
        let bytes = br#"{"foo":1}"#;
        assert_eq!(detect_kind_bytes(bytes), "json");
    }

    #[test]
    fn mime_to_extension_json_variants() {
        assert_eq!(mime_to_extension("application/json"), Some("json"));
        assert_eq!(
            mime_to_extension("application/json; charset=utf-8"),
            Some("json")
        );
        assert_eq!(
            mime_to_extension("application/graphql-response+json"),
            Some("graphql.json")
        );
        assert_eq!(mime_to_extension("application/x-ndjson"), Some("ndjson"));
    }

    #[test]
    fn mime_to_extension_buckets_unknown_image() {
        // Server claims something exotic but it's still an image.
        assert_eq!(mime_to_extension("image/x-exotic"), Some("bin"));
    }

    #[test]
    fn mime_to_extension_returns_none_on_unknown() {
        assert_eq!(mime_to_extension("application/x-fictional"), None);
    }

    #[test]
    fn compose_web_with_content_type_overrides_sniff() {
        // Body looks like JSON but the server says CSV.
        let body = r#"{"x":1}"#;
        let hint = SourceHint::web_with_content_type("text/csv");
        assert_eq!(compose_extension(body, &hint), "api.rest.csv");
    }

    #[test]
    fn compose_web_with_graphql_content_type() {
        let body = "any body";
        let hint = SourceHint::web_with_content_type("application/graphql-response+json");
        assert_eq!(compose_extension(body, &hint), "graphql.json");
    }

    #[test]
    fn compose_web_with_content_type_strips_charset() {
        let hint = SourceHint::web_with_content_type("application/json; charset=utf-8");
        assert_eq!(compose_extension("{}", &hint), "api.rest.json");
    }

    #[test]
    fn compose_web_json_becomes_api_rest_json() {
        let ext = compose_extension(r#"{"users":[]}"#, &SourceHint::web());
        assert_eq!(ext, "api.rest.json");
    }

    #[test]
    fn compose_web_graphql_shape_becomes_graphql_json() {
        let ext = compose_extension(r#"{"data":{"x":1}}"#, &SourceHint::web());
        assert_eq!(ext, "graphql.json");
    }

    #[test]
    fn compose_explicit_graphql_hint_wins() {
        let ext = compose_extension(r#"{"foo":1}"#, &SourceHint::GraphQl);
        assert_eq!(ext, "graphql.json");
    }

    #[test]
    fn compose_web_html() {
        let ext = compose_extension("<!doctype html><html/>", &SourceHint::web());
        assert_eq!(ext, "html");
    }

    #[test]
    fn compose_file_path_extension_wins() {
        let ext = compose_extension("anything", &SourceHint::File(PathBuf::from("/tmp/x.toml")));
        assert_eq!(ext, "toml");
    }

    #[test]
    fn compose_shell_uses_pure_kind() {
        let ext = compose_extension(r#"{"x":1}"#, &SourceHint::Shell);
        assert_eq!(ext, "json");
    }

    #[test]
    fn below_threshold_returns_unchanged() {
        let (_g, _drop, _stash) = isolated_stash();
        let small = "small content".to_string();
        let out = maybe_stash(small.clone(), SourceHint::Unknown);
        assert_eq!(out, small);
    }

    #[test]
    fn above_threshold_stashes_and_summarises() {
        let (_g, _drop, stash_dir) = isolated_stash();
        // Force a low threshold so we don't have to generate 5k tokens.
        let _threshold = tk_base::delivery::threshold_on_this_thread(20);
        let mut body = String::new();
        for i in 0..100 {
            let _ = writeln!(body, "line {i} with some content");
        }
        let out = maybe_stash(body.clone(), SourceHint::Shell);
        assert!(out.contains("[disclose:summary kind="));
        assert!(out.contains("[stashed: "));
        // The stash dir should contain our file.
        let hash = crate::dedup_ref::hash_content(body.as_bytes());
        let stashed = stash_dir.join(format!("{hash}.txt"));
        assert!(stashed.exists(), "stash file should exist at {stashed:?}");
    }

    #[test]
    fn read_threshold_env_override_and_default() {
        // Hermetic: a TOKENADE_PRESSURE inherited from the invoking shell
        // scales the default and breaks the assertion (caught by a release
        // dry-run where the wrapper exported PRESSURE=0.75).
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::remove_var("TOKENADE_PRESSURE");
        }
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::remove_var("TOKENADE_READ_DISCLOSE_THRESHOLD");
        }
        assert_eq!(read_threshold_tokens(), READ_DEFAULT_THRESHOLD_TOKENS);
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::set_var("TOKENADE_READ_DISCLOSE_THRESHOLD", "1234");
        }
        assert_eq!(read_threshold_tokens(), 1234);
        // Garbage falls back to the default — same forgiving semantics
        // as threshold_tokens.
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::set_var("TOKENADE_READ_DISCLOSE_THRESHOLD", "not-a-number");
        }
        assert_eq!(read_threshold_tokens(), READ_DEFAULT_THRESHOLD_TOKENS);
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::remove_var("TOKENADE_READ_DISCLOSE_THRESHOLD");
        }
        // A deliberate Read must clear a HIGHER bar than an incidental
        // tool output before we stash it (the doc-digest fix).
        const { assert!(READ_DEFAULT_THRESHOLD_TOKENS > DEFAULT_THRESHOLD_TOKENS) };
    }

    #[test]
    fn summary_footer_mentions_expand_ref_and_hash() {
        // #705 — companion to #704. The disclose footer must advertise
        // `tokenade expand-ref <hash>` explicitly so agents know
        // the stash-fallback path exists. Pre-#705 they had to extract
        // the hash from the path stem (and the global CLAUDE.md rule
        // pointed at expand-ref without context on which hashes work).
        let (_g, _drop, _stash) = isolated_stash();
        let _threshold = tk_base::delivery::threshold_on_this_thread(20);
        let mut body = String::new();
        for i in 0..100 {
            let _ = writeln!(body, "line {i} with content above the disclose threshold");
        }
        let out = maybe_stash(body.clone(), SourceHint::Shell);
        // Hash is dedup_ref::hash_content of the redacted text. The
        // body has no secrets so redact is a no-op and the hash matches
        // the unredacted body.
        let hash = crate::dedup_ref::hash_content(body.as_bytes());
        assert!(
            out.contains("tokenade expand-ref"),
            "footer must mention expand-ref, got: {}",
            out.lines().last().unwrap_or("")
        );
        assert!(
            out.contains(&hash),
            "footer must include the actual hash for the agent to call expand_ref with, got: {}",
            out.lines().last().unwrap_or("")
        );
    }

    #[test]
    fn single_file_read_target_detects_plain_reader_only() {
        let dir = std::env::temp_dir().join(format!("tkn-sfrt-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("release.log");
        std::fs::write(&f, "x\n").unwrap();
        let fp = f.to_string_lossy().to_string();
        // Plain reader, with or without a flag → detected (the flag value
        // `50` is not a file, so it's correctly ignored).
        assert_eq!(
            single_file_read_target(&format!("cat -n {fp}")),
            Some(f.clone())
        );
        assert_eq!(
            single_file_read_target(&format!("tail -n 50 {fp}")),
            Some(f.clone())
        );
        // Pipe / redirect / non-reader / multi-file / glob → None (keep
        // the generic Shell wording rather than redirect to a wrong path).
        assert_eq!(single_file_read_target(&format!("cat {fp} | grep x")), None);
        assert_eq!(
            single_file_read_target(&format!("cat {fp} > out.txt")),
            None
        );
        assert_eq!(single_file_read_target(&format!("grep x {fp}")), None);
        assert_eq!(single_file_read_target(&format!("cat {fp} {fp}")), None);
        assert_eq!(single_file_read_target("ls -la"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `sed -n 'a,bp' f` and `awk 'NR>=a && NR<=b' f` print lines of ONE
    /// file unchanged: reads, like `cat`. Their quoted programs carry `>`,
    /// `&`, `<`, which only count outside quotes. A program that transforms
    /// is not a read.
    #[test]
    fn a_sed_or_awk_line_selection_is_a_single_file_read() {
        let dir = std::env::temp_dir().join(format!("tkn-sfrt2-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("access-boundary.service.ts");
        std::fs::write(&f, "export class A {}\n").unwrap();
        let fp = f.to_string_lossy().to_string();
        assert_eq!(single_file_read_target(&format!("sed -n '1,400p' {fp}")), Some(f.clone()));
        assert_eq!(single_file_read_target(&format!("sed -n '40,80p;120,$p' {fp}")), Some(f.clone()));
        assert_eq!(single_file_read_target(&format!("awk 'NR>=200 && NR<=300' {fp}")), Some(f.clone()));
        // Unquoted, as agents write it half the time.
        assert_eq!(single_file_read_target(&format!("sed -n 95,120p {fp}")), Some(f.clone()));
        assert_eq!(single_file_read_target(&format!("sed -n s/a/b/p {fp}")), None);
        assert_eq!(single_file_read_target(&format!("sed 's/A/B/' {fp}")), None, "a substitution transforms");
        assert_eq!(single_file_read_target(&format!("sed '1,40p' {fp}")), None, "without -n every line prints twice");
        assert_eq!(single_file_read_target(&format!("awk '{{print $2}}' {fp}")), None);
        // Since 2026-10-02 a pipe of line filters keeps the read (see below).
        assert_eq!(single_file_read_target(&format!("sed -n '1,4p' {fp} | head")), Some(f.clone()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A month of transcripts: 13 of the 67 `for f in a.rs b.rs …; do
    /// echo; sed -n …; done` loops filed at the general threshold, summarised head+tail,
    /// then `expand-ref`. Several code files read at once are a
    /// read.
    #[test]
    fn several_source_files_read_in_one_command_are_a_read() {
        let dir = std::env::temp_dir().join(format!("tkn-multi-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let (a, b, log) = (dir.join("engine.rs"), dir.join("auth.rs"), dir.join("run.log"));
        for f in [&a, &b, &log] {
            std::fs::write(f, "x\n").unwrap();
        }
        let (a, b, log) = (a.to_string_lossy().to_string(), b.to_string_lossy().to_string(), log.to_string_lossy().to_string());
        for cmd in [
            format!("for f in {a} {b}; do echo \"=== $f\"; sed -n 1,80p $f; done"),
            format!("for f in {a} {b}; do echo \"=== $f\" && cat \"$f\"; done"),
            format!("cat {a}; echo ===; sed -n 40,90p {b} 2>/dev/null"),
            format!("head -60 {a} && echo --- && tail -20 {b}"),
            // A leading `cd` says WHERE the files are (429 chains out of 990).
            format!("cd {} && sed -n 1,80p engine.rs; echo ===; sed -n 40,90p auth.rs", dir.display()),
        ] {
            assert!(reads_only_source_files(&cmd), "{cmd}");
        }
        // Relative without the `cd`, or a `cd` that is not leading: nothing read.
        assert!(!reads_only_source_files("sed -n 1,80p engine.rs; sed -n 40,90p auth.rs"));
        assert!(!reads_only_source_files(&format!("cat {a}; cd {}; cat auth.rs", dir.display())));
        for cmd in [
            format!("cat {a}"),
            format!("for f in {a} {b}; do grep -n fn $f; done"),
            format!("cat {a}; cat {log}"),
            format!("cat {a}; rm {b}"),
            format!("cat {a} || cat {b}"),
            format!("for f in {a} {b}; do cat $f"),
        ] {
            assert!(!reads_only_source_files(&cmd), "{cmd}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Piped reads from a week of transcripts, filed at ~2 000
    /// tokens under pressure because the pipe denied them the read
    /// threshold: each one only numbers or selects the lines
    /// of ONE file.
    #[test]
    fn a_pipe_of_line_filters_is_still_a_single_file_read() {
        let dir = std::env::temp_dir().join(format!("tkn-sfrt3-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("sources.rs");
        std::fs::write(&f, "fn a() {}\n").unwrap();
        let fp = f.to_string_lossy().to_string();
        for cmd in [
            format!("awk 'NR>=2485 && NR<=2600' {fp} | nl -ba -v2485 | sed 's/\\t/: /'"),
            format!("grep -n \"\" {fp} | sed -n '385,520p'"),
            format!("nl -ba {fp} | sed -n '400,560p'"),
            format!("cat -n {fp} | sed -n '400,560p'"),
            format!("sed -n '400,560p' {fp} | cat -n"),
        ] {
            assert_eq!(single_file_read_target(&cmd), Some(f.clone()), "{cmd}");
        }
        for cmd in [
            format!("cat {fp} | grep x"),
            format!("cat {fp} | sort"),
            format!("cat {fp} || true"),
            format!("nl -ba {fp} | sed -n '1,9p' {fp}"),
            format!("grep -n fn {fp} | head"),
            format!("cat {fp} | head > out.txt"),
        ] {
            assert_eq!(single_file_read_target(&cmd), None, "{cmd}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn idempotent_stash_doesnt_rewrite() {
        let (_g, _drop, _stash) = isolated_stash();
        let _threshold = tk_base::delivery::threshold_on_this_thread(20);
        let mut body = String::new();
        for i in 0..100 {
            let _ = writeln!(body, "line {i}");
        }
        let _ = maybe_stash(body.clone(), SourceHint::Shell);
        let _ = maybe_stash(body, SourceHint::Shell); // second call same content
    }

    #[test]
    fn evict_stale_removes_expired_files() {
        // Verify the core eviction logic that opportunistic_evict
        // wraps. Create two files in a fresh stash dir — one with a
        // backdated mtime (past TTL), one current — and confirm only
        // the old one is removed.
        let (_g, _drop, stash_dir_path) = isolated_stash();
        let old_path = stash_dir_path.join("old.txt");
        let fresh_path = stash_dir_path.join("fresh.txt");
        std::fs::write(&old_path, b"old data").unwrap();
        std::fs::write(&fresh_path, b"fresh data").unwrap();
        // Backdate `old` past the TTL by 1 second. SystemTime
        // arithmetic via filetime would require a dep; use set_modified
        // on the file handle with a SystemTime in the past.
        let past = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(crate::platform::now_unix_secs() - CACHE_TTL_SECS - 1);
        {
            let f = std::fs::OpenOptions::new()
                .write(true)
                .open(&old_path)
                .unwrap();
            // `set_modified` lands in stable Rust 1.75+.
            f.set_modified(past).unwrap();
        }
        let removed = evict_stale().unwrap();
        assert_eq!(removed, 1, "expected to evict exactly the old file");
        assert!(!old_path.exists(), "old file should be gone");
        assert!(fresh_path.exists(), "fresh file should survive");
    }

    /// The document channels (PDF text-redirect, spreadsheet skeleton, web
    /// recovery) stash through `stash_for_reference`, NOT `maybe_stash` — so
    /// they used to write `.txt` extracts into the stash and never sweep it.
    /// A doc-only session would accumulate them forever. Two guarantees here:
    /// the extract lands in the swept directory, and an expired one is
    /// actually reclaimed.
    #[test]
    fn document_extracts_land_in_the_swept_stash_and_expire() {
        let (_g, _drop, stash_dir_path) = isolated_stash();
        let (_hash, path) = stash_for_reference(
            "extracted document text, as the PDF redirect writes it",
            &SourceHint::File("manual.pdf.txt".into()),
        )
        .expect("stash");

        assert_eq!(
            path.parent(),
            Some(stash_dir_path.as_path()),
            "a document extract must live in the swept stash dir, not off on its own"
        );

        // Backdate past the TTL: the sweep must reclaim it like any other stash.
        let past = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(crate::platform::now_unix_secs() - CACHE_TTL_SECS - 1);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(past)
            .unwrap();
        evict_stale().unwrap();
        assert!(
            !path.exists(),
            "an expired document extract must be reclaimed"
        );
    }

    /// The media frame cache stages a DIRECTORY (`frames-<hash>/*.jpg`) under
    /// the stash. The sweep used to skip anything that was not a file, so those
    /// piled up forever — the same leak class as the document `.txt` extracts.
    #[test]
    fn an_expired_cache_directory_is_reclaimed_too() {
        let (_g, _drop, stash_dir_path) = isolated_stash();
        let dir = stash_dir_path.join("frames-deadbeefcafe");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f001.jpg"), b"jpeg bytes").unwrap();

        let past = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(crate::platform::now_unix_secs() - CACHE_TTL_SECS - 1);
        std::fs::OpenOptions::new()
            .write(true)
            .open(dir.join("f001.jpg"))
            .unwrap()
            .set_modified(past)
            .unwrap();
        // The sweep ages a directory by its NEWEST entry, so backdating the
        // file is enough — no directory-mtime trickery needed.
        evict_stale().unwrap();
        assert!(
            !dir.exists(),
            "an expired frame-cache directory must be reclaimed"
        );
    }

    #[test]
    fn graceful_degradation_on_unwritable_stash() {
        let _g = io_lock().lock().unwrap_or_else(|p| p.into_inner());
        // Point XDG_CACHE_HOME at a path under a non-writable parent.
        // Simplest portable way: a file (not a dir) where the dir is
        // expected.
        let bad = std::env::temp_dir().join(format!("tokenade-disclose-bad-{}", crate::platform::now_unix_secs()));
        std::fs::write(&bad, b"oops").unwrap();
        // For this thread only — `XDG_CACHE_HOME` is process-wide.
        super::set_test_stash_dir(Some(bad.join("stash")));
        let _threshold = tk_base::delivery::threshold_on_this_thread(20);
        let mut body = String::new();
        for i in 0..100 {
            let _ = writeln!(body, "line {i}");
        }
        let out = maybe_stash(body.clone(), SourceHint::Shell);
        // Stash failed → degrade gracefully: return original text.
        assert_eq!(out, body);
        super::set_test_stash_dir(None);
        let _ = std::fs::remove_file(&bad);
    }

    // --- BM25 search_stash: headroom CCR sub-search ---

    #[test]
    fn bm25_tokenize_splits_and_lowercases() {
        let toks = bm25_tokenize("Connection-Refused to HOST_a (UUID 1a2b)");
        assert!(toks.contains(&"connection-refused".to_string()));
        assert!(toks.contains(&"host_a".to_string()));
        assert!(toks.contains(&"uuid".to_string()));
        assert!(toks.contains(&"1a2b".to_string()));
        // Single-char fragments are dropped.
        assert!(!toks.iter().any(|t| t.len() < 2));
    }

    // Two clearly separated topics so each question retrieves its own block.




    /// A minified bundle / one-line JSON IS the whole file on a single line —
    /// an unbounded "matching line" would re-inflate exactly what the stash
    /// avoids. The hit must be clipped around the needle, and the needle must
    /// survive the clipping.
    #[test]
    fn clip_match_line_bounds_a_single_line_file() {
        let pad = "alpha beta gamma delta ".repeat(400);
        let line = format!("{pad}SECRET_NEEDLE{pad}");
        assert!(line.len() > 15_000, "fixture must be a huge single line");

        let clipped = clip_match_line(&line, "SECRET_NEEDLE");
        assert!(
            clipped.chars().count() <= GREP_LINE_MAX_CHARS + 2, // + the two ellipses
            "clipped line must be bounded, got {} chars",
            clipped.chars().count()
        );
        assert!(
            clipped.contains("SECRET_NEEDLE"),
            "the needle must survive clipping"
        );
        assert!(
            clipped.starts_with('…') && clipped.ends_with('…'),
            "elision must be marked"
        );

        // A short line is returned untouched — no gratuitous ellipses.
        let short = "error: connection refused";
        assert_eq!(clip_match_line(short, "refused"), short);
    }

    /// A comma inside one question is prose, not a separator. Splitting it
    /// silently changed what the user asked AND multiplied the budget.
    #[test]
    fn prose_commas_do_not_fan_out_into_several_questions() {
        for prose in [
            "what is the revenue, in euros, for Q3",
            "Combien, au total, coûte le projet ?",
            "the deploy failed, why",
        ] {
            assert_eq!(
                split_scoped_prompts(prose),
                vec![prose.trim().to_string()],
                "prose must stay ONE question: {prose}"
            );
        }
        // A genuine batch still splits.
        assert_eq!(
            split_scoped_prompts("why did the deploy fail, where do backups go"),
            vec!["why did the deploy fail", "where do backups go"]
        );
    }

    /// A comma inside `()`, `[]`, `{}` or a quoted span is never a separator.
    /// Torn there, the tail of a range becomes its OWN retrieval, at the full
    /// budget, under a `### Qn ·` heading that reads like an answered
    /// question. Every string here is a real `--prompt` value from this
    /// machine's transcripts; 3 of the 116 distinct ones were torn this way.
    #[test]
    fn a_comma_inside_brackets_or_quotes_is_not_a_separator() {
        // Parenthetical: the split produced `what tool or platform)?`.
        let paren = "Does the page give an engagement figure to compare against \
                     the personal profile one (and if so, what tool or platform)?";
        assert_eq!(
            split_scoped_prompts(paren),
            vec![paren.trim().to_string()],
            "a parenthetical comma must not separate"
        );

        // Nested comma inside a batch: three questions, not four.
        let batch = "full verbatim text of lines 118-260 (the CPC table, with the bar chart), \
                     full verbatim text of lines 260-330 (CTR section), \
                     KeyTakeaways closing and market-share lines";
        let parts = split_scoped_prompts(batch);
        assert_eq!(
            parts.len(),
            3,
            "nested comma counted as a separator: {parts:?}"
        );
        assert!(
            parts.iter().all(|p| !p.ends_with(')') || p.contains('(')),
            "a fragment closed a bracket it never opened: {parts:?}"
        );

        // A quoted list the caller is asking about, verbatim.
        let quoted = "full sentence with \"like, comment, share, vote\"";
        assert_eq!(
            split_scoped_prompts(quoted),
            vec![quoted.trim().to_string()],
            "a quoted list must not separate"
        );

        // The apostrophe is NOT a quote: French prose would otherwise swallow
        // everything after the first `d'`.
        assert_eq!(
            split_scoped_prompts(
                "pourquoi le module rejette-t-il le pool d'envoi, quelle mesure le justifie"
            ),
            vec![
                "pourquoi le module rejette-t-il le pool d'envoi",
                "quelle mesure le justifie"
            ]
        );

        // An unbalanced closing bracket must not make the rest one blob.
        let stray = "why did the deploy fail), where do backups go";
        assert_eq!(
            split_scoped_prompts(stray).len(),
            2,
            "stray `)` swallowed the split"
        );
    }

    /// The notice fired 50 times in eleven days on this machine and named
    /// the RULE, not the fact: "each part must stand on its own (three words
    /// or more)". One short part vetoes the WHOLE batch, so a six-topic
    /// prompt is declined by the single word at the end, and the caller had
    /// no way to know which one. The same defect, one more time.
    #[test]
    fn the_decline_notice_quotes_the_part_that_caused_it() {
        let m = batch_declined_notice("offer pricing details, product description, founder")
            .expect("a declined batch must produce a notice");
        assert!(
            m.contains("\"product description\""),
            "the offending fragment is not quoted: {m}"
        );
        assert!(
            m.contains("2 of the 3 comma-separated parts"),
            "the notice must say how many are short: {m}"
        );
        // And it says what was DONE, not just the rule: the number of
        // questions comes from the fold itself.
        assert!(
            m.contains("folded into its neighbour") && m.contains("1 question(s)"),
            "the message does not say what the fold gave: {m}"
        );

        // A batch that splits says nothing at all.
        assert_eq!(
            batch_declined_notice("why did the deploy fail, where do backups go"),
            None,
            "a batch that was honoured must stay silent"
        );
        // No comma at all is not a declined batch either.
        assert_eq!(batch_declined_notice("one question only"), None);
        // A comma inside brackets is not a batch, so no notice — the fragment
        // count the notice reports must come from the same splitter the
        // decision uses, or it would name parts nobody was asked about.
        assert_eq!(
            batch_declined_notice("give the range (25-34, 18-24) verbatim"),
            None,
            "a bracketed comma must not be reported as a declined batch"
        );
    }

    /// **A short fragment no longer refuses the whole batch.**
    ///
    /// The old rule required that ALL fragments stand alone, otherwise
    /// none. Measured on transcripts on 2026-09-09: 31 refusals, two of them the
    /// same day, almost always two real questions plus a two-word
    /// shorthand. The message already said what to do — « fold it into its
    /// neighbour » — and made the human do it.
    #[test]
    fn a_short_fragment_joins_its_neighbour_instead_of_vetoing_the_batch() {
        // The over-refused case, seen in production: « quels blocages » is two
        // words and took the two real questions down with it.
        assert_eq!(
            split_scoped_prompts(
                "quels agents non couverts, quelles idées de proxy, quels blocages"
            ),
            vec![
                "quels agents non couverts",
                "quelles idées de proxy, quels blocages"
            ]
        );
        // The case that MOTIVATED the rule returns exactly what it returned: a
        // single search, never three.
        assert_eq!(
            split_scoped_prompts("what is the revenue, in euros, for Q3"),
            vec!["what is the revenue, in euros, for Q3"]
        );
        // Short ones AT THE HEAD belong to the question that follows them:
        // rejoining them leftward would fabricate « Combien, au total », three
        // words, so a second search on half a sentence.
        assert_eq!(
            split_scoped_prompts("Combien, au total, coûte le projet ?"),
            vec!["Combien, au total, coûte le projet ?"]
        );
        // No standalone fragment: there is no batch at all.
        assert_eq!(split_scoped_prompts("a b, c d"), vec!["a b, c d"]);
    }

    #[test]
    fn split_drops_blank_fragments_and_caps_count() {
        // Trailing comma and a whitespace-only fragment are dropped, not queried.
        // (Fragments must look like standalone questions — see
        // `prose_commas_do_not_fan_out_into_several_questions`.)
        assert_eq!(
            split_scoped_prompts("what broke the deploy, , where do backups live,"),
            vec!["what broke the deploy", "where do backups live"]
        );
        // No comma → a single element (the whole prompt).
        assert_eq!(
            split_scoped_prompts("one question only"),
            vec!["one question only"]
        );
        // Overflow is capped.
        let many = (0..MAX_SCOPED_PROMPTS + 20)
            .map(|i| format!("what is metric {i} today"))
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(split_scoped_prompts(&many).len(), MAX_SCOPED_PROMPTS);
    }


    /// The scaffolded instructions promise that reading a .pdf returns text.
    /// When extraction comes back empty, the summary used to say only "Binary
    /// content — not shown as text. Inspect with: pdftotext …", which reads as
    /// "tokenade does not do PDFs" and sends the reader to a tool tokenade has
    /// already run. The file being broken is the fact worth stating.
    #[test]
    fn a_document_that_yielded_no_text_says_so_instead_of_looking_unsupported() {
        let pdf = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog";
        let out = build_binary_metadata(pdf, "pdf", "pdf", std::path::Path::new("/tmp/x.pdf"));
        assert!(
            out.contains("no text could be extracted from THIS one"),
            "a failed extraction is still presented as an unsupported format: {out}"
        );
        // The tool hint stays: a second opinion is still useful.
        assert!(out.contains("pdftotext"), "lost the tool hint: {out}");

        // A format tokenade never claimed to read must NOT gain the sentence —
        // there it would be false, nothing was attempted.
        let bin = [0u8, 1, 2, 3, 4, 5, 6, 7];
        let out = build_binary_metadata(&bin, "bin", "bin", std::path::Path::new("/tmp/x.bin"));
        assert!(
            !out.contains("no text could be extracted"),
            "claimed an extraction that never ran: {out}"
        );
    }

    #[test]
    fn read_threshold_serves_inline_what_incidental_would_stash() {
        let (_g, _drop, _stash) = isolated_stash();
        // A body well above the incidental 5k cutoff but the kind of
        // file an agent deliberately opens (a handbook, a config doc).
        let mut body = String::new();
        for i in 0..400 {
            let _ = writeln!(body, "line {i} of a deliberately-read handbook section");
        }
        let tokens = crate::tokenizer::estimate(&body);
        let hint = SourceHint::File(PathBuf::from("/tmp/handbook.md"));
        // High read threshold (above this body) → served inline, NOT
        // stashed: no doc-digest search_stash round-trips.
        let inline = maybe_stash_with_threshold(body.clone(), hint.clone(), tokens + 1000);
        assert_eq!(
            inline, body,
            "deliberate read under the read threshold stays inline"
        );
        // The SAME bytes below a low threshold still stash — proving the
        // disposition is driven purely by the threshold argument.
        let stashed = maybe_stash_with_threshold(body.clone(), hint, 20);
        assert!(
            stashed.contains("[stashed: "),
            "below the threshold the same bytes are stashed"
        );
    }

    #[test]
    fn footer_redirects_exact_lookups_to_source_for_file_hint() {
        let (_g, _drop, _stash) = isolated_stash();
        let _threshold = tk_base::delivery::threshold_on_this_thread(20);
        let mut body = String::new();
        for i in 0..100 {
            let _ = writeln!(body, "log line {i} status=200");
        }
        // File hint → footer must steer EXACT lookups to the original
        // file (not the head+tail stash preview, not BM25 search_stash).
        let out = maybe_stash(
            body.clone(),
            SourceHint::File(PathBuf::from("/tmp/release.log")),
        );
        assert!(
            out.contains("grep or Read the original"),
            "file footer must redirect exact lookups to the source: {}",
            out.lines().last().unwrap_or("")
        );
        assert!(
            out.contains("/tmp/release.log"),
            "footer must name the source path"
        );
        // A ref is a document you ASK: the question path leads, advertises the
        // multi-question form, and raw bytes are demoted to a rare escape.
        assert!(
            out.contains("--prompt"),
            "footer must lead with the question path: {out}"
        );
        assert!(
            out.contains("q1, q2"),
            "footer must advertise multi-question: {out}"
        );
        // The footer states the SIZE IN TOKENS (the agent picks the right verb
        // from the number) and deliberately does NOT advertise `--raw`:
        // promoting it invites the bare dump measured 107/111 times in the
        // field. It stays reachable via --help and the refusal message.
        assert!(
            out.contains("tokens"),
            "footer must state the size in tokens: {out}"
        );
        assert!(
            !out.contains("--raw"),
            "footer must not promote the raw dump: {out}"
        );
        // A plain Shell hint (no known source) must NOT fabricate a
        // source-file redirect, but still lead with the question path.
        let out2 = maybe_stash(body, SourceHint::Shell);
        assert!(!out2.contains("grep or Read the original"));
        assert!(out2.contains("--prompt"));
        assert!(
            !out2.contains("--raw"),
            "Shell footer uses the same menu as File: no raw dump: {out2}"
        );
    }

    #[test]
    fn cjk_doc_crosses_real_threshold_and_is_searchable() {
        // End-to-end guard for the multi-language chain #8→#10 on a LONG,
        // realistic CJK document (not the toy one-liners the unit tests use):
        //   (1) #8 — estimate() now scores CJK at ~1 token/char, so a doc of
        //       ~6 000 ideographs clears the REAL disclose threshold (5 000).
        //       This is the original bug: a 5k-token Chinese blob estimated at
        //       ~1 100 under the old Latin-tuned CPT and was never stashed.
        //   (2) the real disclosure path stashes it at the DEFAULT threshold
        //       (no env override — we exercise the genuine 5 000 cutoff).
        //   (3) #10 — the stash stays recallable by a CJK needle (literal grep;
        //       the BM25 path went with the command it served, 2026-08-08),
        //       which the pre-fix `to_ascii_lowercase` + `is_alphanumeric`
        //       tokeniser collapsed into one unmatchable token.
        let (_g, _drop, _stash) = isolated_stash();
        // Deliberately NO TOKENADE_DISCLOSE_THRESHOLD override here.
        let para = "令牌优化是减少大型语言模型上下文消耗的关键技术。\
                    通过压缩冗余输出并延迟披露大型结果来显著节省成本。";
        let needle = "灾难恢复区域设定为里斯本，恢复时间目标为四小时。";
        let mut body = String::from("# 运维手册\n");
        for i in 0..120 {
            body.push_str(&format!("\n## 第{}节\n", i + 1));
            body.push_str(para);
            body.push('\n');
            if i == 60 {
                body.push_str(needle);
                body.push('\n');
            }
        }
        // (1) The CJK doc must clear the real disclosure threshold.
        let tokens = crate::tokenizer::estimate(&body);
        assert!(
            tokens >= DEFAULT_THRESHOLD_TOKENS,
            "long CJK doc must clear the real disclose threshold; estimate={tokens}, threshold={DEFAULT_THRESHOLD_TOKENS}"
        );
        // (2) At the default threshold it is stashed, not served inline.
        let out = maybe_stash(body.clone(), SourceHint::Shell);
        assert!(
            out.contains("[stashed: "),
            "a CJK doc above the threshold must be stashed, not inlined; got: {out}"
        );
        // (3) The needle is recalled through CJK bigram tokenisation.
        let hash = crate::dedup_ref::hash_content(body.as_bytes());
        let (hits, _m, _t, _tok) =
            crate::disclose::grep_stash_by_hash(&hash, "灾难恢复区域", false)
                .expect("the stash must stay recallable by the CJK needle");
        let hit: String = hits
            .iter()
            .map(|(_, l)| l.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            hit.contains("里斯本"),
            "recalled line must be the needle (里斯本/Lisbon); got: {hit}"
        );
    }
}
#[cfg(test)]
mod a_stash_is_attributed_to_the_session_that_made_it {
    //! The post-compaction digest offers "raw bytes that tokenade folded
    //! earlier" and chose them by mtime inside a six-hour window. The stash is
    //! ONE directory shared by every agent on this machine: measured over the
    //! local ledger, 1 828 of 3 676 moments (50 %) had more than one session
    //! active in the preceding six hours. Recording who folded what is what
    //! lets the digest keep its own sentence true.

    #[test]
    fn the_hash_lands_in_this_sessions_file_and_no_other() {
        let _g = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let d = std::env::temp_dir().join(format!("tkn-sess-refs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("tmp");
        let prev_home = std::env::var_os("HOME");
        // SAFETY: env is process-wide; ENV_LOCK is held for this test. HOME is
        // put back below — a later test finding it pointed at a deleted temp
        // directory is the same cross-test damage this lock exists to stop.
        unsafe {
            std::env::set_var("HOME", &d);
        }
        super::set_test_stash_dir(Some(d.join("stash")));
        crate::session::set_session_key_authoritative("sessAAA");
        // Process-wide: a later test running with the real HOME would file its
        // refs under `sessAAA` in the operator's `~/.tokenade/sessions/`.
        struct ForgetSession;
        impl Drop for ForgetSession {
            fn drop(&mut self) {
                crate::session::clear_session_key();
            }
        }
        let _forget = ForgetSession;
        let big: String = (0..4000)
            .map(|i| format!("line {i} of an output large enough to be stashed\n"))
            .collect();
        let served = super::maybe_stash(big, super::SourceHint::Shell);
        // The hash is stripped of its formatting: the common menu writes the
        // fallback as a code span (`` `tokenade expand-ref <hash>` ``), so the
        // word after `expand-ref ` carries a glued backtick. SECOND
        // consumer caught by this detail after the e2e guard of the same day —
        // the contract these reads rely on is « the word after
        // `expand-ref ` is the hash », and formatting is not part of it.
        let hash = served
            .split("expand-ref ")
            .nth(1)
            .and_then(|r| r.split_whitespace().next())
            .unwrap_or_default()
            .trim_matches('`')
            .to_string();
        assert!(hash.len() >= 8, "no ref hash in the served text: {served}");

        let mine = super::session_ref_hashes("sessAAA");
        assert!(mine.contains(&hash), "own session lost its ref: {mine:?}");
        assert!(
            super::session_ref_hashes("sessBBB").is_empty(),
            "another session claimed it"
        );
        // An unknown session is not a wildcard.
        assert!(super::session_ref_hashes("").is_empty());
        // SAFETY: still under ENV_LOCK.
        unsafe {
            match prev_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
#[cfg(test)]
mod the_tail_of_a_preview_starts_on_a_line {
    use super::*;

    /// All lines of the SAME length: a tail of an exact number of
    /// lines then falls on a boundary, which the second test needs
    /// to cross the bound it holds. With a bare `{i}`, line 0 and
    /// line 39 do not have the same length and the fixture misses its own case.
    fn body(lines: usize) -> String {
        (0..lines)
            .map(|i| format!("#### section {i:03} (3 lines · \"un aperçu qui remplit la ligne\")\n"))
            .collect()
    }

    /// The measured case: the cut falls in the middle of a line, so the fragment
    /// no longer has its heading. It goes, and the tail opens on a whole line.
    #[test]
    fn a_cut_inside_a_line_drops_the_fragment() {
        let b = body(40);
        let out = head_tail_preview(&b, 200, 201);
        let tail = out.rsplit_once(") …\n").expect("the banner").1;
        assert!(
            tail.starts_with("#### section"),
            "the tail resumes mid-word: {}",
            tail.chars().take(60).collect::<String>()
        );
        // Anti-vacuity: the fixture MUST cut in the middle of a line, otherwise this
        // test passes without crossing the rule.
        let raw: String = b.chars().skip(b.chars().count() - 201).collect();
        assert!(
            !raw.starts_with("####"),
            "the fixture already cut on a boundary, the rule is not reached"
        );
    }

    /// The other side of the bound: the cut ALREADY fell on a boundary, so
    /// nothing is moved forward — a rule that moves forward unconditionally would throw away
    /// a whole line here, which is exactly the loss already refused for
    /// the head.
    #[test]
    fn a_cut_already_on_a_boundary_keeps_its_first_line() {
        let b = body(40);
        let n = b.chars().count();
        // Length of a whole line, so a tail of an EXACT number of
        // lines: the cut falls right after a line break.
        let line = b.lines().next().expect("a line").chars().count() + 1;
        let out = head_tail_preview(&b, 200, line * 3);
        assert!(
            out.ends_with(&b.chars().skip(n - line * 3).collect::<String>()),
            "a whole line was thrown away even though the cut fell right:\n{out}"
        );
    }

    /// And a tail without any line break keeps its fragment: better
    /// a fragment than nothing.
    #[test]
    fn a_tail_without_a_newline_keeps_what_it_has() {
        let b = "x".repeat(500);
        let out = head_tail_preview(&b, 100, 100);
        assert!(out.ends_with(&"x".repeat(100)), "the tail disappeared:\n{out}");
    }
}
