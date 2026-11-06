//! End-to-end tests driving `tokenade hookwrap`.

use super::*;

/// #561 — when an existing `tokenade-pretool`-tagged entry has the
/// OLD matcher (no WebFetch), re-running `install --with-lsp-enforcement-hook`
/// must replace it with the current canonical matcher rather than
/// silently treat the old entry as "AlreadyPresent". Before the fix,
/// users who installed before the WebFetch matcher widening (#426)
/// were stuck without it forever.
#[test]
fn e2e_install_upgrades_stale_pretool_hook_matcher() {
    let h = TempHome::new("hook-matcher-upgrade");
    let claude_dir = h.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    // Seed settings.json with a STALE tokenade-pretool entry whose
    // matcher predates WebFetch.
    let stale = serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "Bash|Grep|Glob|Read|Task|Agent",
                "hooks": [{
                    "type": "command",
                    "command": "sh -c 'exec /old/path/to/tokenade hookwrap-pretool'",
                    "_tag": "tokenade-pretool"
                }]
            }]
        },
        "keep_me": true
    });
    std::fs::write(
        claude_dir.join("settings.json"),
        serde_json::to_string_pretty(&stale).unwrap(),
    )
    .unwrap();

    let (out, _err, code) = run(
        &[
            "install",
            "--with-lsp-enforcement-hook",
            "--yes",
            "--only",
            "claude-code",
        ],
        h.path(),
    );
    assert_eq!(code, 0, "install failed: {out}");
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(claude_dir.join("settings.json")).unwrap())
            .unwrap();
    // User content survives.
    assert_eq!(after["keep_me"], serde_json::json!(true));
    // Exactly one tokenade-pretool entry survives — the upgrade replaced,
    // didn't duplicate.
    let pre = after["hooks"]["PreToolUse"].as_array().unwrap();
    let tokenade_pretool: Vec<&serde_json::Value> = pre
        .iter()
        .filter(|g| {
            g["hooks"]
                .as_array()
                .map(|hs| {
                    hs.iter()
                        .any(|h| h["_tag"].as_str() == Some("tokenade-pretool"))
                })
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        tokenade_pretool.len(),
        1,
        "exactly one tokenade-pretool entry after upgrade; got {}",
        pre.iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
    // The surviving entry has the WIDENED matcher (WebFetch added).
    let new_matcher = tokenade_pretool[0]["matcher"].as_str().unwrap_or("");
    assert!(
        new_matcher.contains("WebFetch"),
        "matcher must be upgraded to include WebFetch; got: {new_matcher}"
    );
    // And the stale `/old/path/to/tokenade` is gone — replaced by the
    // command pointing at the actual binary under test.
    let new_cmd = tokenade_pretool[0]["hooks"][0]["command"]
        .as_str()
        .unwrap_or("");
    assert!(
        !new_cmd.contains("/old/path/to/tokenade"),
        "stale command path must be replaced; got: {new_cmd}"
    );
}

/// The hooks are the product's main integration — most users never type a
/// tokenade command — and they were the last delivery channel with no
/// credential coverage. Same contract as the CLI sweep: plant the secret, run
/// the hook, assert it does not come back.
///
/// Each case asserts the hook ACTUALLY PRODUCED OUTPUT before judging it. A
/// hook that declines emits nothing, and "nothing" contains no secret — so
/// without that assertion this test would pass while testing an inert binary,
/// which is exactly what happened while it was being written.
#[test]
fn e2e_no_hook_hands_back_a_credential() {
    let h = TempHome::new("hook-secret");
    let secret = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    let pem_body = "MIIEowIBAAKCAQEAy8Dbv8prpJ";
    let pem = format!(
        "-----BEGIN RSA PRIVATE KEY-----\n{pem_body}/0kKhlGeJYozo2t60EG8L0561g\n-----END RSA PRIVATE KEY-----"
    );
    let filler: String = (0..2_000)
        .map(|i| format!("line {i} of an ordinary log file with some words in it\n"))
        .collect();

    // A file the Read pre-tool hook will serve compacted.
    let env_file = h.path().join("c.env");
    std::fs::write(
        &env_file,
        format!("HOME=/root\naws_secret_access_key = {secret}\n{filler}"),
    )
    .unwrap();
    let key_file = h.path().join("k.json");
    std::fs::write(
        &key_file,
        format!(
            "{{\"note\": {:?}, \"pad\": {:?}}}",
            format!("deploy key\n{pem}\n"),
            filler
        ),
    )
    .unwrap();

    let read_payload = |p: &std::path::Path| {
        format!(
            r#"{{"hook_event_name":"PreToolUse","session_id":"s1","tool_name":"Read","tool_input":{{"file_path":{:?}}}}}"#,
            p.to_str().unwrap()
        )
    };
    let bash_payload = |stdout: &str| {
        serde_json::json!({
            "hook_event_name": "PostToolUse",
            "session_id": "s1",
            "tool_name": "Bash",
            "tool_input": {"command": "journalctl -u api"},
            "tool_response": {"stdout": stdout, "stderr": ""}
        })
        .to_string()
    };

    let cases: Vec<(&str, &str, String)> = vec![
        (
            "read pretool env",
            "hookwrap-pretool",
            read_payload(&env_file),
        ),
        (
            "read pretool json+pem",
            "hookwrap-pretool",
            read_payload(&key_file),
        ),
        (
            "bash posttool buried",
            "hookwrap",
            bash_payload(&format!(
                "{filler}aws_secret_access_key = {secret}\n{filler}"
            )),
        ),
        (
            "bash posttool structured",
            "hookwrap",
            bash_payload(&format!(
                "{{\"Credentials\":{{\"SecretAccessKey\":\"{secret}\"}},\"pad\":\"{}\"}}",
                &filler[..400]
            )),
        ),
    ];

    let mut leaks = Vec::new();
    let mut inert = Vec::new();
    for (what, cmd, payload) in cases {
        let (out, err, _code) = run_hook(&[cmd], h.path(), &payload);
        if out.trim().is_empty() {
            inert.push(what);
            continue;
        }
        for (stream, text) in [("stdout", &out), ("stderr", &err)] {
            if text.contains(secret) || text.contains(pem_body) {
                leaks.push(format!("{what} on {stream}"));
            }
        }
    }
    assert!(
        inert.is_empty(),
        "these hooks produced nothing, so they were never tested: {inert:?}\n\
         (a hook returns immediately when TOKENADE_HOOK_DISABLED is set — use run_hook)"
    );
    assert!(
        leaks.is_empty(),
        "a credential reached the model through a hook ({} case(s)):\n{}",
        leaks.len(),
        leaks.join("\n")
    );
}

/// The Copilot fold path, executed — both payload shapes, and its attribution.
///
/// `copilot_fold` is the only agent-specific fold besides Gemini's, and the
/// only place `already_folded` was consulted before 2026-08-08. It
/// accepts two shapes because Copilot has shipped both: native camelCase
/// `toolResult.textResultForLlm` and VS Code's snake_case
/// `tool_result.text_result_for_llm`. A shape it stops recognising folds
/// nothing and says nothing.
#[cfg(unix)]
#[test]
fn e2e_the_copilot_hook_folds_both_payload_shapes_and_credits_copilot() {
    let body: String = (0..600)
        .map(|i: usize| {
            format!(
                "2026-08-08T10:{:02}:{:02}Z INFO worker[{}] handled path=/api/v1/items status=200 dur={}ms",
                i / 60, i % 60, i % 8, 10 + i % 40
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    for (shape, payload) in [
        (
            "camelCase",
            serde_json::json!({
                "toolName": "bash",
                "toolResult": {"resultType": "success", "textResultForLlm": body}
            }),
        ),
        (
            "snake_case",
            serde_json::json!({
                "tool_name": "bash",
                "tool_result": {"result_type": "success", "text_result_for_llm": body}
            }),
        ),
    ] {
        let h = TempHome::new(&format!("copilot-{shape}"));
        let (out, _err, code) = run_hook(&["hookwrap-copilot"], h.path(), &payload.to_string());
        assert_eq!(code, 0);
        assert!(
            out.len() * 10 < body.len(),
            "{shape}: the Copilot hook returned {} bytes for {} — it did not fold",
            out.len(),
            body.len()
        );
        let v: serde_json::Value =
            serde_json::from_str(out.trim()).expect("Copilot expects a JSON reply");
        assert!(
            v["modifiedResult"]["textResultForLlm"].is_string(),
            "{shape}: the reply is not in the shape Copilot reads back: {out}"
        );

        // Attribution is DECLARED here (`set_var` in `cmd_hookwrap_copilot`),
        // not inferred — misattribution is what happens when it is left to
        // inference.
        let log = std::fs::read_to_string(h.path().join(".tokenade").join("gain.jsonl"))
            .unwrap_or_default();
        let row: serde_json::Value = log
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("valid JSON record"))
            .next()
            .unwrap_or_else(|| panic!("{shape}: a fold that happened wrote no row:\n{log}"));
        assert_eq!(
            row["agent"].as_str(),
            Some("copilot"),
            "{shape}: the Copilot hook credited another agent: {row}"
        );
    }
}

/// The three hook agents whose PostToolUse entry point is the plain `hookwrap`
/// subcommand — driven, not read.
///
/// An audit found that the six plugin bridges had only ever been VERIFIED BY
/// READING: every test asserted on the text of the asset files, none ran one.
/// Rank 1 of the ladder had the same hole one rank up. Before this test, the
/// executed hook agents were claude-code and copilot; codex, droid and grok
/// (and reasonix, since retired) were wired by an installer nobody had driven
/// end to end.
///
/// Two things are asserted that reading cannot establish:
///
///  * the fold happens on the payload shape the installer's matcher selects —
///    and Droid's shell tool is `Execute`, not `Bash`, so a handler keyed on
///    the name alone would fold two of these three and silently pass the
///    third through;
///  * the ledger row names the RIGHT agent. These three have no per-agent entry
///    point (unlike `hookwrap-copilot`, which declares it), so attribution runs
///    through `detect::from_env()` — the path that was shown to fall back to the
///    process ancestor when nothing declares it.
/// The bypass-nudge counter, counted end to end, because the defect was in
/// the WIRING and not in any predicate: the PreToolUse and PostToolUse
/// handlers both ticked (one bypass counted as two), and neither saw a
/// bypass that sits in a non-first chain segment (the common shape — a `cd`
/// line, then the command). Measured live on the installed binary before the
/// fix: 2 and 0. A unit test on the predicate passes under both wirings.
#[test]
fn one_bypass_ticks_the_nudge_counter_exactly_once() {
    for (label, cmd, want) in [
        ("leading env prefix", "TOKENADE_HOOK_DISABLED=1 echo x", 1),
        (
            "a later segment",
            "cd /tmp\nTOKENADE_HOOK_DISABLED=1 echo x",
            1,
        ),
        ("no bypass at all", "echo plain", 0),
        // Nothing was bypassed: the hook is off BECAUSE tokenade is being
        // run directly, and the output is compacted either way.
        (
            "tokenade itself",
            "TOKENADE_HOOK_DISABLED=1 tokenade wrap ls",
            0,
        ),
        // ...and the same, hidden behind a `cd` line: the question must be
        // asked of the SEGMENT that requested the bypass, not of `cd`.
        (
            "tokenade itself, later segment",
            "cd /tmp\nTOKENADE_HOOK_DISABLED=1 tokenade wrap ls",
            0,
        ),
    ] {
        let h = TempHome::new("bypass-nudge-count");
        let pre = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": cmd},
        });
        let post = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": cmd},
            "tool_response": {"stdout": "hello\n", "stderr": "", "interrupted": false},
        });
        run_hook_as(None, &["hookwrap-pretool"], h.path(), &pre.to_string());
        run_hook_as(None, &["hookwrap"], h.path(), &post.to_string());
        let counter = h.path().join(".tokenade/bypass-counter");
        let got: u64 = std::fs::read_to_string(&counter)
            .ok()
            .and_then(|s| s.split_whitespace().nth(1).and_then(|n| n.parse().ok()))
            .unwrap_or(0);
        assert_eq!(
            got, want,
            "{label}: {cmd:?} ticked {got} time(s), wanted {want}"
        );
    }
}

