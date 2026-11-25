//! Tests for `install`, split out of the flat `tests.rs`.

use super::*;

#[test]
fn install_droid_hooks_targets_execute_tool() {
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let p1 = std::env::var_os("TOKENADE_NO_HOOK");
    std::env::remove_var("TOKENADE_NO_HOOK");
    let tmp = std::env::temp_dir().join(format!("tok-droid-hooks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let r = crate::install::install_droid_hooks("/opt/tokenade", None, Some(&tmp), false);
    assert!(
        matches!(r, HookOutcome::Installed(_)),
        "install outcome wrong: {r:?}"
    );
    let path = tmp.join(".factory").join("hooks.json");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    // Droid's shell tool is `Execute` — a `Bash` matcher would never fire.
    assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "Execute");
    assert_eq!(v["hooks"]["PostToolUse"][0]["matcher"], "Execute");
    assert!(matches!(
        crate::install::install_droid_hooks("/opt/tokenade", None, Some(&tmp), false),
        HookOutcome::AlreadyPresent
    ));
    match p1 {
        Some(x) => std::env::set_var("TOKENADE_NO_HOOK", x),
        None => std::env::remove_var("TOKENADE_NO_HOOK"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Every event key we write for Copilot must be one Copilot KNOWS,
/// either natively or through its PascalCase compat map. A key it does not
/// know is dropped in silence: the file installs, the hook never fires, and
/// nothing anywhere says so. That is how `PreCompact` came to be missing for
/// as long as it was — nothing was watching the set.
///
/// Both lists are read off the shipped `@github/copilot` `app.js`, the same
/// source the installer's comments cite: the native set is the `Set([...])`
/// the loader validates against, and the map is `AYr`.
#[test]
fn every_copilot_event_we_register_is_one_copilot_knows() {
    const NATIVE: &[&str] = &[
        "sessionStart",
        "sessionEnd",
        "userPromptSubmitted",
        "preToolUse",
        "preMcpToolCall",
        "postToolUse",
        "postToolUseFailure",
        "errorOccurred",
        "agentStop",
        "subagentStop",
        "subagentStart",
        "preCompact",
        "permissionRequest",
        "notification",
    ];
    const COMPAT: &[&str] = &[
        "SessionStart",
        "SessionEnd",
        "UserPromptSubmit",
        "PreToolUse",
        "PreMcpToolCall",
        "PostToolUse",
        "PostToolUseFailure",
        "ErrorOccurred",
        "Stop",
        "SubagentStop",
        "SubagentStart",
        "PreCompact",
        "PermissionRequest",
        "Notification",
    ];
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = std::env::temp_dir().join(format!("tok-copilot-events-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    crate::install::install_copilot_hooks("/opt/tokenade", None, Some(&tmp), false);
    let path = tmp.join(".copilot").join("hooks").join("tokenade.json");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let hooks = v["hooks"].as_object().expect("hooks object");
    assert!(!hooks.is_empty(), "no event registered at all");
    for key in hooks.keys() {
        assert!(
            NATIVE.contains(&key.as_str()) || COMPAT.contains(&key.as_str()),
            "`{key}` is not an event Copilot knows — it would be dropped in silence"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// The command REWRITE is tokenade's primary savings mechanism, and on
/// Copilot it was not registered at all — `install_cli.rs` said "Claude Code
/// only — other agents don't expose a comparable Pre hook today", which had
/// stopped being true for this host.
///
/// Every link asserted here is read off the shipped `@github/copilot` `app.js`:
/// `PreToolUse:"preToolUse"` sits in the PascalCase compat map (so Copilot
/// stamps `_vsCodeCompat` itself), the stamped payload comes from `S8s`, which
/// spreads `X$(t,e)` and adds `tool_name`/`tool_input`, and the response is read
/// as `c?.updatedInput ?? l?.modifiedArgs` with
/// `c = _vsCodeCompat ? l?.hookSpecificOutput : undefined`.
#[test]
fn copilot_gets_the_command_rewrite_in_the_channel_it_reads() {
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = std::env::temp_dir().join(format!("tok-copilot-pretool-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    crate::install::install_copilot_hooks("/opt/tokenade", None, Some(&tmp), false);
    let path = tmp.join(".copilot").join("hooks").join("tokenade.json");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let entry = v["hooks"]["PreToolUse"]
        .as_array()
        .and_then(|a| a.first())
        .expect(
            "no PreToolUse entry — Copilot gets the result fold and not the command rewrite,              which is where most of the saving is",
        );
    // PascalCase, because that is the key that makes Copilot stamp
    // `_vsCodeCompat` and hand the handler the snake_case payload it reads.
    assert!(
        v["hooks"].get("preToolUse").is_none(),
        "registered natively, the payload arrives camelCase and `hookwrap-pretool` finds          no tool_name — it would answer nothing, indistinguishable from not being wired"
    );
    assert_eq!(
        entry["matcher"].as_str(),
        Some("bash"),
        "Copilot tests the matcher against the lowercase RUNTIME tool name"
    );
    assert!(
        entry["bash"]
            .as_str()
            .is_some_and(|c| c.contains("hookwrap-pretool")),
        "the entry must run the rewrite handler: {entry}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn install_copilot_hooks_writes_versioned_hook_file() {
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = std::env::temp_dir().join(format!("tok-copilot-hooks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let r = crate::install::install_copilot_hooks("/opt/tokenade", None, Some(&tmp), false);
    assert!(
        matches!(r, HookOutcome::Installed(_)),
        "install outcome wrong: {r:?}"
    );
    let path = tmp.join(".copilot").join("hooks").join("tokenade.json");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    // Copilot merges hooks/*.json files; `version` is required. Native
    // `postToolUse` command hook (`bash` field) returns `modifiedResult` to
    // fold the tool output — matcher uses Claude tool names (bash→Bash,
    // view→Read, grep→Grep).
    assert_eq!(v["version"], 1);
    assert_eq!(v["hooks"]["postToolUse"][0]["matcher"], ".*");
    assert!(v["hooks"]["postToolUse"][0]["bash"]
        .as_str()
        .unwrap()
        .contains("hookwrap-copilot"));
    // The two events Copilot has and tokenade was not using. Written in the
    // CLAUDE spelling on purpose: Copilot's own loader walks a PascalCase→
    // native map (`SessionStart→sessionStart`,
    // `UserPromptSubmit→userPromptSubmitted`), moves the entry and stamps
    // `_vsCodeCompat` itself — which is what makes the Claude payload and the
    // `hookSpecificOutput` response shape apply, so the shared
    // `hookwrap-userprompt` / `hookwrap-sessionstart` handlers work here
    // unchanged. Registering ONE of Copilot's eleven events is what made the
    // compat bench report "no prompt-time hook exists for this agent".
    for (event, cmd) in [
        ("UserPromptSubmit", "hookwrap-userprompt"),
        ("SessionStart", "hookwrap-sessionstart"),
        // Copilot spawns subagents and joins `onSubagentStart`'s
        // `additionalContext` into the child's prompt. Without this entry a
        // Copilot subagent inherited the folding and none of the brief.
        ("subagentStart", "hookwrap-subagentstart"),
        // The compaction write-barrier. tokenade installs it on the
        // Claude family and registered it nowhere else, on a host whose own
        // event set has always carried the seam. Read off the shipped
        // `app.js`: `preCompact` is in the native set, `PreCompact` is in the
        // compat map, and the payload carries `sessionId` / `transcriptPath`
        // exactly like the two events above.
        ("PreCompact", "hookwrap-precompact"),
    ] {
        let entry = &v["hooks"][event][0];
        assert!(
            entry["bash"].as_str().is_some_and(|b| b.contains(cmd)),
            "{event} does not run {cmd}: {entry}"
        );
        assert!(
            entry.get("matcher").is_none(),
            "{event} carries a matcher, which Copilot tests against a runtime \
             tool name these events do not have: {entry}"
        );
    }
    assert!(matches!(
        crate::install::install_copilot_hooks("/opt/tokenade", None, Some(&tmp), false),
        HookOutcome::AlreadyPresent
    ));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn install_codex_passive_hooks_writes_lifecycle_hooks_idempotently() {
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let p1 = std::env::var_os("TOKENADE_NO_HOOK");
    let p2 = std::env::var_os("STS_NO_HOOK");
    std::env::remove_var("TOKENADE_NO_HOOK");
    std::env::remove_var("STS_NO_HOOK");
    let tmp = std::env::temp_dir().join(format!("tok-codex-hooks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let hooks_path = tmp.join(".codex").join("hooks.json");

    // dry-run writes nothing.
    let r = crate::install::install_codex_passive_hooks("/opt/tokenade", None, Some(&tmp), true);
    assert!(
        matches!(r, HookOutcome::Installed(_)),
        "dry-run outcome wrong"
    );
    assert!(!hooks_path.exists(), "dry-run wrote a file");

    // real install writes all five lifecycle events.
    let r = crate::install::install_codex_passive_hooks("/opt/tokenade", None, Some(&tmp), false);
    assert!(
        matches!(r, HookOutcome::Installed(_)),
        "install outcome wrong"
    );
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
    for ev in [
        "SessionStart",
        "PreToolUse",
        "PostToolUse",
        "UserPromptSubmit",
        "PreCompact",
    ] {
        assert!(v["hooks"][ev].is_array(), "missing event {ev}");
    }
    // PreToolUse Bash group is tagged and points at the reused subcommand.
    let pre = &v["hooks"]["PreToolUse"][0];
    assert_eq!(pre["matcher"], "Bash");
    assert_eq!(pre["hooks"][0]["_tag"], "tokenade-pretool");
    assert!(pre["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("hookwrap-pretool"));

    // idempotent: second install is a no-op, no duplicate group.
    let r2 = crate::install::install_codex_passive_hooks("/opt/tokenade", None, Some(&tmp), false);
    assert!(
        matches!(r2, HookOutcome::AlreadyPresent),
        "second install not idempotent"
    );
    let v2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&hooks_path).unwrap()).unwrap();
    assert_eq!(v2["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);

    match p1 {
        Some(x) => std::env::set_var("TOKENADE_NO_HOOK", x),
        None => std::env::remove_var("TOKENADE_NO_HOOK"),
    }
    match p2 {
        Some(x) => std::env::set_var("STS_NO_HOOK", x),
        None => std::env::remove_var("STS_NO_HOOK"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn compact_read_to_temp_redirects_big_files_only() {
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let saved_home = std::env::var_os("HOME");
    let saved_agent = std::env::var_os("TOKENADE_AGENT");
    let tmp = std::env::temp_dir().join(format!("tok-gem-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    std::env::set_var("HOME", &tmp); // isolate the gain ledger

    // Big, compactable JSON file → redirect to a smaller temp under
    // cwd/.tokenade/reads.
    let body = format!(
        "[{}]",
        (0..400)
            .map(|i| format!("{{\"id\":{i},\"name\":\"item{i}\",\"v\":{}}}", i * 7))
            .collect::<Vec<_>>()
            .join(",")
    );
    let big = tmp.join("data.json");
    std::fs::write(&big, &body).unwrap();
    let cwd = tmp.to_string_lossy().to_string();
    let temp = compact_read_to_temp(big.to_str().unwrap(), Some(&cwd), None)
        .expect("a big compactable file should redirect");
    assert!(std::path::Path::new(&temp).exists());
    assert!(
        temp.contains(".tokenade"),
        "temp lives under the workspace .tokenade dir"
    );
    assert!(
        std::fs::read_to_string(&temp).unwrap().len() < body.len(),
        "redirected temp must be smaller than the original"
    );

    // Small file → no redirect (passthrough).
    let small = tmp.join("small.txt");
    std::fs::write(&small, "tiny").unwrap();
    assert!(compact_read_to_temp(small.to_str().unwrap(), Some(&cwd), None).is_none());

    // Multi-langue: a large file with CJK / accented / emoji content must
    // compact without mojibake (the temp must stay valid UTF-8 and keep the
    // non-ASCII text intact).
    let intl = tmp.join("intl.json");
    let intl_body = format!(
        "[{}]",
        (0..400)
            .map(|i| format!("{{\"id\":{i},\"nom\":\"café—日本語—😀 #{i}\"}}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    std::fs::write(&intl, &intl_body).unwrap();
    let it = compact_read_to_temp(intl.to_str().unwrap(), Some(&cwd), None)
        .expect("intl file should redirect");
    let compacted = std::fs::read_to_string(&it).expect("temp must be valid UTF-8");
    assert!(compacted.len() < intl_body.len());
    assert!(
        compacted.contains("日本語") && compacted.contains("café"),
        "non-ASCII content must survive compaction"
    );

    match saved_home {
        Some(v) => std::env::set_var("HOME", v),
        None => std::env::remove_var("HOME"),
    }
    match saved_agent {
        Some(v) => std::env::set_var("TOKENADE_AGENT", v),
        None => std::env::remove_var("TOKENADE_AGENT"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// #775 — lock-in that `group_matches_tokenade_hook` recognises
/// both properly tagged AND untagged-but-recognised entries (Claude
/// Code's settings editor strips `_tag` on save, and pre-#604
/// installs never had it).
#[test]
fn group_matches_tokenade_hook_tagged() {
    let g = serde_json::json!({
        "matcher": "Bash",
        "hooks": [{
            "type": "command",
            "command": "sh -c '/x/tokenade hookwrap'",
            "_tag": "tokenade"
        }]
    });
    let (matched, has_tag) = group_matches_tokenade_hook(&g, "tokenade", "hookwrap");
    assert!(matched, "tagged entry should match");
    assert!(has_tag, "_tag should be detected");
}

#[test]
fn group_matches_tokenade_hook_untagged_heals() {
    // Real-world reproduction: settings.json contains the wrapped
    // shell command but `_tag` has been stripped (Claude Code UI
    // round-trip drops unknown JSON fields).
    let g = serde_json::json!({
        "matcher": "Bash",
        "hooks": [{
            "type": "command",
            "command": "sh -c 'TOKENADE_DEBUG_LOG='\\''/home/u/.tokenade/debug.log'\\'' exec '\\''/home/u/.local/bin/tokenade'\\'' hookwrap'"
        }]
    });
    let (matched, has_tag) = group_matches_tokenade_hook(&g, "tokenade", "hookwrap");
    assert!(
        matched,
        "untagged-but-installed entry must still be recognised"
    );
    assert!(
        !has_tag,
        "has_tag must be false so caller treats as stale + re-adds the tag"
    );
}

#[test]
fn group_matches_tokenade_hook_subcmd_no_overlap() {
    // `hookwrap-pretool` must NOT match when looking for `hookwrap`.
    let g = serde_json::json!({
        "hooks": [{
            "type": "command",
            "command": "sh -c '/x/tokenade hookwrap-pretool'"
        }]
    });
    let (matched, _) = group_matches_tokenade_hook(&g, "tokenade", "hookwrap");
    assert!(
        !matched,
        "subcommand suffix overlap must not produce a false-positive"
    );
}

#[test]
fn group_matches_tokenade_hook_unrelated_command() {
    let g = serde_json::json!({
        "matcher": "Read",
        "hooks": [{ "type": "command", "command": "echo hello" }]
    });
    let (matched, _) = group_matches_tokenade_hook(&g, "tokenade", "hookwrap");
    assert!(!matched, "non-tokenade command must not match");
}

/// #777 — defensive lock-in. If a caller forgets to map a new tag
/// to its dispatch subcommand and passes `subcmd=""`, the helper
/// must refuse to substring-match (`" "` matches almost every
/// command). The `_tag` path still works for properly tagged entries.
#[test]
fn group_matches_tokenade_hook_empty_subcmd_safe() {
    // Untagged entry with any space in the command — pre-#777 this
    // would have false-matched on `format!(" {} ", "")` == "  ".
    let untagged = serde_json::json!({
        "matcher": "Bash",
        "hooks": [{ "type": "command", "command": "echo hello world" }]
    });
    let (matched, has_tag) = group_matches_tokenade_hook(&untagged, "tokenade-unknown", "");
    assert!(
        !matched,
        "empty subcmd MUST NOT false-positive on unrelated commands"
    );
    assert!(!has_tag);

    // Properly tagged entry still recognised via the _tag path even
    // when subcmd is empty (the early `has_tag` return runs first).
    let tagged = serde_json::json!({
        "matcher": "Bash",
        "hooks": [{
            "type": "command",
            "command": "echo whatever",
            "_tag": "tokenade-unknown"
        }]
    });
    let (matched, has_tag) = group_matches_tokenade_hook(&tagged, "tokenade-unknown", "");
    assert!(matched, "tagged entries match regardless of subcmd");
    assert!(has_tag);
}

#[test]
fn ensure_array_entry_creates_empty_when_missing() {
    let mut v = serde_json::json!({});
    let map = v.as_object_mut().unwrap();
    let arr = ensure_array_entry(map, "PreToolUse");
    assert!(arr.is_empty());
    assert!(v["PreToolUse"].is_array());
}

#[test]
fn ensure_array_entry_preserves_existing_array() {
    let mut v = serde_json::json!({"PreToolUse": [{"hooks": []}]});
    let map = v.as_object_mut().unwrap();
    let arr = ensure_array_entry(map, "PreToolUse");
    assert_eq!(arr.len(), 1);
}

#[test]
fn ensure_array_entry_clobbers_non_array_existing_value() {
    // Regression for #486: a user-edited settings.json with
    // hooks.PreToolUse as an OBJECT (instead of array) would
    // panic naive `.as_array_mut().unwrap()`. The helper now
    // clobbers with [] so the installer can still proceed.
    let mut v = serde_json::json!({"PreToolUse": {"oops": "wrong-shape"}});
    let map = v.as_object_mut().unwrap();
    let arr = ensure_array_entry(map, "PreToolUse");
    assert!(arr.is_empty(), "non-array value must be clobbered to []");
    assert!(v["PreToolUse"].is_array());
}

#[test]
fn ensure_array_entry_clobbers_string_value_too() {
    let mut v = serde_json::json!({"PostToolUse": "not-an-array"});
    let map = v.as_object_mut().unwrap();
    let arr = ensure_array_entry(map, "PostToolUse");
    assert!(arr.is_empty());
}

#[test]
fn hook_install_writes_settings_json_with_marker() {
    let d = tmp("hook_install");
    match install_claude_code_hook(&d, "/usr/local/bin/tokenade", None, false, None, false) {
        HookOutcome::Installed(path) => {
            let s = fs::read_to_string(path).unwrap();
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();
            let arr = v["hooks"]["PostToolUse"].as_array().unwrap();
            assert_eq!(arr.len(), 1);
            // Bash (gain log + search hint) AND Read (skeleton/auto
            // read-hint via handle_read_posttool) are both handled by
            // `cmd_hookwrap`, so the matcher covers both tools.
            assert_eq!(arr[0]["matcher"], "Bash|Read");
            assert_eq!(arr[0]["hooks"][0]["_tag"], "tokenade");
            // Command is POSIX-quoted now to survive spaces/quotes
            // in the exe path — check semantic content rather than
            // the literal pre-escape string.
            let cmd = arr[0]["hooks"][0]["command"].as_str().unwrap();
            assert!(cmd.contains("/usr/local/bin/tokenade"));
            assert!(cmd.contains("hookwrap"));
        }
        other => panic!(
            "expected Installed, got {:?}",
            match other {
                HookOutcome::AlreadyPresent => "AlreadyPresent",
                HookOutcome::OptedOut => "OptedOut",
                HookOutcome::Failed(_) => "Failed",
                _ => "?",
            }
        ),
    }
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn hook_install_is_idempotent() {
    let d = tmp("hook_idem");
    let _ = install_claude_code_hook(&d, "/x/tokenade", None, false, None, false);
    match install_claude_code_hook(&d, "/x/tokenade", None, false, None, false) {
        HookOutcome::AlreadyPresent => {}
        _ => panic!("second install should detect existing tag"),
    }
    // And only ONE entry should be present.
    let s = fs::read_to_string(d.join(".claude").join("settings.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
    let _ = fs::remove_dir_all(&d);
}

/// Auto-repair of the "hook wired to an updater temp binary" incident:
/// a settings.json whose tokenade PostToolUse command points at a
/// vanished `.tokenade.new.<pid>` path must be HEALED (Refreshed) to the
/// stable install path on the next hook upsert — not left dead. This is
/// exactly what `ensure_hooks` does on startup once `stable_hook_exe`
/// yields the stable path, so the already-broken fleet self-repairs.
#[test]
fn hook_install_heals_temp_new_pid_command() {
    let d = tmp("hook_heal_temp");
    // Seed the broken state: install pointing at the transient temp path
    // the buggy updater probe baked in.
    let temp = "/home/studio/.npm-global/lib/node_modules/@tokenade/cli/vendor/.tokenade.new.9087";
    let _ = install_claude_code_hook(&d, temp, None, false, None, false);
    let stale = fs::read_to_string(d.join(".claude").join("settings.json")).unwrap();
    assert!(
        stale.contains(".tokenade.new.9087"),
        "precondition: broken cmd seeded"
    );

    // Re-run the upsert with the STABLE destination (what stable_hook_exe
    // now resolves the temp path to). Must heal, not duplicate.
    let stable = "/home/studio/.npm-global/lib/node_modules/@tokenade/cli/vendor/tokenade";
    match install_claude_code_hook(&d, stable, None, false, None, false) {
        HookOutcome::Refreshed(_) => {}
        HookOutcome::AlreadyPresent => panic!("expected Refreshed (heal), got AlreadyPresent"),
        HookOutcome::Installed(_) => panic!("expected Refreshed (heal), got Installed"),
        _ => panic!("expected Refreshed (heal), got other"),
    }
    let s = fs::read_to_string(d.join(".claude").join("settings.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    let arr = v["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(arr.len(), 1, "heal must not duplicate the group");
    let cmd = arr[0]["hooks"][0]["command"].as_str().unwrap();
    assert!(cmd.contains("vendor/tokenade"), "healed to stable path");
    assert!(!cmd.contains(".tokenade.new."), "temp path must be gone");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn postread_hook_install_writes_read_grep_matcher() {
    let d = tmp("postread_install");
    // Per-project (global=false) so the test never touches the real
    // ~/.tokenade marker file.
    match install_claude_code_postread_hook(&d, "/x/tokenade", None, false, None, false) {
        HookOutcome::Installed(path) => {
            let s = fs::read_to_string(path).unwrap();
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();
            let arr = v["hooks"]["PostToolUse"].as_array().unwrap();
            assert_eq!(arr.len(), 1);
            assert_eq!(arr[0]["matcher"], "Read|Grep|Glob");
            assert_eq!(arr[0]["hooks"][0]["_tag"], "tokenade-postread");
            let cmd = arr[0]["hooks"][0]["command"].as_str().unwrap();
            assert!(cmd.contains("hookwrap-postread"));
        }
        _ => panic!("expected Installed"),
    }
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn postread_hook_install_is_idempotent() {
    let d = tmp("postread_idem");
    let _ = install_claude_code_postread_hook(&d, "/x/tokenade", None, false, None, false);
    match install_claude_code_postread_hook(&d, "/x/tokenade", None, false, None, false) {
        HookOutcome::AlreadyPresent => {}
        _ => panic!("second install should detect existing tag"),
    }
    let s = fs::read_to_string(d.join(".claude").join("settings.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn install_claude_code_hook_dry_run_does_not_modify_disk() {
    // Extension of #431/#432. install --dry-run must also avoid
    // touching ~/.claude/settings.json (the hook installer was
    // previously called unconditionally). Verify by checking that
    // no settings.json exists on disk after a dry-run call against
    // a fresh tempdir.
    let d = tmp("hook-dry");
    let r = d.join("project");
    std::fs::create_dir_all(&r).unwrap();
    let outcome = install_claude_code_hook(
        &r,
        "/x/tokenade",
        None,
        false, // project-local install
        None,
        true, // dry-run
    );
    assert!(
        matches!(outcome, HookOutcome::Installed(_)),
        "dry-run hook installer should report Installed (the would-be path)"
    );
    let settings = r.join(".claude").join("settings.json");
    assert!(
        !settings.exists(),
        "dry-run hook installer wrote settings.json to disk at {}",
        settings.display()
    );
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn post_upgrade_refresh_is_presence_gated() {
    // P2c — the silent post-upgrade refresh must be a pure
    // "heal what's installed" pass: it never installs hooks for a
    // user who never opted in, never touches debug-log installs,
    // and DOES heal a drifted opted-in suite (stale exe path →
    // current exe, missing suite hooks added).

    // 1. No tokenade marker anywhere → nothing written.
    let d1 = tmp("pur-untouched");
    std::fs::create_dir_all(d1.join(".claude")).unwrap();
    let s1 = d1.join(".claude").join("settings.json");
    fs::write(&s1, "{\"statusLine\":{\"type\":\"command\"}}").unwrap();
    fs::write(d1.join(".bashrc"), "# untouched\n").unwrap();
    post_upgrade_refresh_installed_surfaces(Some(&d1));
    assert_eq!(
        fs::read_to_string(&s1).unwrap(),
        "{\"statusLine\":{\"type\":\"command\"}}",
        "non-opted-in settings.json must not be touched"
    );
    assert_eq!(
        fs::read_to_string(d1.join(".bashrc")).unwrap(),
        "# untouched\n"
    );
    assert!(
        !d1.join(".tokenade").join("shellproxy.sh").exists(),
        "shell proxy must not be installed fresh by a background refresh"
    );

    // 2. Drifted opted-in suite → healed in place, suite completed.
    let d2 = tmp("pur-heal");
    std::fs::create_dir_all(d2.join(".claude")).unwrap();
    let s2 = d2.join(".claude").join("settings.json");
    fs::write(
        &s2,
        "{\"hooks\":{\"PostToolUse\":[{\"matcher\":\"Bash\",\"hooks\":[{\"type\":\"command\",\"command\":\"'/old/path/tokenade' hookwrap\",\"_tag\":\"tokenade\"}]}]}}",
    )
    .unwrap();
    post_upgrade_refresh_installed_surfaces(Some(&d2));
    let after = fs::read_to_string(&s2).unwrap();
    assert!(
        !after.contains("/old/path/tokenade"),
        "stale exe path must be healed, got:\n{after}"
    );
    assert!(
        after.contains("tokenade-postread") && after.contains("tokenade-index"),
        "suite hooks added in newer versions count as drift of the suite"
    );
    assert!(
        after.contains("tokenade-pretool"),
        "the PreToolUse Bash-rewrite is part of the default suite now, so \
         healing a drifted opted-in suite must add it, got:\n{after}"
    );

    // 3. Debug-log install (the DEFAULT `tokenade install` shape) →
    // healed like any other, with the debug-log path reconstructed
    // and preserved in the refreshed commands.
    let d3 = tmp("pur-debug");
    std::fs::create_dir_all(d3.join(".claude")).unwrap();
    let s3 = d3.join(".claude").join("settings.json");
    let drifted_dbg = build_hook_command("/old/path/tokenade", "hookwrap", Some("/tmp/d.log"));
    fs::write(
        &s3,
        serde_json::to_string(&serde_json::json!({"hooks":{"PostToolUse":[{
            "matcher":"Bash",
            "hooks":[{"type":"command","command":drifted_dbg,"_tag":"tokenade"}]
        }]}}))
        .unwrap(),
    )
    .unwrap();
    post_upgrade_refresh_installed_surfaces(Some(&d3));
    let after3 = fs::read_to_string(&s3).unwrap();
    assert!(
        !after3.contains("/old/path/tokenade"),
        "debug-log install must still be healed, got:\n{after3}"
    );
    assert!(
        after3.contains("TOKENADE_DEBUG_LOG") && after3.contains("/tmp/d.log"),
        "the user's debug-log path must survive the refresh, got:\n{after3}"
    );

    // 4. Unparseable debug entry → left strictly alone (never
    // half-refresh into a debug-log-less shape).
    let d4 = tmp("pur-debug-weird");
    std::fs::create_dir_all(d4.join(".claude")).unwrap();
    let s4 = d4.join(".claude").join("settings.json");
    let weird = "{\"hooks\":{\"PostToolUse\":[{\"matcher\":\"Bash\",\"hooks\":[{\"type\":\"command\",\"command\":\"custom-runner TOKENADE_DEBUG_LOG#mangled /old/tokenade hookwrap\",\"_tag\":\"tokenade\"}]}]}}";
    fs::write(&s4, weird).unwrap();
    post_upgrade_refresh_installed_surfaces(Some(&d4));
    assert_eq!(
        fs::read_to_string(&s4).unwrap(),
        weird,
        "unreconstructable debug installs must never be rewritten"
    );

    for d in [d1, d2, d3, d4] {
        let _ = fs::remove_dir_all(&d);
    }
}

#[test]
fn extract_debug_log_from_settings_reads_env_block_then_hook_command() {
    // New Windows location: the path lives in the settings-level `env`
    // block (the cmd /C inline `set` wrapper was dropped). Upgrades must
    // recover it from there.
    let win = r#"{
      "env": {"TOKENADE_DEBUG_LOG": "C:\\Users\\Admin\\.tokenade\\debug.log"},
      "hooks": {"PostToolUse": [{"hooks": [
        {"_tag": "tokenade", "type": "command",
         "command": "C:\\t\\tokenade.exe hookwrap"}]}]}
    }"#;
    assert_eq!(
        extract_debug_log_from_settings(win).as_deref(),
        Some("C:\\Users\\Admin\\.tokenade\\debug.log"),
        "must recover the path from the env block"
    );
    // Legacy POSIX location: inline in the hook command, no env block.
    let posix = format!(
        "{{\"hooks\":{{\"PostToolUse\":[{{\"hooks\":[{{\"_tag\":\"tokenade\",\
         \"type\":\"command\",\"command\":{}}}]}}]}}}}",
        serde_json::to_string(&build_hook_command(
            "/x/tokenade",
            "hookwrap",
            Some("/tmp/d.log")
        ))
        .unwrap()
    );
    // The hook-command path only round-trips on Unix (the POSIX `sh -c`
    // form carries it inline; the Windows form is a bare exe call).
    #[cfg(unix)]
    assert_eq!(
        extract_debug_log_from_settings(&posix).as_deref(),
        Some("/tmp/d.log")
    );
    let _ = &posix;
}

#[cfg(windows)]
#[test]
fn ensure_debug_log_env_sets_settings_env_block_idempotently() {
    let mut s = serde_json::json!({"env": {"EXISTING": "keep"}});
    ensure_debug_log_env(&mut s, Some("C:\\x\\debug.log"));
    assert_eq!(s["env"]["TOKENADE_DEBUG_LOG"], "C:\\x\\debug.log");
    assert_eq!(s["env"]["EXISTING"], "keep", "must not clobber other env");
    // Idempotent + None is a no-op (leaves the prior value).
    ensure_debug_log_env(&mut s, None);
    assert_eq!(s["env"]["TOKENADE_DEBUG_LOG"], "C:\\x\\debug.log");
}

#[cfg(windows)]
#[test]
fn windows_hook_command_is_direct_exe_not_cmd_c() {
    // The Windows regression guard: the hook command MUST be a bare exe
    // invocation so its stdout is exactly the hook's JSON. A `cmd /C`
    // wrapper corrupts that and silently disables compaction.
    let cmd = build_hook_command("C:\\t\\tokenade.exe", "hookwrap", Some("C:\\x\\debug.log"));
    assert!(!cmd.contains("cmd /C"), "must not wrap in cmd /C: {cmd}");
    assert!(
        !cmd.contains("set TOKENADE_DEBUG_LOG"),
        "no inline set: {cmd}"
    );
    assert!(
        cmd.contains("hookwrap"),
        "still invokes the subcommand: {cmd}"
    );
}

#[test]
fn win_hook_exe_is_cross_shell_safe_backslash_quoted() {
    // The exact npm install path from a user's Windows box.
    let real = r"C:\Users\Admin\AppData\Roaming\npm\node_modules\@tokenade\cli\vendor\tokenade.exe";
    let q = win_hook_exe(real);
    assert_win_exe_token_cross_shell_safe(&q, "win_hook_exe");
    assert_eq!(
        q,
        format!("\"{real}\""),
        "expected native-backslash double-quote: {q}"
    );
    // A forward-slash input is normalised to backslashes (defensive — some
    // callers pass already-slashed paths).
    let slashed = win_hook_exe("C:/Users/Admin/vendor/tokenade.exe");
    assert_eq!(
        slashed, r#""C:\Users\Admin\vendor\tokenade.exe""#,
        "got {slashed}"
    );
    // Spaces are covered by the same quotes (no separate space handling).
    let spaced = win_hook_exe(r"C:\Program Files\tokenade\tokenade.exe");
    assert_win_exe_token_cross_shell_safe(&spaced, "win_hook_exe(spaced)");
}

#[test]
fn wrap_exe_token_matches_hook_quoting_on_windows() {
    // The auto-wrap rewrite (`<exe> wrap '<cmd>'`) is re-parsed by the
    // agent's Bash tool (Git Bash) — it MUST use the same safe form, or
    // the rewrite replaces the real command with a broken path (a user's
    // "mangled path": the agent saw `C:Users…: command not found` and fled
    // to PowerShell → zero command compaction).
    let exe = r"C:\Users\Admin\AppData\Roaming\npm\node_modules\@tokenade\cli\vendor\tokenade.exe";
    let tok = wrap_exe_token(exe);
    if cfg!(windows) {
        assert_win_exe_token_cross_shell_safe(&tok, "wrap_exe_token");
        assert_eq!(
            tok,
            win_hook_exe(exe),
            "rewrite + hook exe forms must agree"
        );
    } else {
        // POSIX: exotic chars (backslashes, `@`) are now defensively
        // single-quoted — a HOME with a space used to emit an unquoted exe
        // token and break every rewritten Bash call. Plain paths stay bare.
        assert_eq!(tok, format!("'{exe}'"));
        assert_eq!(
            wrap_exe_token("/usr/local/bin/tokenade"),
            "/usr/local/bin/tokenade"
        );
        assert_eq!(
            wrap_exe_token("/home/user name/.local/bin/tokenade"),
            "'/home/user name/.local/bin/tokenade'"
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_hook_command_has_no_unquoted_backslash_or_bare_slashpath() {
    // End-to-end: build_hook_command must yield a command whose exe token
    // is cross-shell-safe AND still names the subcommand.
    let cmd = build_hook_command(
        r"C:\Users\Admin\AppData\Roaming\npm\node_modules\@tokenade\cli\vendor\tokenade.exe",
        "hookwrap-pretool",
        Some(r"C:\Users\Admin\.tokenade\debug.log"),
    );
    assert!(
        cmd.contains("hookwrap-pretool"),
        "subcommand dropped: {cmd}"
    );
    assert!(
        !cmd.contains("cmd /C") && !cmd.contains("cmd /c"),
        "no cmd wrapper: {cmd}"
    );
    // The path must be quoted; pull the leading quoted token and check it.
    assert!(cmd.starts_with('"'), "exe token must be quoted: {cmd}");
    let close = cmd[1..].find('"').expect("closing quote") + 1;
    assert_win_exe_token_cross_shell_safe(&cmd[..=close], "build_hook_command");
}

// #612 — uninstall symmetry: every install_claude_code_*hook* helper
// writes a group whose inner hook entry carries `_tag: "tokenade*"`.
// `strip_tokenade_hooks_from_settings` must drop them all without
// touching unrelated entries (rtk, user-owned hooks, …).
#[test]
fn strip_tokenade_hooks_removes_every_tokenade_tag_variant() {
    let d = tmp("strip_hooks_variants");
    let pre = d.join(".claude").join("settings.json");
    fs::create_dir_all(pre.parent().unwrap()).unwrap();
    // Cover every tag prefix the installers actually use today.
    let settings = serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {"matcher": "Bash", "hooks": [{"type":"command","command":"/x/t","_tag":"tokenade-pretool"}]},
                {"matcher": "Read", "hooks": [{"type":"command","command":"/x/t","_tag":"tokenade-pretool"}]},
                {"matcher": "Edit|Write|MultiEdit", "hooks": [{"type":"command","command":"/x/t","_tag":"tokenade-pre-edit"}]}
            ],
            "PostToolUse": [
                {"matcher": "Bash", "hooks": [{"type":"command","command":"/x/t","_tag":"tokenade"}]},
                {"matcher": "Edit", "hooks": [{"type":"command","command":"/x/t","_tag":"tokenade-index"}]},
                {"matcher": "Bash", "hooks": [{"type":"command","command":"rtk-postfilter","_tag":"rtk"}]}
            ],
            "UserPromptSubmit": [
                {"hooks": [{"type":"command","command":"/x/t","_tag":"tokenade-prompt"}]}
            ],
            "SessionStart": [
                {"hooks": [{"type":"command","command":"/x/t","_tag":"tokenade-sessionstart"}]}
            ],
            "PreCompact": [
                {"hooks": [{"type":"command","command":"/x/t","_tag":"tokenade-precompact"}]}
            ]
        },
        "otherUserKey": {"keep": "me"}
    });
    fs::write(&pre, serde_json::to_string_pretty(&settings).unwrap()).unwrap();

    match strip_tokenade_hooks_from_settings(Some(&d), false) {
        StripHooksOutcome::Stripped { groups_removed, .. } => {
            assert_eq!(groups_removed, 8, "should drop 8 tokenade groups, keep rtk");
        }
        other => panic!(
            "expected Stripped, got {:?}",
            std::mem::discriminant(&other)
        ),
    }
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&pre).unwrap()).unwrap();
    // Unrelated user keys untouched.
    assert_eq!(v["otherUserKey"]["keep"], serde_json::json!("me"));
    // PreToolUse: both entries were ours, so the KEY goes with them. An empty
    // array left behind is residue in a file the user diffs after removing us,
    // and it is what this assertion used to require.
    assert!(
        v["hooks"].get("PreToolUse").is_none(),
        "an emptied event key must not survive the strip"
    );
    // PostToolUse: only the rtk entry survives, so the key stays.
    let post = v["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 1);
    assert_eq!(post[0]["hooks"][0]["_tag"], serde_json::json!("rtk"));
    // The three single-event arrays held nothing but ours: all three keys gone.
    for event in ["UserPromptSubmit", "SessionStart", "PreCompact"] {
        assert!(
            v["hooks"].get(event).is_none(),
            "{event} should be gone after strip, not left empty"
        );
    }
    // `hooks` itself survives here because rtk still lives under it. The
    // all-ours case is covered by `strip_tokenade_hooks_drops_the_hooks_map_when_it_empties`.
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn strip_tokenade_hooks_dry_run_does_not_touch_disk() {
    let d = tmp("strip_hooks_dry");
    let pre = d.join(".claude").join("settings.json");
    fs::create_dir_all(pre.parent().unwrap()).unwrap();
    let settings = serde_json::json!({
        "hooks": {
            "PostToolUse": [
                {"matcher": "Bash", "hooks": [{"type":"command","command":"/x/t","_tag":"tokenade"}]}
            ]
        }
    });
    let raw = serde_json::to_string_pretty(&settings).unwrap();
    fs::write(&pre, &raw).unwrap();

    match strip_tokenade_hooks_from_settings(Some(&d), true) {
        StripHooksOutcome::WouldStrip { groups_removed, .. } => {
            assert_eq!(groups_removed, 1);
        }
        other => panic!(
            "expected WouldStrip, got {:?}",
            std::mem::discriminant(&other)
        ),
    }
    // File unchanged byte-for-byte.
    assert_eq!(fs::read_to_string(&pre).unwrap(), raw);
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn strip_tokenade_hooks_absent_file_is_silent() {
    let d = tmp("strip_hooks_absent");
    match strip_tokenade_hooks_from_settings(Some(&d), false) {
        StripHooksOutcome::AbsentOrUnreadable => {}
        other => panic!(
            "expected AbsentOrUnreadable, got {:?}",
            std::mem::discriminant(&other)
        ),
    }
    let _ = fs::remove_dir_all(&d);
}

// #819 — `tokenade <cmd> --help` for a command without dedicated
// help text used to dump the full 60-line command catalogue,
// burying the cmd-specific line. oneline_help_for() extracts the
// matching line from the top-level help body so the user gets a
// focused 1-line usage. Three branches matter: a real cmd
// matches, an alias not in the body returns None (caller falls
// back to print_help), and section headers are skipped (e.g.
// "USAGE:" / "COMMANDS:" — `first == "USAGE:"` would otherwise
// match if someone typed `tokenade USAGE: --help`).
#[test]
fn oneline_help_for_matches_simple_cmd() {
    let line = super::oneline_help_for("read").expect("read should be documented");
    assert!(line.starts_with("read"), "should start with cmd: {line:?}");
    assert!(
        line.contains("auto-detects the format"),
        "should describe read: {line:?}"
    );
}

#[test]
fn oneline_help_for_matches_hyphenated_cmd() {
    let line =
        super::oneline_help_for("audit-claude-md").expect("audit-claude-md should be documented");
    assert!(
        line.starts_with("audit-claude-md"),
        "expected audit-claude-md prefix: {line:?}"
    );
}

#[test]
fn oneline_help_for_returns_none_for_section_headers() {
    // "USAGE:" appears at column 0 in the body but isn't a command.
    // The 2-space-separator check rules it out.
    assert!(super::oneline_help_for("USAGE:").is_none());
    assert!(super::oneline_help_for("COMMANDS:").is_none());
}

#[test]
fn oneline_help_for_returns_none_for_unknown_cmd() {
    assert!(super::oneline_help_for("definitely-not-a-real-cmd").is_none());
}

#[test]
fn oneline_help_for_documents_onboarding_commands() {
    // Same shape as the guards that caught mcp-proxy gaps:
    // lock in the lifecycle commands so they can't silently disappear
    // from the discoverable surface.
    for cmd in ["install", "upgrade", "healthcheck", "dashboard"] {
        assert!(
            super::oneline_help_for(cmd).is_some(),
            "onboarding command {cmd:?} must appear in top-level help — \
             users discover the lifecycle flow through `tokenade --help`",
        );
    }
}

// #825/#826 — every alias in alias_to_primary must map to a
// primary that actually exists in the top-level help body.
// If someone adds a new alias but forgets to also add a help
// line for the primary (or types the primary wrong), this
// catches it before the user sees the full-catalogue fallback.
// The probe list mirrors the dispatcher's match arms; if a new
// alias is added there, it must also resolve here.
#[test]
fn alias_to_primary_targets_all_resolve_in_help_body() {
    let aliases = [
        // From the dispatcher run()::~272-320 match arms.
        "find",
        "ask",
        "sem",
        "google",
        "bing",
        "ddg",
        "bypass",
        "noproxy",
        "stats",
        "dash",
        "health",
        "doctor",
        "selfupdate",
        "self-update",
        "exec",
        "run-script",
        // Underscore variants of hyphenated primaries.
        "evict_stale",
        "mcp_proxy",
        "scrub_cache",
        "scrub_secret",
        "semantic_status",
        "unwrap_mcps",
        // Competitor scan aliases.
        "detect-competitors",
        "scan",
    ];
    for alias in &aliases {
        let primary = super::alias_to_primary(alias)
            .unwrap_or_else(|| panic!("alias {alias:?} missing from alias_to_primary"));
        // Every primary must in turn exist in the help body —
        // otherwise the alias retry returns None and the user
        // gets the full catalogue dump.
        let line = super::oneline_help_for(primary);
        assert!(
            line.is_some(),
            "alias {alias:?} maps to primary {primary:?} but primary is not documented in print_help body"
        );
    }
}

/// #1014 — every entry in KNOWN_COMMANDS must resolve to focused
/// mini-help via `oneline_help_for` (directly OR via alias_to_primary
/// retry), OR be in the curated `INTENTIONALLY_HIDDEN` allow-list.
/// Without this test, a future contributor adding a real user-facing
/// command but forgetting the help-body line silently regresses
/// `tokenade <cmd> --help` to the 108-line catalogue dump (the
/// failure mode #1013 fixed for watch + file-search). The
/// allow-list documents WHY each hidden cmd is hidden so the test
/// stays an audit rather than a rubber stamp.
#[test]
fn every_known_command_resolves_in_help_or_is_explicitly_hidden() {
    // Intentionally hidden: each entry is internal infrastructure
    // the user never types directly. Adding to this list requires
    // a comment explaining why — drift = bug.
    const INTENTIONALLY_HIDDEN: &[&str] = &[
        // `proxy` runs a command directly (no `sh -c`) through the compaction
        // path. Kept dispatched — it is the target the PATH shim (`shim-exec`)
        // and internal callers use — but removed from the user help/README
        // surface (users reach compaction via `wrap`/`read`). 2026-07 product decision.
        "proxy",
        // A sub-agent's brief, called by the six plugin bridges and by no
        // human: the prompt arrives on stdin and the output goes back into
        // the argument of a delegation tool. Documenting it in the help would
        // make it look like a command someone types.
        "subagent-brief",
        "subagent_brief",
        // Called by the plugin bridges on every tool call, never typed.
        "session-tick",
        // Same reason: called by a bridge from its host's outgoing-request
        // seam, never typed by anyone.
        "fold-payload",
        "fold_payload",
        // Claude Code hook entry points — Claude spawns these from
        // settings.json; users never run them by hand. Help would
        // mis-frame them as user-facing.
        "hookwrap",
        "hookwrap-precompact",
        "hookwrap-pre-edit",
        // Emitted ONLY by the PreToolUse replay rewrite (hooks/replay.rs);
        // never typed by a user — help would mis-frame it.
        "replay",
        "hookwrap-postread",
        "hookwrap-postwebfetch",
        "hookwrap-pretool",
        "hookwrap-sessionstart",
        "hookwrap-userprompt",
        // Same block, four entry points that KNOWN_COMMANDS did not carry at
        // all until a later fix — so they answered "unknown command" about hooks the
        // installer itself writes into settings.json.
        "hookwrap-subagentstart",
        "hookwrap-subagentstop",
        "hookwrap-websearch-post",
        "hookwrap-mcp-post",
        "hookwrap-cursor-mcp-post",
        "hookwrap-copilot",
        // PreToolUse snapshot hook — fires before Edit/Write/MultiEdit;
        // populates dedup_ref so file_diff hints work on blind writes.
        "pre-edit-snapshot",
        // MCP-side internal cache-warmer, called by the MCP code
        // intel handlers; documenting it would invite agents to
        // call it directly and bypass the cache logic.
        "index-touch",
        // `tokenade help` is the catalogue printer itself — there
        // is no mini-help for "show the catalogue".
        "help",
        // Gemini CLI BeforeTool hook entry-point — Gemini invokes
        // `tokenade gemini-hook` automatically; users never call it.
        // `geminihook` is its hyphen-free alias kept for typo-suggest.
        "gemini-hook",
        "geminihook",
        // The other three Gemini CLI entry points, same reasoning: Gemini
        // invokes them from its own lifecycle, nobody types them.
        "gemini-after-tool",
        "gemini-afterhook",
        "gemini-lifecycle",
        // `version` is a flag (--version), not a subcommand;
        // KNOWN_COMMANDS includes it only so `tokenade verison`
        // typos get suggested to `version`.
        "version",
        // `list-commands` is a release-pipeline introspection helper:
        // it prints the user-facing command set (machine-readable) so
        // the release guard can verify the public README documents
        // every command. It is not a user-facing command itself, so it
        // has no help-body line (and must stay out of its own output).
        "list-commands",
        "list_commands",
        // `compact-sessions` masks oversized stale tool_result payloads in idle
        // Claude Code transcripts so a `/resume` rebuilds cheaper. The value
        // path is the automatic SessionStart trigger; this manual whole-machine
        // sweep is a power-user maintenance extra (like `evict-stale`), kept out
        // of the catalogue so it doesn't need a README docs-guard entry.
        "compact-sessions",
        "compact_sessions",
        // `scan-savings` estimates, from the user's EXISTING transcripts, what
        // tokenade would compact (a pre-install conversion pitch). Read-only.
        // Kept out of the catalogue for now (no README docs-guard entry yet) —
        // promote to onboarding + document when it's wired into the funnel.
        "scan-savings",
        "scan_savings",
        // `ensure-hooks` is the self-heal entry point: re-asserts the
        // hook suite + shell proxy if another tool overwrote them. Run
        // automatically (SessionStart / `tokenade wrap` / shell proxy)
        // and available for manual repair, but not a workflow command a
        // user reaches for day-to-day — kept out of the catalogue.
        "ensure-hooks",
        "ensure_hooks",
        // `shim-exec` is the internal target every PATH shim (no-hook
        // fallback) re-execs into — it resolves the real binary and routes
        // its output through the compaction path. Not a command a user runs
        // by hand, so it's kept out of the catalogue (it has `--help`).
        "shim-exec",
        // `calibrate` (0.6.4) recomputes the approximate-savings
        // coefficients from a THOL benchmark DB into
        // ~/.tokenade/coefficients.json. A dev/maintenance tool, not an
        // end-user command — kept out of the catalogue (and the README
        // docs guard) on purpose.
        "calibrate",
        // `bench` / `benchmark` (in-session A/B token+cost proof) is a
        // dev/bench tool a user runs occasionally against their own repo to
        // prove the savings — like `calibrate`, kept out of the day-to-day
        // catalogue + README docs guard on purpose (it has full `--help`).
        "bench",
        "benchmark",
        // `review-context` (+ alias `review`) — pulled from the help/README
        // surface 2026-07-08 by product decision: never impact-studied, 3 calls by 1
        // user fleet-wide. Command stays dispatched (has full `--help`) so the
        // existing user doesn't break; re-benchmark before re-promoting.
        "review",
        "review-context",
        // `judge` (+ alias `groundedness`) proves a single compaction kept the
        // answer, by asking a model the same question of raw vs compacted. A
        // proof tool like `bench`, run occasionally, not a day-to-day command
        // — and it has a full `--help`, reachable since then.
        "judge",
        "groundedness",
        // Background/maintenance entry points, invoked by tokenade itself:
        // `reporter-run` and `real-savings-refresh` are scheduled work,
        // `audit-gains` is a ledger self-check, `codex-trust` is the Codex
        // bridge's approval step. None is typed by a user; each was simply
        // absent from KNOWN_COMMANDS, which is why `--help` denied them.
        "audit-gains",
        "codex-trust",
        "codex_trust",
        "real-savings-refresh",
        "reporter-run",
        // `crawl` follows links from a page `web` already fetched. Kept out of
        // the catalogue and the README docs guard: promoting it is a product
        // decision (it needs a documented depth/politeness contract), not a
        // side effect of fixing its `--help`.
        "crawl",
    ];
    for &cmd in super::KNOWN_COMMANDS {
        if INTENTIONALLY_HIDDEN.contains(&cmd) {
            continue;
        }
        assert!(
            super::oneline_help_for(cmd).is_some(),
            "KNOWN_COMMANDS entry {cmd:?} resolves to no mini-help. Either add a help-body line in print_help, an alias_to_primary entry, OR add {cmd:?} to INTENTIONALLY_HIDDEN with a comment explaining why."
        );
    }
}

#[test]
fn strip_tokenade_hooks_drops_the_hooks_map_when_it_empties() {
    // The install's own shape: every hook under `hooks` is ours. Removing them
    // must leave the file as the user wrote it, not as six empty arrays under a
    // `hooks` key they never created.
    //
    // Measured on a disposable HOME before this was fixed: a settings.json
    // holding one key came back from a full uninstall carrying
    // `"hooks":{"PostToolUse":[],…}` — five more keys than it went in with.
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let d = std::env::temp_dir().join(format!("tk-strip-empty-{}", std::process::id()));
    let dir = d.join(".claude");
    fs::create_dir_all(&dir).unwrap();
    let pre = dir.join("settings.json");
    let settings = serde_json::json!({
        "userKey": {"keep": "me"},
        "hooks": {
            "PreToolUse": [
                {"hooks": [{"type":"command","command":"/x/t hookwrap-pretool","_tag":"tokenade"}]}
            ],
            "PostToolUse": [
                {"hooks": [{"type":"command","command":"/x/t hookwrap","_tag":"tokenade"}]}
            ]
        }
    });
    fs::write(&pre, serde_json::to_string_pretty(&settings).unwrap()).unwrap();

    let _ = strip_tokenade_hooks_from_settings(Some(&d), false);

    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&pre).unwrap()).unwrap();
    assert_eq!(
        v["userKey"]["keep"],
        serde_json::json!("me"),
        "user key survives"
    );
    assert!(
        v.get("hooks").is_none(),
        "`hooks` must go when nothing but ours was in it, got {v}"
    );
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn strip_tokenade_hooks_no_tokenade_tags_returns_no_groups() {
    let d = tmp("strip_hooks_nogroups");
    let pre = d.join(".claude").join("settings.json");
    fs::create_dir_all(pre.parent().unwrap()).unwrap();
    let settings = serde_json::json!({
        "hooks": {
            "PostToolUse": [
                {"matcher": "Bash", "hooks": [{"type":"command","command":"rtk-postfilter","_tag":"rtk"}]}
            ]
        }
    });
    let raw = serde_json::to_string_pretty(&settings).unwrap();
    fs::write(&pre, &raw).unwrap();
    match strip_tokenade_hooks_from_settings(Some(&d), false) {
        StripHooksOutcome::NoGroups => {}
        other => panic!(
            "expected NoGroups, got {:?}",
            std::mem::discriminant(&other)
        ),
    }
    // File unchanged.
    assert_eq!(fs::read_to_string(&pre).unwrap(), raw);
    let _ = fs::remove_dir_all(&d);
}

/// The denylist exists because tokenade HAS compactors for these
/// commands — without it they'd be aliased. Pin the safety-critical
/// ones: text-stream tools feed pipelines, exact-bytes tools feed
/// checksums/parsers, prefix wrappers re-enter the proxy, and shells
/// would recurse.
#[test]
fn shell_proxy_denylist_never_aliased() {
    let all = proxyable_all();
    for c in [
        "cat", "sed", "awk", "grep", "head", "tail", // text-stream
        "jq", "base64", "tar", "md5sum", // exact-bytes
        "sudo", "env", "timeout", // prefix wrappers
        "ls", "find", "which", "date", // $()-consumed
        "sh", "bash", "ssh", "tokenade", // shells / remote / self
        "echo", "printf", "pwd", "test", "kill", "realpath", // shell builtins
        // shell-STATE builtins — aliasing them runs in a subshell and loses
        // the parent-shell effect (cd doesn't change dir, export drops env…).
        // `cd` is a known_program_names() entry with a /usr/bin/cd stub on
        // macOS, so it was actually leaking and breaking cd for proxy users.
        "cd", "pushd", "popd", "export", "unset", "source", ".", "set", "alias", "eval", "exec",
        "trap", "exit",
    ] {
        assert!(
            PROXY_DENYLIST.contains(&c),
            "{c} missing from PROXY_DENYLIST"
        );
        assert!(
            !all.iter().any(|n| n == c),
            "{c} leaked into proxyable_all()"
        );
    }
    // And none of them may appear as an alias in any generated fragment.
    let cmds: Vec<String> = all;
    for frag in [
        shell_init_posix(&cmds),
        shell_init_fish(&cmds),
        shell_init_powershell(&cmds),
        shell_init_nushell(&cmds),
        shell_init_csh(&cmds),
        shell_init_cmd(&cmds),
    ] {
        assert!(!frag.contains("alias cat "), "cat aliased:\n{frag}");
        assert!(!frag.contains("alias sed "), "sed aliased:\n{frag}");
    }
}

/// Windows (PowerShell) shell-proxy coverage: aliases can't carry logic, so
/// the proxy defines a `function global:<cmd>` per command. Real commands get
/// a wrapper; the shell-STATE builtins (cd/Set-Location, export, set, …) must
/// NEVER get one — a `function global:cd` would shadow PowerShell's
/// `Set-Location` alias and break `cd` exactly like the POSIX bug.
#[test]
fn powershell_fragment_wraps_real_commands_not_builtins() {
    let frag = shell_init_powershell(&["git".to_string(), "docker".to_string()]);
    // The proxy mechanism: a global proxy function + per-command global
    // function wrappers (NOT Set-Alias), gated on input redirection.
    assert!(
        frag.contains("function global:__tokenade_proxy"),
        "proxy fn missing:\n{frag}"
    );
    assert!(
        frag.contains("IsInputRedirected"),
        "non-interactive guard missing"
    );
    assert!(
        frag.contains("function global:$__tc"),
        "per-command wrapper form missing"
    );
    assert!(
        frag.contains("'git'") && frag.contains("'docker'"),
        "real cmds not listed:\n{frag}"
    );
    // The state builtins are filtered upstream (PROXY_DENYLIST) so they can
    // never reach the PS fragment — assert at the source.
    let all = proxyable_all();
    for builtin in ["cd", "pushd", "popd", "export", "set", "source", "unset"] {
        assert!(
            !all.iter().any(|n| n == builtin),
            "{builtin} would reach the PowerShell fragment and shadow the builtin"
        );
        // And the generated fragment for an --all set never names it.
        let f = shell_init_powershell(&all);
        assert!(
            !f.contains(&format!("'{builtin}'")),
            "{builtin} present in PS fragment cmd list"
        );
    }
}

/// Regression: fish's own `alias` helper builds each function body
/// with the `echo` builtin, so shadowing `echo` broke every alias
/// defined after it (alphabetically) at every fish startup; `test` is
/// a fish reserved keyword and a hard parse error as a function name.
/// And nushell implements coreutils names as builtins with different
/// semantics — those must be filtered from the nu fragment only,
/// while remaining aliased on POSIX shells.
#[test]
fn shell_proxy_never_shadows_shell_builtins() {
    let cmds: Vec<String> = ["cp", "du", "echo", "git", "ps", "test"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    // echo/test never reach any fragment builder: proxyable_all()
    // (and the PATH-derived variant) drop them via PROXY_DENYLIST.
    let all = proxyable_all();
    for builtin in ["echo", "printf", "pwd", "test", "kill", "realpath"] {
        assert!(
            !all.iter().any(|n| n == builtin),
            "{builtin} leaked into proxyable_all()"
        );
    }
    // nu builtins are stripped from the nushell fragment only.
    let nu = shell_init_nushell(&cmds);
    for builtin in ["cp", "du", "ps"] {
        assert!(
            !nu.contains(&format!("alias {builtin} ")),
            "{builtin} aliased in nushell fragment:\n{nu}"
        );
    }
    assert!(nu.contains("__tokenade_proxy git"), "git missing:\n{nu}");
    let fish = shell_init_fish(&cmds);
    assert!(
        fish.contains(" cp "),
        "cp must stay proxied on fish:\n{fish}"
    );
    let posix = shell_init_posix(&cmds);
    assert!(
        posix.contains(" du "),
        "du must stay proxied on POSIX:\n{posix}"
    );
}

/// Regression: pyenv (and sibling version managers) ship dedicated
/// compactor modules, so the registry-derived alias list WOULD proxy
/// `pyenv`. But `pyenv rehash` regenerates its shims by resolving its own
/// path via `command -v pyenv` — which, with the proxy active, returns
/// `pyenv='__tokenade_proxy pyenv'`. That alias string got baked into the
/// shims and every python3 shim broke after a reboot. They must never be
/// aliased; the Bash hook still compacts their output independently.
#[test]
fn shell_proxy_never_aliases_version_managers() {
    let managers = [
        // self-referencing version managers / env shims
        "pyenv",
        "rbenv",
        "nodenv",
        "goenv",
        "asdf",
        "mise",
        "direnv",
        "nvm",
        "conda",
        "pyenv-install",
        "pyenv-versions",
        "conda-env-list",
        // shell prompt / hook / init generators (eval "$(X init …)")
        "starship",
        "starship-prompt",
        "zoxide",
        "zsh-zinit",
        "fzf",
    ];
    // 1. Each must be on the denylist, and the diagnostic `--all` set must
    //    not surface the bare manager name.
    let all = proxyable_all();
    for m in managers {
        assert!(
            PROXY_DENYLIST.contains(&m),
            "{m} missing from PROXY_DENYLIST"
        );
        assert!(
            !all.iter().any(|n| n == m),
            "{m} leaked into proxyable_all()"
        );
    }
    // 2. End-to-end on the REAL install path: a genuine `pyenv` binary
    //    sitting on PATH is exactly what broke — `pyenv rehash` resolved its
    //    own location via `command -v pyenv`, got the alias string, and
    //    baked `__tokenade_proxy pyenv` into every shim. proxyable_on_path
    //    must drop every manager while still aliasing a normal tool (git).
    let d = tmp("proxy-vm");
    let mk = |name: &str| {
        let file = if cfg!(windows) {
            d.join(format!("{name}.exe"))
        } else {
            d.join(name)
        };
        fs::write(&file, "").unwrap();
    };
    mk("git"); // control: known + allowed → must survive
    for m in managers {
        mk(m);
    }
    let cmds = proxyable_on_path(d.as_os_str());
    assert!(
        cmds.iter().any(|n| n == "git"),
        "git should still be aliased"
    );
    for m in managers {
        assert!(
            !cmds.iter().any(|n| n == m),
            "{m} present on PATH was aliased"
        );
    }
    // No shell fragment built from the real install set may name a manager.
    for frag in [
        shell_init_posix(&cmds),
        shell_init_fish(&cmds),
        shell_init_powershell(&cmds),
        shell_init_nushell(&cmds),
        shell_init_csh(&cmds),
        shell_init_cmd(&cmds),
    ] {
        for m in managers {
            assert!(
                !frag.contains(&format!("__tokenade_proxy {m}")),
                "{m} aliased to the proxy in a generated fragment"
            );
        }
    }
    let _ = fs::remove_dir_all(&d);
}

/// The alias set is DERIVED from the compactor registry — adding a
/// cmd module or filter arm must surface here with no manual list.
#[test]
fn shell_proxy_list_is_registry_derived_and_sorted() {
    let all = proxyable_all();
    for expected in ["git", "cargo", "kubectl", "docker", "terraform"] {
        assert!(
            all.iter().any(|n| n == expected),
            "{expected} missing from derived proxy list"
        );
    }
    let mut sorted = all.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(all, sorted, "proxyable_all must be sorted + deduped");
}

/// PATH harvest: present + known + non-denylisted → aliased; present
/// but unknown, or known but denylisted, or known but absent → not.
#[test]
fn proxyable_on_path_filters_presence_knownness_denylist() {
    let d = tmp("proxypath");
    let mk = |name: &str| {
        let file = if cfg!(windows) {
            d.join(format!("{name}.exe"))
        } else {
            d.join(name)
        };
        fs::write(&file, "").unwrap();
    };
    mk("git"); // known, allowed → aliased
    mk("cat"); // known, denylisted → dropped
    mk("frobnicator-9000"); // present but unknown → dropped
    let cmds = proxyable_on_path(d.as_os_str());
    assert_eq!(cmds, vec!["git".to_string()]);
    // kubectl is known but NOT in the dir → never shadowed.
    assert!(!cmds.iter().any(|n| n == "kubectl"));
    let _ = fs::remove_dir_all(&d);
}

/// Pipeline safety: each fragment must carry the guards that keep
/// compacted text out of pipes and file redirects (the agent-context
/// corruption class: `git log | head`, `git show x > f`).
#[test]
fn shell_proxy_fragments_carry_pipeline_guards() {
    let cmds = vec!["git".to_string(), "cargo".to_string()];

    let posix = shell_init_posix(&cmds);
    assert!(posix.contains("[ -t 0 ]"), "posix TTY guard missing");
    assert!(
        posix.contains("BASH_SUBSHELL"),
        "bash pipeline-stage guard missing"
    );
    assert!(
        posix.contains("ZSH_SUBSHELL"),
        "zsh pipeline-stage guard missing"
    );
    assert!(
        posix.contains("-f /dev/stdout"),
        "posix redirect guard missing"
    );
    assert!(
        posix.contains("expand_aliases"),
        "bash non-interactive aliases missing"
    );
    assert!(
        posix.contains("for __tc in git cargo; do"),
        "alias loop missing the command set"
    );

    let fish = shell_init_fish(&cmds);
    assert!(fish.contains("isatty stdin"), "fish TTY guard missing");
    assert!(
        fish.contains("is-command-substitution"),
        "fish command-substitution guard missing"
    );
    assert!(
        fish.contains("command test -f /dev/stdout"),
        "fish redirect guard missing, or it uses the BUILTIN `test` — which does not \
         see a redirect applied to a function call"
    );

    let ps = shell_init_powershell(&cmds);
    assert!(
        ps.contains("IsInputRedirected"),
        "powershell TTY guard missing"
    );
    assert!(
        ps.contains("PipelineLength"),
        "powershell pipeline guard missing"
    );

    let nu = shell_init_nushell(&cmds);
    assert!(
        nu.contains("is-terminal --stdin"),
        "nushell TTY guard missing"
    );

    // csh/cmd carry no inline logic — every alias must route through
    // `tokenade shellwrap`, which applies the guards in-process.
    let csh = shell_init_csh(&cmds);
    assert!(csh.contains("alias git 'tokenade shellwrap git \\!*'"));
    let cmdfrag = shell_init_cmd(&cmds);
    assert!(cmdfrag.contains("doskey git=tokenade shellwrap git $*"));
}

/// Empty command set → header-only fragments, no stray alias lines.
#[test]
fn shell_proxy_fragments_empty_set() {
    let none: Vec<String> = Vec::new();
    for frag in [
        shell_init_posix(&none),
        shell_init_fish(&none),
        shell_init_powershell(&none),
        shell_init_nushell(&none),
        shell_init_csh(&none),
    ] {
        assert!(
            frag.contains("no proxy-able commands"),
            "missing empty-set notice:\n{frag}"
        );
        assert!(!frag.contains("alias git"), "phantom alias:\n{frag}");
    }
}

/// Uninstall must remove the auto-compact override it installed (marker
/// `_tokenadeAutoCompact`) but never a hand-set `autoCompactWindow`.
#[test]
fn uninstall_strips_only_owned_autocompact_override() {
    let home = tmp("acuninst");
    let dir = home.join(".claude");
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join("settings.json");
    fs::write(&p, r#"{"autoCompactWindow":140000,"_tokenadeAutoCompact":true,"env":{"CLAUDE_CODE_AUTO_COMPACT_WINDOW":"140000"},"model":"opus"}"#).unwrap();
    let msg = crate::install::strip_autocompact_override(Some(&home), false);
    assert!(msg.is_some());
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    assert!(v.get("autoCompactWindow").is_none());
    assert!(v.get("_tokenadeAutoCompact").is_none());
    assert!(v.get("env").is_none(), "empty env removed");
    assert_eq!(v["model"], "opus", "user settings survive");

    // Hand-set value (no marker): untouched.
    fs::write(&p, r#"{"autoCompactWindow":90000}"#).unwrap();
    assert!(crate::install::strip_autocompact_override(Some(&home), false).is_none());
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(v["autoCompactWindow"], 90000);
}

/// `Integration::shell_hook` is the field a compatibility matrix reads to
/// decide "does this agent get hook-grade coverage". It is declared in
/// tokenade-core, while the installers that make it TRUE live here — so
/// **A capability probe does not start another agent's binary for nothing.**
///
/// `gemini --version` writes `~/.gemini/projects.json` and leaves a `.tmp` in
/// it: on the macOS VM, `install --only X` knocked down the `scoping` cell of
/// 17 agents out of 24 because the MCP exclusion asked for the capability
/// BEFORE looking whether there was a path to exclude.
///
/// The proof is read on the probe's CACHE (`~/.tokenade/cap-*`): it is only
/// written when the probe runs. A home without any MCP config must produce
/// none. It is the same witness as the `--dry-run` one, which promises to
/// write nothing under `HOME`.
#[test]
fn no_capability_probe_on_a_home_without_mcp_config() {
    let home = tokenade_core::platform::ScratchHome::new("cap-no-probe");
    let covered = crate::install_cli::mcp_paths_covered_by_a_hook(false);
    assert!(
        covered.is_empty(),
        "no MCP path exists in this home, nothing can be covered: {covered:?}"
    );
    let dir = home.path().join(".tokenade");
    let caps: Vec<String> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .filter(|n| n.starts_with("cap-"))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        caps.is_empty(),
        "a probe ran with nothing to exclude (caches written: {caps:?}) — \
         that is an `<agent> --version` launched for a user who does not have that agent"
    );
}

/// **The install line names the event it REALLY writes.**
///
/// `BEFORE_TOOL_INSTALLS` announced "BeforeTool hook" for both its hosts,
/// and since Qwen Code writes `PreToolUse` (its host speaks Claude Code's
/// names, not Gemini's) the line boasted of an event absent from the file: an
/// operator who goes to check does not find it. The same mistake had already
/// been fixed for grok, in the other table.
///
/// The phrase is therefore compared to the event the installer WRITES, read
/// from the file it just wrote — not to a constant copied alongside, which
/// would only prove that two tables agree.
#[test]
fn the_install_line_names_the_event_it_writes() {
    // **No `INSTALL_ENV_LOCK` here.** `ScratchHome` takes `ENV_LOCK`, which IS
    // that lock (see `tests/mod.rs`), and a `std::sync::Mutex` is not
    // reentrant: taking it first blocks this test forever — measured,
    // "has been running for over 60 seconds". Isolation comes from the scratch
    // home, which is also what serialises.
    for (agent, install, label, phrase) in crate::install_cli::BEFORE_TOOL_INSTALLS {
        let home = tokenade_core::platform::ScratchHome::new(&format!("evt-{agent:?}"));
        // The config directory must EXIST: install does not create the nest
        // of an absent agent, and without a seed it writes nothing at all.
        let dir = home
            .path()
            .join(if *agent == tokenade_core::detect::Agent::Qwen {
                ".qwen"
            } else {
                ".gemini"
            });
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), "{}").unwrap();
        let outcome = install("/usr/bin/tokenade", Some(home.path()), false);
        let path = match &outcome {
            crate::HookOutcome::Installed(p) | crate::HookOutcome::Refreshed(p) => p.clone(),
            other => panic!("{label}: install wrote nothing: {other:?}"),
        };
        let body = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        let events: Vec<String> = json["hooks"]
            .as_object()
            .expect("a hooks object")
            .keys()
            .cloned()
            .collect();
        let named = phrase.split_whitespace().next().unwrap();
        assert!(
            events.iter().any(|e| e == named),
            "{label}: the line announces \"{phrase}\" and the file carries {events:?}"
        );
    }
}

/// nothing ever forced the two to agree. They didn't: Codex and Copilot
/// both shipped hook installers while declaring `shell_hook: false`.
/// This test is the missing link between the claim and the code.
#[test]
fn every_agent_with_a_hook_installer_declares_shell_hook() {
    use tokenade_core::detect::Agent;
    for (agent, _, label, _) in crate::install_cli::HOOK_INSTALLS {
        assert!(
            agent.integration().shell_hook,
            "{label} ({agent:?}) has a row in HOOK_INSTALLS but declares shell_hook: false"
        );
    }
    for (agent, _, label, _) in crate::install_cli::BEFORE_TOOL_INSTALLS {
        assert!(
            agent.integration().shell_hook,
            "{label} ({agent:?}) has a row in BEFORE_TOOL_INSTALLS but declares shell_hook: false"
        );
    }
    // Wired outside both tables (dedicated call sites in `install_cli`).
    for agent in [Agent::ClaudeCode, Agent::Codex] {
        assert!(
            agent.integration().shell_hook,
            "{agent:?} gets passive hooks installed but declares shell_hook: false"
        );
    }
}

/// Same missing link as `every_agent_with_a_hook_installer_declares_shell_hook`,
/// for the third channel. `Integration` has no plugin field at all, so
/// `Agent::has_plugin` is the declaration and `PLUGIN_INSTALLS` is the truth.
#[test]
fn every_agent_with_a_plugin_installer_declares_it() {
    use tokenade_core::detect::Agent;
    // Cursor is in PLUGIN_INSTALLS because its installer matches that
    // table's SIGNATURE, not because it installs a plugin: it writes a
    // `preToolUse` hook. The table groups by function type, so it is not a
    // reliable answer to "what channel covers this agent" on its own.
    for (agent, _, label, _) in crate::install_cli::PLUGIN_INSTALLS {
        // aider is the THIRD kind of row in this table, and it confirms what
        // the comment on cursor already says: it groups by function
        // SIGNATURE, not by channel. aider has neither bridge nor hook — what
        // its installer writes is a startup LAUNCHER (`load:` → `/run`,
        // verified at runtime on 2026-08-22). Declaring it `has_plugin()`
        // would be lying about what is installed to make a test pass.
        if *agent == Agent::Aider {
            assert!(
                !agent.has_plugin() && !agent.integration().shell_hook,
                "aider has neither a plugin nor a hook; its row installs a startup launcher"
            );
            continue;
        }
        if *agent == Agent::Cursor {
            assert!(
                agent.integration().shell_hook && !agent.has_plugin(),
                "cursor is a HOOK agent misfiled in PLUGIN_INSTALLS"
            );
            continue;
        }
        assert!(
            agent.has_plugin(),
            "{label} ({agent:?}) has a row in PLUGIN_INSTALLS but has_plugin() is false"
        );
    }
    let declared = Agent::all().iter().filter(|a| a.has_plugin()).count();
    assert_eq!(
        declared,
        crate::install_cli::PLUGIN_INSTALLS.len() - 2,
        "has_plugin() claims a plugin for an agent with no installer"
    );
}

/// Walk every file under `dir`, returning (relative path, contents).
fn tree(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(dir).unwrap_or(&p).display().to_string();
                out.push((rel, fs::read_to_string(&p).unwrap_or_default()));
            }
        }
    }
    out
}

/// The declaration must match what the installer PUTS ON DISK.
///
/// Every guard before this one compared a declaration to a TABLE, and a
/// table is just another declaration. That is how Cursor slipped through:
/// its hook installer sits in `PLUGIN_INSTALLS` because it matches that
/// table's function signature, so a guard reading the table learned the
/// wrong channel and agreed with the wrong answer. This one runs the
/// installers against a throwaway HOME and reads the files that appear.
///
/// Cline and OpenClaw additionally shell out to their own CLIs to register
/// the plugin; those binaries are absent here, the calls fail open, and the
/// files are still written — which is exactly what this test looks at.
/// The declaration must match what the installer PUTS ON DISK.
///
/// Every guard before this one compared a declaration to a TABLE, and a table
/// is just another declaration. That is how Cursor slipped through: its hook
/// installer sits in `PLUGIN_INSTALLS` because it matches that table's
/// function signature, so a guard reading the table learned the wrong channel
/// and agreed with the wrong answer.
///
/// This was `#[ignore]`d for one run: `opencode_plugin_dir` / `kilo_plugin_dir`
/// read `XDG_CONFIG_HOME` unconditionally, so a throwaway `home` did not
/// isolate them and a parallel test mutating that variable made the write land
/// somewhere this test was not looking. `xdg_config_root` now honours the
/// variable only when `home` IS the process home, so an explicitly named home
/// is authoritative and this test is deterministic again.
#[test]
fn declared_channel_matches_what_the_installer_writes() {
    use tokenade_core::detect::Agent;

    let run = |agent: Agent| -> (bool, bool) {
        let home = tmp(&format!("chan-{}", agent.as_str()));
        let _ = fs::create_dir_all(&home);
        for (a, f, _, _) in crate::install_cli::HOOK_INSTALLS {
            if *a == agent {
                let _ = f("/opt/tokenade", None, Some(&home), false);
            }
        }
        for (a, f, _, _) in crate::install_cli::BEFORE_TOOL_INSTALLS {
            if *a == agent {
                let _ = f("/opt/tokenade", Some(&home), false);
            }
        }
        for (a, f, _, _) in crate::install_cli::PLUGIN_INSTALLS {
            if *a == agent {
                let _ = f(&home, false);
            }
        }
        if agent == Agent::Codex {
            let _ = crate::install::install_codex_passive_hooks(
                "/opt/tokenade",
                None,
                Some(&home),
                false,
            );
        }
        if agent == Agent::ClaudeCode {
            let _ = crate::install::install_claude_code_passive_hooks(
                &home,
                "/opt/tokenade",
                None,
                true,
                Some(&home),
                false,
            );
        }
        // `opencode_plugin_dir` / `kilo_plugin_dir` honour `XDG_CONFIG_HOME`
        // and IGNORE the `home` argument when it is set — so walking `home`
        // alone made this test pass solo and fail in a parallel run where
        // another test had that variable set. Ask the installers' own
        // resolver where they write instead of assuming.
        let mut files = tree(&home);
        for extra in [
            crate::install_cli::opencode_plugin_dir(&home),
            crate::install_cli::kilo_plugin_dir(&home),
        ] {
            if extra.starts_with(&home) {
                continue; // already covered by the walk above
            }
            files.extend(tree(&extra));
        }
        // Classify by CONTENT, not filename: Copilot's hook config is
        // `~/.copilot/hooks/tokenade.json`, and a filename-only rule read
        // that as "no hook written" — the classifier was wrong, not the
        // declaration. A hook config is a structured file naming one of the
        // lifecycle events; a plugin is source the agent loads.
        const HOOK_EVENTS: &[&str] = &[
            "preToolUse",
            "postToolUse",
            "PreToolUse",
            "PostToolUse",
            "BeforeTool",
            "SessionStart",
            "pre_tool_use",
            "post_tool_use",
        ];
        let hookish = files.iter().any(|(rel, body)| {
            (rel.ends_with(".json") || rel.ends_with(".yaml") || rel.ends_with(".yml"))
                && body.contains("tokenade")
                && HOOK_EVENTS.iter().any(|e| body.contains(e))
        });
        let pluginish = files
            .iter()
            .any(|(rel, _)| rel.ends_with(".ts") || rel.ends_with(".py") || rel.contains("plugin"));
        let _ = fs::remove_dir_all(&home);
        (hookish, pluginish)
    };

    for agent in Agent::all() {
        let declared_hook = agent.integration().shell_hook;
        let declared_plugin = agent.has_plugin();
        if !declared_hook && !declared_plugin {
            continue; // uncovered agents install nothing; nothing to compare
        }
        let (wrote_hook, wrote_plugin) = run(*agent);
        assert_eq!(
            wrote_hook, declared_hook,
            "{agent:?}: declares shell_hook={declared_hook} but wrote_hook_config={wrote_hook}"
        );
        assert_eq!(
            wrote_plugin, declared_plugin,
            "{agent:?}: declares plugin={declared_plugin} but wrote_plugin_file={wrote_plugin}"
        );
    }
}

/// One authority per payload, on the Copilot hook.
///
/// The guard used to be "did the fold get shorter", which is a different
/// question: measured 2026-08-07, a SECOND fold of a git log still shaved 54
/// bytes off the summary (and a third grew it back), so the shrink test
/// happily let a second channel chew on an already-folded result. A result
/// carrying a tokenade marker has an owner already.
#[test]
fn copilot_fold_passes_through_already_folded_results() {
    // Deliberately repetitive: the point is to exercise the SECOND fold, so
    // the first one has to actually shrink. A corpus of distinct one-liners
    // does not fold at all (tried), and would only prove the guard is never
    // reached.
    let raw = (0..600)
        .map(|i| {
            format!("2026-08-07T10:00:00Z INFO  worker-{} handled request id=abc{} status=200 duration=12ms", i % 4, i)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let payload =
        |t: &str| serde_json::json!({ "toolResult": { "textResultForLlm": t } }).to_string();

    let (out, before, after) =
        crate::hookwrap::copilot_fold(&payload(&raw)).expect("raw output must fold");
    assert!(after < before, "{before} -> {after}");

    // Text folded by the WRAP path — the one that stashes and appends an
    // `expand-ref <hash>` pointer — must pass straight through.
    let marked = format!(
        "{raw}\n[tokenade: 4210 tokens folded away — full output: \
         `tokenade expand-ref 8cbb0d6ffff8`]"
    );
    assert!(tokenade_core::fold_marker::already_folded(&marked));
    assert!(
        crate::hookwrap::copilot_fold(&payload(&marked)).is_none(),
        "an already-folded result must pass through untouched"
    );

    // The hook's OWN fold must carry a recovery pointer, for two reasons that
    // used to both be unmet: a Copilot user needs a way back to the dropped
    // bytes, and a downstream channel needs to see that this payload has an
    // owner. Every other folding channel already attached one.
    let folded: String = serde_json::from_str::<serde_json::Value>(&out).unwrap()["modifiedResult"]
        ["textResultForLlm"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        tokenade_core::fold_marker::already_folded(&folded),
        "copilot_fold must mark its own output: {}",
        &folded[folded.len().saturating_sub(200)..]
    );
    assert!(
        crate::hookwrap::copilot_fold(&payload(&folded)).is_none(),
        "and therefore re-folding it must pass through"
    );
}

/// The Gemini/Qwen AfterTool fold is the twin of `copilot_fold`, and it
/// carried the same two gaps: no recovery pointer on its own output, and a
/// "did it shrink" guard where the question is "does this payload already
/// have an owner". Same corpus, same two assertions — a twin that drifts is
/// how half of this document's findings started.
#[test]
fn gemini_after_fold_marks_its_output_and_skips_folded_input() {
    let raw = (0..600)
        .map(|i| {
            format!("2026-08-07T10:00:00Z INFO  worker-{} handled request id=abc{} status=200 duration=12ms", i % 4, i)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let payload = |t: &str| {
        serde_json::json!({
            "tool_name": "run_shell_command",
            "tool_response": { "llmContent": t }
        })
        .to_string()
    };

    let (out, before, after) =
        crate::install::gemini_after_fold(&payload(&raw)).expect("raw output must fold");
    assert!(after < before, "{before} -> {after}");
    let folded = serde_json::from_str::<serde_json::Value>(&out).unwrap()["reason"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        tokenade_core::fold_marker::already_folded(&folded),
        "must leave a way back to the dropped bytes"
    );
    assert!(
        crate::install::gemini_after_fold(&payload(&folded)).is_none(),
        "already-folded input must pass through"
    );
}

// Codex, Droid, Copilot, Grok, Reasonix, Qwen and Cursor had installers with a
// single caller — the interactive `tokenade install`. Their hook command
// carries an ABSOLUTE path to the binary, so a move (new npm prefix, reinstall)
// left all seven invoking a path that no longer exists, silently, for ever;
// Claude Code was repaired on the next upgrade and they were not. Both halves
// pinned: a marked config is re-pointed, an unmarked one is not touched.
#[test]
fn upgrade_repoints_other_agents_hooks_and_leaves_unmarked_configs_alone() {
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let no_hook = std::env::var_os("TOKENADE_NO_HOOK");
    std::env::remove_var("TOKENADE_NO_HOOK");
    let tmp = std::env::temp_dir().join(format!(
        "tok-other-refresh-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // A Codex install made by an older tokenade that lived somewhere else.
    install_codex_passive_hooks("/old/prefix/bin/tokenade", None, Some(&tmp), false);
    let codex = tmp.join(".codex").join("hooks.json");
    let before = std::fs::read_to_string(&codex).expect("codex hooks written");
    assert!(
        before.contains("/old/prefix/bin/tokenade"),
        "fixture wired to the old path"
    );
    assert!(
        before.contains("hookwrap"),
        "fixture carries the install marker"
    );

    // An agent config that exists but was never wired by tokenade.
    let factory = tmp.join(".factory");
    std::fs::create_dir_all(&factory).unwrap();
    let untouched = "{\"hooks\": {}}\n";
    std::fs::write(factory.join("hooks.json"), untouched).unwrap();

    crate::hook_health::refresh_other_hook_agents("/new/prefix/bin/tokenade", &tmp, Some(&tmp));

    let after = std::fs::read_to_string(&codex).unwrap();
    assert!(
        after.contains("/new/prefix/bin/tokenade"),
        "upgrade must re-point a hook whose binary moved"
    );
    assert!(
        !after.contains("/old/prefix/bin/tokenade"),
        "the dead path must be gone: {after}"
    );
    assert_eq!(
        std::fs::read_to_string(factory.join("hooks.json")).unwrap(),
        untouched,
        "an upgrade must repair, never install"
    );

    let _ = std::fs::remove_dir_all(&tmp);
    if let Some(v) = no_hook {
        std::env::set_var("TOKENADE_NO_HOOK", v);
    }
}

/// Qwen Code bakes `gemini-hook`, not `hookwrap`. A `hookwrap`-only gate
/// left `~/.qwen/settings.json` pointing at a wiped `target/debug/tokenade`
/// after `cargo clean` (playbook 1-in-4, live host 2026-09-19).
#[test]
fn config_proves_a_tokenade_hook_sees_qwen_gemini_family() {
    assert!(crate::hook_health::config_proves_a_tokenade_hook(
        r#"{"command":"tokenade hookwrap"}"#
    ));
    assert!(crate::hook_health::config_proves_a_tokenade_hook(
        r#"sh -c '/old/target/debug/tokenade' gemini-hook --agent qwen-code; exit 0"#
    ));
    assert!(crate::hook_health::config_proves_a_tokenade_hook(
        "tokenade gemini-lifecycle --agent qwen-code"
    ));
    assert!(crate::hook_health::config_proves_a_tokenade_hook(
        "tokenade gemini-afterhook"
    ));
    assert!(!crate::hook_health::config_proves_a_tokenade_hook(
        r#"{"hooks":{}}"#
    ));
    assert!(!crate::hook_health::config_proves_a_tokenade_hook(
        r#"{"mcpServers":{"tokenade":{}}}"#
    ));
}

#[test]
fn upgrade_repoints_qwen_gemini_hook_and_leaves_unmarked_qwen_alone() {
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let no_hook = std::env::var_os("TOKENADE_NO_HOOK");
    std::env::remove_var("TOKENADE_NO_HOOK");
    let tmp = std::env::temp_dir().join(format!(
        "tok-qwen-refresh-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // Live shape: gemini-hook baked to a build-dir binary that no longer exists.
    let dead = "/home/username/src/tokenade/target/debug/tokenade";
    let _ = crate::install::install_qwen_cli_hook(dead, Some(&tmp), false);
    let qwen = tmp.join(".qwen").join("settings.json");
    let before = std::fs::read_to_string(&qwen).expect("qwen settings written");
    assert!(
        before.contains("gemini-hook"),
        "fixture must be the Gemini/Qwen command, not hookwrap: {before}"
    );
    assert!(
        !before.contains("hookwrap"),
        "a hookwrap-only gate would have skipped this file: {before}"
    );
    assert!(
        before.contains(dead),
        "precondition: dead debug path seeded"
    );
    assert_eq!(
        crate::install_cli::any_agent_has_tokenade_in(&tmp),
        Some(true),
        "badge must see a Qwen-only gemini-hook install"
    );

    crate::hook_health::refresh_other_hook_agents("/new/prefix/bin/tokenade", &tmp, Some(&tmp));

    let after = std::fs::read_to_string(&qwen).unwrap();
    assert!(
        after.contains("/new/prefix/bin/tokenade"),
        "upgrade must re-point Qwen's gemini-hook: {after}"
    );
    assert!(
        !after.contains(dead),
        "the missing debug binary must be gone: {after}"
    );

    // Unmarked Qwen stays a first-install, never a refresh.
    let bare = std::env::temp_dir().join(format!(
        "tok-qwen-bare-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&bare);
    std::fs::create_dir_all(bare.join(".qwen")).unwrap();
    let untouched = "{\"hooks\": {}}\n";
    std::fs::write(bare.join(".qwen").join("settings.json"), untouched).unwrap();
    crate::hook_health::refresh_other_hook_agents("/new/prefix/bin/tokenade", &bare, Some(&bare));
    assert_eq!(
        std::fs::read_to_string(bare.join(".qwen").join("settings.json")).unwrap(),
        untouched,
        "an upgrade must repair, never install"
    );
    assert_eq!(
        crate::install_cli::any_agent_has_tokenade_in(&bare),
        Some(false),
        "empty Qwen settings are present but not wired"
    );

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&bare);
    if let Some(v) = no_hook {
        std::env::set_var("TOKENADE_NO_HOOK", v);
    }
}

// Three private lists of agents had drifted: what `install` writes, what an
// upgrade repairs, and what the dashboard badge counts as "tokenade is wired
// here". The badge's copy knew nothing about Droid, Copilot, Reasonix or the
// hermes bridge, so a user with ONLY one of those four was told tokenade was
// wired nowhere while it was working. This walks every agent an install can
// wire and asserts the badge sees each one — behaviour, not list equality, so
// it still catches a future agent added to one list and not the others.
/// Every host whose event set HAS a compaction seam must get
/// `hookwrap-precompact`. That handler protects the recovery pointers the
/// summary is about to drop, voids pending replays and writes the per-session
/// manifest; a host that does not receive it silently loses all of it, and the
/// only way anyone notices is by reading two installers side by side.
///
/// Which hosts have the seam is EVIDENCE, not a guess, and the provenance is
/// carried with each entry so the next reader can check it rather than trust
/// this list.
#[test]
fn every_host_with_a_compaction_seam_gets_the_write_barrier() {
    type Installer = Box<dyn Fn(&std::path::Path)>;
    let cases: Vec<(&str, Installer, &str)> = vec![
        (
            // `~/.copilot/hooks/tokenade.json`. `preCompact` is in the native
            // event set of the shipped `@github/copilot` `app.js`, and
            // `PreCompact:"preCompact"` is in its PascalCase compat map.
            "copilot",
            Box::new(|h: &std::path::Path| {
                install_copilot_hooks("/bin/tokenade", None, Some(h), false);
            }),
            "app.js native set + AYr compat map",
        ),
        (
            // `~/.grok/hooks/tokenade.json` (`user-settings.json` is
            // not a hook source; `grok inspect` reads 0 hooks there). The host
            // ships its own hook documentation, `docs/user-guide/10-hooks.md`,
            // whose event table lists `PreCompact` — "Conversation compaction
            // is about to run".
            "grok",
            Box::new(|h: &std::path::Path| {
                install_grok_hooks("/bin/tokenade", None, Some(h), false);
            }),
            "shipped docs/user-guide/10-hooks.md event table",
        ),
        (
            "droid",
            Box::new(|h: &std::path::Path| {
                install_droid_hooks("/bin/tokenade", None, Some(h), false);
            }),
            "droid hook schema",
        ),
        (
            "codex",
            Box::new(|h: &std::path::Path| {
                install_codex_passive_hooks("/bin/tokenade", None, Some(h), false);
            }),
            "codex hook schema",
        ),
    ];
    // And the hosts that must NOT be in the list above, so nobody
    // "completes" it later. `antigravity` is the one that looks like an
    // omission and is not: its `agy` binary embeds its own `hooks.json`
    // documentation, whose Supported Event Types table is exactly
    // `PreToolUse`, `PostToolUse`, `PreInvocation`, `PostInvocation`, `Stop`.
    // There is no compaction event to hook. The binary carries no `PreCompact`
    // string at all, against 7 `PreToolUse` and 9 `PostToolUse`.
    //
    // Its `PostToolUse` is absent from tokenade's installer for a second
    // measured reason, recorded here because it reads like a gap too: the same
    // documentation says the handler's stdout "Expects an empty JSON object
    // `{}`". It can deliver NOTHING — no replacement, no added context — so a
    // fold computed there could never reach the model.
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    for (host, install, provenance) in cases {
        let tmp = std::env::temp_dir().join(format!("tok-barrier-{host}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        install(&tmp);
        let mut found = false;
        let mut stack = vec![tmp.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if std::fs::read_to_string(&p)
                    .map(|c| c.contains("hookwrap-precompact"))
                    .unwrap_or(false)
                {
                    found = true;
                }
            }
        }
        assert!(
            found,
            "{host} has a compaction seam ({provenance}) and got no write barrier"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[test]
fn the_badge_sees_an_install_made_for_any_single_agent() {
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let no_hook = std::env::var_os("TOKENADE_NO_HOOK");
    std::env::remove_var("TOKENADE_NO_HOOK");
    let hermes_home = std::env::var_os("HERMES_HOME");
    std::env::remove_var("HERMES_HOME");

    let base = std::env::temp_dir().join(format!(
        "tok-badge-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&base);

    // (agent label, install one agent into a virgin home)
    let cases: Vec<(&str, Box<dyn Fn(&std::path::Path)>)> = vec![
        (
            "codex",
            Box::new(|h: &std::path::Path| {
                install_codex_passive_hooks("/bin/tokenade", None, Some(h), false);
            }),
        ),
        (
            "droid",
            Box::new(|h: &std::path::Path| {
                install_droid_hooks("/bin/tokenade", None, Some(h), false);
            }),
        ),
        (
            "copilot",
            Box::new(|h: &std::path::Path| {
                install_copilot_hooks("/bin/tokenade", None, Some(h), false);
            }),
        ),
        (
            "grok",
            Box::new(|h: &std::path::Path| {
                install_grok_hooks("/bin/tokenade", None, Some(h), false);
            }),
        ),
        (
            "hermes",
            Box::new(|h: &std::path::Path| {
                install_hermes_plugin(h, false).unwrap();
            }),
        ),
        (
            "opencode",
            Box::new(|h: &std::path::Path| {
                install_opencode_plugin(h, false).unwrap();
            }),
        ),
        (
            "pi",
            Box::new(|h: &std::path::Path| {
                install_pi_extension(h, false).unwrap();
            }),
        ),
        (
            "cline",
            Box::new(|h: &std::path::Path| {
                install_cline_plugin(h, false).unwrap();
            }),
        ),
    ];
    for (label, install) in cases {
        let home = base.join(label);
        std::fs::create_dir_all(&home).unwrap();
        install(&home);
        assert_eq!(
            any_agent_has_tokenade_in(&home),
            Some(true),
            "the badge does not see a {label}-only install"
        );
    }
    // A home with a known agent present but never wired must still say "no".
    let bare = base.join("bare");
    std::fs::create_dir_all(bare.join(".claude")).unwrap();
    std::fs::write(bare.join(".claude").join("settings.json"), "{}\n").unwrap();
    assert_eq!(any_agent_has_tokenade_in(&bare), Some(false));

    let _ = std::fs::remove_dir_all(&base);
    if let Some(v) = no_hook {
        std::env::set_var("TOKENADE_NO_HOOK", v);
    }
    if let Some(v) = hermes_home {
        std::env::set_var("HERMES_HOME", v);
    }
}

// The global CLAUDE.md fence is a short "Tokenade is installed" pointer at
// the project `AGENTS.md`. The CLI lecture (map/wrap/lean, MCP-vs-hooks)
// lives in the project scaffold — repeating it here stacked a third copy
// onto every Grok/Claude session that also loads `~/.claude/CLAUDE.md`.
#[test]
fn the_global_block_does_not_claim_hooks_cover_mcp() {
    use crate::hooks::is_shell_tool_name;
    use crate::install::claude_md::GLOBAL_CLAUDE_MD_BLOCK;

    assert!(!is_shell_tool_name("mcp__github__get_issue"));
    for matcher in [
        "Edit|Write|MultiEdit",
        "Read|Grep|Glob",
        "WebFetch",
        "Bash|Read",
    ] {
        assert!(
            !matcher.contains("mcp"),
            "a matcher now covers MCP — the block's wording needs revisiting"
        );
    }
    assert!(
        !GLOBAL_CLAUDE_MD_BLOCK.contains("automatically on hooked agents"),
        "the block promises MCP folding that no hook performs"
    );
    assert!(
        GLOBAL_CLAUDE_MD_BLOCK.contains("this machine's agent config"),
        "the short global must point at the machine lecture: {GLOBAL_CLAUDE_MD_BLOCK}"
    );
    assert!(
        !GLOBAL_CLAUDE_MD_BLOCK.contains("tokenade map"),
        "the global fence must not dump the CLI lecture: {GLOBAL_CLAUDE_MD_BLOCK}"
    );
}

// The global block is the copy of the tokenade instructions that every Claude
// Code user reads, and it had drifted from the project scaffold twice over.
// Both drifts are pinned as ABSENCES, because both were wrong in the direction
// an agent would act on.
#[test]
fn the_global_block_does_not_teach_a_sandbox_or_a_kind_parameter() {
    use crate::install::claude_md::GLOBAL_CLAUDE_MD_BLOCK;

    // `exec` is not a sandbox — it runs with the caller's permissions. The
    // project scaffold was corrected on 2026-08-04 with that reasoning spelled
    // out ("the one word an agent would lean on when deciding what is safe to
    // run"); this copy kept the wrong word for four days.
    assert!(
        !GLOBAL_CLAUDE_MD_BLOCK.contains("sandbox"),
        "the block calls `exec` a sandbox; it is a capped subprocess with the caller's rights"
    );
    // `kind=page` / `kind=serp` were MCP tool parameters, retired with the
    // zero-MCP pivot. Measured 2026-08-08: `tokenade web page.html kind=serp`
    // treats the pair as ANOTHER FILE ARGUMENT — it does not select a mode, it
    // silently changes the output shape.
    assert!(
        !GLOBAL_CLAUDE_MD_BLOCK.contains("kind="),
        "the block teaches a `kind=` parameter no command accepts"
    );
}

/// Existing-client path: a fat fenced `~/.claude/CLAUDE.md` is spliced to
/// the short body by the same `refresh_global_claude_md_block_in_place`
/// that `tokenade install` and the post-upgrade hook already call.
#[test]
fn refresh_global_claude_md_block_in_place_shortens_a_fat_fence() {
    // No `INSTALL_ENV_LOCK`: `ScratchHome` takes `ENV_LOCK`, and the mutex
    // is not reentrant — taking it first blocks the test.
    let home = tokenade_core::platform::ScratchHome::new("global-fat-refresh_");
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    // Today's fat body, frozen here so the test still proves the splice
    // after GLOBAL_CLAUDE_MD_BLOCK itself is short. Fences must be present:
    // refresh returns None when they are absent.
    let fat = "\
<!-- tokenade:start -->
## Tokenade is installed

The `tokenade` CLI is available globally. Prefer these commands
over their raw equivalents — they save 60-95% of tokens with no quality
loss:

- `tokenade exec-compact <cmd>` instead of raw `Bash <cmd>` for noisy
  commands (git, cargo, kubectl, terraform, docker)
- `tokenade map` / `tokenade skeleton <file>` instead of reading source files whole

## Keep output lean
Keep prose terse and code minimal.
<!-- tokenade:end -->
";
    let claude_md = claude_dir.join("CLAUDE.md");
    std::fs::write(
        &claude_md,
        format!("# My personal rules\n\n{fat}\nKeep me.\n"),
    )
    .unwrap();
    let first = crate::install::claude_md::refresh_global_claude_md_block_in_place(
        Some(home.path()),
        false,
    );
    assert_eq!(first, Some(true), "fat fence must splice");
    let after = std::fs::read_to_string(&claude_md).unwrap();
    assert!(after.contains("Tokenade is installed"), "{after}");
    assert!(after.contains("this machine's agent config"), "{after}");
    assert!(
        !after.contains("tokenade map"),
        "fat CLI lecture survived: {after}"
    );
    assert!(
        !after.contains("60-95%"),
        "savings lecture survived: {after}"
    );
    assert!(
        !after.contains("Keep output lean"),
        "brevity dump survived: {after}"
    );
    assert!(after.contains("# My personal rules") && after.contains("Keep me."));
    let second = crate::install::claude_md::refresh_global_claude_md_block_in_place(
        Some(home.path()),
        false,
    );
    assert_eq!(second, Some(false), "second refresh must be a no-op");
    let empty = home.path().join("elsewhere");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(
        crate::install::claude_md::refresh_global_claude_md_block_in_place(Some(&empty), false),
        None,
        "absent fences must stay untouched"
    );
}

/// The user's refusal is remembered, and an explicit request cancels it.
///
/// This is the safeguard that makes automatic installation acceptable: the
/// proxy installs itself wherever it brings something, so it MUST know how to
/// stay quiet once it has been removed. Without this list, the startup repair
/// would put back every 24 h what the user just removed.
#[test]
fn a_removed_proxy_stays_removed_until_asked_for_again() {
    use crate::llm_proxy::install::{clear_declined, is_declined, mark_declined};
    use tokenade_core::detect::Agent;
    let home = std::env::temp_dir().join(format!(
        "tkn-declined-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(home.join(".tokenade")).unwrap();

    assert!(
        !is_declined(&home, Agent::Aider),
        "nothing is declined at the start"
    );
    mark_declined(&home, Agent::Aider);
    assert!(is_declined(&home, Agent::Aider));
    // Idempotent: two refusals do not make two lines.
    mark_declined(&home, Agent::Aider);
    let body = std::fs::read_to_string(crate::llm_proxy::install::declined_path(&home)).unwrap();
    assert_eq!(
        body.lines().filter(|l| l.trim() == "aider").count(),
        1,
        "{body:?}"
    );

    // A refusal does not spill over to another agent.
    assert!(!is_declined(&home, Agent::OpenCode));
    mark_declined(&home, Agent::OpenCode);
    assert!(is_declined(&home, Agent::Aider) && is_declined(&home, Agent::OpenCode));

    // Asking for the install by hand cancels the refusal — and only its own.
    clear_declined(&home, Agent::Aider);
    assert!(!is_declined(&home, Agent::Aider));
    assert!(
        is_declined(&home, Agent::OpenCode),
        "the other refusal survives"
    );

    // Removing the last refusal deletes the file rather than leaving an empty
    // shell, which would read back as "a blank line is declined".
    clear_declined(&home, Agent::OpenCode);
    assert!(!crate::llm_proxy::install::declined_path(&home).exists());
    let _ = std::fs::remove_dir_all(&home);
}

/// The subagent brief exists for exactly one moment — a child agent
/// starting with no idea tokenade is installed — and it reaches that moment
/// only where `SubagentStart` is registered. Codex knows the event: its shipped
/// binary carries `subagent-start.command.input` and `.output` as full JSON
/// Schema documents, and `HookEventNameWire` lists `SubagentStart` beside
/// `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`,
/// `PostCompact`, `SessionStart`, `SessionEnd` and `SubagentStop`.
///
/// The table is (host, installer, provenance) rather than a list of names, so
/// the next host cannot be added without saying where the claim was read.
#[test]
fn every_host_with_a_subagent_seam_briefs_the_child() {
    type Installer = Box<dyn Fn(&std::path::Path)>;
    let cases: Vec<(&str, Installer, &str)> = vec![
        (
            // `onSubagentStart` in the shipped `app.js`, whose reader is
            // `l?.additionalContext !== void 0 ? {additionalContext: …}`.
            "copilot",
            Box::new(|h: &std::path::Path| {
                install_copilot_hooks("/bin/tokenade", None, Some(h), false);
            }),
            "app.js onSubagentStart",
        ),
        // grok was a case here and is NOT any more, which is the kind
        // of removal this table is supposed to make hard. Its shipped
        // `docs/user-guide/10-hooks.md` does list `SubagentStart`, so the
        // EVENT is real; the delivery is not. That same document says
        // "`PreToolUse` can block a tool call, and `Stop`/`SubagentStop` can
        // block the agent from stopping; every other event is passive", and
        // spells passive out as "stdout is ignored". The binary agrees: its
        // only code-side `additionalContext` sits beside `stopReason` and
        // `hookCallbackId` on the stop path, and it carries no
        // `updatedToolOutput` or `updatedInput` at all. A brief written to a
        // stream nobody reads is not a brief, and the entry cost a process per
        // subagent to produce nothing. The seam is an event AND a way to carry
        // the answer back — this table tests the pair.
        (
            // `subagent-start.command.{input,output}` shipped inside the
            // codex binary as JSON Schema.
            "codex",
            Box::new(|h: &std::path::Path| {
                install_codex_passive_hooks("/bin/tokenade", None, Some(h), false);
            }),
            "codex subagent-start.command.output schema",
        ),
    ];
    let _g = INSTALL_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    for (host, install, provenance) in cases {
        let tmp = std::env::temp_dir().join(format!("tok-subagent-{host}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        install(&tmp);
        let mut found = false;
        let mut stack = vec![tmp.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if std::fs::read_to_string(&p)
                    .map(|c| c.contains("hookwrap-subagentstart"))
                    .unwrap_or(false)
                {
                    found = true;
                }
            }
        }
        assert!(
            found,
            "{host} exposes a subagent seam ({provenance}) but no \
             hookwrap-subagentstart was written; a child agent there starts \
             with no brief"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// The negatives, measured rather than assumed, so nobody "completes" the table
/// above by adding a host that cannot use it:
///
/// - **droid** — its hook set, read at the source named in
///   `every_host_with_a_compaction_seam_gets_the_write_barrier`, carries no
///   subagent event at all.
/// - **antigravity** — its shipped `hooks.json` documentation lists
///   `PreToolUse`, `PostToolUse`, `PreInvocation`, `PostInvocation` and `Stop`,
///   and nothing about subagents.
const _SUBAGENT_SEAM_NEGATIVES: () = ();

/// The `--with-hook` note used to end with "(<agent> has no compatible
/// PostToolUse hook)", a claim about the HOST that is false for at least two of
/// them — tokenade registers a `postToolUse` for Copilot and a `PostToolUse` for
/// Codex. The comment above it had already been corrected to say the note is
/// about the FLAG; the printed sentence had not.
///
/// Pinned against the REGISTRATIONS rather than against the wording, so the
/// sentence cannot drift back into contradicting what the installer writes.
#[test]
fn the_with_hook_note_never_denies_a_hook_we_register() {
    // The family, not the trunk: `install_cli.rs` has a sibling directory, and
    // a guard that only reads the trunk stops seeing what moves down into the
    // branch — without saying so.
    let cli_src = tokenade_core::source_guard::crates_dir()
        .join("tokenade-cli")
        .join("src");
    let src = tokenade_core::source_guard::module_family(&cli_src, "install_cli");
    let src = src.as_str();
    let note = src
        .lines()
        .find(|l| l.contains("--with-hook") && l.contains("note:"))
        .expect("the --with-hook note moved or was renamed");
    assert!(
        !note.contains("no compatible"),
        "the note denies the host a hook; tokenade registers PostToolUse for \
         Copilot and Codex, and PreToolUse for Copilot: {note}"
    );
    assert!(
        note.contains("Claude Code"),
        "the note must still say WHICH hook the flag names: {note}"
    );
}

/// Twice now, in the same file, the CODE was corrected and the prose was
/// not: `install_cli.rs` still said Copilot had no comparable Pre hook after
/// one was wired, and `install_grok_hooks` still said "omit PreCompact, which
/// Grok doesn't emit" twenty lines above the `PreCompact` entry added later.
///
/// A doc comment that denies what its own function does is worse than no
/// comment — it sends the next reader to re-derive a settled question. This
/// reads the SOURCE and refuses the contradiction directly: no comment may say
/// an event is omitted or unemitted while a registration for it stands.
#[test]
fn no_comment_denies_an_event_this_file_registers() {
    // The FAMILY: the per-agent installers moved down into
    // `lifecycle_hooks/peers.rs`, and that is where the comments this test
    // refuses to let lie now live.
    let root = tokenade_core::source_guard::crates_dir()
        .join("tokenade-cli")
        .join("src");
    let src = tokenade_core::source_guard::module_family(&root.join("install"), "lifecycle_hooks");
    let src = src.as_str();
    // The events this file actually writes, taken from the registration
    // literals rather than from a list someone has to maintain.
    const EVENTS: &[&str] = &[
        "PreCompact",
        "SubagentStart",
        "PreToolUse",
        "PostToolUse",
        "SessionStart",
        "UserPromptSubmit",
    ];
    const DENIALS: &[&str] = &["doesn't emit", "does not emit", "omit ", "has no "];
    let mut offenders = Vec::new();
    for (n, line) in src.lines().enumerate() {
        let t = line.trim_start();
        if !t.starts_with("///") && !t.starts_with("//") {
            continue;
        }
        for ev in EVENTS {
            if !t.contains(ev) {
                continue;
            }
            // Registered anywhere in this file? (a quoted key in a literal)
            if !src.contains(&format!("\"{ev}\"")) {
                continue;
            }
            if DENIALS.iter().any(|d| t.contains(d)) {
                offenders.push(format!("{}: {}", n + 1, t.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these comments deny an event this file registers: {offenders:#?}"
    );
}

/// The binary the hooks name keeps them: a visitor (the compat bench's staged
/// copy, run with the operator's real HOME on 2026-10-03) must not refresh
/// them, the owner may, and a named binary that is gone may be repaired.
#[test]
fn a_visitor_binary_leaves_the_installed_ones_hooks_alone() {
    let home = tokenade_core::platform::ScratchHome::new("hook-owner");
    let installed = home.path().join("bin").join("tokenade");
    std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
    std::fs::write(&installed, "").unwrap();
    let installed = installed.display().to_string();
    let claude = home.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    let settings = serde_json::json!({
        "hooks": {"PostToolUse": [{"hooks": [{"type": "command",
            "command": format!("sh -c 'TOKENADE_DEBUG_LOG='\\''/x/debug.log'\\'' exec {installed} hookwrap'")}]}]},
        "statusLine": {"type": "command", "command": format!("{installed} statusline")}
    });
    std::fs::write(claude.join("settings.json"), settings.to_string()).unwrap();

    assert_eq!(
        crate::hook_binaries_named_in(&settings.to_string()),
        vec![installed.clone()]
    );
    let visitor = "/home/u/.cache/tokenade-compat-bin/abc/tokenade";
    assert!(crate::another_installed_binary_owns_the_hooks(
        visitor,
        Some(home.path())
    ));
    assert!(!crate::another_installed_binary_owns_the_hooks(
        &installed,
        Some(home.path())
    ));

    std::fs::remove_file(&installed).unwrap();
    assert!(
        !crate::another_installed_binary_owns_the_hooks(visitor, Some(home.path())),
        "a named binary that is gone is a broken install: repairing it stays allowed"
    );
}
