// The tests of `gemini_hooks`.
//
// Spliced in by `include!` from `gemini_hooks.rs`: the same module, hence the
// same private access and the same `super::` paths, without a single line of
// visibility changed. It is the shape this repository already uses for 33
// other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod after_tool_tests {
    use super::{gemini_after_fold, gemini_redact_only};

    fn big_log() -> String {
        let mut s = String::new();
        for i in 0..500 {
            s.push_str(&format!(
                "2026-07-04T10:00:{:02} INFO worker {i} status=ok latency={}ms\n",
                i % 60,
                i % 13
            ));
        }
        s.push_str("2026-07-04T10:08:00 ERROR upstream timeout: order-service unreachable");
        s
    }

    #[test]
    fn folds_string_llmcontent_into_deny_reason() {
        let log = big_log();
        let input = serde_json::json!({
            "tool_name": "run_shell_command",
            "tool_response": { "llmContent": log, "returnDisplay": "…", "error": null }
        })
        .to_string();
        let (out, before, after) = gemini_after_fold(&input).expect("should fold");
        assert!(after < before);
        let ov: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(ov["decision"], "deny");
        let reason = ov["reason"].as_str().unwrap();
        assert!(reason.len() < log.len(), "reason folds the result");
        assert!(reason.contains("upstream timeout"), "needle preserved");
    }

    #[test]
    fn joins_structured_parts() {
        let log = big_log();
        let input = serde_json::json!({
            "tool_response": { "llmContent": [{ "text": log }], "error": null }
        })
        .to_string();
        assert!(gemini_after_fold(&input).is_some());
    }

    // Twin of the Copilot pin: a result carrying a tokenade banner is declined
    // by the fold (one authority per payload) and must still be scrubbed by
    // the redact-only fallback that runs immediately after it.
    //
    // The shape matters and cost a red test: Gemini's model-visible text is
    // `tool_response.llmContent`, not `output`. With the wrong key BOTH
    // functions returned `None` and the fold assertion passed vacuously — a
    // green test that proved nothing about the path it names.
    #[test]
    fn a_banner_carrying_result_still_reaches_the_redact_only_fallback() {
        let secret = format!("sk-ant-api03-{}", "A".repeat(80));
        let text = format!(
            "[tokenade:git] 82 line(s)\nkey: {secret}\n{}",
            "filler line to clear the 800-byte floor\n".repeat(40)
        );
        let input = serde_json::json!({
            "tool_name": "run_shell_command",
            "tool_response": { "llmContent": text, "error": null }
        })
        .to_string();
        assert!(
            gemini_after_fold(&input).is_none(),
            "one authority per payload: a folded result must not be folded again"
        );
        let out = gemini_redact_only(&input).expect("redact-only must still fire");
        assert!(
            !out.contains(&secret),
            "declining to fold leaked the key: {out}"
        );
    }

    // SECURITY (#4-followup): folded output scrubbed; small/error results
    // that decline the fold still get the redact-only deny; parts arrays
    // and web tools are left to their owners; clean text stays untouched.
    #[test]
    fn redact_only_and_fold_scrub_secrets() {
        let mut log = big_log();
        log.push_str("\nAPI_KEY=sk-ant-SECRET123\n");
        let input = serde_json::json!({
            "tool_name": "run_shell_command",
            "tool_response": { "llmContent": log, "error": null }
        })
        .to_string();
        let (out, _, _) = gemini_after_fold(&input).expect("folds");
        assert!(!out.contains("sk-ant-SECRET123"), "fold leaked: {out:.200}");

        // Small ERROR result with a secret → fold declines, redact-only fires.
        let err = serde_json::json!({
            "tool_name": "run_shell_command",
            "tool_response": {
                "llmContent": "curl: Authorization: Bearer TOPSECRET456 rejected",
                "error": { "message": "http 401" }
            }
        })
        .to_string();
        assert!(gemini_after_fold(&err).is_none(), "errors never fold");
        let red = gemini_redact_only(&err).expect("redact-only fires on errors");
        assert!(!red.contains("TOPSECRET456"), "leak: {red}");

        // Parts array (may carry images) → redact-only must NOT flatten it.
        let parts = serde_json::json!({
            "tool_response": { "llmContent": [{ "text": "token=TOPSECRET456" }], "error": null }
        })
        .to_string();
        assert!(gemini_redact_only(&parts).is_none());

        // Web tools are owned by gemini-afterhook.
        let web = serde_json::json!({
            "tool_name": "web_fetch",
            "tool_response": { "llmContent": "token=TOPSECRET456", "error": null }
        })
        .to_string();
        assert!(gemini_redact_only(&web).is_none());

        // Clean → pure passthrough.
        let clean = serde_json::json!({
            "tool_response": { "llmContent": "all fine", "error": null }
        })
        .to_string();
        assert!(gemini_redact_only(&clean).is_none());
    }

    #[test]
    fn passthrough_on_error_small_or_unknown() {
        // Error result → verbatim (None).
        let err = serde_json::json!({
            "tool_response": { "llmContent": big_log(), "error": { "message": "boom" } }
        })
        .to_string();
        assert!(gemini_after_fold(&err).is_none(), "error kept verbatim");
        // Small → None.
        assert!(gemini_after_fold(
            &serde_json::json!({"tool_response":{"llmContent":"tiny","error":null}}).to_string()
        )
        .is_none());
        // No tool_response → None.
        assert!(gemini_after_fold(&serde_json::json!({"x":1}).to_string()).is_none());
        assert!(gemini_after_fold("not json").is_none());
    }

    #[test]
    fn after_fold_skips_web_tools_owned_by_gemini_afterhook() {
        for tool in ["google_web_search", "web_fetch"] {
            let input = serde_json::json!({
                "tool_name": tool,
                "tool_response": { "llmContent": big_log(), "error": null }
            })
            .to_string();
            assert!(
                gemini_after_fold(&input).is_none(),
                "{tool} is owned by the specialized web compactor"
            );
        }
    }

    /// **None of our entries carries `env`, and that is measured, not style.**
    ///
    /// The docs shipped with qwen-code list `env: Record<string, string>` among
    /// the fields of a `command` hook. MEASURED on 2026-09-12 on `qwen 0.7.x`:
    /// an otherwise identical entry, which WORKS without that field, stops
    /// firing entirely as soon as it is added. No error, no log line, `--debug`
    /// silent: the hook disappears.
    ///
    /// Found by adding `TOKENADE_FORCE_GAIN=1` to our entries so that a dev
    /// binary would record its savings during a validation. Three runs were
    /// spent suspecting the event name, the `enabled` flag and the matcher
    /// before isolating the real variable — and the command we write ends with
    /// `2>/dev/null; exit 0`, so a failing hook says nothing by construction.
    ///
    /// Whatever needs an environment variable puts it IN the command.
    #[test]
    fn no_installed_hook_entry_carries_an_env_block() {
        let _g = crate::install::capability::cap_env_lock();
        std::env::remove_var("TOKENADE_NO_HOOK");
        std::env::remove_var("STS_NO_HOOK");
        let home = std::env::temp_dir().join(format!("tok-noenv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".qwen")).unwrap();
        let _ = super::install_qwen_cli_hook("tokenade", Some(&home), false);
        let mut offenders = Vec::new();
        for dir in [".qwen"] {
            let path = home.join(dir).join("settings.json");
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            let Some(events) = v["hooks"].as_object() else {
                continue;
            };
            for (event, entries) in events {
                for entry in entries.as_array().into_iter().flatten() {
                    for h in entry["hooks"].as_array().into_iter().flatten() {
                        if h.get("env").is_some() {
                            offenders.push(format!("{dir} / {event} / {}", h["name"]));
                        }
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "`env` on a hook entry makes it INERT in qwen-code: {offenders:?}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// **The two hosts do not speak the same vocabulary, and that is MEASURED.**
    ///
    /// Qwen Code is a fork of Gemini CLI, so this module long wrote it Gemini's
    /// events verbatim — `BeforeTool`, `AfterTool`, `BeforeAgent`. An event
    /// name a host does not know fires nothing AND says nothing: the file was
    /// read, the entries dead, the installer announced success.
    ///
    /// MEASURED on 2026-09-12 on the installed binary (`qwen 0.7.x`), with a
    /// `matcher: ""` probe that writes the received payload to a file: the only
    /// events that arrive are `PreToolUse` and `PostToolUse`, the payload
    /// carries Claude Code's keys (`hook_event_name`, `tool_name`,
    /// `tool_input`, `tool_response`), and the shell's `tool_name` is
    /// `run_shell_command` — NOT `Shell` or `Bash`, which qwen-code's shipped
    /// documentation gives as matcher examples.
    ///
    /// This test freezes that measurement. If one day it stops being true, it
    /// is the measurement that must be redone, not this test that must be
    /// relaxed.
    #[test]
    fn each_host_gets_the_event_names_its_binary_answers_to() {
        let qwen = super::HookDialect::for_config_dir(".qwen");
        assert_eq!(qwen.tool_event, "PreToolUse");
        assert_eq!(qwen.prompt_event, "UserPromptSubmit");
        assert!(
            qwen.fold_event.is_none(),
            "hook output substitution is not verified in Qwen; \
             an entry placed there would be written then removed in the same call"
        );
        assert!(
            qwen.needs_enabled_flag,
            "MEASURED: without `hooks.enabled`, NO hook fires in \
             Qwen Code — voir `for_config_dir`"
        );

        // The matcher is a regex evaluated on the observed `tool_name`.
        let re = regex::Regex::new(super::GEMINI_HOOK_MATCHER).expect("compilable matcher");
        for observed in ["run_shell_command", "read_file", "read_many_files"] {
            assert!(re.is_match(observed), "the matcher misses `{observed}`");
        }
        for other in ["Shell", "Bash", "ReadFile", "write_file"] {
            assert!(
                !re.is_match(other),
                "`{other}` is not a name these hosts emit: the matcher \
                 must not catch it"
            );
        }
    }
}

#[cfg(test)]
mod hint_tests {
    /// The AfterTool fold must reach the per-command compactors, not just
    /// content detection. Content detection cannot tell which program wrote a
    /// listing, so without the hint Gemini CLI and Qwen Code got the generic
    /// pass where every hook-family agent got the named one.
    #[test]
    fn gemini_after_fold_uses_the_command_hint() {
        // A `git log --oneline`: content detection does not recognise it (no
        // date, no author — 2026-10-03, once `ls -l` without `total` became a
        // listing it DOES recognise, the old fixture stopped telling the two
        // paths apart), and the named compactor folds it.
        let text: String = (0..300)
            .map(|i| {
                format!(
                    "{:08x} feat(mod{}): change number {i} touching the {} path\n",
                    (i as u64).wrapping_mul(2_654_435_761) as u32 | 0x1000_0000,
                    i % 17,
                    i % 5
                )
            })
            .collect();
        let with_hint = serde_json::json!({
            "tool_name": "run_shell_command",
            "tool_input": {"command": "git log --oneline"},
            "tool_response": {"llmContent": text, "error": null},
        })
        .to_string();
        let without = serde_json::json!({
            "tool_name": "run_shell_command",
            "tool_input": {},
            "tool_response": {"llmContent": text, "error": null},
        })
        .to_string();
        let (_, _, hinted_after) =
            super::gemini_after_fold(&with_hint).expect("hinted payload must fold");
        // Content detection leaves this text alone: no fold, so the bar is
        // the text itself.
        let plain_after = super::gemini_after_fold(&without).map_or(text.len(), |(_, _, a)| a);
        assert!(
            hinted_after < plain_after,
            "the command hint must beat content detection: {hinted_after} vs {plain_after}"
        );
    }
}

#[cfg(test)]
mod every_gemini_hook_command_fails_open {
    use super::gemini_hook_command;

    /// Read off the bundled Gemini CLI, not assumed: the host takes
    /// `stdout.trim() || stderr.trim()` as the hook's output, and
    /// `convertPlainTextToHookOutput` maps any exit code that is neither 0 nor
    /// `EXIT_CODE_NON_BLOCKING_ERROR` (1) to `{decision: "deny", reason: text}`.
    /// A relocated binary exits 127 with the shell's own error on stderr, so
    /// without the wrapper every matched tool call would be DENIED and the
    /// reason shown would be "No such file or directory".
    #[test]
    #[cfg(not(windows))]
    fn a_missing_binary_cannot_deny_a_tool_call() {
        for sub in [
            "gemini-afterhook",
            "gemini-hook --agent qwen-code",
            "gemini-lifecycle --agent kilo",
        ] {
            let cmd = gemini_hook_command("/nowhere/tokenade", sub);
            assert!(cmd.ends_with("; exit 0'"), "{sub}: {cmd}");
            assert!(cmd.contains("2>/dev/null"), "{sub}: {cmd}");
            assert!(
                !cmd.contains("exec "),
                "a failed exec is fatal in sh: {cmd}"
            );
        }
    }

    /// The subcommand and its arguments survive the wrapping, `--agent`
    /// attribution included.
    #[test]
    fn the_subcommand_survives_the_wrapper() {
        let cmd = gemini_hook_command("/opt/tokenade", "gemini-hook --agent qwen-code");
        assert!(cmd.contains("gemini-hook --agent qwen-code"), "{cmd}");
        assert!(cmd.contains("tokenade"), "{cmd}");
    }

    /// A bare `tokenade` on PATH still resolves through the wrapper.
    #[test]
    #[cfg(not(windows))]
    fn a_path_resolved_binary_still_works() {
        let cmd = gemini_hook_command("tokenade", "gemini-afterhook");
        assert!(cmd.starts_with("sh -c "), "{cmd}");
        assert!(cmd.contains("tokenade"), "{cmd}");
    }

    /// The single-declarer invariant: this helper must not grow a wrapper of
    /// its own, it must return exactly what `build_hook_command` returns.
    #[test]
    fn it_is_the_same_declarer_the_claude_writers_use() {
        assert_eq!(
            gemini_hook_command("/opt/tokenade", "gemini-afterhook"),
            super::super::hook_util::build_hook_command("/opt/tokenade", "gemini-afterhook", None)
        );
    }
}

#[cfg(test)]
mod a_settings_file_of_any_shape {
    use super::install_after_tool_hook;

    /// A user's `settings.json` may hold `"hooks": []` or `"hooks": null`
    /// (a hand edit, another tool). The AfterTool writer unwrapped it as an
    /// object and panicked the installer; it now resets it, as the BeforeTool
    /// writer always did.
    #[test]
    fn a_hooks_value_that_is_not_an_object_is_replaced_not_unwrapped() {
        for hooks in [serde_json::json!([]), serde_json::json!(null), serde_json::json!("x")] {
            let mut settings = serde_json::json!({ "hooks": hooks });
            assert!(install_after_tool_hook(&mut settings, "tokenade gemini-after"));
            assert!(settings["hooks"]["AfterTool"].is_array(), "{settings}");
        }
    }
}