#[test]
fn the_plain_hookwrap_agents_fold_and_credit_themselves() {
    let body: String = (0..600)
        .map(|i: usize| {
            format!(
                "2026-08-08T11:{:02}:{:02}Z INFO worker[{}] handled path=/api/v1/items status=200 dur={}ms",
                i / 60, i % 60, i % 8, 10 + i % 40
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    // (agent slug, the shell tool name that agent's matcher selects)
    for (agent, tool) in [("codex", "Bash"), ("droid", "Execute"), ("grok", "bash")] {
        let payload = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": tool,
            "tool_input": {"command": "journalctl -n 600"},
            "tool_response": {"stdout": body, "stderr": "", "interrupted": false},
        });
        let h = TempHome::new(&format!("hookwrap-{agent}"));
        let (out, err, code) =
            run_hook_as(Some(agent), &["hookwrap"], h.path(), &payload.to_string());
        assert_eq!(code, 0, "{agent}: the hook failed: {err}");
        assert!(
            out.len() * 5 < body.len(),
            "{agent} ({tool}): the hook returned {} bytes for {} — it did not fold",
            out.len(),
            body.len()
        );

        let log = std::fs::read_to_string(h.path().join(".tokenade").join("gain.jsonl"))
            .unwrap_or_default();
        let row: serde_json::Value = log
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(|v| v["before"].as_u64().unwrap_or(0) > 0)
            .unwrap_or_else(|| panic!("{agent}: a fold that happened wrote no row:\n{log}"));
        assert_eq!(
            row["agent"].as_str(),
            Some(agent),
            "{agent}: the fold was credited elsewhere: {row}"
        );
    }
}

/// The Gemini-family AfterTool hook — both slugs, both `llmContent` shapes.
///
/// Second half of the rank-1 sweep. `gemini-cli` and `qwen` share one entry
/// point (`gemini-after-tool --agent <slug>`) whose payload contract differs
/// from the plain `hookwrap` one: the text lives under
/// `tool_response.llmContent`, which is EITHER a string or a parts array. Both
/// are exercised, because a handler that reads only the string form passes the
/// structured one through in silence.
///
/// The `--agent` flag is asserted through the ledger row rather than assumed:
/// this hook declares its own attribution (`set_var` from `agent_from_argv`)
/// precisely because inference credited the process ancestor before.
#[test]
fn the_gemini_family_aftertool_hook_folds_both_content_shapes() {
    let body: String = (0..600)
        .map(|i: usize| {
            format!(
                "2026-08-08T12:{:02}:{:02}Z INFO worker[{}] handled path=/api/v1/items status=200 dur={}ms",
                i / 60, i % 60, i % 8, 10 + i % 40
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    // `qwen-code`, not `qwen`: that is the slug `Agent::as_str` emits and the
    // only one `agent_slug_for` ever writes. Passing `qwen` here first — my
    // error — credited `gemini-cli`, which is `agent_from_argv`'s deliberate
    // fallback for a slug it cannot resolve, not a defect.
    for agent in ["qwen-code"] {
        for (shape, content) in [
            ("string", serde_json::json!(body)),
            ("parts", serde_json::json!([{"text": body.clone()}])),
        ] {
            let payload = serde_json::json!({
                "tool_name": "run_shell_command",
                "tool_input": {"command": "journalctl -n 600"},
                "tool_response": {"error": null, "llmContent": content},
            });
            let h = TempHome::new(&format!("gemini-{agent}-{shape}"));
            let (out, err, code) = run_hook(
                &["gemini-after-tool", "--agent", agent],
                h.path(),
                &payload.to_string(),
            );
            assert_eq!(code, 0, "{agent}/{shape}: the hook failed: {err}");
            assert!(
                out.len() * 5 < body.len(),
                "{agent}/{shape}: returned {} bytes for {} — it did not fold",
                out.len(),
                body.len()
            );

            let log = std::fs::read_to_string(h.path().join(".tokenade").join("gain.jsonl"))
                .unwrap_or_default();
            let row: serde_json::Value = log
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .find(|v| v["before"].as_u64().unwrap_or(0) > 0)
                .unwrap_or_else(|| {
                    panic!("{agent}/{shape}: a fold that happened wrote no row:\n{log}")
                });
            assert_eq!(
                row["agent"].as_str(),
                Some(agent),
                "{agent}/{shape}: `--agent` did not reach the ledger: {row}"
            );
        }
    }
}

/// The Gemini-family BeforeTool hook: the rewrite it emits must survive the
/// shell that will run it.
///
/// This hook rewrites `run_shell_command` into `tokenade wrap '<cmd>'`, so the
/// single-quote escaping is not cosmetic — a command carrying a quote that is
/// escaped wrongly either fails to parse or, worse, parses into a DIFFERENT
/// command than the user asked for. Nothing had ever driven it.
///
/// The round trip is done through a real `sh`, not by eyeballing the string:
/// swap the `tokenade wrap` prefix for `printf %s` and the shell must hand back
/// the original command byte for byte. That is the property that matters, and
/// it is checked against the shell rather than against my reading of quoting
/// rules.
#[cfg(unix)]
#[test]
fn the_gemini_before_tool_rewrite_round_trips_through_a_real_shell() {
    let cases = [
        "journalctl -n 600",
        "echo 'hello world'",
        "grep -r \"needle\" . | awk '{print $1}'",
        "sh -c 'echo it'\\''s fine'",
    ];
    for cmd in cases {
        let payload = serde_json::json!({
            "tool_name": "run_shell_command",
            "tool_input": {"command": cmd},
        });
        let h = TempHome::new("gemini-beforetool");
        let (out, err, code) = run_hook(
            &["gemini-hook", "--agent", "qwen-code"],
            h.path(),
            &payload.to_string(),
        );
        assert_eq!(code, 0, "{cmd:?}: hook failed: {err}");
        let v: serde_json::Value =
            serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{cmd:?}: {e}: {out}"));
        let rewritten = v["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap_or_else(|| panic!("{cmd:?}: no rewrite in {out}"));
        // The prefix now carries the binary's ABSOLUTE path, quoted — a bare
        // name assumed `tokenade` on the PATH of the agent's shell (measured
        // false on a Windows VM: empty output, exit code 1). So the cut is
        // made on "… wrap", and the shell stays the authority on the rest.
        let (head, inner) = rewritten
            .split_once(" wrap ")
            .unwrap_or_else(|| panic!("{cmd:?}: unexpected rewrite {rewritten}"));
        assert!(
            head.starts_with('\'') && head.ends_with('\'') && head.contains('/'),
            "{cmd:?}: the binary must be named by a quoted absolute path, not by {head:?}"
        );

        // The shell is the authority on whether the quoting is right.
        let echoed = Command::new("sh")
            .arg("-c")
            .arg(format!("printf %s {inner}"))
            .output()
            .expect("spawn sh");
        assert_eq!(
            String::from_utf8_lossy(&echoed.stdout),
            cmd,
            "{cmd:?}: the escaped form does not parse back to the original: {rewritten}"
        );

        // Already-wrapped commands must not be wrapped twice.
        let again = serde_json::json!({
            "tool_name": "run_shell_command",
            "tool_input": {"command": rewritten},
        });
        let (out2, _e, _c) = run_hook(
            &["gemini-hook", "--agent", "qwen-code"],
            h.path(),
            &again.to_string(),
        );
        let v2: serde_json::Value = serde_json::from_str(out2.trim()).expect("json");
        assert!(
            v2["hookSpecificOutput"].is_null(),
            "{cmd:?}: an already-wrapped command was wrapped a second time: {out2}"
        );
    }
}

/// A RANGED `read_file` must not be redirected to a compacted copy.
///
/// The BeforeTool hook points a whole-file read at a folded temp file. With
/// `offset`/`limit` set, the line numbers refer to the ORIGINAL file, so
/// redirecting would hand back a different region than the caller asked for —
/// silently, with no error anywhere. The guard exists and carries its reason
/// (Gemini merges our input over the original, per `coreToolHookTriggers.ts`);
/// nothing had ever exercised it.
///
/// Both halves are asserted together: the redirect must happen without a range
/// and must not happen with one. Testing only the second would pass on a hook
/// that redirects nothing at all.
#[cfg(unix)]
#[test]
fn a_ranged_read_file_is_not_redirected_to_a_compacted_copy() {
    let h = TempHome::new("gemini-readfile");
    let big = h.path().join("big.log");
    let body: String = (0..800)
        .map(|i: usize| format!("line {i} — some payload text to make this worth folding"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&big, &body).expect("write");

    let redirect_target = |ti: serde_json::Value| -> Option<String> {
        let payload = serde_json::json!({"tool_name": "read_file", "tool_input": ti});
        let (out, _err, code) = run_hook(
            &["gemini-hook", "--agent", "qwen-code"],
            h.path(),
            &payload.to_string(),
        );
        assert_eq!(code, 0);
        let v: serde_json::Value = serde_json::from_str(out.trim()).expect("json");
        v["hookSpecificOutput"]["updatedInput"]["file_path"]
            .as_str()
            .map(str::to_string)
    };

    let whole = redirect_target(serde_json::json!({"file_path": big.to_string_lossy()}));
    let ranged = redirect_target(
        serde_json::json!({"file_path": big.to_string_lossy(), "offset": 10, "limit": 20}),
    );

    assert!(
        whole.is_some_and(|p| p != big.to_string_lossy()),
        "a whole-file read was not redirected — the control for this test does not hold"
    );
    assert!(
        ranged.is_none(),
        "a ranged read was redirected to {ranged:?}: the offsets refer to the ORIGINAL file, \
         so this hands back the wrong lines with no error"
    );
}

/// Cursor's `preToolUse` rewrite — the last rank-1 surface, and the one whose
/// failure mode is a HANG.
///
/// This hook rewrites Shell commands into `tokenade wrap '<cmd>'` via
/// `updated_input`. Its guards are not hygiene: `cmd_wrap` always pipes, so a
/// rewritten `vim`, `ssh` or `tail -f` leaves the user's agent waiting on a
/// process that will never finish on a pipe. The handler's own comment records
/// that these gates were once MISSING. Nothing had ever driven them.
///
/// The quoting is settled by a real `sh`, as in the Gemini rewrite: the
/// A launcher prefix must not hide an interactive program, on EVERY
/// agent envelope tokenade rewrites for.
///
/// `timeout 60 vim f` resolved to `timeout`, missed the exclude list and was
/// wrapped; `tokenade wrap 'timeout 60 vim f'` has to be killed by an outer
/// timeout. The gate existed three times over — the Claude-format path, the
/// Cursor path and the Antigravity path each resolved the program their own
/// way — so a fix applied to one left the other two hanging.
///
/// Driven through the three real envelopes rather than through the predicate:
/// a guard on the shared function stays green when a caller is reverted to its
/// own resolver, which is exactly the regression this pins.
#[cfg(unix)]
#[test]
fn no_agent_envelope_wraps_an_interactive_program_behind_a_launcher() {
    let h = TempHome::new("launcher-peel");
    // (label, payload builder, pointer to the rewritten command)
    let envelopes: [(&str, fn(&str) -> serde_json::Value); 3] = [
        ("claude", |cmd| {
            serde_json::json!({
                "session_id": "peel-1",
                "tool_name": "Bash",
                "tool_input": {"command": cmd},
            })
        }),
        ("cursor", |cmd| {
            serde_json::json!({
                "hook_event_name": "preToolUse",
                "session_id": "peel-1",
                "tool_name": "Shell",
                "tool_input": {"command": cmd},
            })
        }),
        ("antigravity", |cmd| {
            serde_json::json!({
                "toolCall": {"name": "run_command", "args": {"CommandLine": cmd}},
            })
        }),
    ];
    let rewrote = |build: fn(&str) -> serde_json::Value, cmd: &str| -> bool {
        let (out, _err, code) = run_hook(&["hookwrap-pretool"], h.path(), &build(cmd).to_string());
        assert_eq!(code, 0, "{cmd:?}: the hook failed");
        out.contains("tokenade wrap ") || out.contains("/tokenade wrap ")
    };

    for (agent, build) in envelopes {
        // Each of these hangs if wrapped: `wrap` pipes, and none of them
        // terminates on a pipe.
        for cmd in [
            "timeout 60 vim notes.txt",
            "timeout -k 5 120 vim notes.txt",
            "nohup less big.log",
            "env EDITOR=x vim notes.txt",
            "command less big.log",
            "nice -n 5 htop",
            "setsid top",
            "vim notes.txt",
            // Comment-led, and the reason the TYPED spelling is still asked
            // alongside the peeled one: `extract_first_meaningful_word` skips
            // comment lines, `program_basename` does not — it reads `#` as the
            // program. Drop the typed check and these two wrap, and hang.
            "# open the file\nvim notes.txt",
            "#!/bin/sh\nless big.log",
        ] {
            assert!(
                !rewrote(build, cmd),
                "{agent}: {cmd:?} was wrapped — it would never return"
            );
        }
        // The direction that pays for the peel: a launcher in front of a
        // command with a real compactor must still be wrapped. Without this
        // half, excluding everything would pass the block above.
        for cmd in [
            "timeout 60 cargo test",
            "timeout -k 5 120 cargo test",
            "nohup cargo build",
            "env FOO=1 git log --oneline",
            "cargo test",
        ] {
            assert!(
                rewrote(build, cmd),
                "{agent}: {cmd:?} lost its compactor to the peel"
            );
        }
    }
}

/// `TOKENADE_SESSION=` prefix is stripped first, since it is a shell
/// assignment and not part of the command.
#[cfg(unix)]
#[test]
fn the_cursor_pretool_rewrite_wraps_shell_and_leaves_interactive_alone() {
    let h = TempHome::new("cursor-pretool");
    let ask = |cmd: &str| -> Option<String> {
        // `hook_event_name: "preToolUse"` — lowercase — is what selects the
        // Cursor envelope in `hooks::mod`. Omitting it (my first attempt) sends
        // the payload down the Claude path, which rewrites nothing here and
        // reads as "the guard works" while testing another handler entirely.
        let payload = serde_json::json!({
            "hook_event_name": "preToolUse",
            "session_id": "cursor-sess-1",
            "tool_name": "Shell",
            "tool_input": {"command": cmd},
        });
        let (out, _err, code) = run_hook(&["hookwrap-pretool"], h.path(), &payload.to_string());
        assert_eq!(code, 0, "{cmd:?}: the hook failed");
        if out.trim().is_empty() {
            return None;
        }
        let v: serde_json::Value = serde_json::from_str(out.trim()).ok()?;
        v["updated_input"]["command"].as_str().map(str::to_string)
    };

    // Rewritten, and the escaping survives the shell.
    for cmd in ["journalctl -n 600", "grep -r 'needle' . | awk '{print $1}'"] {
        let got = ask(cmd).unwrap_or_else(|| panic!("{cmd:?}: no rewrite"));
        let inner = got
            .split_once("tokenade wrap ")
            .map(|(_, rest)| rest)
            .unwrap_or_else(|| panic!("{cmd:?}: unexpected rewrite {got}"));
        let echoed = Command::new("sh")
            .arg("-c")
            .arg(format!("printf %s {inner}"))
            .output()
            .expect("spawn sh");
        assert_eq!(
            String::from_utf8_lossy(&echoed.stdout),
            cmd,
            "{cmd:?}: the escaped form does not parse back: {got}"
        );
    }

    // Left alone. Each of these HANGS if wrapped: `wrap` pipes, and none of
    // them terminates on a pipe.
    for cmd in [
        "vim notes.txt",
        "ssh example.invalid",
        "tail -f /var/log/syslog",
        "tokenade wrap 'echo already'",
    ] {
        assert_eq!(
            ask(cmd),
            None,
            "{cmd:?} was rewritten into `tokenade wrap` — it would never return"
        );
    }
}

/// Every agent tokenade covers with a HOOK must be driven on its own contract
/// by one of the tests above.
///
/// The gap this closes was real and invisible: the shared handler was tested
/// through claude-code, so a new hook agent could be wired — its own payload
/// shape, its own shell-tool name, its own attribution — with nothing
/// exercising any of it, and every existing test would stay green. That is how
/// six of the nine got here.
///
/// The list below is DECLARED, not derived, on purpose: deriving it from the
/// same `coverage()` call it checks would make the test agree with itself. Add
/// an agent here only when a test in this file actually drives it.
#[test]
fn every_hook_covered_agent_has_a_contract_test() {
    // (slug, the test that drives its contract)
    const DRIVEN: &[(&str, &str)] = &[
        (
            "antigravity",
            "e2e_the_antigravity_hook_rewrites_the_command_and_asks_rather_than_allows              + e2e_the_antigravity_hook_stays_silent_on_a_non_shell_step",
        ),
        (
            "claude-code",
            "e2e_install_upgrades_stale_pretool_hook_matcher + hookwrap paths",
        ),
        (
            "copilot",
            "e2e_the_copilot_hook_folds_both_payload_shapes_and_credits_copilot",
        ),
        (
            "codex",
            "the_plain_hookwrap_agents_fold_and_credit_themselves",
        ),
        (
            "droid",
            "the_plain_hookwrap_agents_fold_and_credit_themselves",
        ),
        (
            "grok",
            "the_plain_hookwrap_agents_fold_and_credit_themselves",
        ),
        (
            "qwen-code",
            "the_gemini_family_aftertool_hook_folds_both_content_shapes",
        ),
        (
            "cursor",
            "the_cursor_pretool_rewrite_wraps_shell_and_leaves_interactive_alone",
        ),
    ];

    let mut missing = Vec::new();
    let mut hook_agents = 0;
    for a in tokenade_core::detect::Agent::all() {
        if !matches!(a.coverage(), tokenade_core::detect::Coverage::Hook(_)) {
            continue;
        }
        hook_agents += 1;
        if !DRIVEN.iter().any(|(slug, _)| *slug == a.as_str()) {
            missing.push(a.as_str());
        }
    }
    assert!(
        missing.is_empty(),
        "these agents are covered by a hook but no test drives their contract: {missing:?} — \
         wire one in this file, then add it to DRIVEN"
    );
    // The reverse direction: a slug listed here that stopped being a hook agent
    // means the list is stale and someone is reading a guarantee that moved.
    let stale: Vec<&str> = DRIVEN
        .iter()
        .map(|(slug, _)| *slug)
        .filter(|slug| {
            !tokenade_core::detect::Agent::all().iter().any(|a| {
                a.as_str() == *slug
                    && matches!(a.coverage(), tokenade_core::detect::Coverage::Hook(_))
            })
        })
        .collect();
    assert!(stale.is_empty(), "DRIVEN lists non-hook agents: {stale:?}");
    assert_eq!(
        hook_agents,
        DRIVEN.len(),
        "the hook-agent count moved; DRIVEN is out of date"
    );
}

/// The same promise on the channel this product's own guidance got wrong.
///
/// The test below it checks the PostToolUse channel. Claude Code is driven at
/// PreToolUse — the command is REWRITTEN rather than the result folded — and
/// nothing checked that rung, while two texts tokenade writes into an agent's
/// context said flatly that an absolute path "bypasses the tokenade hook, so
/// that command's output isn't compacted". Measured 2026-09-03: it does not.
/// Both halves are asserted, because the rewrite alone proves nothing if the
/// compactor then falls back to the generic fold for `/usr/bin/journalctl`.
#[test]
fn an_absolute_program_path_is_rewritten_and_folded_like_the_bare_name() {
    let h = TempHome::new("abs-path-pretool");
    let home = h.path();

    let rewritten = |cmd: &str| -> String {
        let payload = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": cmd},
        });
        let (out, err, code) = run_hook(&["hookwrap-pretool"], home, &payload.to_string());
        assert_eq!(code, 0, "{cmd}: hook failed: {err}");
        let v: serde_json::Value = serde_json::from_str(&out).expect("hook JSON");
        v["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let bare = rewritten("git status");
    assert!(
        bare.contains("wrap 'git status'"),
        "the CONTROL was not wrapped ({bare}) — this test can prove nothing"
    );
    let absolute = rewritten("/usr/bin/git status");
    assert!(
        absolute.contains("wrap '/usr/bin/git status'"),
        "an absolute program path was not wrapped: {absolute}"
    );

    // …and the compactor keys on the program NAME, not on the token it was
    // spelled with, or the wrap above would hand the generic fold a command
    // that has its own spec.
    //
    // The payload has to be one where argv DECIDES. A timestamped log folds to
    // the same bytes whatever argv says — the content router claims it — so an
    // equality measured on one holds even if argv[0] is ignored entirely, and
    // proves nothing about the promise. Hence the third reading: an unknown
    // program is the control that shows the spec was in play at all.
    let mut body =
        String::from("  UNIT                                LOAD   ACTIVE SUB     DESCRIPTION\n");
    // NAMED units, not `worker-{i:03}`: a numbered template is folded without
    // loss by the generic pass (`N= 000…199`), lighter than any per-command
    // answer, and argv stops deciding (2026-10-03).
    let (a, b) = (
        [
            "alpha", "bravo", "cedar", "delta", "ember", "fjord", "grove", "harbor", "iris",
            "juniper", "kestrel", "lumen", "maple", "nectar", "onyx", "pylon", "quartz", "raven",
            "sable", "tundra",
        ],
        [
            "sync", "relay", "cache", "index", "probe", "vault", "mirror", "audit", "beacon",
            "ledger",
        ],
    );
    for x in a {
        for y in b {
            body.push_str(&format!(
                "  {x}-{y}.service                  loaded active running {x} {y} daemon\n"
            ));
        }
    }
    body.push_str("\n200 loaded units listed.\n");

    let fold = |argv: &[&str]| -> String {
        let (out, _e, code) = run_with_stdin(argv, home, &body);
        assert_eq!(code, 0, "{argv:?} failed");
        out
    };
    let bare = fold(&["filter", "systemctl", "list-units", "--no-pager"]);
    let generic = fold(&["filter", "nosuchprog-xyz"]);
    assert_ne!(
        bare, generic,
        "argv[0] did not change the answer on this payload, so the equality below \
         would hold even if the program name were ignored — the fixture, not the product"
    );
    let absolute = fold(&["filter", "/usr/bin/systemctl", "list-units", "--no-pager"]);
    assert_eq!(
        bare, absolute,
        "the absolute spelling picked a different compactor"
    );
}

/// The scaffold promises an absolute path is intercepted exactly like the bare
/// name — on the hook channel.
///
/// Shipped in every rules file: "An absolute path (`/usr/bin/git`) is
/// intercepted exactly like `git` when hooks are installed; where interception
/// goes through your PATH instead, only the bare name is seen." The second half
/// is the shell proxy's business (aliases key on the bare name); the FIRST half
/// is a promise about this hook, and nothing checked it.
///
/// It matters because agents read that sentence and act on it: an agent told
/// absolute paths are safe will use them, and if the hook keyed on the literal
/// command string it would silently stop compacting for that agent.
///
/// Asserted as a COMPARISON, not a threshold — the bare form is the control, so
/// the test cannot pass by both forms failing to fold.
#[test]
fn an_absolute_program_path_folds_like_the_bare_name() {
    let body: String = (0..600)
        .map(|i: usize| {
            format!("2026-08-08T13:00:00Z INFO worker[{}] handled path=/api/v1/items status=200 dur={}ms", i % 8, 10 + i % 40)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let fold = |cmd: &str| -> usize {
        let payload = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": cmd},
            "tool_response": {"stdout": body, "stderr": "", "interrupted": false},
        });
        let h = TempHome::new("abs-path");
        let (out, err, code) =
            run_hook_as(Some("codex"), &["hookwrap"], h.path(), &payload.to_string());
        assert_eq!(code, 0, "{cmd}: hook failed: {err}");
        out.len()
    };

    let bare = fold("journalctl -n 600");
    assert!(
        bare * 5 < body.len(),
        "the CONTROL did not fold ({bare} bytes for {}) — this test can prove nothing",
        body.len()
    );
    // A fold is not enough: this payload is claimed by the CONTENT router, so
    // it folds to the same bytes under any argv, and both readings below would
    // agree even if the program name were dropped on the floor. Measured
    // 2026-09-03 — `journalctl -n 600` and `nosuchprog-xyz` returned 146 bytes
    // each. The spec-keyed assertion lives in the PreToolUse test above, which
    // uses a payload where argv decides; here we can at least record that this
    // one does NOT, so nobody reads more into it than it says.
    assert_eq!(
        bare,
        fold("nosuchprog-xyz"),
        "this payload turned out to be argv-sensitive — then assert equality with \
         the absolute spelling instead of a size threshold"
    );
    let absolute = fold("/usr/bin/journalctl -n 600");
    assert!(
        absolute * 5 < body.len(),
        "an absolute program path was NOT folded ({absolute} bytes vs {bare} for the bare \
         name) — every rules file the product ships promises it is"
    );
}

/// Antigravity CLI (`agy`) drives the same rung as Cursor: its `PostToolUse`
/// "expects an empty JSON object", so the result never comes back through a
/// hook and folding has to happen by REWRITING the command at `PreToolUse`.
///
/// The contract is camelCase protojson throughout (`toolCall.name`,
/// `toolCall.args`), the shell tool is `run_command`, its argument is
/// `CommandLine`, and the reply carries `overwrite` — "the modified tool call
/// is what actually executes". Read out of the shipped binary, not inferred
/// from the Claude family it resembles: a handler written for `tool_name` /
/// `tool_input` reads nothing here and answers nothing, which is exactly what
/// an unwired agent looks like from the outside.
#[test]
fn e2e_the_antigravity_hook_rewrites_the_command_and_asks_rather_than_allows() {
    let h = TempHome::new("antigravity-pretool");
    let payload = serde_json::json!({
        "conversationId": "c1",
        "stepIdx": 3,
        "toolCall": {
            "name": "run_command",
            "args": {"CommandLine": "git status"}
        }
    });
    let (out, _err, code) = run_hook(&["hookwrap-pretool"], h.path(), &payload.to_string());
    assert_eq!(code, 0, "the hook must not fail the step: {out}");

    let v: serde_json::Value =
        serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("not JSON: {out} ({e})"));
    let rewritten = v
        .pointer("/overwrite/CommandLine")
        .and_then(|c| c.as_str())
        .unwrap_or_else(|| panic!("no overwrite.CommandLine — Antigravity reads nothing: {out}"));
    assert!(
        rewritten.contains("wrap") && rewritten.contains("git status"),
        "the rewrite lost the user's command or never routed it: {rewritten}"
    );

    // "ask", NOT "allow". `allow` on a PreToolUse that fires for every shell
    // step would auto-approve the lot — a hook that exists to save tokens must
    // not quietly become a hook that removes the user's confirmation.
    assert_eq!(
        v["decision"].as_str(),
        Some("ask"),
        "a token-saving rewrite must not also grant approval: {out}"
    );
}

/// The other half of the same contract: a step that is NOT the shell tool gets
/// no opinion at all. Answering `ask` on every step would put a confirmation in
/// front of tool calls we have nothing to say about.
#[test]
fn e2e_the_antigravity_hook_stays_silent_on_a_non_shell_step() {
    let h = TempHome::new("antigravity-nonshell");
    let payload = serde_json::json!({
        "toolCall": {"name": "read_file", "args": {"Path": "/etc/hosts"}}
    });
    let (out, _err, code) = run_hook(&["hookwrap-pretool"], h.path(), &payload.to_string());
    assert_eq!(code, 0);
    assert!(
        out.trim().is_empty() || out.trim() == "{}",
        "the hook opined on a step it does not handle: {out}"
    );
}

/// #290 — a newline separates two commands exactly as `;` does, and the
/// exclusion gate did not know it. An agent writing `cd <dir>` on one line and
/// its real work on the next had the WHOLE call excluded from wrapping, so the
/// real work's output was never folded. Measured over 37 892 real Bash calls:
/// 5 223 excluded there, every one of them multi-line, not one a lone builtin.
///
/// Drives the real binary: the predicate alone would pass with the gate
/// restored, since `split_chain_segments` has known about `\n` all along.
#[test]
fn a_leading_cd_on_its_own_line_no_longer_excludes_the_work() {
    let home = crate::TempHome::new("newline-chain");
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "session_id": "t290",
        "tool_input": {"command": "cd /tmp\ngrep -rn needle ."},
    })
    .to_string();
    // `run_with_stdin` sets `TOKENADE_HOOK_DISABLED`, which exercises the
    // kill-switch path and nothing else — the first version of this guard
    // asserted against an empty decision and called it a failure.
    let (out, err, rc) = crate::run_hook_as(None, &["hookwrap-pretool"], home.path(), &payload);
    assert_eq!(rc, 0, "{err}");
    assert!(out.contains(" wrap "), "the grep must be wrapped: {out}");
    assert!(
        out.contains("grep -rn needle ."),
        "the work must survive intact: {out}"
    );
    // The builtin itself stays OUT of the wrapper — it runs in the outer
    // shell, which is the whole reason the exclusion existed.
    assert!(
        !out.contains("wrap 'cd") && !out.contains("wrap \\\"cd"),
        "cd must run in the outer shell: {out}"
    );
}

/// A genuinely solo builtin still has nothing to fold and is still skipped.
#[test]
fn a_lone_shell_builtin_is_still_excluded() {
    let home = crate::TempHome::new("lone-builtin");
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "session_id": "t290b",
        "tool_input": {"command": "cd /tmp"},
    })
    .to_string();
    let (out, _err, rc) = crate::run_hook_as(None, &["hookwrap-pretool"], home.path(), &payload);
    assert_eq!(rc, 0);
    assert!(!out.contains("wrap"), "nothing to fold: {out}");
}

/// A command that died on a signal read exactly like one that finished
/// quietly: `sh -c 'kill -SEGV $$'` wrapped gave exit 139 and **zero bytes of
/// output**, and a SIGKILLed loop gave 137 under a fold banner that invited
/// reading the shortness as compaction. The exit code was always right; nothing
/// ever SAID it.
mod a_killed_command_says_so {
    use super::*;

    fn wrap_it(h: &TempHome, cmd: &str) -> (String, String, i32) {
        crate::run(&["wrap", cmd], h.path())
    }

    #[test]
    fn a_segfault_is_named_and_keeps_its_code() {
        let h = TempHome::new("sig-segv");
        let (_out, err, code) = wrap_it(&h, "sh -c 'kill -SEGV $$'");
        assert_eq!(
            code, 139,
            "the 128+signal convention must survive; stderr:\n{err}"
        );
        assert!(
            err.contains("SIGSEGV"),
            "the model sees no output at all here; the signal is the only fact:\n{err}"
        );
        assert!(
            err.contains("did not finish"),
            "and it must say what that means"
        );
    }

    #[test]
    fn a_kill_is_named() {
        let h = TempHome::new("sig-kill");
        // The shell kills ITSELF, like the SIGSEGV case above. `timeout -s
        // KILL` reports 124 or 137 depending on the coreutils build and on
        // timing (137 here, 124 on the CI runner), which tested `timeout`.
        let (_out, err, code) = wrap_it(&h, "sh -c 'echo w; kill -KILL $$'");
        assert_eq!(code, 137, "stderr:\n{err}");
        assert!(err.contains("SIGKILL"), "stderr:\n{err}");
        assert!(
            err.contains("out-of-memory"),
            "SIGKILL on a build is most often the OOM killer, and that is the \
             one thing worth naming:\n{err}"
        );
    }

    /// `… | head -20` closes the pipe on purpose and the producer dies every
    /// time. Naming it would put a scary line under the most ordinary shell
    /// idiom there is.
    ///
    /// Asserted on `kill -PIPE $$`, NOT on `seq … | head -3`. The pipeline
    /// exits 0 — its status is `head`'s — so it never reaches the signal branch
    /// and the first version of this guard proved nothing: removing the
    /// exemption left it green.
    #[test]
    fn sigpipe_stays_silent() {
        let h = TempHome::new("sig-pipe");
        let (_out, err, code) = wrap_it(&h, "sh -c 'kill -PIPE $$'");
        assert_eq!(code, 141, "precondition: this is a SIGPIPE death");
        assert!(!err.contains("did not finish"), "stderr:\n{err}");

        // And the ordinary idiom is untouched either way.
        let (_out, err, code) = wrap_it(&h, "seq 1 100000 | head -3");
        assert_eq!(code, 0);
        assert!(!err.contains("did not finish"), "stderr:\n{err}");
    }

    #[test]
    fn an_ordinary_failure_stays_silent() {
        let h = TempHome::new("sig-none");
        let (_out, err, code) = wrap_it(&h, "exit 3");
        assert_eq!(code, 3, "a plain non-zero exit is not a signal death");
        assert!(!err.contains("did not finish"), "stderr:\n{err}");
    }

    /// `exit_code_of` has six callers and several may see the same status.
    #[test]
    fn it_is_said_once() {
        let h = TempHome::new("sig-once");
        let (_out, err, _) = wrap_it(&h, "sh -c 'kill -SEGV $$'");
        assert_eq!(err.matches("did not finish").count(), 1, "stderr:\n{err}");
    }
}

/// `recovery:offered` is the DENOMINATOR of `recovery:expanded` — the pair
/// exists to answer "when a fold stashes content and offers `expand-ref`, does
/// anyone come back for it?". Measured 2026-08-27 on a real ledger:
/// **54 offers / 1 058 719 tokens against 13 expansions / 15 077 869 tokens**,
/// fourteen times more recovered than was ever offered. Impossible, and the
/// cause is that only ONE of the two kinds of offer was booked:
/// `disclose::maybe_stash` books what the pointer REPLACED, while the fold's
/// drop-recovery pointer — the larger by far, a `grep -r` folding 14 000
/// matches — booked nothing while staying expandable by hash.
mod both_kinds_of_offer_are_booked {
    use super::*;

    fn offered_rows(home: &std::path::Path) -> Vec<u64> {
        let p = home.join(".tokenade").join("gain_estimated.jsonl");
        let Ok(t) = std::fs::read_to_string(&p) else {
            return Vec::new();
        };
        t.lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|d| d.get("op").and_then(|o| o.as_str()) == Some("recovery:offered"))
            .filter_map(|d| d.get("before").and_then(|b| b.as_u64()))
            .collect()
    }

    #[test]
    fn a_fold_that_drops_a_lot_books_what_it_dropped() {
        let h = TempHome::new("recovery-offered");
        assert!(
            offered_rows(h.path()).is_empty(),
            "precondition: a fresh ledger"
        );

        let (out, err, code) = crate::run(&["wrap", "seq 1 40000"], h.path());
        assert_eq!(code, 0, "stderr:\n{err}");
        assert!(
            out.contains("tokens folded away"),
            "precondition: this fold must attach a drop-recovery pointer:\n{}",
            &out[..out.len().min(200)]
        );

        let rows = offered_rows(h.path());
        assert!(
            !rows.is_empty(),
            "the offer was made and never booked, so `expanded` has no denominator"
        );
        assert!(
            rows.iter().any(|t| *t > 10_000),
            "the booked figure must be what the fold DROPPED, not a token or two: {rows:?}"
        );
    }

    /// Both sites, not one. The two offers are different moments — what the
    /// pointer REPLACED and what the fold DROPPED — so each books at its own,
    /// and a guard on one would not have seen the other missing.
    #[test]
    fn both_offer_sites_record() {
        for (file, what) in [
            (
                "crates/tokenade-core/src/disclose.rs",
                "the pointer that replaces content",
            ),
            (
                "crates/tokenade-cli/src/wrap/nudges.rs",
                "the pointer to what the fold dropped",
            ),
        ] {
            // The module's FAMILY: `record_recovery("offered")` moved down
            // into `disclose/gate.rs`, and reading only the trunk made this
            // guard say the stashing had STOPPED — when it had only changed
            // file.
            let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join(file);
            let src = tokenade_core::source_guard::module_family(
                p.parent().expect("a directory"),
                p.file_stem().expect("a name").to_string_lossy().as_ref(),
            );
            assert!(
                src.contains("record_recovery(\"offered\""),
                "{file} stopped booking {what}; `expanded` counts both, so the \
                 ratio goes impossible again"
            );
        }
    }
}

