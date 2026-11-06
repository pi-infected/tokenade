// The tests of `claude_hooks`.
//
// Spliced in by `include!` from `claude_hooks.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed. This is the form this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a
// module, and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod symlink_scope_tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "tok-symlink-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// A checked-out repo shipping `.claude/settings.json` as a symlink to a
    /// sensitive file must not get project-scope hooks written through it.
    #[cfg(unix)]
    #[test]
    fn project_scope_refuses_a_symlinked_settings_file() {
        let root = tmp("file");
        let victim = root.join("victim-authorized_keys");
        fs::write(&victim, "ssh-ed25519 AAAA original\n").unwrap();
        fs::create_dir_all(root.join(".claude")).unwrap();
        std::os::unix::fs::symlink(&victim, root.join(".claude/settings.json")).unwrap();

        assert!(
            hook_settings_path(&root, false, None).is_none(),
            "a symlinked settings.json inside the repo must be refused"
        );
        assert_eq!(
            fs::read_to_string(&victim).unwrap(),
            "ssh-ed25519 AAAA original\n",
            "the link target must be untouched"
        );
    }

    /// The directory is the same attack one level up.
    #[cfg(unix)]
    #[test]
    fn project_scope_refuses_a_symlinked_claude_dir() {
        let root = tmp("dir");
        let elsewhere = root.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join(".claude")).unwrap();
        assert!(hook_settings_path(&root, false, None).is_none());
    }

    #[test]
    fn a_plain_project_path_is_still_allowed() {
        let root = tmp("plain");
        let p = hook_settings_path(&root, false, None).expect("plain path allowed");
        assert_eq!(p, root.join(".claude").join("settings.json"));
        // Nothing needs to exist yet — a fresh install is the normal case.
        assert!(!p.exists());
    }

    /// User scope keeps following links: `~/.claude` symlinked into a dotfiles
    /// repo is the user's own decision, and chezmoi/stow users depend on it.
    #[cfg(unix)]
    #[test]
    fn global_scope_still_follows_a_dotfiles_symlink() {
        let home = tmp("global");
        let dotfiles = home.join("dotfiles-claude");
        fs::create_dir_all(&dotfiles).unwrap();
        std::os::unix::fs::symlink(&dotfiles, home.join(".claude")).unwrap();
        assert_eq!(
            hook_settings_path(&home, true, Some(&home)),
            Some(home.join(".claude").join("settings.json")),
            "global scope must not inherit the repo-scope refusal"
        );
    }
}

#[cfg(test)]
mod autocompact_install_tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tok-ac-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read(p: &Path) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn sets_threshold_and_respects_user_value() {
        let dir = tmp("basic");
        let p = dir.join("settings.json");

