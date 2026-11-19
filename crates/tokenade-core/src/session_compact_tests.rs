// The tests of `session_compact`.
//
// Spliced by `include!` from `session_compact.rs`: the same module, so the
// same private access and the same `super::` paths, without a single
// visibility line changed. This is the shape this repository already uses
// for 33 other modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod tests {

    /// A masked tool call still says what it was.
    ///
    /// The result of a superseded call is gone, so the block's only remaining
    /// job is to tell a resumed session what was already attempted. Replacing
    /// the whole `command` with a pointer removed exactly that: MEASURED
    /// 2026-09-05 over 805 local transcripts, 540 masked values landed in
    /// `input.command`, the smallest at 90 tokens — and an agent that cannot
    /// read what it ran runs it again.
    #[test]
    fn a_masked_tool_call_still_says_what_it_was() {
        let mut ids = std::collections::HashSet::new();
        ids.insert("t1".to_string());
        let body = "x".repeat(6000);
        let line = serde_json::json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use", "id": "t1", "name": "Bash",
                "input": {"command": format!("cd /srv/app && python3 - <<'PY'\n{body}\nPY")}
            }]}
        })
        .to_string();
        let mut stash = |_: &str| "tokenade expand-ref deadbeefcafe".to_string();
        let (out, before, after) =
            compact_tool_use_line(&line, &mut stash, &ids).expect("a 6 KB script must mask");
        assert!(after < before, "masking must pay: {before} -> {after}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let cmd = v["message"]["content"][0]["input"]["command"]
            .as_str()
            .unwrap();
        assert!(
            cmd.starts_with("cd /srv/app && python3 - <<'PY'"),
            "the call lost its identity: {cmd:?}"
        );
        assert!(cmd.contains(MARKER), "no pointer back to the body: {cmd:?}");
        assert!(!cmd.contains(&body), "the 6 KB body survived");

        // And a short command is not worth masking at all once the head is
        // kept — this is the 90-token case that was being masked.
        let short = serde_json::json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use", "id": "t1", "name": "Bash",
                "input": {"command": "cargo test -p tokenade-core --lib session_compact -- --nocapture"}
            }]}
        })
        .to_string();
        assert!(
            compact_tool_use_line(&short, &mut stash, &ids).is_none(),
            "a one-line command must be left alone"
        );

        // Idempotent: a value already carrying the marker is never re-masked.
        let again = compact_tool_use_line(&out, &mut stash, &ids);
        assert!(again.is_none(), "re-masked an already-masked call");
    }

    #[test]
    fn a_superseded_tool_calls_script_is_masked_but_its_shape_survives() {
        let script = "x".repeat(8_000);
        let line = serde_json::json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use",
                "id": "toolu_dead",
                "name": "Bash",
                "input": {"command": script, "description": "run it"}
            }]}
        })
        .to_string();
        let dead: std::collections::HashSet<String> = ["toolu_dead".to_string()].into();
        let mut stash = |_: &str| "tokenade expand-ref 0123456789ab".to_string();
        let (out, before, after) = compact_tool_use_line(&line, &mut stash, &dead).unwrap();
        assert!(after < before);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let input = &v["message"]["content"][0]["input"];
        assert!(
            input["command"].as_str().unwrap().contains("expand-ref"),
            "the script is replaced by its recovery pointer"
        );
        assert_eq!(
            input["description"], "run it",
            "a short value is left alone — masking it would cost more than it saves"
        );
        assert_eq!(
            v["message"]["content"][0]["name"], "Bash",
            "the call is still readable"
        );
    }

    #[test]
    fn a_live_tool_call_is_never_masked() {
        let line = serde_json::json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use",
                "id": "toolu_live",
                "name": "Bash",
                "input": {"command": "y".repeat(8_000)}
            }]}
        })
        .to_string();
        let dead: std::collections::HashSet<String> = ["toolu_other".to_string()].into();
        let mut stash = |_: &str| "tokenade expand-ref 0123456789ab".to_string();
        assert!(compact_tool_use_line(&line, &mut stash, &dead).is_none());
    }

    #[test]
    fn a_user_line_is_not_treated_as_a_tool_call() {
        let line = serde_json::json!({
            "type": "user",
            "message": {"content": [{"type": "tool_use", "id": "toolu_dead", "input": {"command": "z".repeat(8_000)}}]}
        })
        .to_string();
        let dead: std::collections::HashSet<String> = ["toolu_dead".to_string()].into();
        let mut stash = |_: &str| "tokenade expand-ref 0123456789ab".to_string();
        assert!(compact_tool_use_line(&line, &mut stash, &dead).is_none());
    }
    use super::*;

    fn big_text(n: usize) -> String {
        "word ".repeat(n)
    }

    fn user_tool_result(content: serde_json::Value) -> String {
        serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "t1", "content": content }
            ]}
        })
        .to_string()
    }

    #[test]
    fn masks_big_string_tool_result_and_preserves_structure() {
        let line = user_tool_result(serde_json::json!(big_text(3_000)));
        let mut calls = 0;
        let mut stash = |_: &str| {
            calls += 1;
            "hash123".to_string()
        };
        let (out, before, after) = compact_line(&line, &mut stash).expect("should mask");
        assert_eq!(calls, 1, "original stashed exactly once");
        assert!(before > after && after < MASK_MIN_TOKENS);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        // Structure intact: still a user tool_result, content now the marker.
        let c = &v["message"]["content"][0];
        assert_eq!(c["type"], "tool_result");
        assert!(c["content"].as_str().unwrap().starts_with(MARKER));
    }

    /// A read the session later EDITED away is masked at the low floor; the
    /// same-sized read of a file nobody touched is not. This is the whole point
    /// of the detector: 700 tokens of stale file content survive the generic
    /// 2 000-token bar and would be re-billed on every cold resume.
    #[test]
    fn the_superseded_gate_prices_the_saving_instead_of_guessing_a_floor() {
        let mut stash = |_: &str| "tokenade expand-ref 0123456789ab".to_string();
        let line = |tokens: usize| {
            serde_json::json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": big_text(tokens)}
            ]}})
            .to_string()
        };
        let gate = Gate::NetGain(MASK_SUPERSEDED_MIN_NET_GAIN);

        // 250 tokens: the old fixed 300 floor refused this, and refusing it
        // declined a ~90 % saving on a block the session had already replaced.
        let (_, before, after) = compact_line_gated(&line(250), &mut stash, gate).unwrap();
        assert!(
            before - after > 150,
            "a 250-token stale block must be masked, saving {before}→{after}"
        );

        // Small enough that the placeholder eats the saving: refused, and no
        // stash write was paid for it.
        let mut never = |_: &str| panic!("must not stash a block it will refuse");
        assert!(compact_line_gated(&line(30), &mut never, gate).is_none());
    }

    #[test]
    fn a_superseded_read_is_masked_where_a_live_one_of_the_same_size_is_not() {
        let _iso = crate::platform::StoreIsolation::new("a_superseded_read_is_mas");
        let read = |id: &str, path: &str| {
            serde_json::json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": id, "name": "Read", "input": {"file_path": path}}
            ]}})
            .to_string()
        };
        let result = |id: &str| {
            serde_json::json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": id, "content": big_text(700)}
            ]}})
            .to_string()
        };
        let edit = serde_json::json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "e1", "name": "Edit", "input": {"file_path": "/a/stale.rs"}}
        ]}})
        .to_string();

        let transcript = format!(
            "{}\n{}\n{}\n{}\n{}\n",
            read("t1", "/a/stale.rs"),
            result("t1"),
            read("t2", "/a/live.rs"),
            result("t2"),
            edit
        );
        let path =
            std::env::temp_dir().join(format!("tokenade-superseded-{}.jsonl", std::process::id()));
        std::fs::write(&path, &transcript).unwrap();

        let stats = compact_transcript(&path).unwrap();
        assert_eq!(stats.blocks_masked, 1, "only the superseded read");

        let after = std::fs::read_to_string(&path).unwrap();
        let masked: Vec<&str> = after.lines().filter(|l| l.contains(MARKER)).collect();
        assert_eq!(masked.len(), 1);
        assert!(masked[0].contains("\"t1\""), "t1 is the stale one");
        // The live read is still there in full.
        assert!(after.contains(&big_text(700)));
        // Every line keeps its byte length, so an incremental reader's offset
        // still lands on the lines appended after the rewrite.
        let lens = |t: &str| t.split_inclusive('\n').map(str::len).collect::<Vec<_>>();
        assert_eq!(lens(&after), lens(&transcript));
        assert!(
            serde_json::from_str::<serde_json::Value>(masked[0]).is_ok(),
            "padding keeps it JSON"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn masks_text_parts_but_leaves_images() {
        let line = user_tool_result(serde_json::json!([
            { "type": "text", "text": big_text(3_000) },
            { "type": "image", "source": { "data": "BASE64BASE64" } }
        ]));
        let mut stash = |_: &str| "h".to_string();
        let (out, _, _) = compact_line(&line, &mut stash).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let parts = &v["message"]["content"][0]["content"];
        assert!(parts[0]["text"].as_str().unwrap().starts_with(MARKER));
        // Image untouched.
        assert_eq!(parts[1]["source"]["data"], "BASE64BASE64");
    }

    #[test]
    fn small_blocks_and_non_tool_lines_are_untouched() {
        let small = user_tool_result(serde_json::json!("short output"));
        let mut stash = |_: &str| "h".to_string();
        assert!(
            compact_line(&small, &mut stash).is_none(),
            "small block skipped"
        );
        let assistant =
            serde_json::json!({"type":"assistant","message":{"content":[]}}).to_string();
        assert!(
            compact_line(&assistant, &mut stash).is_none(),
            "non-user skipped"
        );
    }

    #[test]
    fn already_masked_is_idempotent() {
        let masked = format!("{MARKER} 3000 tokens on cold resume — recover: x]");
        let line = user_tool_result(serde_json::json!(masked));
        let mut stash = |_: &str| "h".to_string();
        assert!(compact_line(&line, &mut stash).is_none(), "no re-mask");
    }

    #[test]
    fn foldable_tokens_counts_big_unmasked_blocks_only() {
        let big = user_tool_result(serde_json::json!(big_text(3_000)));
        let small = user_tool_result(serde_json::json!("short"));
        let masked = user_tool_result(serde_json::json!(format!(
            "{MARKER} 9999 tokens on cold resume — recover: x]"
        )));
        let transcript = format!("{big}\n{small}\n{masked}\n");
        let t = foldable_tokens(&transcript);
        assert!(t >= MASK_MIN_TOKENS, "counts the big block");
        // Small + already-masked contribute nothing.
        assert!(t < 2 * MASK_MIN_TOKENS, "only the one big block counted");
    }

    #[test]
    fn trim_threshold_is_cache_ceiling_plus_margin_and_never_below_max_ttl() {
        // Default (no override) = 1 h ceiling + 1 h margin = 2 h, and always
        // strictly greater than the max possible cache TTL so a still-cached
        // prefix is never trimmed.
        let t = trim_idle_threshold_secs();
        assert_eq!(t, TRIM_FRESHNESS_SECS);
        assert_eq!(t, ANTHROPIC_MAX_CACHE_TTL_SECS + TRIM_SAFETY_MARGIN_SECS);
        assert!(
            t > ANTHROPIC_MAX_CACHE_TTL_SECS,
            "must exceed the cache ceiling"
        );
    }

    #[test]
    fn detect_cache_ttl_from_tail_evidence() {
        let d = std::env::temp_dir();
        let w = |name: &str, body: &str| {
            let p = d.join(format!("ttl-detect-{name}-{}.jsonl", std::process::id()));
            std::fs::write(&p, body).unwrap();
            p
        };
        let one_h = w(
            "1h",
            r#"{"type":"assistant","message":{"usage":{"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":17080}}}}"#,
        );
        let five_m = w(
            "5m",
            r#"{"type":"assistant","message":{"usage":{"cache_creation":{"ephemeral_5m_input_tokens":900,"ephemeral_1h_input_tokens":0}}}}"#,
        );
        let none = w(
            "none",
            r#"{"type":"assistant","message":{"usage":{"input_tokens":5}}}"#,
        );
        // last positive occurrence wins: 5m write followed by a 1h write -> 1h
        let mixed = w(
            "mixed",
            &(std::fs::read_to_string(&five_m).unwrap()
                + "\n"
                + &std::fs::read_to_string(&one_h).unwrap()),
        );
        assert_eq!(
            detect_cache_ttl_secs(&one_h),
            Some(ANTHROPIC_EXTENDED_CACHE_TTL_SECS)
        );
        assert_eq!(
            detect_cache_ttl_secs(&five_m),
            Some(ANTHROPIC_MAX_CACHE_TTL_SECS)
        );
        assert_eq!(detect_cache_ttl_secs(&none), None);
        assert_eq!(
            detect_cache_ttl_secs(&mixed),
            Some(ANTHROPIC_EXTENDED_CACHE_TTL_SECS)
        );
        for p in [one_h, five_m, none, mixed] {
            std::fs::remove_file(p).ok();
        }
    }

    #[test]
    fn fresh_file_with_1h_evidence_is_not_trim_safe() {
        let p = std::env::temp_dir().join(format!("tk-ttl-fresh-{}.jsonl", std::process::id()));
        std::fs::write(&p, r#"{"type":"assistant","message":{"usage":{"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":17080}}}}"#).unwrap();
        // mtime = now -> inside every window
        assert!(!idle_beyond_cache_window(&p));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn is_idle_beyond_missing_file_is_not_idle() {
        // Missing/unknown mtime → conservative false (never trim on no evidence).
        let p = std::path::Path::new("/nonexistent/tokenade/session/xyz.jsonl");
        assert!(!is_idle_beyond(p, 0));
    }

    #[test]
    fn mask_then_scan_reports_same_resume_saving() {
        // Pure encode→scan round-trip: no disk, no global env (compact_transcript's
        // real stashing is covered by the CLI end-to-end smoke, not here — a
        // process-global TOKENADE_STASH_DIR would race the parallel test suite).
        let line = user_tool_result(serde_json::json!(big_text(3_000)));
        let mut stash = |_: &str| "abc123".to_string();
        let (masked_line, before, after) = compact_line(&line, &mut stash).unwrap();
        assert!(before > after && before > 2_000);

        // A transcript containing that masked line scans back to the same delta.
        let transcript = format!(
            "{}\n{}\n",
            masked_line,
            serde_json::json!({"type":"assistant","message":{"content":[]}})
        );
        let scan = scan_masked(&transcript);
        assert_eq!(scan.blocks_masked, 1);
        assert_eq!(
            scan.tokens_before, before,
            "encoded N matches the elided count"
        );
        assert_eq!(scan.tokens_after, after, "placeholder estimate matches");

        // A second compaction of an already-masked line is a no-op (idempotent).
        assert!(compact_line(&masked_line, &mut stash).is_none());
    }
}

#[cfg(test)]
mod a_delegated_report_keeps_its_conclusion {
    use super::*;

    /// A subagent's report is the one result nothing can produce again,
    /// so masking it whole removes the deliverable.
    ///
    /// Every other tool_result names something recoverable: a file is on disk,
    /// a command can be re-run, a page re-fetched. A `Task`/`Agent` report is
    /// the distilled output of a run that will not happen the same way twice.
    /// MEASURED 2026-09-05 across this machine's transcripts: 6 reports were
    /// masked whole on cold resume — 2 173, 2 241, 2 538 and 2 676 tokens among
    /// them, all just over the 2 000-token bar, so it was the generic size gate
    /// treating a deliverable like any other output.
    ///
    /// Both halves are asserted: the report keeps its opening, and an ordinary
    /// result of the same size does not (the head is not free, and it is only
    /// bought where re-running is impossible).
    #[test]
    fn a_report_keeps_its_opening_and_a_command_output_does_not() {
        let body: String =
            std::iter::repeat_n("assorted finding prose about the wiring\n", 400).collect();
        let report = format!(
            "The category page is wired through `routes/[locale]/c/[slug]`.\n\
             Three call sites, listed below.\n{body}"
        );
        let line_for = |tool: &str, id: &str, text: &str| {
            (
                serde_json::json!({
                    "type": "assistant",
                    "message": {"content": [
                        {"type": "tool_use", "id": id, "name": tool, "input": {"command": "x"}}
                    ]}
                }),
                serde_json::json!({
                    "type": "user",
                    "message": {"content": [
                        {"type": "tool_result", "tool_use_id": id, "content": text}
                    ]}
                }),
            )
        };
        let (use_a, res_a) = line_for("Agent", "toolu_a", &report);
        let (use_b, res_b) = line_for("Bash", "toolu_b", &report);
        let transcript = [use_a, res_a, use_b, res_b]
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";

        let dir = std::env::temp_dir().join(format!("tokenade-report-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        std::fs::write(&path, &transcript).unwrap();
        let _home = crate::platform::ScratchHome::new("report-head");
        compact_transcript(&path).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();

        let agent_line = out
            .lines()
            .find(|l| l.contains("toolu_a") && l.contains("tool_result"))
            .expect("the report line survives");
        let bash_line = out
            .lines()
            .find(|l| l.contains("toolu_b") && l.contains("tool_result"))
            .expect("the command line survives");
        assert!(
            agent_line.contains("The category page is wired through"),
            "a delegated report must keep its conclusion:\n{agent_line}"
        );
        assert!(
            agent_line.contains(MARKER),
            "and must still be masked — the head is not the whole report:\n{agent_line}"
        );
        assert!(
            !bash_line.contains("The category page is wired through"),
            "an ordinary result must not buy a head it does not need:\n{bash_line}"
        );
        assert!(
            bash_line.contains(MARKER),
            "the control must actually have been masked, or it proves nothing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #12: a line the session appends while its transcript is compacted is
    /// still on disk afterwards. The old temp + rename replaced the file with
    /// the copy read at the start, dropping about half of such appends.
    #[test]
    fn lines_appended_during_compaction_survive() {
        use std::io::Write;
        let _iso = crate::platform::StoreIsolation::new("lines_appended_during_c");
        let path =
            std::env::temp_dir().join(format!("tokenade-append-race-{}.jsonl", std::process::id()));
        let big = |i: usize| {
            serde_json::json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": format!("t{i}"), "content": "word ".repeat(3_000)}
            ]}})
            .to_string()
        };
        std::fs::write(&path, (0..40).map(|i| big(i) + "\n").collect::<String>()).unwrap();
        let writer = {
            let path = path.clone();
            std::thread::spawn(move || {
                for i in 0..200 {
                    let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
                    writeln!(f, r#"{{"type":"system","n":{i}}}"#).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            })
        };
        let mut masked = 0;
        while !writer.is_finished() {
            masked += compact_transcript(&path).unwrap().blocks_masked;
        }
        writer.join().unwrap();
        masked += compact_transcript(&path).unwrap().blocks_masked;
        let out = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        for i in 0..200 {
            assert!(out.contains(&format!(r#""n":{i}}}"#)), "append {i} was lost");
        }
        assert_eq!(masked, 40, "each block is masked once, and counted once");
        assert_eq!(out.lines().filter(|l| l.contains(MARKER)).count(), 40);
    }
}