/// The redundant-bypass notice reaches the agent, through the hook.
///
/// Driven end to end on purpose. The notice was first placed on the
/// exclude-list path, a unit test on the function passed, and production never
/// reached it: `cmdline_requests_hook_bypass` returns a hundred lines EARLIER,
/// so a leading `TOKENADE_HOOK_DISABLED=1 sudo …` never got there. Feeding the
/// real hook is the only thing that says which return happens first.
#[test]
fn e2e_a_bypass_of_a_pass_through_command_is_named() {
    let h = TempHome::new("bypass-notice-wire");
    let payload = |sess: &str, cmd: &str| {
        serde_json::json!({
            "session_id": sess,
            "tool_name": "Bash",
            "tool_input": { "command": cmd }
        })
        .to_string()
    };

    let (out, _e, _c) = run_hook(
        &["hookwrap-pretool"],
        &h.dir,
        &payload("byp-a", "TOKENADE_HOOK_DISABLED=1 sudo -A systemctl status"),
    );
    assert!(
        out.contains("additionalContext") && out.contains("`sudo`"),
        "a leading bypass of a pass-through command must be named:\n{out}"
    );
    assert!(
        out.contains("changed nothing"),
        "and must say the variable did nothing:\n{out}"
    );

    // A `tokenade …` command is skipped for its own reason and says so.
    let (out, _e, _c) = run_hook(
        &["hookwrap-pretool"],
        &h.dir,
        &payload("byp-b", "TOKENADE_HOOK_DISABLED=1 tokenade gain"),
    );
    assert!(
        out.contains("wrapping itself"),
        "a bypass of tokenade itself must be named for what it is:\n{out}"
    );

    // Anti-vacuity, both ways: a command tokenade really folds gets nothing,
    // and neither does a pass-through command run WITHOUT the variable.
    for (why, sess, cmd) in [
        (
            "a real bypass",
            "byp-c",
            "TOKENADE_HOOK_DISABLED=1 cargo test --workspace",
        ),
        ("no bypass at all", "byp-d", "sudo -A systemctl status"),
    ] {
        let (out, _e, _c) = run_hook(&["hookwrap-pretool"], &h.dir, &payload(sess, cmd));
        assert!(
            !out.contains("changed nothing"),
            "{why}: the notice fired on {cmd:?}:\n{out}"
        );
    }
}

