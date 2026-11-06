#![allow(unused_imports)]
//! Mechanical module split of install.rs (no behaviour change). Wired
//! back into the `install` namespace by mod.rs via `pub(crate) use <name>::*;`.
use super::*;
use crate::hooks::*;
use crate::wrap::*;
use crate::*;
use std::fs;
use std::path::{Path, PathBuf};

/// #565 — install the three matcher-less hooks (UserPromptSubmit,
/// SessionStart, PreCompact). Lived inline inside
/// `install_claude_code_lsp_enforcement_hook` for historical reasons,
/// but none are LSP-related: UPS injects the response-style preamble
/// (no-op when `tokenade style off`), SS emits a 1-line state pointer
/// at session boot, PC nudges the agent to write verbatim notes to
/// a project file before context compaction (#702; pre-#702 it
/// pointed at the retired `memory_set` MCP tool).
/// All three are "install unconditionally per the doc comments" but
/// were silently gated behind `--with-lsp-enforcement-hook` because
/// of the call-site location. Extracted so plain `tokenade install`
/// gets them.
pub(crate) fn install_claude_code_passive_hooks(
    root: &Path,
    exe: &str,
    debug_log_path: Option<&str>,
    global: bool,
    home: Option<&Path>,
    dry_run: bool,
) -> HookOutcome {
    let (path, mut settings) = match open_claude_settings(root, global, home, dry_run) {
        Ok(v) => v,
        Err(o) => return o,
    };
    // On Windows the hook commands are direct exe calls (no inline `set`
    // env injection — see build_hook_command), so the debug-log path is
    // carried by the settings.json global `env` block instead. Idempotent;
    // a no-op on Unix and when no debug-log path was requested.
    ensure_debug_log_env(&mut settings, debug_log_path);
    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert(serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let hooks_obj = hooks.as_object_mut().unwrap();
    // Same content-aware upsert pattern as #564. Returns true if a
    // stale entry was replaced; false otherwise (already canonical OR
    // freshly pushed — both subsumed under "no refresh needed" for
    // the outcome combination below).
    let upsert = |array: &mut Vec<serde_json::Value>, tag: &str, command: String| -> (bool, bool) {
        // (refreshed, did_any_work)
        let mut stale_idx: Option<usize> = None;
        let mut already = false;
        // Each tag corresponds to a dispatch subcommand of the same
        // stem (tokenade-X → hookwrap-X). The detection accepts both
        // properly tagged AND untagged-but-recognised entries (#775).
        let subcmd = match tag {
            "tokenade-userprompt" => "hookwrap-userprompt",
            "tokenade-sessionstart" => "hookwrap-sessionstart",
            "tokenade-subagentstop" => "hookwrap-subagentstop",
            "tokenade-precompact" => "hookwrap-precompact",
            _ => "",
        };
        for (i, g) in array.iter().enumerate() {
            let (matched, has_tag) = group_matches_tokenade_hook(g, tag, subcmd);
            if !matched {
                continue;
            }
            let command_matches = g
                .get("hooks")
                .and_then(|h| h.as_array())
                .and_then(|arr| arr.first())
                .and_then(|h| h.get("command"))
                .and_then(|c| c.as_str())
                .map(|s| s == command)
                .unwrap_or(false);
            if command_matches && has_tag {
                already = true;
            } else {
                // Drift OR missing `_tag` — heal in place (#775).
                stale_idx = Some(i);
            }
            break;
        }
        if already {
            return (false, false);
        }
        let refresh = stale_idx.is_some();
        if let Some(idx) = stale_idx {
            array.remove(idx);
        }
        array.push(serde_json::json!({
            "hooks": [{
                "type": "command",
                "command": command,
                "_tag": tag,
            }]
        }));
        (refresh, true)
    };
    let (ups_refresh, ups_work) = upsert(
        ensure_array_entry(hooks_obj, "UserPromptSubmit"),
        "tokenade-userprompt",
        build_hook_command(exe, "hookwrap-userprompt", debug_log_path),
    );
    let (ss_refresh, ss_work) = upsert(
        ensure_array_entry(hooks_obj, "SessionStart"),
        "tokenade-sessionstart",
        build_hook_command(exe, "hookwrap-sessionstart", debug_log_path),
    );
    let (pc_refresh, pc_work) = upsert(
        ensure_array_entry(hooks_obj, "PreCompact"),
        "tokenade-precompact",
        build_hook_command(exe, "hookwrap-precompact", debug_log_path),
    );
    // Sidechain-exact window: Task pretool increments a per-session counter,
    // SubagentStop decrements it — in-context claims are suspended only
    // while a subagent is ACTUALLY running (the 15-min TTL stays as a
    // crash-safety net, no longer the primary mechanism).
    let (sas_refresh, sas_work) = upsert(
        ensure_array_entry(hooks_obj, "SubagentStop"),
        "tokenade-subagentstop",
        build_hook_command(exe, "hookwrap-subagentstop", debug_log_path),
    );
    // ── statusLine: live session-savings ticker (user decision 2026-07-04).
    // Idempotent + non-destructive: a genuine user statusLine is WRAPPED
    // (its text preserved, our ticker appended at render time), our own
    // wrapper is left as-is, and NOTHING is written when already in the
    // desired state — so the SessionStart self-heal can re-run every session
    // without ever clobbering the user's statusline or churning the file.
    // Opt-out (env TOKENADE_NO_STATUSLINE / TOKENADE_STATUSLINE=0, or the
    // `tokenade statusline off` sentinel) skips wiring entirely.
    let mut sl_work = false;
    if !statusline_opted_out(home) {
        // Hooks already installed on this machine before this run = an
        // install that predates the wiring record: the statusline was wired
        // here back then, so a non-tokenade one now is the user's doing.
        // Global settings only — a project file was never wired by default.
        let installed_before = global
            && home
                .map(|h| h.to_path_buf())
                .or_else(tokenade_core::platform::home_dir)
                .is_some_and(|h| h.join(".tokenade").join("claude-hooks-installed").exists());
        let settings_obj = settings.as_object_mut().unwrap();
        sl_work = wire_statusline_unless_removed(
            &path,
            settings_obj,
            exe,
            home,
            installed_before,
            dry_run,
        );
    }
    let any_work = ups_work || ss_work || pc_work || sas_work || sl_work;
    // Marker consumed by warn_if_hooks_vanished: proves Claude hooks were
    // wired on THIS machine (so a later disappearance is a real incident,
    // not another agent's install).
    if let Some(h) = (!dry_run).then(tokenade_core::platform::home_dir).flatten() {
        let m = h.join(".tokenade").join("claude-hooks-installed");
        tokenade_core::platform::ensure_parent_dir(&m);
        let _ = std::fs::write(&m, b"1");
    }
    let any_refresh = ups_refresh || ss_refresh || pc_refresh || sas_refresh;
    let any_fresh_install = any_work && !any_refresh;
    let outcome = |p: PathBuf| -> HookOutcome {
        if !any_work {
            HookOutcome::AlreadyPresent
        } else if any_fresh_install {
            HookOutcome::Installed(p)
        } else {
            HookOutcome::Refreshed(p)
        }
    };
    if dry_run {
        return outcome(path);
    }
    if let Err(e) = write_json_atomic(&path, &serde_json::to_string_pretty(&settings).unwrap()) {
        return HookOutcome::Failed(format!("write {}: {e}", path.display()));
    }
    outcome(path)
}

/// Persistent opt-out sentinel for the statusLine ticker, written by
/// `tokenade statusline off`. Honoured by BOTH the installer and the
/// SessionStart self-heal so an opted-out user is never re-wired.
fn statusline_off_sentinel(home: Option<&Path>) -> Option<PathBuf> {
    home.map(|h| h.to_path_buf())
        .or_else(tokenade_core::platform::home_dir)
        .map(|h| h.join(".tokenade").join("statusline-off"))
}

/// True when the live-ticker statusLine must NOT be wired into Claude
/// Code's `settings.json` — either via environment (`TOKENADE_NO_STATUSLINE`
/// set to anything, or `TOKENADE_STATUSLINE` = 0/false/off/no) or the
/// persistent opt-out sentinel. Checked by the installer and the self-heal.
pub(crate) fn statusline_opted_out(home: Option<&Path>) -> bool {
    if std::env::var("TOKENADE_NO_STATUSLINE").is_ok() {
        return true;
    }
    if matches!(
        std::env::var("TOKENADE_STATUSLINE").ok().as_deref(),
        Some("0") | Some("false") | Some("off") | Some("no")
    ) {
        return true;
    }
    statusline_off_sentinel(home)
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Is `cmd` a tokenade-managed statusLine command? Recognises our wrapper
/// even when the binary is spelled non-canonically — an auto-updater temp
/// name (`.tokenade.new.<pid>`), a renamed or symlinked install. The old
/// basename-only check (`base == "tokenade"`) MISSED those, so the FOREIGN
/// branch then RE-WRAPPED our own command, base64-burying the user's real
/// statusline behind a now-dead inner exe path and showing only the ticker
/// (the "tokenade keeps overwriting my statusline, I have to re-add it"
/// report). The reliable signal is the `statusline` subcommand sitting
/// right after a tokenade-ish binary.
pub(crate) fn is_tokenade_statusline(cmd: &str) -> bool {
    // The exe is everything before the ` statusline` word, quoted or not: an
    // older install wrote it unquoted, and a Windows path may hold spaces
    // (`C:\Program Files\…`), so it is not simply the first word.
    cmd.match_indices(" statusline").any(|(i, m)| {
        let after = &cmd[i + m.len()..];
        if !(after.is_empty() || after.starts_with(char::is_whitespace)) {
            return false;
        }
        let head = cmd[..i].trim();
        let exe = head.trim_matches(['"', '\'']);
        // One program, not `node ~/tokenade-bar.js statusline`: quoted, a
        // single word, or an unquoted Windows path (`C:\Program Files\…`).
        let one_program = exe.len() + 2 == head.len()
            || !exe.contains(char::is_whitespace)
            || exe.get(1..3) == Some(":\\");
        if !one_program {
            return false;
        }
        let base = exe.rsplit(['/', '\\']).next().unwrap_or(exe);
        // CONTAINS, not starts-with: a scratch copy named
        // `.autoresearch-tokenade-<pid>` was taken for a foreign statusline and
        // wrapped by the real binary — the line died with the copy.
        base.to_ascii_lowercase().contains("tokenade")
    })
}

/// The user's own command under a (possibly nested) tokenade wrapper, as the
/// ` --wrap-b64 <b64>` suffix to carry over — or "" when there is none. A
/// wrapped command that is itself a tokenade statusline is peeled, so a
/// stale wrap-of-ourselves collapses to the user's real command.
fn user_wrap_suffix(cmd: &str) -> String {
    use base64::Engine as _;
    let Some(i) = cmd.find("--wrap-b64") else {
        return String::new();
    };
    let inner = cmd[i + "--wrap-b64".len()..]
        .split_whitespace()
        .next()
        .and_then(|b| {
            let raw = base64::engine::general_purpose::STANDARD.decode(b).ok()?;
            String::from_utf8(raw).ok()
        });
    match inner {
        Some(inner) if is_tokenade_statusline(&inner) => user_wrap_suffix(&inner),
        _ => format!(" {}", &cmd[i..]),
    }
}

/// Wire the live-ticker statusLine into a Claude-Code `settings.json`
/// object. Returns true IFF the object was mutated, so the caller only
/// writes on a real change (idempotence — two consecutive self-heals write
/// once, not twice). Cases:
///   1. absent           → install `<exe> statusline`;
///   2. FOREIGN command  → wrap it (`--wrap-b64 <base64>`), preserving the
///                         user's command verbatim (rendered + ticker appended);
///   3. already ours & canonical (right exe) → NO-OP;
///   4. ours but stale exe (old/temp/dist path) → refresh to the stable exe,
///      PRESERVING any wrapped user command (`--wrap-b64` suffix carried over).
/// Sibling keys (e.g. `padding`) are preserved — only `command`/`type` change.
pub(crate) fn wire_statusline_into(
    settings_obj: &mut serde_json::Map<String, serde_json::Value>,
    exe: &str,
) -> bool {
    use base64::Engine as _;
    let current = settings_obj
        .get("statusLine")
        .and_then(|v| v.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let ours = is_tokenade_statusline(&current);
    // Quoted as the hooks are (#10): Claude Code runs the status line through
    // Git Bash on Windows, which eats the backslashes of an unquoted path
    // (`C:\Users\…\tokenade` → `C:Users…tokenade: command not found`); on
    // Unix a path with a space would split. An old unquoted entry no longer
    // matches the prefix, so it is refreshed (case 4).
    let canonical_prefix = format!("{} statusline", super::hook_util::wrap_exe_token(exe));
    // Case 3: already ours AND already pointing at the stable exe → nothing
    // to do. `starts_with(prefix)` also matches an existing `--wrap-b64`
    // wrap (the suffix follows the prefix), so a wrapped user command is a
    // no-op too — never re-wrapped, never lost. Unless what it wraps is
    // tokenade again: that layer is peeled (case 4).
    let wraps_ourselves = current
        .find("--wrap-b64")
        .is_some_and(|i| format!(" {}", &current[i..]) != user_wrap_suffix(&current));
    if ours && current.starts_with(&canonical_prefix) && !wraps_ourselves {
        return false;
    }
    let cmd = if ours {
        // Case 4: stale tokenade entry — refresh to the stable exe while
        // carrying over the user's wrapped original, if any.
        format!("{canonical_prefix}{}", user_wrap_suffix(&current))
    } else if current.trim().is_empty() {
        // Case 1: no prior statusLine.
        canonical_prefix
    } else {
        // Case 2: foreign statusLine — wrap, preserving the user's command.
        format!(
            "{canonical_prefix} --wrap-b64 {}",
            base64::engine::general_purpose::STANDARD.encode(current.trim())
        )
    };
    let mut obj = settings_obj
        .get("statusLine")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    obj.insert("type".into(), serde_json::json!("command"));
    obj.insert("command".into(), serde_json::json!(cmd));
    settings_obj.insert("statusLine".into(), serde_json::Value::Object(obj));
    true
}

/// `~/.tokenade/statusline-wired.json`: settings path → the command we last
/// wrote there. The VALUE, not a flag: it is what lets a later run tell "we
/// never wired this file" from "we did, and the user took it out".
fn statusline_wired_record(home: Option<&Path>) -> Option<PathBuf> {
    home.map(|h| h.to_path_buf())
        .or_else(tokenade_core::platform::home_dir)
        .map(|h| h.join(".tokenade").join("statusline-wired.json"))
}

fn read_statusline_wired(home: Option<&Path>) -> serde_json::Map<String, serde_json::Value> {
    statusline_wired_record(home)
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn write_statusline_wired(home: Option<&Path>, rec: &serde_json::Map<String, serde_json::Value>) {
    if let Some(p) = statusline_wired_record(home) {
        tokenade_core::platform::ensure_parent_dir(&p);
        let _ = crate::write_json_atomic(&p, &serde_json::Value::Object(rec.clone()).to_string());
    }
}

/// [`wire_statusline_into`], but never AGAINST the user. Every upgrade and
/// every SessionStart self-heal re-runs this, and the only persisted opt-out
/// was the `tokenade statusline off` sentinel: a user who removed the line
/// from `settings.json` by hand (or replaced it with their own) got it back
/// the next day. Reported by a user who had turned it off.
///
/// Rule: if the file holds a statusline that is not ours, and we wired this
/// file before (record, or `installed_before` for installs predating the
/// record), the user removed ours — leave it alone. `tokenade statusline on`
/// clears the record and re-wires.
pub(crate) fn wire_statusline_unless_removed(
    path: &Path,
    settings_obj: &mut serde_json::Map<String, serde_json::Value>,
    exe: &str,
    home: Option<&Path>,
    installed_before: bool,
    dry_run: bool,
) -> bool {
    let current = settings_obj
        .get("statusLine")
        .and_then(|v| v.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    if statusline_removed_by_user(path, current, home, installed_before) {
        return false;
    }
    let changed = wire_statusline_into(settings_obj, exe);
    if !dry_run {
        if let Some(now) = settings_obj["statusLine"]["command"].as_str() {
            record_statusline_wired(path, now, home);
        }
    }
    changed
}

/// The test of [`wire_statusline_unless_removed`], for a writer with its own
/// wiring rules (Qwen's, which also leaves a `preset` alone): the file holds
/// a statusline that is not ours, and we wired this file before.
pub(crate) fn statusline_removed_by_user(
    path: &Path,
    current_cmd: &str,
    home: Option<&Path>,
    installed_before: bool,
) -> bool {
    !is_tokenade_statusline(current_cmd)
        && (installed_before
            || read_statusline_wired(home).contains_key(path.to_string_lossy().as_ref()))
}

/// Note `cmd` as the statusline we wrote into `path`. Writes only on change.
pub(crate) fn record_statusline_wired(path: &Path, cmd: &str, home: Option<&Path>) {
    let key = path.to_string_lossy().into_owned();
    let mut rec = read_statusline_wired(home);
    if rec.get(&key).and_then(|v| v.as_str()) != Some(cmd) {
        rec.insert(key, serde_json::json!(cmd));
        write_statusline_wired(home, &rec);
    }
}

/// Remove a tokenade-managed statusLine, restoring the user's wrapped
/// original command if we had wrapped one (else drop the key). FOREIGN
/// statuslines are left untouched. Returns true IFF mutated. Used by
/// `tokenade statusline off`.
pub(crate) fn unwire_statusline(
    settings_obj: &mut serde_json::Map<String, serde_json::Value>,
) -> bool {
    use base64::Engine as _;
    let current = match settings_obj
        .get("statusLine")
        .and_then(|v| v.get("command"))
        .and_then(|c| c.as_str())
    {
        Some(c) => c.to_string(),
        None => return false,
    };
    if !is_tokenade_statusline(&current) {
        return false;
    }
    let restored = current
        .split_once("--wrap-b64 ")
        .map(|(_, b64)| b64.trim())
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .and_then(|raw| String::from_utf8(raw).ok())
        .filter(|s| !s.trim().is_empty());
    match restored {
        Some(orig) => {
            if let Some(sl) = settings_obj
                .get_mut("statusLine")
                .and_then(|v| v.as_object_mut())
            {
                sl.insert("command".into(), serde_json::json!(orig));
            }
        }
        None => {
            settings_obj.remove("statusLine");
        }
    }
    true
}

/// `tokenade statusline off|on` — persist the opt-out choice in the
/// sentinel AND (un)wire the ticker in the global Claude Code settings
/// immediately, so the change is visible on the next status render without
/// waiting for a re-install. Idempotent: writes settings.json only on a
/// real change.
pub(crate) fn set_statusline_enabled(home: Option<&Path>, enabled: bool) -> Result<String, String> {
    let sentinel =
        statusline_off_sentinel(home).ok_or_else(|| "no home directory resolved".to_string())?;
    if enabled {
        let _ = fs::remove_file(&sentinel);
        // An explicit `on` overrides every hand removal we were respecting.
        if let Some(p) = statusline_wired_record(home) {
            let _ = fs::remove_file(p);
        }
    } else {
        if let Some(p) = sentinel.parent() {
            let _ = fs::create_dir_all(p);
        }
        fs::write(&sentinel, b"1").map_err(|e| format!("write {}: {e}", sentinel.display()))?;
    }
    // Apply to the live global settings.json.
    if let Some(settings_path) = home
        .map(|h| h.to_path_buf())
        .or_else(tokenade_core::platform::home_dir)
        .map(|h| tokenade_core::platform::claude_config_dir_for(&h).join("settings.json"))
    {
        if let Ok(raw) = fs::read_to_string(&settings_path) {
            if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(obj) = v.as_object_mut() {
                    let changed = if enabled {
                        let exe = crate::install_cli::stable_hook_exe();
                        wire_statusline_into(obj, &exe)
                    } else {
                        unwire_statusline(obj)
                    };
                    if changed {
                        let _ = crate::write_json_atomic(
                            &settings_path,
                            &serde_json::to_string_pretty(&v).unwrap_or_default(),
                        );
                    }
                }
            }
        }
    }
    // Same switch for every OTHER host whose status line is a spawned command
    // (Copilot CLI, Cursor CLI — see `statusline_hosts.rs`). `off` used to
    // silence the ticker on Claude Code alone and leave it running everywhere
    // else, which reads as the switch being broken.
    let mut also: Vec<String> = Vec::new();
    if enabled {
        let exe = crate::install_cli::stable_hook_exe();
        for msg in crate::install::wire_statusline_hosts(home, &exe, false)
            .into_iter()
            .flatten()
        {
            also.push(msg);
        }
    } else {
        for p in crate::install::unwire_statusline_hosts(home, false) {
            also.push(format!("removed from {}", p.display()));
        }
    }
    let extra = if also.is_empty() {
        String::new()
    } else {
        format!("\n  {}", also.join("\n  "))
    };
    Ok(if enabled {
        format!("tokenade statusline ticker ENABLED{extra}")
    } else {
        format!(
            "tokenade statusline ticker DISABLED — run `tokenade statusline on` to re-enable{extra}"
        )
    })
}

/// Install a PostToolUse hook for `Edit|Write|MultiEdit` that fires
/// `tokenade index-touch` after every code modification. This is the
/// **codegraph-parity** mechanism: the agent never waits at search
/// time because the index is kept warm proactively.
///
/// Cost per Edit: one incremental rebuild — sub-100ms on projects up
/// to ~1000 files. The hook intentionally doesn't BLOCK the agent
/// long; the rebuild runs synchronously so Claude Code's spinner
/// reflects the time (visibility per user request), but typical
/// rebuilds are imperceptible.
///
/// Tagged with `_tag: tokenade-index` so uninstall is surgical and
/// re-running install is idempotent.
/// Resolve the settings.json path for hook install. When `global` is
/// true (the default for `tokenade install`), hooks go to
/// `~/.claude/settings.json` so they fire in EVERY Claude Code session
/// regardless of cwd — which is what the user expects from a "global"
/// install. Per-project install (`--per-project`) writes the hook to
/// `<cwd>/.claude/settings.json`, scoped to that one project.
/// The prologue every Claude-settings hook installer opens with: the opt-out
/// gate, the settings path for this scope, its parent directory, and the parsed
/// document.
///
/// `Err` is the outcome the caller must return unchanged — an opt-out, an
/// unresolvable home, a directory it could not create, or a settings file it
/// refuses to rewrite. Six installers carried these fifteen lines verbatim,
/// which is how a seventh forgets one of the four checks (#556: the opt-out
/// was missing from exactly one of them, and that installer ignored it).
pub(crate) fn open_claude_settings(
    root: &Path,
    global: bool,
    home: Option<&Path>,
    dry_run: bool,
) -> Result<(PathBuf, serde_json::Value), HookOutcome> {
    if is_hook_opted_out() {
        return Err(HookOutcome::OptedOut);
    }
    let Some(path) = hook_settings_path(root, global, home) else {
        return Err(HookOutcome::Failed(
            "no home dir resolved for global install".into(),
        ));
    };
    if !dry_run {
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return Err(HookOutcome::Failed(format!(
                    "mkdir {}: {e}",
                    parent.display()
                )));
            }
        }
    }
    let settings = match super::json_util::read_json_object_or_bail(&path) {
        Ok(v) => v,
        Err(e) => return Err(HookOutcome::Failed(e)),
    };
    Ok((path, settings))
}

pub(crate) fn hook_settings_path(
    root: &Path,
    global: bool,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if global {
        // `~/.claude` is the user's own filesystem: if they symlinked it into a
        // dotfiles repo, following the link is exactly what they want, and
        // `write_json_atomic` canonicalises so the manager keeps owning it.
        home.map(|h| tokenade_core::platform::claude_config_dir_for(h).join("settings.json"))
    } else {
        // Project scope is different: this path arrived with a `git clone`. A
        // hostile repo can ship `.claude` or `.claude/settings.json` as a
        // symlink to `~/.ssh/authorized_keys`, and installing hooks would write
        // JSON straight through it. Refuse the whole path rather than write.
        let p = root.join(".claude").join("settings.json");
        if let Some(link) = crate::first_symlink_below(root, &p) {
            eprintln!(
                "tokenade: refusing project-scope install — {} is a symlink inside the \
                 repository. Re-run with --global to install under your home directory.",
                link.display()
            );
            tokenade_core::debug_log::log(
                "install",
                "project_scope_symlink_refused",
                serde_json::json!({ "link": link.display().to_string() }),
            );
            return None;
        }
        Some(p)
    }
}
mod autocompact;
mod entries;
pub(crate) use autocompact::*;
pub(crate) use entries::*;

// ── Shell proxy ────────────────────────────────────────────────────────────
#[cfg(test)]
include!("claude_hooks_tests.rs");
