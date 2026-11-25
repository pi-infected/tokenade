//! **Write the hook entries for Qwen Code (Gemini CLI family).**
//!
//! A fork, its parent's settings file, its own dialect. The operation is
//! idempotent and REVERSIBLE: an entry recognised as ours is replaced, never
//! duplicated, and the events of an earlier version are removed — otherwise
//! an update leaves behind a dead hook that still runs.

use super::*;

/// BeforeTool matcher for the Gemini hook: the shell tool (rewritten to
/// `tokenade wrap`) plus the file-read tools (redirected to a compacted temp).
pub(crate) const GEMINI_HOOK_MATCHER: &str = "^(run_shell_command|read_file|read_many_files)$";

/// #288 — every Gemini-family hook command, in the ONE fail-open shape.
///
/// These three were built bare — `<exe> <subcommand>` — while the Claude
/// writers went through `build_hook_command`, whose whole reason for existing
/// is that a baked exe path stops resolving (an npm reinstall relocates the
/// binary, an auto-update swaps it, Gatekeeper kills an unsigned one). On
/// Claude that costs a per-prompt nag. On Gemini it costs the tool call:
/// read off the bundled host, `stdout.trim() || stderr.trim()` means the
/// SHELL's "No such file or directory" becomes the hook's own output, and
/// `convertPlainTextToHookOutput` turns any exit code that is neither 0 nor
/// `EXIT_CODE_NON_BLOCKING_ERROR` (1) into `{decision: "deny", reason: text}`.
/// A missing binary would deny every matched tool call, with a shell error as
/// the reason.
///
/// Routed through the same declarer rather than given a second wrapper of its
/// own: the POSIX/Windows split, the quoting, and the two rules that actually
/// make it fail-open (no `exec`, trailing `; exit 0`) then have one home.
pub(crate) fn gemini_hook_command(exe: &str, subcommand: &str) -> String {
    super::hook_util::build_hook_command(exe, subcommand, None)
}

/// Write the tool hook into Qwen Code's `~/.qwen/settings.json`.
/// Qwen Code is a hard fork of Gemini CLI with an identical settings schema
/// and identical tool names, so the entire mechanism — matcher, hook entry,
/// and the `gemini-hook` handler it invokes — is reused verbatim; only the
/// config directory differs. Best-effort: a Qwen build that predates Gemini's
/// hooks feature ignores the (harmless) entry.
pub(crate) fn install_qwen_cli_hook(exe: &str, home: Option<&Path>, dry_run: bool) -> HookOutcome {
    let out = install_before_tool_hook(exe, home, ".qwen", dry_run);
    if !dry_run {
        retire_dead_gemini_events(home);
    }
    out
}

/// **Gemini's events are DEAD on Qwen Code.**
///
/// Qwen Code was a fork of Gemini CLI and this module wrote its entries
/// verbatim — `BeforeTool`, `AfterTool`, `BeforeAgent`. Its shipped
/// documentation now says otherwise
/// (<https://qwenlm.github.io/qwen-code-docs/en/users/features/hooks/>): its
/// events carry Claude's names (`PreToolUse`, `PostToolUse`,
/// `SessionStart`, `UserPromptSubmit`, `PreCompact`, `SubagentStart`…), its
/// `matcher` is a regex evaluated on the tool's EXECUTION ID
/// (`run_shell_command`, `read_file`…), and `PreToolUse` accepts
/// `hookSpecificOutput.updatedInput` — « modified tool input parameters to use
/// instead of original ».
///
/// This is exactly what an earlier measurement saw without being able to name
/// it: file READ, entries DEAD. An event name a host does not know triggers
/// nothing and says nothing.
///
/// This function removes OUR entries from the three dead events. It touches
/// neither another tool's entries nor the rest of the file — a user who wrote
/// their own `BeforeTool` keeps them.
pub(crate) fn retire_dead_gemini_events(home: Option<&Path>) {
    let Some(path) = qwen_settings_path(home) else {
        return;
    };
    let Ok(mut settings) = super::json_util::read_json_object_or_bail(&path) else {
        return;
    };
    let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return;
    };
    let mut changed = false;
    for dead in ["BeforeTool", "AfterTool", "BeforeAgent"] {
        let Some(arr) = hooks.get_mut(dead).and_then(|a| a.as_array_mut()) else {
            continue;
        };
        let before = arr.len();
        arr.retain(|entry| !entry_is_ours(entry));
        if arr.len() != before {
            changed = true;
        }
        if arr.is_empty() {
            hooks.remove(dead);
        }
    }
    if changed {
        let _ = write_json_atomic(&path, &serde_json::to_string_pretty(&settings).unwrap());
    }
}