        // 1M model → 700k, stamped + env mirrored.
        fs::write(&p, r#"{"model":"claude-opus-4-8"}"#).unwrap();
        assert_eq!(
            set_autocompact_window(&p, false, None).unwrap(),
            AutoCompact::Set(700_000, 70)
        );
        let v = read(&p);
        assert_eq!(v["autoCompactWindow"], 700_000);
        assert_eq!(v[AUTOCOMPACT_MARKER]["pct"], 70);
        assert_eq!(v[AUTOCOMPACT_MARKER]["window"], 700_000);
        assert_eq!(v["env"]["CLAUDE_CODE_AUTO_COMPACT_WINDOW"], "700000");

        // Re-run refreshes our own stamped value (idempotent, no skip).
        assert_eq!(
            set_autocompact_window(&p, false, None).unwrap(),
            AutoCompact::Set(700_000, 70)
        );

        // A hand-set value (no stamp) is respected — reported, not rewritten.
        fs::write(&p, r#"{"autoCompactWindow":123456}"#).unwrap();
        assert_eq!(
            set_autocompact_window(&p, false, None).unwrap(),
            AutoCompact::HandSet(123_456)
        );
        assert_eq!(read(&p)["autoCompactWindow"], 123_456);

        let _ = fs::remove_dir_all(&dir);
    }

    /// The heart of the request: re-running install with another percentage
    /// CHANGES the value, and the upgrade carries that choice forward.
    #[test]
    fn an_explicit_pct_changes_the_value_and_an_upgrade_keeps_it() {
        let dir = tmp("pct");
        let p = dir.join("settings.json");
        fs::write(&p, r#"{"model":"claude-opus-4-8"}"#).unwrap();

        assert_eq!(
            set_autocompact_window(&p, false, Some(70)).unwrap(),
            AutoCompact::Set(700_000, 70)
        );
        // Second pass with another threshold: it must take effect.
        assert_eq!(
            set_autocompact_window(&p, false, Some(85)).unwrap(),
            AutoCompact::Set(850_000, 85)
        );
        assert_eq!(read(&p)["autoCompactWindow"], 850_000);

        // `tokenade upgrade` imposes nothing (None): it carries 85, not 70.
        assert_eq!(
            set_autocompact_window(&p, false, None).unwrap(),
            AutoCompact::Set(850_000, 85)
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// The other half: a value hand-edited AFTER our write is not restored on
    /// upgrade. That is the complaint we are trying not to create — "I change
    /// the number and it comes back on its own".
    #[test]
    fn a_hand_edit_after_our_write_survives_an_upgrade() {
        let dir = tmp("edit");
        let p = dir.join("settings.json");
        fs::write(&p, r#"{"model":"claude-opus-4-8"}"#).unwrap();
        set_autocompact_window(&p, false, None).unwrap();

        // The user opens the file and puts 400000 instead.
        let mut v = read(&p);
        v["autoCompactWindow"] = serde_json::json!(400_000);
        fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();

        assert_eq!(
            set_autocompact_window(&p, false, None).unwrap(),
            AutoCompact::HandSet(400_000)
        );
        let after = read(&p);
        assert_eq!(after["autoCompactWindow"], 400_000);
        // Both spellings are aligned on THEIR number, otherwise the env
        // entry would keep 700000 and their edit would have no effect.
        assert_eq!(after["env"]["CLAUDE_CODE_AUTO_COMPACT_WINDOW"], "400000");
        // And we no longer claim to own it.
        assert!(after.get(AUTOCOMPACT_MARKER).is_none());

        // A second upgrade does not revert it either.
        assert_eq!(
            set_autocompact_window(&p, false, None).unwrap(),
            AutoCompact::HandSet(400_000)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Editing the ENV ENTRY alone counts just as much: it is the one Claude
    /// Code reads on a 1M window, and looking only at the key would overwrite
    /// it.
    #[test]
    fn a_hand_edit_of_the_env_entry_counts_too() {
        let dir = tmp("env");
        let p = dir.join("settings.json");
        fs::write(&p, r#"{"model":"claude-opus-4-8"}"#).unwrap();
        set_autocompact_window(&p, false, None).unwrap();

        let mut v = read(&p);
        v["env"]["CLAUDE_CODE_AUTO_COMPACT_WINDOW"] = serde_json::json!("300000");
        fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();

        assert_eq!(
            set_autocompact_window(&p, false, None).unwrap(),
            AutoCompact::HandSet(300_000)
        );
        assert_eq!(read(&p)["autoCompactWindow"], 300_000);
        let _ = fs::remove_dir_all(&dir);
    }

    /// And taking back control: `--autocompact-pct` is an explicit order, it
    /// wins over a hand-set value. Without it, a user who edited the file once
    /// could never again hand the threshold back to the tool.
    #[test]
    fn an_explicit_pct_takes_back_a_hand_set_value() {
        let dir = tmp("takeback");
        let p = dir.join("settings.json");
        fs::write(&p, r#"{"model":"claude-opus-4-8","autoCompactWindow":111111}"#).unwrap();
        assert_eq!(
            set_autocompact_window(&p, false, Some(50)).unwrap(),
            AutoCompact::Set(500_000, 50)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_legacy_bool_stamp_is_still_ours() {
        let dir = tmp("legacy");
        let p = dir.join("settings.json");
        fs::write(
            &p,
            r#"{"model":"claude-opus-4-8","autoCompactWindow":140000,"_tokenadeAutoCompact":true}"#,
        )
        .unwrap();
        // No trace of what we had written: nothing to detect, we refresh.
        assert_eq!(
            set_autocompact_window(&p, false, None).unwrap(),
            AutoCompact::Set(700_000, 70)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn out_of_range_pct_falls_back_rather_than_clamping() {
        // 200 is a typo. Compacting at 95 % while believing you asked for
        // 200 is worse than falling back to the default.
        assert_eq!(autocompact_pct(Some(200), None), 70);
        assert_eq!(autocompact_pct(Some(9), None), 70);
        assert_eq!(autocompact_pct(Some(10), None), 10);
        assert_eq!(autocompact_pct(Some(95), None), 95);
        // Without a flag, the stored value wins over the default.
        assert_eq!(autocompact_pct(None, Some(55)), 55);
        assert_eq!(autocompact_pct(Some(40), Some(55)), 40);
    }
}

#[cfg(test)]
mod statusline_wire_tests {
    use super::*;
    use base64::Engine as _;

    fn obj(json: &str) -> serde_json::Map<String, serde_json::Value> {
        serde_json::from_str::<serde_json::Value>(json)
            .unwrap()
            .as_object()
            .unwrap()
            .clone()
    }
    fn cmd(o: &serde_json::Map<String, serde_json::Value>) -> String {
        o["statusLine"]["command"].as_str().unwrap().to_string()
    }
    const EXE: &str = "/home/u/.tokenade/bin/tokenade";

    #[test]
    fn absent_installs_plain_ticker() {
        let mut o = obj("{}");
        assert!(wire_statusline_into(&mut o, EXE));
        assert_eq!(cmd(&o), format!("{EXE} statusline"));
        // idempotent second pass → no write.
        assert!(!wire_statusline_into(&mut o, EXE));
    }

    /// #10 follow-up: the host runs the status line through a shell (Git Bash
    /// on Windows), so the exe is quoted exactly as in the hooks — an
    /// unquoted path lost its backslashes there, or split at a space here.
    #[test]
    fn the_exe_is_quoted_for_the_hosts_shell_and_old_entries_heal() {
        use base64::Engine as _;
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let spaced = "/home/u v/.tokenade/bin/tokenade";
        let mut o = obj("{}");
        assert!(wire_statusline_into(&mut o, spaced));
        assert_eq!(cmd(&o), format!("'{spaced}' statusline"));
        assert!(is_tokenade_statusline(&cmd(&o)));
        assert!(!wire_statusline_into(&mut o, spaced), "quoted form is canonical");

        // A differently-quoted entry (hand-edited, other host) is refreshed, and
        // the user's wrapped command rides along.
        let old = format!("\"{spaced}\" statusline --wrap-b64 {}", b64("mybar.sh"));
        let mut o = obj(&serde_json::json!({ "statusLine": { "command": old } }).to_string());
        assert!(wire_statusline_into(&mut o, spaced));
        assert_eq!(cmd(&o), format!("'{spaced}' statusline --wrap-b64 {}", b64("mybar.sh")));

        // Windows spelling, as `win_hook_exe` writes it.
        assert!(is_tokenade_statusline(
            r#""C:\Program Files\nodejs\tokenade.exe" statusline --wrap-b64 eA=="#
        ));
        assert!(!is_tokenade_statusline(r#""C:\tools\tokenade.exe" --version"#));
        assert!(is_tokenade_statusline(
            r"C:\Program Files\nodejs\tokenade.exe statusline"
        ));
        assert!(!is_tokenade_statusline("node ~/tokenade-bar.js statusline"));
    }

    #[test]
    fn custom_user_statusline_is_wrapped_not_overwritten() {
        let mut o = obj(
            r#"{"statusLine":{"type":"command","command":"~/bin/mybar.sh --fancy","padding":1}}"#,
        );
        assert!(wire_statusline_into(&mut o, EXE));
        let c = cmd(&o);
        assert!(
            c.starts_with(&format!("{EXE} statusline --wrap-b64 ")),
            "got {c}"
        );
        // sibling keys preserved
        assert_eq!(o["statusLine"]["padding"], 1);
        // user's command recoverable, verbatim
        let b64 = c.rsplit(' ').next().unwrap();
        let decoded = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(decoded, "~/bin/mybar.sh --fancy");
        // CRITICAL: re-running the self-heal does NOT touch it → 0 writes.
        assert!(
            !wire_statusline_into(&mut o, EXE),
            "second wire must be a no-op"
        );
        assert_eq!(cmd(&o), c, "wrapped command must be byte-identical");
    }

    #[test]
    fn a_wrap_of_a_tokenade_copy_is_peeled_to_the_users_command() {
        // The operator's line since August: the real binary wrapping a
        // scratch COPY of itself (taken for foreign), which wrapped the
        // user's bar. Deleting the copy killed the statusline.
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let copy = format!(
            "/w/slm-scout/.autoresearch-tokenade-1854079 statusline --wrap-b64 {}",
            b64("mybar.sh")
        );
        assert!(is_tokenade_statusline(&copy));
        let mut o = obj(&format!(
            r#"{{"statusLine":{{"command":{}}}}}"#,
            serde_json::json!(format!("{EXE} statusline --wrap-b64 {}", b64(&copy)))
        ));
        assert!(wire_statusline_into(&mut o, EXE));
        assert_eq!(cmd(&o), format!("{EXE} statusline --wrap-b64 {}", b64("mybar.sh")));
        assert!(!wire_statusline_into(&mut o, EXE), "peeled form is canonical");
        // a bare copy is ours too: never wrapped as foreign.
        let mut o = obj(&format!(
            r#"{{"statusLine":{{"command":{}}}}}"#,
            serde_json::json!("/w/.autoresearch-tokenade-1854079 statusline")
        ));
        assert!(wire_statusline_into(&mut o, EXE));
        assert_eq!(cmd(&o), format!("{EXE} statusline"));
    }

    #[test]
    fn temp_exe_wrapper_is_recognised_never_double_wrapped() {
        // A wrapper baked by an updater temp binary (basename NOT "tokenade").
        // The old basename-only check treated this as FOREIGN and re-wrapped
        // it, burying the user's command. It must be recognised as ours and
        // refreshed to the stable exe, preserving the wrap.
        let inner = base64::engine::general_purpose::STANDARD.encode("mybar.sh");
        let stale =
            format!("/home/u/.tokenade/bin/.tokenade.new.9999 statusline --wrap-b64 {inner}");
        assert!(is_tokenade_statusline(&stale));
        let mut o = obj(&format!(
            r#"{{"statusLine":{{"command":{}}}}}"#,
            serde_json::json!(stale)
        ));
        assert!(wire_statusline_into(&mut o, EXE));
        let c = cmd(&o);
        assert_eq!(c, format!("{EXE} statusline --wrap-b64 {inner}"));
        // the user's original still decodes to their command, NOT to a nested
        // tokenade command.
        let b64 = c.rsplit(' ').next().unwrap();
        assert_eq!(
            String::from_utf8(
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .unwrap()
            )
            .unwrap(),
            "mybar.sh"
        );
        assert!(!wire_statusline_into(&mut o, EXE));
    }

    #[test]
    fn already_ours_canonical_is_noop() {
        let mut o = obj(&format!(
            r#"{{"statusLine":{{"command":"{EXE} statusline"}}}}"#
        ));
        assert!(!wire_statusline_into(&mut o, EXE));
    }

    #[test]
    fn unwire_restores_user_command_or_removes() {
        // wrapped → restore original
        let mut o = obj("{}");
        o.insert(
            "statusLine".into(),
            serde_json::json!({"command": "~/bin/mybar.sh"}),
        );
        wire_statusline_into(&mut o, EXE);
        assert!(unwire_statusline(&mut o));
        assert_eq!(o["statusLine"]["command"], "~/bin/mybar.sh");
        // plain ours → remove entirely
        let mut o2 = obj("{}");
        wire_statusline_into(&mut o2, EXE);
        assert!(unwire_statusline(&mut o2));
        assert!(o2.get("statusLine").is_none());
        // foreign → untouched
        let mut o3 = obj(r#"{"statusLine":{"command":"/usr/bin/other"}}"#);
        assert!(!unwire_statusline(&mut o3));
    }

    #[test]
    fn an_install_predating_the_record_respects_a_hand_removal() {
        let home = std::env::temp_dir().join(format!("tok-sl-pre-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        let sp = home.join("settings.json");
        // No record yet, but hooks were installed before: a missing or
        // foreign statusline is the user's doing.
        let mut o = obj(r#"{"statusLine":{"command":"mine.sh"}}"#);
        assert!(!wire_statusline_unless_removed(&sp, &mut o, EXE, Some(&home), true, false));
        assert_eq!(cmd(&o), "mine.sh");
        let mut o = obj("{}");
        assert!(!wire_statusline_unless_removed(&sp, &mut o, EXE, Some(&home), true, false));
        // Still ours → refreshed as before, and recorded.
        let mut o = obj(r#"{"statusLine":{"command":"/old/tokenade statusline"}}"#);
        assert!(wire_statusline_unless_removed(&sp, &mut o, EXE, Some(&home), true, false));
        assert_eq!(cmd(&o), format!("{EXE} statusline"));
        // `statusline on` forgets the record: a removal is no longer respected.
        let mut o = obj("{}");
        assert!(!wire_statusline_unless_removed(&sp, &mut o, EXE, Some(&home), false, false));
        fs::remove_file(home.join(".tokenade").join("statusline-wired.json")).unwrap();
        assert!(wire_statusline_unless_removed(&sp, &mut o, EXE, Some(&home), false, false));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn opt_out_sentinel_and_toggle() {
        let home = std::env::temp_dir().join(format!("tok-sl-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(home.join(".claude")).unwrap();
        // seed a settings.json with a tokenade-wrapped custom statusline
        let sp = home.join(".claude").join("settings.json");
        let mut o = obj(r#"{"statusLine":{"command":"~/bin/mybar.sh"}}"#);
        wire_statusline_into(&mut o, EXE);
        fs::write(
            &sp,
            serde_json::to_string_pretty(&serde_json::Value::Object(o)).unwrap(),
        )
        .unwrap();

        assert!(!statusline_opted_out(Some(&home)));
        // off → sentinel written, self-heal must skip, statusLine restored
        set_statusline_enabled(Some(&home), false).unwrap();
        assert!(statusline_opted_out(Some(&home)));
        assert!(home.join(".tokenade").join("statusline-off").exists());
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&sp).unwrap()).unwrap();
        assert_eq!(
            v["statusLine"]["command"], "~/bin/mybar.sh",
            "off must restore the user's command"
        );
        // on → sentinel cleared
        set_statusline_enabled(Some(&home), true).unwrap();
        assert!(!statusline_opted_out(Some(&home)));

        let _ = fs::remove_dir_all(&home);
    }
}