/// The rewritten command runs in a DIFFERENT directory from the one the hook
/// saw, so the tokenade it names must not depend on that directory.
///
/// `try_cd_peel` turns `cd X && cmd` into `cd X && <exe> wrap 'cmd'`. The
/// source says why the exe must be absolute — "survives PATH games, multiple
/// installs, and brew shadowing" — and nothing held it. A relative token there
/// does not degrade: the user's command is REPLACED by
/// `…/tokenade: No such file or directory`, and measured 2026-09-06, 59.1% of
/// bash commands (39 484 of 66 827) carry a leading `cd`.
///
/// Two acceptable forms, and only two: an ABSOLUTE path, or a bare name with
/// no separator (resolved through PATH, which no `cd` can move). Anything in
/// between is relative to a directory the rewrite has just left.
#[test]
fn e2e_the_rewritten_command_names_a_tokenade_no_cd_can_move() {
    let h = TempHome::new("rewrite-exe-absolute");
    // Both rewrite sites, because they each derive the exe separately and the
    // PowerShell one was outside this guard until the mutation said so.
    for (tool, cmd) in [
        ("Bash", "cd /tmp && cat /etc/hostname"),
        ("Bash", "cd /usr && ls -la"),
        ("Bash", "cat /etc/hostname"),
        ("PowerShell", "Get-ChildItem C:\\Windows"),
    ] {
        let pre = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": tool,
            "tool_input": {"command": cmd},
        });
        let (out, _err, _rc) = run_hook_as(None, &["hookwrap-pretool"], h.path(), &pre.to_string());
        let Ok(v) = serde_json::from_str::<serde_json::Value>(out.trim()) else {
            continue; // not rewritten (gated / excluded) — nothing to assert
        };
        let Some(rewritten) = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
        else {
            continue;
        };
        // The exe token is the first word that is neither a `cd …` prefix nor
        // a `NAME=value` env assignment.
        let tail = rewritten.rsplit("&&").next().unwrap_or(rewritten);
        // Skip the `cd` prefix, any `NAME=value` env assignment, and
        // PowerShell's call operator.
        let exe = tail
            .split_whitespace()
            .find(|w| !w.contains('=') && *w != "cd" && *w != "&")
            .unwrap_or("");
        assert!(
            !exe.is_empty(),
            "{cmd:?} rewrote to {rewritten:?} with no exe token"
        );
        let exe = exe.trim_matches('\'');
        assert!(
            exe.starts_with('/') || !exe.contains('/'),
            "{cmd:?} rewrote to an exe relative to the directory the rewrite \
             LEAVES: {exe:?} (full: {rewritten:?}). Absolute, or a bare PATH \
             name — nothing else survives the `cd`."
        );
        // Anti-vacuity: it must actually be tokenade being named.
        assert!(
            exe.contains("tokenade"),
            "{cmd:?}: exe token {exe:?} is not tokenade (full: {rewritten:?})"
        );
    }
}