/// The path of Qwen Code's `settings.json`, test home included.
pub(crate) fn qwen_settings_path(home: Option<&Path>) -> Option<PathBuf> {
    let h = match home {
        Some(h) => h.to_path_buf(),
        None => tokenade_core::platform::home_dir()?,
    };
    Some(h.join(".qwen").join("settings.json"))
}

/// Is a hook entry OURS? The name we write, or a command that calls our
/// binary — never a `contains("tokenade")` on the whole line, which would
/// catch another tool's command stored under a tokenade-named path.
pub(crate) fn entry_is_ours(entry: &serde_json::Value) -> bool {
    entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|hs| {
            hs.iter().any(|h| {
                let named = h.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let cmd = h.get("command").and_then(|c| c.as_str()).unwrap_or("");
                named.starts_with("tokenade-")
                    || cmd.contains("gemini-hook")
                    || cmd.contains("gemini-after-tool")
                    || cmd.contains("gemini-lifecycle")
            })
        })
        .unwrap_or(false)
}

/// Qwen's status line, written into the same settings file.
///
/// Qwen Code is the only agent of this family that exposes one; the block sat
/// inside the hook installer purely because both write `settings.json`.
/// `true` when it changed something.
pub(crate) fn install_qwen_statusline(settings: &mut serde_json::Value, exe: &str) -> bool {
    let mut changed = false;
    let root = settings.as_object_mut().unwrap();
    let sl_obj = root
        .get("statusLine")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    // A curated `type: "preset"` config is the USER's explicit choice of
    // built-in items — there is no command to wrap; leave it alone
    // (audit C: we silently replaced presets with our command).
    let is_preset = sl_obj.get("type").and_then(|t| t.as_str()) == Some("preset");
    let current = sl_obj
        .get("command")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let canonical_sl = format!("{exe} statusline");
    use base64::Engine as _;
    // Precise ownership predicate (audit C): first token's basename is
    // the tokenade binary — `contains("tokenade")` also matched foreign
    // commands under tokenade-named paths and REPLACED them.
    let is_ours = current.split_whitespace().next().is_some_and(|tok| {
        let base = tok.rsplit(['/', '\\']).next().unwrap_or(tok);
        base == "tokenade" || base == "tokenade.exe"
    });
    let new_cmd = if is_preset {
        None
    } else if is_ours {
        if current.starts_with(&canonical_sl) {
            None // current and ours — untouched
        } else {
            let suffix = current
                .find("--wrap-b64")
                .map(|i| format!(" {}", &current[i..]))
                .unwrap_or_default();
            Some(format!("{canonical_sl}{suffix}"))
        }
    } else if current.trim().is_empty() {
        Some(canonical_sl.clone())
    } else {
        Some(format!(
            "{canonical_sl} --wrap-b64 {}",
            base64::engine::general_purpose::STANDARD.encode(current.trim())
        ))
    };
    if let Some(cmd) = new_cmd {
        // MUTATE the command key only — refreshInterval,
        // respectUserColors, hideContextIndicator survive (audit C:
        // json!({type,command}) wiped them on every heal).
        let mut obj = sl_obj.as_object().cloned().unwrap_or_default();
        obj.insert("type".into(), serde_json::json!("command"));
        obj.insert("command".into(), serde_json::json!(cmd));
        root.insert("statusLine".into(), serde_json::Value::Object(obj));
        changed = true;
    }
    changed
}