/// The rewrite must not change what a command DOES.
///
/// Every guard we had on `hookwrap-pretool` asserts the SHAPE of the rewritten
/// string. None ran it. A rewrite splits a command line on `&&`, `||` and `;`,
/// and those characters also live inside `for … do … done`, `case … esac`,
/// heredocs, quoted strings and command substitutions — so the split is one
/// missed quote away from turning a loop into three broken fragments, on a
/// path that touches EVERY Bash call the agent makes.
///
/// So: run both, compare stdout and exit code. Payloads stay small on purpose
/// — a compactor is free to fold a large output, and folding is not what this
/// is about.
#[test]
fn e2e_a_rewritten_command_still_does_what_it_did() {
    let h = TempHome::new("rewrite-preserves-behaviour");
    let dir = h.path().join("cf");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("two.txt"), "beta\nalpha\n").unwrap();
    let d = dir.display().to_string();

    let cases: Vec<String> = vec![
        "for i in 1 2 3; do echo \"n=$i\"; done".to_string(),
        format!("if [ -f {d}/two.txt ]; then echo yes; else echo no; fi"),
        format!("while read -r l; do echo \"L:$l\"; done < {d}/two.txt"),
        "case x in x) echo matched;; *) echo other;; esac".to_string(),
        "i=0; while [ $i -lt 3 ]; do echo \"i=$i\"; i=$((i+1)); done".to_string(),
        "f(){ echo hi; }; f".to_string(),
        "echo \"a;b\" && echo two".to_string(),
        "echo 'x && y' ; echo after".to_string(),
        "echo $(echo one; echo two)".to_string(),
        "false || echo recovered".to_string(),
        "test -f /nope && echo yes || echo no".to_string(),
        "for i in 1 2; do\n  echo \"multi=$i\"\ndone".to_string(),
        format!("sort {d}/two.txt | while read x; do echo \"got $x\"; done"),
        "python3 - <<'PY'\nprint('here;doc && ok')\nPY".to_string(),
        format!("cat <<EOF | sort\nb\na\nEOF"),
        "ls /nonexistent 2>/dev/null; echo \"rc=$?\"".to_string(),
        // Two shapes whose past breakage the splitter's own comments record,
        // and which no test ever RAN. A `#` comment disables the rest of the
        // line: a split at the `&&` resurrected the half the user had turned
        // off. A backslash-newline is a line continuation bash removes before
        // parsing: re-quoting the segment text made the backslash a literal
        // argument, and `grep` started receiving `\\`.
        "echo one # && echo two".to_string(),
        // The continuation must sit in a CHAIN: without a separator the
        // command is one segment and the bail changes nothing, so the case
        // proves nothing (the mutation that removes the bail survived it).
        format!("echo first && grep beta \\\n  {d}/two.txt"),
    ];

    let mut rewritten_count = 0usize;
    for cmd in &cases {
        let pre = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": cmd},
        });
        let (out, _e, _rc) = run_hook_as(None, &["hookwrap-pretool"], h.path(), &pre.to_string());
        let Ok(v) = serde_json::from_str::<serde_json::Value>(out.trim()) else {
            continue;
        };
        let Some(rewritten) = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
        else {
            continue;
        };
        rewritten_count += 1;

        let run = |script: &str| {
            let mut c = Command::new("bash");
            c.arg("-c").arg(script);
            let o = e2e_env(&mut c, h.path())
                .env("TOKENADE_HOOK_DISABLED", "0")
                .output()
                .expect("run bash");
            (
                String::from_utf8_lossy(&o.stdout).trim().to_string(),
                o.status.code().unwrap_or(-1),
            )
        };
        let (want_out, want_rc) = run(cmd);
        let (got_out, got_rc) = run(rewritten);
        assert_eq!(
            want_out, got_out,
            "the rewrite changed what this command PRINTS\n  original : {cmd:?}\n  \
             rewritten: {rewritten:?}"
        );
        assert_eq!(
            want_rc, got_rc,
            "the rewrite changed this command's EXIT CODE\n  original : {cmd:?}\n  \
             rewritten: {rewritten:?}"
        );
    }
    assert!(
        rewritten_count >= 10,
        "only {rewritten_count} of {} cases were rewritten at all — the comparison \
         had almost nothing to compare, which is how this guard goes quietly vacuous",
        cases.len()
    );
}

/// The Glob fold ANNOUNCES itself on the shape the shipped host actually uses.
///
/// `handle_glob_posttool` drops vendored and build paths from a Glob result.
/// On the OBJECT shape it wrote its explanation into a field it invented,
/// `updatedToolOutput.tokenadeGlobFooter` — and the host never renders that.
/// Measured against the shipped Claude Code 2.1.263, a Glob result reaches the
/// model as `content: [...e.filenames, ...(e.truncated?[note]:[])].join("\n")`:
/// `filenames`, and nothing else. Measured on a 66-path listing: six paths
/// delivered, `numFiles` rewritten to 6 so the count agreed with the short
/// list, and no statement anywhere that sixty had been hidden. The host's own
/// Glob returns an object, so that was the live branch.
///
/// The STRING shape carried its footer inside the text and did announce, so one
/// command told the truth or not depending on which shape it was handed. Both
/// announce now, and neither says it twice: the object shape through
/// `additionalContext` (in the PostToolUse output schema, honoured by every
/// host), the string shape inline where `fold_text` already puts it.
#[test]
fn e2e_the_glob_fold_announces_itself_on_both_response_shapes() {
    let h = TempHome::new("glob-fold-announces");
    let kept: Vec<String> = (0..6).map(|i| format!("src/app/module{i}.ts")).collect();
    let noise: Vec<String> = (0..40)
        .map(|i| format!("node_modules/pkg{i}/dist/index.js"))
        .collect();
    let all: Vec<String> = kept.iter().chain(noise.iter()).cloned().collect();

    // --- object shape: what the shipped host sends.
    let payload = serde_json::json!({
        "tool_name": "Glob",
        "tool_input": {"pattern": "**/*"},
        "tool_response": {"filenames": all, "numFiles": all.len(), "truncated": false},
    })
    .to_string();
    let (out, _e, _c) = run_with_stdin(&["hookwrap-postread"], h.path(), &payload);
    let v: serde_json::Value = serde_json::from_str(out.trim()).expect("hook emitted JSON");
    let hs = &v["hookSpecificOutput"];
    let updated = &hs["updatedToolOutput"];

    // Anti-vacuity: the fold must actually have dropped something, or an
    // announcement would have nothing to announce.
    let served = updated["filenames"]
        .as_array()
        .expect("filenames array")
        .len();
    assert!(
        served < all.len(),
        "the fixture did not trigger a fold: {served} of {} served",
        all.len()
    );
    // The count must agree with the list it now describes. This is half of why
    // the silent fold was convincing: six paths AND `numFiles: 6` read as a
    // complete, small answer rather than a trimmed one.
    assert_eq!(
        updated["numFiles"].as_u64(),
        Some(served as u64),
        "numFiles must describe the list actually served\n{out}"
    );
    // The announcement reaches the model, through a channel the host renders.
    let ctx = hs["additionalContext"].as_str().unwrap_or("");
    assert!(
        ctx.contains("hidden"),
        "the object shape must say what it hid, on a channel the host serves; \
         additionalContext was {ctx:?}\n{out}"
    );
    // And NOT through the field that is rendered nowhere.
    assert!(
        updated.get("tokenadeGlobFooter").is_none(),
        "the footer must not ride a field of the replacement: the host shows \
         `filenames` and nothing else\n{out}"
    );

    // --- string shape: the footer is already a line of the listing, so the
    // announcement must not be sent a second time.
    let text = all.join("\n");
    let payload2 = serde_json::json!({
        "tool_name": "Glob",
        "tool_input": {"pattern": "**/*"},
        "tool_response": text,
    })
    .to_string();
    let (out2, _e, _c) = run_with_stdin(&["hookwrap-postread"], h.path(), &payload2);
    let v2: serde_json::Value = serde_json::from_str(out2.trim()).expect("hook emitted JSON");
    let hs2 = &v2["hookSpecificOutput"];
    let served2 = hs2["updatedToolOutput"].as_str().expect("string shape");
    assert!(
        served2.contains("hidden"),
        "the string shape announces inline, where `fold_text` puts it\n{out2}"
    );
    assert!(
        hs2.get("additionalContext").is_none(),
        "the string shape already carries the footer in its text; sending it \
         again would bill the caller twice for one sentence\n{out2}"
    );
}