/// The `AfterTool` counterpart of the hook, in the same settings file.
/// `true` when it changed something.
pub(crate) fn install_after_tool_hook(settings: &mut serde_json::Value, after_cmd: &str) -> bool {
    let mut changed = false;
    let Some(root) = settings.as_object_mut() else {
        return false;
    };
    let hooks = crate::install::json_util::ensure_object_entry(root, "hooks");
    let aarr = crate::install::json_util::ensure_array_entry(hooks, "AfterTool");
    let has_fold = aarr.iter().any(|e| {
        e.get("hooks")
            .and_then(|h| h.as_array())
            .map(|hs| {
                hs.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .map(|c| c.contains("gemini-after-tool"))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    });
    if !has_fold {
        aarr.push(serde_json::json!({
            "matcher": ".*",
            "hooks": [{ "name": "tokenade-fold", "type": "command", "command": after_cmd }]
        }));
        changed = true;
    }
    changed
}

/// Which agent of the Gemini family a config dir belongs to.
///
/// `gemini-cli` and `qwen-code` install the SAME two hook commands, so the
/// hook process cannot tell them apart at runtime — and both
/// `cmd_gemini_hook` and `cmd_gemini_afterhook` hardcoded
/// `TOKENADE_AGENT=gemini-cli`, which credits every Qwen saving to Gemini.
/// The bridges hit the identical problem with kilo and opencode and solved it
/// by injecting the agent at install time; same answer here, passed as a flag
/// so no shell wrapper is needed on Windows.
pub(crate) fn agent_slug_for(config_dir: &str) -> &'static str {
    if config_dir.contains("qwen") {
        "qwen-code"
    } else {
        "gemini-cli"
    }
}

/// The event vocabulary of a host of this family.
///
/// An entry written under a name the host does not know triggers nothing AND
/// says nothing — the most expensive failure shape we have seen.
pub(crate) struct HookDialect {
    /// The event that precedes a tool call.
    pub(crate) tool_event: &'static str,
    /// The event on which to attach the style preamble.
    pub(crate) prompt_event: &'static str,
    /// The event that carries the output FOLD hook, or `None` when this host
    /// does not get one.
    ///
    /// This fact used to be read from `needs_enabled_flag`, which answers a
    /// completely different question — « does this host require
    /// `hooks.enabled`? ». A flag that means two things goes wrong as soon as
    /// one of them changes: the 2026-09-12 measurement made
    /// `needs_enabled_flag` true for Qwen, which immediately switched on a
    /// fold hook there that the dead-event removal pass deleted in the same
    /// call. Written then erased on every install, and idempotence reported a
    /// change every time.
    pub(crate) fold_event: Option<&'static str>,
    pub(crate) needs_enabled_flag: bool,
    /// The event that precedes a context compaction: what was in it stops
    /// being re-read, on the credit side as on the cost side. Without it, a
    /// fold and an injection stayed counted as re-read until the horizon.
    pub(crate) compact_event: &'static str,
}

impl HookDialect {
    pub(crate) fn for_config_dir(config_dir: &str) -> Self {
        if config_dir == ".qwen" {
            Self {
                tool_event: "PreToolUse",
                prompt_event: "UserPromptSubmit",
                // **`hooks.enabled` is required on Qwen Code TOO, and that is
                // measured.**
                //
                // This field was `false`, on the reasoning that the flag
                // belongs to Gemini and that Qwen's opt-out is
                // `disableAllHooks` — so writing it would add a key the host
                // walks as an event name (it does: under `--debug` it
                // complains about « Invalid hook event name: enabled »).
                //
                // MEASURED on 2026-09-12 on `qwen 0.7.x`, twice in each
                // direction, with the same 401-line file: WITHOUT the flag,
                // no hook fires and the model answers « no error » on the
                // output qwen itself truncates to 102 lines; WITH it, the
                // hook folds and the model names the `ERROR` line number
                // 401. A cosmetic complaint under `--debug` costs less than
                // an entire inert stage.
                // Qwen knows `PostToolUse`, but output substitution by a hook
                // is not verified there: an entry placed there would launch a
                // process per tool call for a result nobody receives.
                fold_event: None,
                needs_enabled_flag: true,
                compact_event: "PreCompact",
            }
        } else {
            // The parent's dialect, Gemini CLI (removed from the product): no
            // installer goes through here anymore; kept as the family default.
            Self {
                tool_event: "BeforeTool",
                prompt_event: "BeforeAgent",
                fold_event: Some("AfterTool"),
                needs_enabled_flag: true,
                compact_event: "PreCompress",
            }
        }
    }
}

/// Shared installer for the Gemini-family `BeforeTool` hook. `config_dir` is
/// the agent's dot-dir under `$HOME` (`.gemini` / `.qwen`); everything else is
/// identical across the family.
pub(crate) fn install_before_tool_hook(
    exe: &str,
    home: Option<&Path>,
    config_dir: &str,
    dry_run: bool,
) -> HookOutcome {
    {
        let _p = tokenade_core::profile::phase("gemini:opted_out_probe");
        if is_hook_opted_out() {
            return HookOutcome::OptedOut;
        }
    }
    let settings_path = if let Some(h) = home {
        h.join(config_dir).join("settings.json")
    } else {
        match tokenade_core::platform::home_dir() {
            Some(h) => h.join(config_dir).join("settings.json"),
            None => return HookOutcome::Failed("cannot resolve home directory".into()),
        }
    };

    // Keep the user's own bytes BEFORE anything in this install touches them.
    //
    // Late is the same as never here: measured 2026-08-19, by the time the
    // shared writers took their copy, an earlier pass had already dropped an
    // empty `"mcpServers": {}` from the file — so the "original" on disk was
    // already ours, and the uninstall put back `{}` for a user who had written
    // `{"mcpServers":{}}`. That is a CONTENT change surviving an uninstall, not
    // a formatting one.
    if !dry_run {
        tokenade_core::paths::backup_once(&settings_path);
    }

    // **Two hosts, two vocabularies.** Qwen Code was a fork of Gemini CLI and
    // this writer wrote Gemini's events to it verbatim; its shipped docs now
    // say it speaks Claude's
    // (<https://qwenlm.github.io/qwen-code-docs/en/users/features/hooks/>):
    // `PreToolUse` instead of `BeforeTool`, `UserPromptSubmit` instead of
    // `BeforeAgent`, a regex `matcher` on the tool's execution ID, and
    // `hookSpecificOutput.updatedInput` to rewrite the call. An earlier
    // measurement already saw it without naming it: file READ, entries DEAD.
    //
    // `enabled` is GEMINI's master switch (without it, zero active entries);
    // Qwen has none — its opt-out is `disableAllHooks`. Writing it there
    // added a key the host walks as an event name.
    let dialect = HookDialect::for_config_dir(config_dir);
    // Build canonical hook entry.
    let agent = agent_slug_for(config_dir);
    let canonical_command = gemini_hook_command(exe, &format!("gemini-hook --agent {agent}"));
    let canonical_hook = serde_json::json!({
        "name": "tokenade-wrap",
        "type": "command",
        "command": canonical_command,
    });
    // Matches the shell tool (rewritten to `tokenade wrap`) AND the file-read
    // tools (redirected to a pre-compacted temp file). Gemini evaluates the
    // matcher as `new RegExp(matcher).test(toolName)`.
    let canonical_entry = serde_json::json!({
        "matcher": GEMINI_HOOK_MATCHER,
        "hooks": [canonical_hook],
    });

    // Load existing settings (or start fresh).
    let mut settings: serde_json::Value =
        match tokenade_core::profile::timed("gemini:read_settings", || {
            super::json_util::read_json_object_or_bail(&settings_path)
        }) {
            Ok(v) => v,
            Err(e) => return HookOutcome::Failed(e),
        };

    // Ensure hooks object.
    let hooks_obj = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert(serde_json::json!({}));
    if !hooks_obj.is_object() {
        *hooks_obj = serde_json::json!({});
    }
    // REQUIRED master switch: Gemini gates whether hook entries fire on
    // `settings.hooks.enabled` (settingsSchema getEnableHooksUI → `?? false`;
    // verified: without it the registry loads zero enabled entries and NO
    // hook fires). Gemini also iterates every `hooks` key as an event name, so
    // it logs a cosmetic `Invalid hook event name: enabled` under `--debug` —
    // unavoidable while the flag is required, and invisible in normal runs.
    let enabled_changed = if dialect.needs_enabled_flag {
        let enabled_ok = hooks_obj.get("enabled") == Some(&serde_json::json!(true));
        hooks_obj["enabled"] = serde_json::json!(true);
        !enabled_ok
    } else {
        false
    };

    // Inspect existing BeforeTool array.
    let before_tool = hooks_obj
        .as_object_mut()
        .unwrap()
        .entry(dialect.tool_event)
        .or_insert(serde_json::json!([]));
    if !before_tool.is_array() {
        *before_tool = serde_json::json!([]);
    }
    let arr = before_tool.as_array_mut().unwrap();

    // Check for existing tokenade-wrap entry.
    let _p_scan = tokenade_core::profile::phase("gemini:scan_entries");
    let mut existing_idx: Option<usize> = None;
    let mut already_present = false;
    for (i, entry) in arr.iter().enumerate() {
        let hooks_in_entry = entry.get("hooks").and_then(|h| h.as_array());
        let has_tokenade = hooks_in_entry
            .map(|hs| {
                hs.iter().any(|h| {
                    h.get("name").and_then(|n| n.as_str()) == Some("tokenade-wrap")
                        || h.get("command")
                            .and_then(|c| c.as_str())
                            .map(|c| c.contains("tokenade") && c.contains("gemini-hook"))
                            .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if has_tokenade {
            // Check if command matches canonical.
            let cmd_matches = hooks_in_entry
                .map(|hs| {
                    hs.iter().any(|h| {
                        h.get("command").and_then(|c| c.as_str()) == Some(&canonical_command)
                    })
                })
                .unwrap_or(false);
            let matcher_matches =
                entry.get("matcher").and_then(|m| m.as_str()) == Some(GEMINI_HOOK_MATCHER);
            if cmd_matches && matcher_matches {
                already_present = true;
            } else {
                existing_idx = Some(i);
            }
            break;
        }
    }

    if !already_present {
        if let Some(idx) = existing_idx {
            arr[idx] = canonical_entry;
        } else {
            arr.push(canonical_entry);
        }
    }

    // ── statusLine (Qwen Code only — a fork addition, verified ABSENT from
    // upstream Gemini CLI 0.25). Same protocol as Claude Code (JSON context
    // on stdin, session_id/model/workspace), so `tokenade statusline` works
    // verbatim. Same 3-case policy as the Claude wiring: install ours, wrap
    // a foreign command (base64), heal a stale tokenade path (keeping any
    // wrap suffix), leave a current tokenade entry untouched.
    drop(_p_scan);
    let _p_tail = tokenade_core::profile::phase("gemini:after_scan");
    let mut sl_changed = false;
    // Same opt-outs as Claude Code's: the `statusline off` sentinel (only the
    // env var was read here), and a ticker the user took out by hand. A file
    // that already carried our hook predates the record: it was wired then.
    if config_dir == ".qwen" && !statusline_opted_out(home) {
        let current = settings["statusLine"]["command"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let installed_before = already_present || existing_idx.is_some();
        if !statusline_removed_by_user(&settings_path, &current, home, installed_before) {
            sl_changed = install_qwen_statusline(&mut settings, exe);
            if !dry_run {
                if let Some(now) = settings["statusLine"]["command"].as_str() {
                    record_statusline_wired(&settings_path, now, home);
                }
            }
        }
    }

    // AfterTool output-fold hook — Antigravity's clean replace-output seam
    // (`{decision:"deny",reason}` where `reason` replaces the tool result the
    // model sees). Gemini/Qwen builds predating AfterTool ignore the unknown
    // event (harmless). Covers tool results the BeforeTool wrap/redirect can't
    // pre-compact (MCP + misc tools); already-folded shell/read results here
    // just pass through as a no-op.
    let after_cmd = if exe == "tokenade" {
        format!(
            "tokenade gemini-after-tool --agent {}",
            agent_slug_for(config_dir)
        )
    } else {
        format!(
            "{exe} gemini-after-tool --agent {}",
            agent_slug_for(config_dir)
        )
    };
    // **No `AfterTool` on Qwen.** This stage replaces a tool's OUTPUT, and the
    // output options Qwen's docs grant `PostToolUse` are `decision`, `reason`
    // and `additionalContext` — no substitution. An entry placed there would
    // launch a process per tool call for a result nobody receives: the exact
    // reason four of grok's six hooks were removed.
    let after_changed = match dialect.fold_event {
        Some(_) => install_after_tool_hook(&mut settings, &after_cmd),
        None => false,
    };

    // The two events this host has and we were not using. `BeforeAgent` is the
    // per-TURN seam (the style preamble rides as `additionalContext`);
    // `SessionStart` is the one that opens the session. Both are declared in
    // the host's own `HookEventName` enum, and both accept `additionalContext`
    // in their output type — see `cmd_gemini_lifecycle` for why not
    // `BeforeModel`. The compaction event resets what the context no longer
    // holds. Registered before the "already present" exit: an event added in
    // a later version must reach an install that has the others.
    let lifecycle_command = gemini_hook_command(exe, &format!("gemini-lifecycle --agent {agent}"));
    let hooks_obj = settings
        .as_object_mut()
        .unwrap()
        .get_mut("hooks")
        .expect("hooks object was created above");
    let mut lifecycle_changed = false;
    for (event, tag) in [
        (dialect.prompt_event, "tokenade-style"),
        ("SessionStart", "tokenade-sessionstart"),
        (dialect.compact_event, "tokenade-precompact"),
    ] {
        lifecycle_changed |= register_gemini_event(hooks_obj, event, tag, &lifecycle_command);
    }

    if already_present && !sl_changed && !after_changed && !enabled_changed && !lifecycle_changed {
        return HookOutcome::AlreadyPresent;
    }

    if dry_run {
        return HookOutcome::Installed(settings_path);
    }

    if let Some(parent) = settings_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            return HookOutcome::Failed(format!("mkdir {}: {e}", parent.display()));
        }
    }
    let _p_write = tokenade_core::profile::phase("gemini:write_settings");
    match write_json_atomic(
        &settings_path,
        &serde_json::to_string_pretty(&settings).unwrap(),
    ) {
        Ok(()) => {
            if existing_idx.is_some() {
                HookOutcome::Refreshed(settings_path)
            } else {
                HookOutcome::Installed(settings_path)
            }
        }
        Err(e) => HookOutcome::Failed(format!("write {}: {e}", settings_path.display())),
    }
}

/// Register one non-tool Gemini event pointing at `gemini-lifecycle`.
///
/// Idempotent by TAG, not by position: an entry carrying our `name` is
/// refreshed in place, anything else the user wrote is left alone. Returns
/// true when the settings actually changed.
///
/// No `matcher`: Gemini evaluates it as `new RegExp(matcher).test(toolName)`
/// and these events have no tool name. The tool entries keep theirs.
pub(crate) fn register_gemini_event(
    hooks_obj: &mut serde_json::Value,
    event: &str,
    tag: &str,
    command: &str,
) -> bool {
    let arr = hooks_obj
        .as_object_mut()
        .unwrap()
        .entry(event)
        .or_insert(serde_json::json!([]));
    if !arr.is_array() {
        *arr = serde_json::json!([]);
    }
    let list = arr.as_array_mut().unwrap();
    let canonical = serde_json::json!({
        "hooks": [{ "name": tag, "type": "command", "command": command }]
    });
    for entry in list.iter_mut() {
        let ours = entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|hs| {
                hs.iter().any(|h| {
                    h.get("name").and_then(|n| n.as_str()) == Some(tag)
                        || h.get("command")
                            .and_then(|c| c.as_str())
                            .map(|c| c.contains("gemini-lifecycle"))
                            .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if ours {
            if *entry == canonical {
                return false;
            }
            *entry = canonical;
            return true;
        }
    }
    list.push(canonical);
    true
}