/// The Glob hook scrubs what it serves, like every other channel.
///
/// A Glob result is a list of PATHS, and a path carries whatever the filesystem
/// was told to call it — a token in a directory name, a key-shaped segment, a
/// credentials directory. `websearch_post` states the rule this hook was the
/// exception to: "SECURITY: same scrub as every other served channel."
///
/// MEASURED against the published 1.1.8 and against this build before the fix:
/// a path segment shaped like an AWS access key reached the model verbatim
/// through this hook, while `wrap`, `read`, `filter`, `exec`, `web`,
/// `expand-ref` (`--raw` included), the Read PreToolUse hook and the Bash
/// PostToolUse hook all scrubbed the same string.
///
/// Both response shapes are covered, because the hook handles both and the
/// scrub had to be added to each. The footer is scrubbed too, as
/// defence-in-depth — it names only classifier-matched vendored directories
/// (`node_modules/`, `target/`), so no input reaches it with a secret in it and
/// this test does not pretend to cover that arm.
///
/// The credential below is the AWS documentation's own example key: it is
/// public, permanently invalid, and exists precisely to be written down.
#[test]
fn e2e_the_glob_hook_scrubs_the_paths_it_serves() {
    let h = TempHome::new("glob-scrub");
    const FAKE_KEY: &str = "AKIAIOSFODNN7EXAMPLE";
    let mut paths: Vec<String> = vec![
        "src/app/main.ts".to_string(),
        format!("{FAKE_KEY}/leaked.ts"),
    ];
    paths.extend((0..30).map(|i| format!("node_modules/pkg{i}/index.js")));

    // --- object shape, which the shipped host sends.
    let payload = serde_json::json!({
        "tool_name": "Glob",
        "tool_input": {"pattern": "**/*"},
        "tool_response": {"filenames": paths, "numFiles": paths.len()},
    })
    .to_string();
    let (out, _e, _c) = run_hook(&["hookwrap-postread"], h.path(), &payload);
    assert!(
        !out.contains(FAKE_KEY),
        "the object shape served a credential-shaped path verbatim\n{out}"
    );
    // Anti-vacuity, and the thing a blunt "redact everything" would break:
    // ordinary paths must still arrive intact, or the fold has destroyed the
    // listing it exists to serve.
    assert!(
        out.contains("src/app/main.ts"),
        "an ordinary path must survive the scrub\n{out}"
    );

    // --- string shape: same hook, other branch, same rule.
    let payload2 = serde_json::json!({
        "tool_name": "Glob",
        "tool_input": {"pattern": "**/*"},
        "tool_response": paths.join("\n"),
    })
    .to_string();
    let (out2, _e, _c) = run_hook(&["hookwrap-postread"], h.path(), &payload2);
    assert!(
        !out2.contains(FAKE_KEY),
        "the string shape served a credential-shaped path verbatim\n{out2}"
    );
    assert!(
        out2.contains("src/app/main.ts"),
        "an ordinary path must survive the scrub (string shape)\n{out2}"
    );
}

/// A user `deny`/`ask` rule is matched by Claude Code against the command the
/// hook RETURNS — wrapped, `rm -rf out` no longer started with `rm`. The hook
/// leaves a command such a rule may cover untouched, and a rewrite keeps the
/// rest of the tool input (`updatedInput` replaces it whole).
#[cfg(unix)]
#[test]
fn a_user_deny_rule_still_sees_the_command_and_a_rewrite_keeps_its_fields() {
    let h = TempHome::new("deny-rule");
    let project = h.path().join("proj");
    std::fs::create_dir_all(project.join(".claude")).unwrap();
    std::fs::write(
        project.join(".claude").join("settings.json"),
        r#"{"permissions":{"deny":["Bash(rm:*)","PowerShell(Remove-Item:*)"],"ask":["Bash(git push:*)"]}}"#,
    )
    .unwrap();
    let ask_tool = |tool: &str, cmd: &str| -> String {
        let payload = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "session_id": "deny-rule-sess",
            "cwd": project.display().to_string(),
            "tool_name": tool,
            "tool_input": {"command": cmd, "timeout": 300000, "run_in_background": true, "description": "probe"},
        });
        let (out, _err, code) = run_hook(&["hookwrap-pretool"], h.path(), &payload.to_string());
        assert_eq!(code, 0, "{cmd:?}: the hook failed");
        out
    };
    let ask = |cmd: &str| ask_tool("Bash", cmd);
    assert!(
        ask_tool(
            "PowerShell",
            "Get-ChildItem; Remove-Item -Recurse C:\\build"
        )
        .trim()
        .is_empty(),
        "PowerShell rule bypassed"
    );
    assert!(
        !ask_tool("PowerShell", "Get-ChildItem C:\\Windows")
            .trim()
            .is_empty(),
        "an unguarded PowerShell command is still wrapped"
    );
    for guarded in ["cd out && rm -rf build", "git status && git push --force"] {
        assert!(
            ask(guarded).trim().is_empty(),
            "{guarded:?} was rewritten past the user's rule"
        );
    }
    let out = ask("journalctl -n 600");
    let v: serde_json::Value = serde_json::from_str(out.trim()).expect("a rewrite");
    let input = &v["hookSpecificOutput"]["updatedInput"];
    assert!(
        input["command"].as_str().unwrap_or("").contains(" wrap "),
        "{out}"
    );
    assert_eq!(input["timeout"], 300000, "{out}");
    assert_eq!(input["run_in_background"], true, "{out}");
    assert_eq!(input["description"], "probe", "{out}");
}

/// A `Read(…)` deny rule is judged on the path the Read hook RETURNS; served
/// from a staged copy, a denied file went past it. The hook leaves it alone.
#[cfg(unix)]
#[test]
fn a_read_deny_rule_still_sees_the_path() {
    let h = TempHome::new("read-deny-rule");
    let project = h.path().join("proj");
    for d in [".claude", "private", "public"] {
        std::fs::create_dir_all(project.join(d)).unwrap();
    }
    std::fs::write(
        project.join(".claude").join("settings.json"),
        r#"{"permissions":{"deny":["Read(./private/**)"]}}"#,
    )
    .unwrap();
    let body: String = (0..3_000u64)
        .map(|i| {
            format!(
                "{i} GET /api/v{}/item?id={} status={} ms={}\n",
                i % 7,
                i * 7919 % 99_991,
                [200, 404, 500][(i % 3) as usize],
                i * 37 % 900
            )
        })
        .collect();
    let ask = |rel: &str| -> String {
        let file = project.join(rel);
        std::fs::write(&file, &body).unwrap();
        let payload = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "session_id": "read-deny-sess",
            "cwd": project.display().to_string(),
            "tool_name": "Read",
            "tool_input": {"file_path": file.display().to_string()},
        });
        let (out, _err, code) = run_hook(&["hookwrap-pretool"], h.path(), &payload.to_string());
        assert_eq!(code, 0, "{rel}: the hook failed");
        out
    };
    assert!(
        !ask("public/app.log").trim().is_empty(),
        "the control file must be served by the hook"
    );
    assert!(
        ask("private/app.log").trim().is_empty(),
        "a denied file was served past the user's rule"
    );
}

/// The Grep hook runs its own `rg` over the tree and stages what it found: a
/// file under a `Read(…)` deny rule could be quoted. With any Read rule the
/// hook steps aside; without one it still compacts.
#[cfg(unix)]
#[test]
fn a_read_rule_keeps_the_grep_hook_out() {
    let h = TempHome::new("grep-read-rule");
    let ask = |project: &std::path::Path| -> String {
        std::fs::create_dir_all(project.join("src")).unwrap();
        let body: String = (0..600u64)
            .map(|i| format!("fn needle_{i}() -> u64 {{ {} }}\n", i * 7919 % 10_007))
            .collect();
        std::fs::write(project.join("src").join("lib.rs"), body).unwrap();
        let payload = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "session_id": "grep-rule-sess",
            "cwd": project.display().to_string(),
            "tool_name": "Grep",
            "tool_input": {"pattern": "needle_", "path": project.display().to_string(), "output_mode": "content", "-n": true},
        });
        let (out, _err, code) = run_hook(&["hookwrap-pretool"], h.path(), &payload.to_string());
        assert_eq!(code, 0, "the hook failed");
        out
    };
    let open = h.path().join("open");
    assert!(
        !ask(&open).trim().is_empty(),
        "the control grep must be compacted by the hook"
    );
    let ruled = h.path().join("ruled");
    std::fs::create_dir_all(ruled.join(".claude")).unwrap();
    std::fs::write(
        ruled.join(".claude").join("settings.json"),
        r#"{"permissions":{"deny":["Read(./secrets/**)"]}}"#,
    )
    .unwrap();
    assert!(
        ask(&ruled).trim().is_empty(),
        "the Grep hook ran its own rg despite a Read rule"
    );
}

/// An image's saving is counted in VISION tokens, which the model bills as
/// such: the row says so (`billed`), so the valuation does not convert it.
#[test]
fn a_downscaled_image_row_is_stamped_billed() {
    let h = TempHome::new("image-billed");
    let file = h.path().join("big.png");
    image::RgbImage::from_fn(3000, 2000, |x, y| {
        image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x ^ y) % 239) as u8])
    })
    .save(&file)
    .unwrap();
    let payload = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "session_id": "image-billed-sess",
        "cwd": h.path().display().to_string(),
        "tool_name": "Read",
        "tool_input": {"file_path": file.display().to_string()},
    });
    let (out, err, code) = run_hook(&["hookwrap-pretool"], h.path(), &payload.to_string());
    assert_eq!(code, 0, "{out}{err}");
    let ledger =
        std::fs::read_to_string(h.path().join(".tokenade").join("gain.jsonl")).unwrap_or_default();
    let row = ledger
        .lines()
        .find(|l| l.contains("\"image-downscale\""))
        .unwrap_or_else(|| panic!("no image row:\n{ledger}\n{out}{err}"));
    assert!(row.contains("\"billed\":true"), "{row}");
}

/// #6 — a Read shrunk to a head slice gets its staged content back.
///
/// PreToolUse rewrites `limit` to the head; PostToolUse is handed that
/// REWRITTEN input. Staged under the caller's `limit`, the entry was never
/// found: a repeat `Read(offset 1, limit 60)` came back as 20 lines with no
/// « unchanged » note — a silent cut the model took for the whole slice.
#[test]
fn e2e_a_shrunk_read_delivers_its_staged_context() {
    let h = TempHome::new("shrunk-read-delivery");
    let marker = h.path().join(".tokenade").join("postread-hook-ok");
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, "").unwrap();
    let f = h.path().join("big.log");
    let body: String = (0..4_000)
        .map(|i| {
            format!(
                "2024-01-01T00:00:{:02}Z INFO worker {i} processed batch {i}\n",
                i % 60
            )
        })
        .collect();
    std::fs::write(&f, &body).unwrap();
    let pre = serde_json::json!({
        "hook_event_name": "PreToolUse", "session_id": "shrunk", "transcript_path": "/t",
        "tool_name": "Read",
        "tool_input": { "file_path": f.to_str().unwrap(), "offset": 1, "limit": 60 },
    })
    .to_string();
    let _ = run_hook(&["hookwrap-pretool"], h.path(), &pre);
    let (out, err, _) = run_hook(&["hookwrap-pretool"], h.path(), &pre);
    let v: serde_json::Value = serde_json::from_str(out.trim())
        .unwrap_or_else(|_| panic!("repeat Read not shrunk:\n{out}\n{err}"));
    let updated = v["hookSpecificOutput"]["updatedInput"].clone();
    assert_eq!(updated["limit"], 20, "{out}");
    let resp: String = body.lines().take(20).collect::<Vec<_>>().join("\n");
    let post = serde_json::json!({
        "hook_event_name": "PostToolUse", "session_id": "shrunk", "transcript_path": "/t",
        "tool_name": "Read", "tool_input": updated,
        "tool_response": { "type": "text", "file": { "content": resp } },
    });
    let (out, err, _) = run_hook(&["hookwrap-postread"], h.path(), &post.to_string());
    assert!(
        out.contains("additionalContext") && out.contains("read-dedup unchanged"),
        "staged note not delivered:\n{out}\n{err}"
    );
}

/// #8: in a Claude Code worktree-isolated agent, git must never be an operand
/// of `tokenade` (Claude Code refuses the call), yet its output still folds.
#[cfg(unix)]
#[test]
fn e2e_git_in_an_isolated_worktree_runs_plainly_and_still_folds() {
    let h = TempHome::new("worktree-git");
    let wt = "/repo/.claude/worktrees/agent-1";
    let pre = |cmd: &str| {
        let p = serde_json::json!({
            "hook_event_name": "PreToolUse", "session_id": "wt-1", "agent_id": "a1",
            "tool_name": "Bash", "tool_input": {"command": cmd}, "cwd": wt,
        });
        let (out, _err, code) = run_hook(&["hookwrap-pretool"], h.path(), &p.to_string());
        assert_eq!(code, 0);
        out
    };
    let rewritten = |cmd: &str| -> String {
        let out = pre(cmd);
        let v: serde_json::Value = serde_json::from_str(&out).expect(&out);
        v["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let run = |cmd: &str| {
        let mut c = Command::new("bash");
        // The hook names the binary as the suite spawned it: workspace-relative.
        c.arg("-c").arg(cmd);
        e2e_env(&mut c, h.path())
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .output()
            .unwrap()
    };

    let pending = h.path().join(".tokenade/read-pending");
    let staged = || {
        std::fs::read_dir(&pending)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("filterargv-"))
    };
    let cmd = rewritten("git log --oneline -3");
    assert!(staged());
    assert!(
        cmd.starts_with("set -o pipefail; git log --oneline -3 2>&1 | "),
        "{cmd}"
    );
    let after_git = &cmd[cmd.find(" | ").unwrap()..];
    assert!(
        !after_git.contains("git") && !cmd.contains(" wrap "),
        "{cmd}"
    );
    let out = run(&cmd);
    assert!(
        out.status.success(),
        "{cmd}\n{:?}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout)
    );
    // Same lines as git itself prints (a shallow CI checkout has fewer than 3).
    assert_eq!(out.stdout, run("git log --oneline -3").stdout);
    // `filter` received the git argv: the staged copy was consumed.
    assert!(!staged());

    // git's failure is the call's failure, as under `wrap`.
    assert!(!run(&rewritten("git nosuchsubcommand")).status.success());

    // Shapes the pipe form does not cover run unwrapped, never wrapped.
    for cmd in [
        "cd /tmp && git status",
        "git log | head -2",
        "ls; /usr/bin/git status",
    ] {
        assert!(pre(cmd).trim().is_empty(), "{cmd:?} was rewritten");
    }
    // Everything else in the worktree is wrapped as usual.
    assert!(rewritten("ls -la").contains(" wrap 'ls -la'"));
}
