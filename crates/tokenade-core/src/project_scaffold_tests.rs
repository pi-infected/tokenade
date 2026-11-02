// The tests of `project_scaffold`.
//
// Spliced by `include!` from `project_scaffold.rs`: the same module, so the same
// private access and the same `super::` paths, without a single visibility
// line changed. This is the form this repo already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod disk_gc_is_off_the_session_path {
    /// The sweep must be handed to a detached child, not run inline.
    ///
    /// Measured 2026-08-28 with `TOKENADE_PROFILE=1` on a real SessionStart
    /// event: `sessionstart:scaffold_ensure` **3 285 931 µs** against 38 056 µs
    /// for the whole hook once the stamp was fresh — once a day, in front of
    /// the agent's first turn. After the hand-off, 1 069 µs.
    ///
    /// Asserted on the SOURCE because the alternative is a race: the child may
    /// finish before a test can observe that the sweep had not run inline. What
    /// this pins is the decision, and the ORDER — the stamp before the spawn,
    /// which is what stops the child (running this same code) from chaining.
    #[test]
    fn the_sweep_is_handed_to_a_child_before_the_inline_list() {
        let src = &crate::source_guard::library_module_family("project_scaffold");
        let body = src
            .split_once("fn maybe_run_disk_gc")
            .expect("the function")
            .1;
        let body = &body[..body.find("\n}\n").expect("its end")];
        let stamp = body
            .find("write_atomic_repo_scoped")
            .expect("the stamp write");
        let spawn = body
            .find("spawn_fully_detached_in")
            .expect("the sweep must be handed to a detached child, not run inline");
        let inline = body
            .find("dedup_ref::evict_stale")
            .expect("the inline fallback");
        assert!(stamp < spawn, "the stamp must be written BEFORE the spawn");
        assert!(
            spawn < inline,
            "the hand-off must come before the inline sweep, or the session pays for it anyway"
        );
    }
}

#[cfg(test)]
mod search_capability_tests {
    use super::*;

    /// The scaffold must not name WHICH agents have hooks.
    ///
    /// Every agent reads this block, and it cannot know which one is reading.
    /// So a sentence like "if you are not Claude Code or Gemini CLI" is a
    /// roster frozen in prose: true when written, false by 2026-08-08 when nine
    /// agents carried hooks, and read by the seven it was wrong about as "wrap
    /// every command by hand" — advice to redo what their hook already does
    /// (a stale-roster defect, in the most-read surface the product ships).
    ///
    /// The fix is not a longer roster, it is no roster: point at
    /// `tokenade detect`, which answers for the agent actually running. This
    /// test refuses the shape, not the specific pair.
    #[test]
    fn the_scaffold_never_hardcodes_which_agents_have_hooks() {
        let block = scaffold_block(false);
        let hooked: Vec<&str> = crate::detect::Agent::all()
            .iter()
            .filter(|a| matches!(a.coverage(), crate::detect::Coverage::Hook(_)))
            .map(|a| a.as_str())
            .collect();
        assert!(hooked.len() > 2, "the hook roster shrank to {hooked:?}");

        let clause = block
            .split("## Commands")
            .nth(1)
            .expect("the Commands section")
            .split("\n##")
            .next()
            .unwrap();
        assert!(
            clause.contains("tokenade health"),
            "the hooks clause must send the reader to `tokenade health` instead of \
             asserting who has hooks: {clause}"
        );
        for name in ["Claude Code", "Gemini CLI", "Codex", "Cursor", "Copilot"] {
            assert!(
                !clause.contains(name),
                "the hooks clause names {name:?} — a roster in prose rots, and this one \
                 was wrong for seven of nine agents: {clause}"
            );
        }
    }

    /// The Gemini family gets a project rules file like every other agent.
    ///
    /// MEASURED 2026-08-08: the scaffold knew seven conventions — `CLAUDE.md`,
    /// `AGENTS.md`, `.cursorrules`, `.cursor/rules/`, `.clinerules`,
    /// `.roo/rules/`, `.kilocode/rules/`, `.github/copilot-instructions.md`,
    /// `.windsurfrules` — and not `GEMINI.md` / `QWEN.md`, while
    /// `claude_md_audit::rule_files_for` modelled BOTH cascades and `measure`
    /// read `GEMINI.md`. The product audited and measured a file it never
    /// wrote, so two rank-1 hook agents got the channel and none of the
    /// guidance.
    ///
    /// Asserted with the CONTROL that the branch is conditional: a home with
    /// neither marker must get neither file, or this "fix" would start writing
    /// two files into every project on earth.
    #[test]
    fn the_gemini_family_project_rules_files_are_scaffolded_when_installed() {
        let base = std::env::temp_dir().join(format!(
            "tokenade-gemini-scaffold-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("proj");
        std::fs::create_dir_all(&root).unwrap();

        // Neither installed: neither file. The control comes first.
        let bare = base.join("home-bare");
        std::fs::create_dir_all(&bare).unwrap();
        let got = with_home(&bare, || detect_installed_agents(&root));
        assert!(
            !got.iter().any(|p| {
                ["QWEN.md", ".grokrules", ".hermes.md", ".hermes_rules.md"]
                    .iter()
                    .any(|f| p.ends_with(f))
            }),
            "a home with no agent marker was still scaffolded: {got:?}"
        );

        for (marker, file) in [
            (".qwen", "QWEN.md"),
            (".grok", ".grokrules"),
            (".hermes", ".hermes.md"),
        ] {
            let home = base.join(format!("home{marker}"));
            std::fs::create_dir_all(home.join(marker)).unwrap();
            let got = with_home(&home, || detect_installed_agents(&root));
            assert!(
                got.iter().any(|p| p.ends_with(file)),
                "{marker} present but {file} not scaffolded: {got:?}"
            );
        }
        // Retired agents (Gemini CLI, Reasonix, Windsurf): their directory
        // may remain on the machine — `~/.gemini` is also Antigravity's —
        // and is no longer worth any project file.
        for (marker, file) in [
            (".gemini", "GEMINI.md"),
            (".reasonix", "REASONIX.md"),
            (".codeium", ".windsurfrules"),
        ] {
            let home = base.join(format!("home{marker}"));
            std::fs::create_dir_all(home.join(marker)).unwrap();
            let got = with_home(&home, || detect_installed_agents(&root));
            assert!(
                !got.iter().any(|p| p.ends_with(file)),
                "{marker} present and {file} still scaffolded: {got:?}"
            );
        }
    }

    /// **Cursor is only served through the addresses it READS.**
    ///
    /// Its package declares `.cursor/rules/**/*.mdc` — the `.mdc` extension,
    /// and only that. The `.cursor/rules/tokenade.md` this module had always
    /// written was never read: one more file in the repo, for nothing.
    /// `.cursorrules` stays, and `AGENTS.md` (written unconditionally) is the
    /// modern address — both are in the package's list.
    #[test]
    fn cursor_does_not_get_a_file_it_does_not_read() {
        let base = std::env::temp_dir().join(format!(
            "tokenade-cursor-rules-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let home = base.join("home");
        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        let got = with_home(&home, || detect_installed_agents(&root));
        assert!(
            got.iter().any(|p| p.ends_with(".cursorrules")),
            "Cursor installed and not even its historical address: {got:?}"
        );
        let never_read = root.join(".cursor").join("rules").join("tokenade.md");
        assert!(
            !got.contains(&never_read),
            "a file Cursor does not read is still written: {got:?}"
        );
    }

    /// **A backup copy has no business in a `git status`.**
    ///
    /// `paths::backup_once` writes `<file>.tokenade-original` next to the file
    /// it saves, `.mcp.json` included — so inside the repo. Reported by a
    /// tester: "it creates a lot of files, lots of changes, not ignored
    /// (.mcp.json.tokenade-original)". The copy stays (it is what makes the
    /// install reversible); what changes is that it no longer shows up to be
    /// committed. The GLOBAL excludes, not the project's `.gitignore`: that
    /// one is tracked, so writing to it would be one more change to commit.
    #[test]
    fn backup_copies_are_ignored_by_git() {
        let base = std::env::temp_dir().join(format!(
            "tokenade-gitignore-sidecar-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        assert!(
            with_home(&base, ensure_global_gitignored),
            "nothing was written"
        );
        let body =
            std::fs::read_to_string(base.join(".config").join("git").join("ignore")).unwrap();
        for pat in ["*.tokenade-original", "*.tokenade-backup-*"] {
            assert!(
                body.lines().any(|l| l.trim() == pat),
                "{pat} missing from the .gitignore:\n{body}"
            );
        }
        // Idempotent: a second pass rewrites nothing.
        assert!(
            !with_home(&base, ensure_global_gitignored),
            "the global excludes are rewritten on every session"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **The rules are written ONCE, outside the repo, per agent.**
    ///
    /// An agent with a global address receives the lesson there; the project
    /// file is only a pointer. Qwen has `~/.qwen/QWEN.md`, so the project's
    /// `QWEN.md` is a pointer too.
    #[test]
    fn rules_are_written_only_once() {
        let home = crate::platform::ScratchHome::new("rules_once_");
        for agent in [".claude", ".cursor", ".grok", ".qwen"] {
            std::fs::create_dir_all(home.path().join(agent)).unwrap();
        }
        let root = std::env::temp_dir().join(format!(
            "tokenade-rules-once-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        ensure(&root);
        // Every agent has its lesson outside the repo: no project file is created.
        for name in [
            "AGENTS.md",
            "CLAUDE.md",
            ".cursorrules",
            ".grokrules",
            "QWEN.md",
        ] {
            assert!(
                !root.join(name).exists(),
                "{name} was written into the project"
            );
        }
        let lecture = std::fs::read_to_string(home.path().join(".qwen").join("QWEN.md")).unwrap();
        assert!(
            lecture.contains("tokenade map"),
            "the Qwen lesson is not in the global:\n{lecture}"
        );
        let cursor = std::fs::read_to_string(
            home.path()
                .join(".cursor")
                .join("rules")
                .join("tokenade.mdc"),
        )
        .unwrap();
        assert!(
            cursor.contains("tokenade map") && cursor.contains("alwaysApply: true"),
            "Cursor did not receive the lesson as .mdc:\n{cursor}"
        );
        // Grok reads `~/.claude/CLAUDE.md` (compat): the lesson is there, no copy.
        let grok_path = home.path().join(".grok").join("rules").join("tokenade.md");
        assert!(!grok_path.exists(), "Grok would load the lecture twice");
        let claude =
            std::fs::read_to_string(home.path().join(".claude").join("CLAUDE.md")).unwrap();
        assert!(
            claude.contains("tokenade map"),
            "Claude global lost the lecture:\n{claude}"
        );
        assert!(
            !home.path().join(".claude").join("AGENTS.md").exists()
                || !std::fs::read_to_string(home.path().join(".claude").join("AGENTS.md"))
                    .unwrap_or_default()
                    .contains("tokenade map"),
            "~/.claude/AGENTS.md would double-load for Grok"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Run `f` with HOME pointed at `home`, restoring it afterwards.
    fn with_home<T>(home: &std::path::Path, f: impl FnOnce() -> T) -> T {
        let _g = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", home);
        let out = f();
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        out
    }

    /// A `CLAUDE.md` symlinked to `AGENTS.md` must keep the user's content.
    ///
    /// Reported by a user who keeps the two in sync with a link. Both names are
    /// in `PRIMARY_AGENT_FILES`, so one pass writes the SAME inode twice, and
    /// the two writes do not even carry the same block (`CLAUDE.md` omits the
    /// subagent note). This test states the contract that survives any
    /// resolution: whatever the pass does, the user's own prose is still there
    /// afterwards, and it is still there after a second pass.
    #[test]
    fn a_symlinked_rule_file_keeps_the_user_content() {
        let base = std::env::temp_dir().join(format!(
            "tokenade-symlink-rules-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("proj");
        let home = base.join("home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&home).unwrap();

        const MINE: &str = "# House rules\n\nAlways run `make check` before pushing.\n";
        std::fs::write(root.join("AGENTS.md"), MINE).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("AGENTS.md", root.join("CLAUDE.md")).unwrap();
        #[cfg(not(unix))]
        return;

        for pass in 1..=2 {
            let r = with_home(&home, || ensure(&root));
            // Pass 2 must SKIP. The symlink write was refused (correctly) and
            // the refusal counted as a failure, so the flag was never stamped
            // and this entire pass re-ran on every session, forever.
            if pass == 2 {
                assert!(r.skipped, "pass 2 did not converge: {:?}", r.files_written);
            }
            let agents = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
            assert!(
                agents.contains("Always run `make check` before pushing."),
                "pass {pass}: the user's line is gone from AGENTS.md:\n{agents}"
            );
            assert_eq!(
                agents.matches(SCAFFOLD_MARKER).count(),
                1,
                "pass {pass}: the block was written more than once into one file:\n{agents}"
            );
            assert!(
                std::fs::symlink_metadata(root.join("CLAUDE.md"))
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "pass {pass}: the user's symlink was replaced by a regular file"
            );
            // One file under two names carries the note the non-Claude half
            // needs; picking the variant off the name wrote whichever came
            // last into a file both agents read.
            assert!(
                agents.contains("Applies to the subagents you spawn.")
                    || agents.contains("subagent"),
                "pass {pass}: the shared file lost the subagent note:\n{agents}"
            );
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A rule file linked OUTSIDE the repository is refused, reported, and
    /// does not re-run forever.
    ///
    /// The refusal is the point of the repo-symlink guard: a cloned repo must
    /// not be able to pick a file on your machine for us to write ~4 KB into.
    /// What was wrong is what it cost — the refusal was counted as a write
    /// FAILURE, so the scaffold flag stayed unstamped and the whole pass re-ran
    /// on every session, and nothing ever told the user their linked rule file
    /// had no rules in it.
    #[test]
    #[cfg(unix)]
    fn a_rule_file_linked_out_of_the_repo_is_refused_and_said_so() {
        let base = std::env::temp_dir().join(format!(
            "tokenade-escaping-link-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("proj");
        let home = base.join("home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let outside = base.join("elsewhere.md");
        const MINE: &str = "# Not ours\n\nkeep me\n";
        std::fs::write(&outside, MINE).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("CLAUDE.md")).unwrap();

        let r1 = with_home(&home, || ensure(&root));
        assert!(
            r1.files_written
                .iter()
                .any(|f| f.contains("CLAUDE.md") && f.contains("outside the project")),
            "the refusal was silent: {:?}",
            r1.files_written
        );
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            MINE,
            "we wrote through a link that leaves the repository"
        );
        let r2 = with_home(&home, || ensure(&root));
        assert!(r2.skipped, "the refused link re-ran the pass: {r2:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Telling a hooked agent to run `tokenade search` makes it call a command
    /// for something its own tool already routes through us — a redundant
    /// round-trip we would be asking for in writing.
    #[test]
    fn hooked_agents_are_not_told_to_call_the_command() {
        assert_eq!(search_capability_line(true), "");
    }

    /// And NOT telling a hookless agent means it never searches with us at all,
    /// which was the state of the world: hooked agents got a five-engine union,
    /// Codex/Cursor/Grok/Droid/Aider got nothing.
    #[test]
    fn hookless_agents_get_the_command() {
        let s = search_capability_line(false);
        assert!(s.contains("tokenade search"), "{s}");
        assert!(s.contains("--json"), "{s}");
        assert!(
            s.contains("tokenade web <url>"),
            "must chain to reading: {s}"
        );
    }

    #[test]
    fn the_predicate_splits_the_fleet_the_way_the_hooks_do() {
        use crate::detect::Agent;
        assert!(Agent::ClaudeCode.hooks_websearch());
        for a in [
            Agent::Codex,
            Agent::Cursor,
            Agent::Grok,
            Agent::Droid,
            Agent::Aider,
        ] {
            assert!(!a.hooks_websearch(), "{a:?} has no websearch hook");
        }
    }
}

#[cfg(test)]
mod read_capability_tests {
    use super::*;

    /// The two phrasings must not be interchangeable. On a hooked agent the
    /// capability is invisible (Read just works); on a shell-only agent it must
    /// be invoked. Telling the second kind that its Read handles PDFs sends it
    /// looping on binary garbage.
    #[test]
    fn hooked_and_unhooked_agents_get_different_truths() {
        let hooked = read_capability_line(true);
        let plain = read_capability_line(false);

        assert!(hooked.contains("extends your `Read` tool"), "{hooked}");
        assert!(!hooked.contains("your own file reader\ncannot"));

        assert!(plain.contains("tokenade read <file>"), "{plain}");
        assert!(
            !plain.contains("extends your `Read` tool"),
            "a shell-only agent must NOT be told its Read changed: {plain}"
        );

        // Both must name the formats, and both must push the cheapest option.
        for block in [&hooked, &plain] {
            assert!(block.contains(".pdf"), "formats must be listed");
            assert!(block.contains(".mp4"), "media must be listed");
            assert!(
                block.contains("CHEAPEST"),
                "multi-prompt must be framed as cheapest"
            );
        }
    }

    /// The advertised lists must match what we actually handle, or the agent
    /// loops on a format we claimed and cannot process. Covers documents,
    /// images and media in one place so no list can drift alone.
    #[test]
    fn advertised_formats_are_all_really_supported() {
        for ext in DOC_EXTENSIONS.split_whitespace() {
            // The list ends in a prose tail covering the alias families.
            if !ext.starts_with('.') {
                continue;
            }
            let name = format!("x{ext}");
            assert!(
                crate::docs::Format::from_filename(&name).is_some(),
                "scaffold advertises {ext} but docs::Format does not handle it"
            );
        }
        for ext in IMAGE_EXTENSIONS.split_whitespace() {
            assert!(
                crate::image_downscale::is_downscalable_ext(ext.trim_start_matches('.')),
                "scaffold advertises image {ext} but image_downscale cannot decode it"
            );
        }
        for ext in MEDIA_EXTENSIONS.split_whitespace() {
            // The media list ends in a prose tail ("(and other common …)").
            if !ext.starts_with('.') {
                continue;
            }
            let name = format!("x{ext}");
            assert!(
                crate::media::detect_media(b"", &name).is_some(),
                "scaffold advertises media {ext} but media::detect_media does not know it"
            );
        }
    }

    /// Rewriting the rules is only half the job: a project that already carries
    /// the block keeps its old copy until `SCAFFOLD_VERSION` moves. Correct text
    /// that never reaches an existing user is the same as no correction — and
    /// this has already happened, twice in one release, to a line that told
    /// agents the OPPOSITE of the truth.
    ///
    /// So: change the text, and this fails until you bump the version too. The
    /// two environment-dependent pieces are folded in explicitly, so the
    /// fingerprint is the same on Windows, on a hooked agent and on a bare one.
    #[test]
    fn changing_the_rules_text_forces_a_version_bump() {
        // `TOKENADE_NO_WEBSEARCH` changes the rendered text; another test sets
        // it under this lock.
        let _env = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // FNV-1a — a std hasher's output is not promised to be stable across
        // releases, and a gate that moves on its own teaches people to ignore it.
        fn fnv1a(s: &str) -> u64 {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for b in s.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
            h
        }

        // REMOVE the pieces the environment chooses — blanking them to a marker
        // would not equalize a section that is present on one agent and absent
        // on another. What remains is identical everywhere; the variants are
        // added back below, so none of their text escapes the fingerprint.
        let variants = [
            read_capability_line(true),
            read_capability_line(false),
            search_capability_line(true),
            search_capability_line(false),
            WINDOWS_SHELL_NOTE.to_string(),
        ];
        let normalized = |mut block: String| {
            for variable in variants.iter().filter(|v| !v.is_empty()) {
                block = block.replace(variable.as_str(), "");
            }
            block
        };
        // Both callers, then every variant added back unconditionally: full
        // coverage of the text, and no dependence on which agent is running.
        let mut canonical = normalized(scaffold_block(true));
        canonical.push('\u{3}');
        canonical.push_str(&normalized(scaffold_block(false)));
        for variable in &variants {
            canonical.push('\u{2}');
            canonical.push_str(variable);
        }

        // 2026-08-04: "runs in a sandbox" → "runs a capped subprocess with a
        // scrubbed env — your permissions, not a jail". SCAFFOLD_VERSION 40→41
        // alongside, which is the half this guard exists to force.
        // 2026-08-08: the hooks clause stopped naming which agents have
        // hooks and now points at `tokenade detect`. SCAFFOLD_VERSION 42→43.
        // This guard is what made the bump non-optional — it fired on the same
        // run that shipped the text change.
        // 2026-08-08: `exec`'s language list derived from `sandbox::LANGS`
        // (it had lost `bash`). SCAFFOLD_VERSION 43→44.
        // 2026-08-29: `map` gained its path narrowing, so the
        // exploration line names it. SCAFFOLD_VERSION 46→47.
        // 2026-09-03: the batching advice names the round-trip that
        // makes it cheaper — measured at +0.5% on output tokens alone, so a
        // bare "CHEAPEST in tokens spent" promised what it does not deliver.
        // SCAFFOLD_VERSION 47→48.
        // 2026-10-04: `.doc`, `.ppt` (Office 97–2003) and
        // `.msg` (Outlook) are read.
        // SCAFFOLD_VERSION 57→58, unreleased in between.
        const RULES_TEXT_FINGERPRINT: u64 = 7840019940643024240;
        assert_eq!(
            fnv1a(&canonical),
            RULES_TEXT_FINGERPRINT,
            "the scaffolded rules text changed. Bump SCAFFOLD_VERSION (currently \
             {SCAFFOLD_VERSION}) so projects that already have the block receive \
             the new one, then set RULES_TEXT_FINGERPRINT to the value above."
        );
    }

    /// The mirror of [`no_supported_format_stays_invisible`], and the direction
    /// that costs the AGENT rather than us: an extension the rules advertise
    /// but nothing routes. The agent Reads the file, it fails, and the turn is
    /// spent — plus the rules are now a thing it has learned not to trust.
    ///
    /// Verified by hand 2026-08-04 (0 gaps in either list, and a real `.mp4`
    /// read end to end); this pins it so a handler cannot be dropped while its
    /// advertisement stays.
    #[test]
    fn every_advertised_format_is_actually_routed() {
        let routed_docs: std::collections::HashSet<&str> = crate::docs::KNOWN_EXTENSIONS
            .iter()
            .map(|(e, _)| *e)
            .collect();
        for ext in DOC_EXTENSIONS
            .split_whitespace()
            .filter_map(|t| t.strip_prefix('.'))
        {
            assert!(
                routed_docs.contains(ext),
                "the rules advertise .{ext} but `docs::KNOWN_EXTENSIONS` does not \
                 route it — an agent that Reads one spends a turn to find out"
            );
        }
        let routed_imgs: std::collections::HashSet<&str> =
            crate::image_downscale::DOWNSCALABLE_EXTENSIONS
                .iter()
                .copied()
                .collect();
        for ext in IMAGE_EXTENSIONS
            .split_whitespace()
            .filter_map(|t| t.strip_prefix('.'))
        {
            assert!(
                routed_imgs.contains(ext),
                "the rules advertise .{ext} but `image_downscale::DOWNSCALABLE_EXTENSIONS` \
                 does not decode it"
            );
        }
        // Both lists are parsed out of prose, so a formatting change could
        // silently reduce them to nothing and leave this test asserting air.
        assert!(
            DOC_EXTENSIONS
                .split_whitespace()
                .filter(|t| t.starts_with('.'))
                .count()
                >= 10
                && IMAGE_EXTENSIONS
                    .split_whitespace()
                    .filter(|t| t.starts_with('.'))
                    .count()
                    >= 10,
            "the extension lists stopped parsing — this test is asserting nothing"
        );
    }

    /// The other direction, and the one that actually rots: a format we ship
    /// but never mention is a format nobody tries. Every extension we route
    /// must be named in the rules, or listed here with a reason — so staying
    /// silent about one is a decision someone made, not an oversight.
    #[test]
    fn no_supported_format_stays_invisible() {
        // Named in the prose tail rather than enumerated: alias families that
        // behave exactly like the extension already listed, so spelling each
        // one out would cost every session tokens and teach nothing.
        let covered_by_prose = [
            "docm", "dotx", "dotm", "xlsm", "xltx", "xltm", "pptm", "potx", "potm", "ppsx", "ppsm",
            "ott", "ots", "otp", "otg", "fodt", "fods", "fodp", "fodg", "dot", "pps", "pot",
        ];
        // Words, not substrings: `.docx` CONTAINS `.doc`, so a substring test
        // passed for `.doc` while the rules never named it.
        let advertised: std::collections::HashSet<&str> =
            DOC_EXTENSIONS.split_whitespace().collect();
        for (ext, _) in crate::docs::KNOWN_EXTENSIONS {
            if covered_by_prose.contains(ext) {
                continue;
            }
            assert!(
                advertised.contains(format!(".{ext}").as_str()),
                "we read .{ext} but the rules never say so — advertise it or \
                 add it to the prose-covered list with a reason"
            );
        }
        // `ff` (farbfeld) decodes for free with the image crate but is vanishing
        // rare; naming it would cost every session tokens for a format almost
        // nobody has.
        for ext in crate::image_downscale::DOWNSCALABLE_EXTENSIONS {
            if *ext == "ff" {
                continue;
            }
            assert!(
                IMAGE_EXTENSIONS.contains(&format!(".{ext}")),
                "we decode .{ext} but the rules never say so"
            );
        }
    }
}

#[cfg(test)]
mod tests {

    /// The scaffold block sits in a CACHED position: `CLAUDE.md` is part of the
    /// prefix a provider caches once per session, and the same rules text goes
    /// into the subagent preamble on EVERY Task call. A single volatile token
    /// in there — a timestamp, a session id, a uuid — changes the prefix between
    /// calls and silently turns every cache hit into a full re-encode. Nothing
    /// else would catch it: no test fails, no output changes, only the bill
    /// moves, and it moves for every user at once.
    ///
    /// `SCAFFOLD_VERSION` bumps are fine and deliberately not guarded against:
    /// they change the block once per release, not between calls.
    #[test]
    fn scaffold_block_is_cache_stable() {
        for (name, text) in [
            ("scaffold_block(false)", scaffold_block(false)),
            ("scaffold_block(true)", scaffold_block(true)),
            ("scaffold_block_plain", scaffold_block_plain()),
            ("OUTPUT_BREVITY_RULES", OUTPUT_BREVITY_RULES.to_string()),
        ] {
            let hits = crate::cache_align::detect_dynamic(&text);
            assert!(
                hits.is_empty(),
                "{name} carries {} runtime-volatile token(s) {:?} — that busts the \
                 cached prefix for every user on every session. Move it out of the \
                 block, or into a hook-injected line, which is not cached.",
                hits.len(),
                hits
            );
        }
    }
    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tokenade-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// One marker pair exactly — the anti-double-scaffold assertion.
    fn assert_single_block(path: &std::path::Path) {
        let text = std::fs::read_to_string(path).unwrap();
        assert_eq!(
            text.matches(super::SCAFFOLD_MARKER).count(),
            1,
            "exactly one open marker in {}:\n{text}",
            path.display()
        );
        assert_eq!(
            text.matches(super::SCAFFOLD_END_MARKER).count(),
            1,
            "exactly one close marker in {}",
            path.display()
        );
    }

    #[test]
    fn repeated_gate_flapping_never_duplicates_or_leaks_blocks() {
        let _home = crate::platform::ScratchHome::new("gate_flapping_");
        // Gate flip-flop (monthly reset → re-exceeded → upgrade → …) across
        // several cycles: the block count must stay 0 (gated) / 1 (restored),
        // never 2, and user content must survive every cycle byte-for-byte.
        let dir = tmpdir("flap");
        let user = "# Team\n\nHouse rules.\n";
        std::fs::write(
            dir.join("CLAUDE.md"),
            format!("{user}\n{}", super::scaffold_block(false)),
        )
        .unwrap();
        for cycle in 0..3 {
            assert_eq!(
                super::sync_gate_state(&dir, true),
                Some("retracted"),
                "cycle {cycle}"
            );
            let text = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
            assert_eq!(
                text.matches(super::SCAFFOLD_MARKER).count(),
                0,
                "cycle {cycle}"
            );
            assert!(text.contains("House rules."), "cycle {cycle}");
            // Double-retract (two gated channels racing the same project after
            // the marker exists) is a no-op, not a second splice.
            assert_eq!(super::sync_gate_state(&dir, true), None, "cycle {cycle}");
            assert_eq!(
                super::sync_gate_state(&dir, false),
                Some("restored"),
                "cycle {cycle}"
            );
            assert_single_block(&dir.join("CLAUDE.md"));
            assert!(std::fs::read_to_string(dir.join("CLAUDE.md"))
                .unwrap()
                .contains("House rules."));
            assert_eq!(super::sync_gate_state(&dir, false), None, "cycle {cycle}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_over_manually_rescaffolded_file_does_not_duplicate() {
        let _home = crate::platform::ScratchHome::new("restore_manual_");
        // While gated, the user re-adds the block themselves (copy-paste from
        // docs, or a colleague's un-gated machine scaffolds the shared repo).
        // The restore pass must recognise the existing block (write path is
        // marker-replacing) and end with ONE block, not two.
        let dir = tmpdir("manual");
        std::fs::write(
            dir.join("CLAUDE.md"),
            format!("# Team\n\n{}", super::scaffold_block(false)),
        )
        .unwrap();
        assert_eq!(super::sync_gate_state(&dir, true), Some("retracted"));
        // Manual/other-machine re-add while gated.
        let text = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
        std::fs::write(
            dir.join("CLAUDE.md"),
            format!("{text}\n{}", super::scaffold_block(false)),
        )
        .unwrap();
        assert_eq!(super::sync_gate_state(&dir, false), Some("restored"));
        assert_single_block(&dir.join("CLAUDE.md"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retract_ignores_mangled_marker_order_and_partial_blocks() {
        let _home = crate::platform::ScratchHome::new("retract_ignores_mang_");
        // Git-merge damage: close marker BEFORE open marker — the write path
        // refuses these files; retract must refuse identically (no partial
        // removal, no content loss), and the sync must not record state for
        // a file it didn't clean.
        let dir = tmpdir("mangled");
        let mangled = format!(
            "# Team\n\n{}\nsome stranded text\n{}\ntail\n",
            super::SCAFFOLD_END_MARKER,
            super::SCAFFOLD_MARKER
        );
        std::fs::write(dir.join("CLAUDE.md"), &mangled).unwrap();
        // Open marker with NO close marker (truncated file): also refused.
        let truncated = format!(
            "# Other\n\n{}\n## Explore code with…\n",
            super::SCAFFOLD_MARKER
        );
        std::fs::write(dir.join("AGENTS.md"), &truncated).unwrap();
        assert_eq!(
            super::sync_gate_state(&dir, true),
            None,
            "nothing cleanly retractable → no state recorded"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap(),
            mangled
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("AGENTS.md")).unwrap(),
            truncated
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retract_then_restore_upgrades_stale_scaffold_versions() {
        // A project scaffolded by an OLD tokenade: same markers, different
        // block body. Retract must remove it (markers are version-stable),
        // and restore must write the CURRENT block — the retract/restore
        // cycle doubles as the in-place upgrade.
        let _home = crate::platform::ScratchHome::new("retract_restore_oldver_");
        let dir = tmpdir("oldver");
        let old_block = format!(
            "{}\n## Codebase exploration\nUse the tokenade MCP tools for all reads.\n\n\
             ## Engineering principles\nOld v2 prose that current versions no longer write.\n{}\n",
            super::SCAFFOLD_MARKER,
            super::SCAFFOLD_END_MARKER
        );
        std::fs::write(dir.join("AGENTS.md"), format!("# Team\n\n{old_block}")).unwrap();
        assert_eq!(super::sync_gate_state(&dir, true), Some("retracted"));
        let text = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
        assert!(
            !text.contains("Codebase exploration"),
            "old body fully removed"
        );
        assert_eq!(text.matches(super::SCAFFOLD_MARKER).count(), 0);
        assert_eq!(super::sync_gate_state(&dir, false), Some("restored"));
        let text = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
        assert_single_block(&dir.join("AGENTS.md"));
        assert!(
            text.contains("## Explore code with the `tokenade` CLI"),
            "current body"
        );
        assert!(
            !text.contains("Engineering principles"),
            "no stale resurrection"
        );
        assert!(text.contains("# Team"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_after_manual_marker_deletion_while_retracted_stays_paused() {
        let _home = crate::platform::ScratchHome::new("ensure_after_manual_");
        // User deletes .tokenade/ (or the flag) while retracted → marker gone
        // → ensure() would normally re-scaffold. The gate check still holds
        // the line when the machine is gated; this test covers the marker
        // path only (gate env not simulatable here): with marker present,
        // ensure is a no-op even with a stale/missing version flag.
        let dir = tmpdir("flagless");
        std::fs::write(
            dir.join("CLAUDE.md"),
            format!("x\n{}", super::scaffold_block(false)),
        )
        .unwrap();
        assert_eq!(super::sync_gate_state(&dir, true), Some("retracted"));
        let _ = std::fs::remove_file(dir.join(".tokenade").join("scaffold.flag"));
        assert!(
            super::ensure(&dir).skipped,
            "retracted marker dominates ensure"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("CLAUDE.md"))
                .unwrap()
                .matches(super::SCAFFOLD_MARKER)
                .count(),
            0
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retract_and_restore_roundtrip_is_surgical() {
        let _home = crate::platform::ScratchHome::new("retract_and_restore_");
        let dir = std::env::temp_dir().join(format!("tokenade-retract-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let user_content = "# My project\n\nTeam rules the user wrote.\n";
        std::fs::write(
            dir.join("CLAUDE.md"),
            format!("{user_content}\n{}", super::scaffold_block(false)),
        )
        .unwrap();
        std::fs::write(dir.join("AGENTS.md"), "no tokenade block here\n").unwrap();

        // Gated: block removed from CLAUDE.md only; user content intact.
        assert_eq!(super::sync_gate_state(&dir, true), Some("retracted"));
        let claude = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
        assert!(!claude.contains(super::SCAFFOLD_MARKER));
        assert!(claude.contains("Team rules the user wrote."));
        assert_eq!(
            std::fs::read_to_string(dir.join("AGENTS.md")).unwrap(),
            "no tokenade block here\n",
            "files without our block are untouched"
        );
        // Steady-state while gated: one stat, no change.
        assert_eq!(super::sync_gate_state(&dir, true), None);
        // ensure() cannot re-add while retracted (the update-must-not-reactivate guard).
        assert!(super::ensure(&dir).skipped);

        // Un-gated: ONLY the retracted file gets its block back.
        assert_eq!(super::sync_gate_state(&dir, false), Some("restored"));
        let claude = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
        assert!(claude.contains(super::SCAFFOLD_MARKER));
        assert!(claude.contains("Team rules the user wrote."));
        assert!(
            !std::fs::read_to_string(dir.join("AGENTS.md"))
                .unwrap()
                .contains(super::SCAFFOLD_MARKER),
            "restore is surgical — never seeds new files"
        );
        assert_eq!(super::sync_gate_state(&dir, false), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::*;
    use std::sync::Mutex;

    /// Tests that mutate `FASTEMBED_CACHE` (process-wide env var) must
    /// serialise — otherwise a parallel test's `set_var` clobbers the
    /// override mid-run and the migrator reads the wrong path.
    fn fastembed_env_lock() -> &'static Mutex<()> {
        static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn tmp_root(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-scaffold-{}-{}",
            label,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A project already flagged at the previous scaffold version, with the
    /// **full** rules block in every co-loaded file, is spliced in place by
    /// the same `ensure` SessionStart already calls. Existing clients do not
    /// re-scaffold by hand. The lecture moves to each agent's global file.
    #[test]
    fn full_block_co_loaded_files_drop_the_block_on_version_bump() {
        let home = crate::platform::ScratchHome::new("upgrade_v50_globals_");
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        std::fs::create_dir_all(home.path().join(".grok")).unwrap();
        let root = tmp_root("upgrade-v50-globals");
        std::fs::create_dir_all(root.join(".tokenade")).unwrap();
        let full = scaffold_block(true);
        assert!(
            full.contains("tokenade map"),
            "fixture must start from the full rules block"
        );
        let wrapped = |name: &str| format!("# House rules\n\n{full}\nkeep me\n/* {name} */\n");
        for name in ["AGENTS.md", "CLAUDE.md", ".grokrules", ".cursorrules"] {
            std::fs::write(root.join(name), wrapped(name)).unwrap();
        }
        std::fs::write(
            root.join(".tokenade").join("scaffold.flag"),
            format!(
                "version: {}\nstyle: {}\nscaffolded: AGENTS.md, CLAUDE.md\n",
                SCAFFOLD_VERSION - 1,
                crate::style::load(&root).as_str()
            ),
        )
        .unwrap();

        let first = ensure(&root);
        assert!(
            !first.skipped,
            "version behind current must re-enter ensure: {first:?}"
        );

        const CLAUSE: &str = "this machine's agent config";
        for name in ["AGENTS.md", "CLAUDE.md", ".grokrules", ".cursorrules"] {
            let body = std::fs::read_to_string(root.join(name)).unwrap();
            assert!(
                !body.contains("tokenade map"),
                "{name} still holds the full block after upgrade:\n{body}"
            );
            assert!(
                !body.contains(SCAFFOLD_MARKER) && !body.contains(CLAUSE),
                "{name} still carries a Tokenade block after upgrade:\n{body}"
            );
            assert!(
                body.contains("# House rules") && body.contains("keep me"),
                "{name} lost user prose:\n{body}"
            );
        }
        assert!(
            !home
                .path()
                .join(".grok")
                .join("rules")
                .join("tokenade.md")
                .exists(),
            "Grok reads the Claude global; a native copy doubles it"
        );
        let cursor = std::fs::read_to_string(
            home.path()
                .join(".cursor")
                .join("rules")
                .join("tokenade.mdc"),
        )
        .unwrap();
        assert!(
            cursor.contains("tokenade map") && cursor.contains("alwaysApply: true"),
            "Cursor global lost the lecture:\n{cursor}"
        );
        let flag = std::fs::read_to_string(root.join(".tokenade").join("scaffold.flag")).unwrap();
        assert!(
            flag.contains(&format!("version: {SCAFFOLD_VERSION}")),
            "flag did not record current version: {flag}"
        );
        let second = ensure(&root);
        assert!(second.skipped, "second ensure must be a no-op: {second:?}");
    }

    /// Moving from 1.1.18 to the next version: every agent with a global
    /// address receives ONE copy in its own place; the project and the
    /// Grok-compat leftovers no longer carry `tokenade map`. Copilot / Hermes
    /// keep the project. The files of retired agents (`GEMINI.md`,
    /// `REASONIX.md`, `.windsurfrules`) lose our block and keep the prose.
    #[test]
    fn upgrade_from_duplicated_lecture_moves_it_to_every_agent_global() {
        let home = crate::platform::ScratchHome::new("upgrade_all_agents_");
        for dir in [
            ".claude",
            ".cursor",
            ".grok",
            ".gemini",
            ".qwen",
            ".codex",
            ".hermes",
            ".reasonix",
            ".cline",
            ".factory",
            ".codeium/windsurf",
        ] {
            std::fs::create_dir_all(home.path().join(dir)).unwrap();
        }
        std::fs::create_dir_all(home.path().join(".pi").join("agent")).unwrap();
        std::fs::create_dir_all(home.path().join(".config").join("github-copilot")).unwrap();
        std::fs::create_dir_all(
            home.path()
                .join(".config")
                .join("Code")
                .join("User")
                .join("globalStorage")
                .join("saoudrizwan.claude-dev"),
        )
        .unwrap();
        let leftover_claude_agents = home.path().join(".claude").join("AGENTS.md");
        let leftover_cursor_md = home
            .path()
            .join(".cursor")
            .join("rules")
            .join("tokenade.md");
        std::fs::create_dir_all(leftover_cursor_md.parent().unwrap()).unwrap();
        let full = scaffold_block(true);
        std::fs::write(&leftover_claude_agents, &full).unwrap();
        std::fs::write(&leftover_cursor_md, &full).unwrap();

        let root = tmp_root("upgrade-all-agents");
        std::fs::create_dir_all(root.join(".tokenade")).unwrap();
        std::fs::create_dir_all(root.join(".github")).unwrap();
        let wrapped = |name: &str| format!("# House {name}\n\n{full}\nkeep {name}\n");
        for name in [
            "AGENTS.md",
            "CLAUDE.md",
            ".grokrules",
            ".cursorrules",
            "GEMINI.md",
            "QWEN.md",
            ".hermes.md",
            "REASONIX.md",
            ".clinerules",
            ".windsurfrules",
        ] {
            std::fs::write(root.join(name), wrapped(name)).unwrap();
        }
        std::fs::write(
            root.join(".github").join("copilot-instructions.md"),
            wrapped("copilot"),
        )
        .unwrap();
        std::fs::write(
            root.join(".tokenade").join("scaffold.flag"),
            format!(
                "version: {}\nstyle: {}\nscaffolded: AGENTS.md\n",
                SCAFFOLD_VERSION - 1,
                crate::style::load(&root).as_str()
            ),
        )
        .unwrap();

        let first = ensure(&root);
        assert!(!first.skipped, "version bump must re-enter: {first:?}");

        const CLAUSE: &str = "this machine's agent config";
        for name in [
            "AGENTS.md",
            "CLAUDE.md",
            ".grokrules",
            ".cursorrules",
            "GEMINI.md",
            "QWEN.md",
            "REASONIX.md",
            ".clinerules",
            ".windsurfrules",
        ] {
            let body = std::fs::read_to_string(root.join(name)).unwrap();
            assert!(
                !body.contains("tokenade map"),
                "{name} still duplicated after upgrade:\n{body}"
            );
            assert!(
                !body.contains(CLAUSE) && !body.contains(SCAFFOLD_MARKER) && body.contains("keep"),
                "{name} still carries a Tokenade block, or lost user prose:\n{body}"
            );
        }
        for name in [".github/copilot-instructions.md", ".hermes.md"] {
            let body = std::fs::read_to_string(root.join(name)).unwrap();
            assert!(
                body.contains("tokenade map"),
                "{name} is project-only and lost the lecture:\n{body}"
            );
        }

        let globals: &[(&str, bool)] = &[
            (".claude/CLAUDE.md", true),
            (".cursor/rules/tokenade.mdc", true),
            (".qwen/QWEN.md", true),
            (".codex/AGENTS.md", true),
            (".pi/agent/AGENTS.md", true),
            (".factory/AGENTS.md", true),
            (".cline/rules/tokenade.md", true),
        ];
        // Hermes has no global address: the project keeps the lesson.
        assert!(
            !home.path().join(".hermes").join("rules.md").exists()
                || !std::fs::read_to_string(home.path().join(".hermes").join("rules.md"))
                    .unwrap_or_default()
                    .contains("tokenade map"),
            "Hermes has no global rules file"
        );
        for (rel, want_map) in globals {
            let body = std::fs::read_to_string(home.path().join(rel)).unwrap();
            assert_eq!(
                body.contains("tokenade map"),
                *want_map,
                "{rel} lecture: {body}"
            );
        }
        // Grok already loads ~/.claude/CLAUDE.md: no second copy.
        let grok_path = home.path().join(".grok").join("rules").join("tokenade.md");
        if grok_path.exists() {
            let grok = std::fs::read_to_string(&grok_path).unwrap();
            assert!(
                !grok.contains("tokenade map"),
                "Grok would load Claude's lecture AND its own:\n{grok}"
            );
        }
        for leftover in [&leftover_claude_agents, &leftover_cursor_md] {
            let body = std::fs::read_to_string(leftover).unwrap_or_default();
            assert!(
                !body.contains("tokenade map"),
                "leftover still loaded by Grok: {} \n{body}",
                leftover.display()
            );
        }
        let second = ensure(&root);
        assert!(second.skipped, "second ensure must be a no-op: {second:?}");
    }

    #[test]
    fn post_upgrade_refresh_writes_globals_without_a_project() {
        let home = crate::platform::ScratchHome::new("post_upgrade_globals_");
        for dir in [".claude", ".cursor", ".grok", ".gemini", ".qwen", ".codex"] {
            std::fs::create_dir_all(home.path().join(dir)).unwrap();
        }
        let leftover = home.path().join(".claude").join("AGENTS.md");
        std::fs::write(&leftover, scaffold_block(true)).unwrap();
        let claude_md = home.path().join(".claude").join("CLAUDE.md");
        std::fs::write(
            &claude_md,
            "<!-- tokenade:start -->\nshort fence\n<!-- tokenade:end -->\nkeep me\n",
        )
        .unwrap();
        let written = ensure_global_rules(Some(home.path()));
        assert!(
            !written.is_empty(),
            "globals must be written on a fresh home: {written:?}"
        );
        for rel in [
            ".claude/CLAUDE.md",
            ".cursor/rules/tokenade.mdc",
            ".qwen/QWEN.md",
            ".codex/AGENTS.md",
        ] {
            let body = std::fs::read_to_string(home.path().join(rel)).unwrap();
            assert!(
                body.contains("tokenade map"),
                "{rel} missing lecture:\n{body}"
            );
        }
        let claude_body = std::fs::read_to_string(&claude_md).unwrap();
        assert!(
            !claude_body.contains("<!-- tokenade:start -->"),
            "short 1.1.18 fence must not sit next to the lecture:\n{claude_body}"
        );
        assert!(
            claude_body.contains("keep me") && claude_body.contains("tokenade map"),
            "user prose around the old fence must survive:\n{claude_body}"
        );
        assert!(
            !home.path().join("AGENTS.md").exists()
                || !std::fs::read_to_string(home.path().join("AGENTS.md"))
                    .unwrap_or_default()
                    .contains("tokenade map"),
            "~/AGENTS.md must not carry the lecture (Codex reads ~/.codex/AGENTS.md)"
        );
        let leftover_body = std::fs::read_to_string(&leftover).unwrap_or_default();
        assert!(
            !leftover_body.contains("tokenade map"),
            "leftover still loaded: {leftover_body}"
        );
        let again = ensure_global_rules(Some(home.path()));
        assert!(again.is_empty(), "second pass must be a no-op: {again:?}");
        let retracted = retract_global_rules(Some(home.path()));
        assert!(
            !retracted.is_empty(),
            "uninstall must strip globals: {retracted:?}"
        );
        for rel in [
            ".claude/CLAUDE.md",
            ".cursor/rules/tokenade.mdc",
            ".qwen/QWEN.md",
        ] {
            let body = std::fs::read_to_string(home.path().join(rel)).unwrap_or_default();
            assert!(
                !body.contains("tokenade map"),
                "{rel} survived retract:\n{body}"
            );
        }
    }

    /// Paths EACH agent really loads (package / shipped docs).
    /// After `ensure`, `tokenade map` appears only once per agent.
    fn confirmed_load_paths(agent: crate::detect::Agent, home: &Path, root: &Path) -> Vec<PathBuf> {
        use crate::detect::Agent;
        let mut out = Vec::new();
        let push = |p: PathBuf, out: &mut Vec<PathBuf>| {
            if !out.contains(&p) {
                out.push(p);
            }
        };
        match agent {
            Agent::ClaudeCode | Agent::T3Code | Agent::OpenClaw => {
                push(home.join(".claude").join("CLAUDE.md"), &mut out);
                push(root.join("CLAUDE.md"), &mut out);
                push(root.join("AGENTS.md"), &mut out);
            }
            Agent::Antigravity => {
                push(home.join(".gemini").join("GEMINI.md"), &mut out);
                push(root.join("GEMINI.md"), &mut out);
                push(root.join("AGENTS.md"), &mut out);
            }
            Agent::Cursor => {
                push(
                    home.join(".cursor").join("rules").join("tokenade.mdc"),
                    &mut out,
                );
                push(root.join("AGENTS.md"), &mut out);
                push(root.join("CLAUDE.md"), &mut out);
                push(root.join(".cursorrules"), &mut out);
            }
            Agent::Grok => {
                push(
                    home.join(".grok").join("rules").join("tokenade.md"),
                    &mut out,
                );
                push(home.join(".claude").join("CLAUDE.md"), &mut out);
                push(home.join(".claude").join("AGENTS.md"), &mut out);
                push(
                    home.join(".cursor").join("rules").join("tokenade.md"),
                    &mut out,
                );
                push(root.join("AGENTS.md"), &mut out);
                push(root.join("CLAUDE.md"), &mut out);
                push(root.join(".grokrules"), &mut out);
            }
            Agent::Qwen => {
                push(home.join(".qwen").join("QWEN.md"), &mut out);
                push(root.join("QWEN.md"), &mut out);
            }
            Agent::Codex => {
                push(home.join(".codex").join("AGENTS.md"), &mut out);
                push(root.join("AGENTS.md"), &mut out);
            }
            Agent::Pi => {
                push(home.join(".pi").join("agent").join("AGENTS.md"), &mut out);
                push(root.join("AGENTS.md"), &mut out);
                push(root.join("CLAUDE.md"), &mut out);
            }
            Agent::Droid => {
                push(home.join(".factory").join("AGENTS.md"), &mut out);
                push(root.join("AGENTS.md"), &mut out);
                push(root.join("CLAUDE.md"), &mut out);
            }
            Agent::Cline => {
                push(
                    home.join(".cline").join("rules").join("tokenade.md"),
                    &mut out,
                );
                push(root.join(".clinerules"), &mut out);
                push(root.join("AGENTS.md"), &mut out);
            }
            Agent::Hermes => {
                push(home.join(".hermes").join("SOUL.md"), &mut out);
                push(root.join(".hermes.md"), &mut out);
                push(root.join("HERMES.md"), &mut out);
                push(root.join("AGENTS.md"), &mut out);
                push(root.join(".hermes_rules.md"), &mut out);
            }
            Agent::Copilot | Agent::CopilotVsCode => {
                push(root.join("AGENTS.md"), &mut out);
                push(
                    root.join(".github").join("copilot-instructions.md"),
                    &mut out,
                );
            }
            Agent::OpenCode => {
                push(home.join(".claude").join("CLAUDE.md"), &mut out);
                push(root.join("AGENTS.md"), &mut out);
                push(root.join("CLAUDE.md"), &mut out);
            }
            Agent::DevinDesktop => {
                push(
                    home.join(".codeium")
                        .join("windsurf")
                        .join("memories")
                        .join("global_rules.md"),
                    &mut out,
                );
                push(root.join(".windsurfrules"), &mut out);
                push(root.join("AGENTS.md"), &mut out);
            }
            Agent::Aider | Agent::KiloCode | Agent::Odysseus | Agent::Unknown => {}
        }
        out
    }

    #[test]
    fn each_present_agent_loads_the_lecture_once() {
        let home = crate::platform::ScratchHome::new("one_lecture_per_agent_");
        for dir in [
            ".claude",
            ".cursor",
            ".grok",
            ".gemini/antigravity",
            ".qwen",
            ".codex",
            ".hermes",
            ".reasonix",
            ".cline",
            ".factory",
            ".codeium",
        ] {
            std::fs::create_dir_all(home.path().join(dir)).unwrap();
        }
        std::fs::create_dir_all(home.path().join(".pi").join("agent")).unwrap();
        std::fs::create_dir_all(home.path().join(".config").join("github-copilot")).unwrap();
        let root = tmp_root("one-lecture");
        std::fs::create_dir_all(root.join(".github")).unwrap();
        let first = ensure(&root);
        assert!(!first.skipped, "fresh project must write: {first:?}");
        for agent in crate::detect::Agent::all() {
            let lectures: Vec<_> = confirmed_load_paths(*agent, home.path(), &root)
                .into_iter()
                .filter(|p| {
                    std::fs::read_to_string(p)
                        .ok()
                        .is_some_and(|s| s.contains("tokenade map"))
                })
                .collect();
            assert!(
                lectures.len() <= 1,
                "{} loads the lecture {} times: {lectures:?}",
                agent.as_str(),
                lectures.len()
            );
        }
    }

    /// `retract` removes a pointer the same way it removes a full block.
    /// Binary uninstall leaves project files (git-tracked); this is the
    /// shipped strip for `<!-- tokenade-scaffold -->`.
    #[test]
    fn retract_strips_pointer_blocks() {
        let home = crate::platform::ScratchHome::new("retract_pointers_");
        std::fs::create_dir_all(home.path().join(".grok")).unwrap();
        let root = tmp_root("retract-pointers");
        let ptr = super::pointer_block();
        std::fs::write(
            root.join("CLAUDE.md"),
            format!("# House\n\n{ptr}\nkeep me\n"),
        )
        .unwrap();
        std::fs::write(
            root.join(".grokrules"),
            format!("# Grok\n\n{ptr}\nkeep me\n"),
        )
        .unwrap();
        std::fs::write(
            root.join("AGENTS.md"),
            format!("# A\n\n{}\nkeep me\n", scaffold_block(true)),
        )
        .unwrap();
        let touched = retract(&root);
        assert!(
            touched.iter().any(|f| f.contains("CLAUDE.md")),
            "CLAUDE.md pointer not stripped: {touched:?}"
        );
        assert!(
            touched.iter().any(|f| f.contains(".grokrules")),
            ".grokrules pointer not stripped: {touched:?}"
        );
        for name in ["CLAUDE.md", ".grokrules", "AGENTS.md"] {
            let text = std::fs::read_to_string(root.join(name)).unwrap();
            assert!(
                !text.contains(SCAFFOLD_MARKER),
                "{name} still has a scaffold marker:\n{text}"
            );
            assert!(text.contains("keep me"), "{name} lost user prose:\n{text}");
        }
        let _ = home;
    }

    #[test]
    fn roo_rules_target_when_project_has_roo_dir() {
        let d = std::env::temp_dir().join(format!("tk-roo-{}", std::process::id()));
        let _ = std::fs::create_dir_all(d.join(".roo"));
        let targets = detect_installed_agents(&d);
        assert!(
            targets
                .iter()
                .any(|t| t.ends_with(".roo/rules/tokenade.md")),
            "project with .roo/ gets the Roo rules target: {targets:?}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn primary_files_always_written_in_fresh_dir() {
        let _home = crate::platform::ScratchHome::new("primary_files_always_w");
        let root = tmp_root("fresh");
        let r = ensure(&root);
        assert!(!r.skipped);
        assert!(r.files_written.contains(&"CLAUDE.md".to_string()));
        assert!(r.files_written.contains(&"AGENTS.md".to_string()));
        assert!(root.join("CLAUDE.md").exists());
        assert!(root.join("AGENTS.md").exists());
        // Marker is present so a re-run is a no-op.
        let content = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        assert!(content.contains(SCAFFOLD_MARKER));
    }

    /// A cloned repo can ship `CLAUDE.md` as a symlink pointing anywhere on
    /// the machine. MEASURED before the fix: `tokenade install` in that repo
    /// wrote ~4 KB of scaffold THROUGH the link into the chosen file. Cloning
    /// a repo must not let it pick a write target for us.
    #[cfg(unix)]
    #[test]
    fn scaffold_refuses_a_symlinked_rules_file() {
        let _home = crate::platform::ScratchHome::new("scaffold_refuses_a_s_");
        let root = tmp_root("scaffold_symlink");
        let outside = root.parent().unwrap().join(format!(
            "outside-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&outside, "precious").unwrap();
        let target = root.join("CLAUDE.md");
        std::os::unix::fs::symlink(&outside, &target).unwrap();

        assert_eq!(
            write_scaffold_to(&root, &target, false),
            None,
            "must refuse, not write"
        );
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "precious",
            "the link target must be untouched"
        );
        // A plain rules file in the same repo is still scaffolded.
        let plain = root.join("AGENTS.md");
        assert_eq!(write_scaffold_to(&root, &plain, false), Some(true));
        assert!(std::fs::read_to_string(&plain)
            .unwrap()
            .contains(SCAFFOLD_MARKER));
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn global_lecture_matches_the_agent_that_reads_it() {
        let home = crate::platform::ScratchHome::new("agent_specific_lecture_");
        for dir in [".claude", ".codex"] {
            std::fs::create_dir_all(home.path().join(dir)).unwrap();
        }
        let written = ensure_global_rules(Some(home.path()));
        assert!(!written.is_empty(), "fresh home must write: {written:?}");
        let claude =
            std::fs::read_to_string(home.path().join(".claude").join("CLAUDE.md")).unwrap();
        let codex = std::fs::read_to_string(home.path().join(".codex").join("AGENTS.md")).unwrap();
        assert!(
            claude.contains("extends your `Read` tool"),
            "Claude reads via hooked Read:\n{claude}"
        );
        assert!(
            !claude.contains("your own file reader cannot"),
            "Claude must not be told to call tokenade read:\n{claude}"
        );
        assert!(
            !claude.contains("## Searching the web"),
            "Claude hooks websearch; no search command section:\n{claude}"
        );
        assert!(
            codex.contains("tokenade read <file>"),
            "Codex has no Read hook:\n{codex}"
        );
        assert!(
            !codex.contains("extends your `Read` tool"),
            "Codex must not be told its Read changed:\n{codex}"
        );
        assert!(
            codex.contains("## Searching the web"),
            "Codex has no websearch hook:\n{codex}"
        );
    }

    /// cwd = `$HOME`: `root/.claude/CLAUDE.md` IS the global. Project healing
    /// rewrote it as a pointer, `ensure_global_rules` put the lesson back — a
    /// flip-flop on every command (measured 2026-09-28 on `cd ~`). Same for
    /// cwd = `~/.codex`, whose `AGENTS.md` is Codex's global.
    /// No more pointer: a project file carries the lesson or nothing. With no
    /// recognised `AGENTS.md` reader, we write it (an unknown agent may read
    /// it); when each one has its own outside the repo, our block goes and the
    /// wrapper with it — the user's prose stays.
    #[test]
    fn project_file_holds_the_lecture_or_nothing() {
        let home = crate::platform::ScratchHome::new("lecture_or_nothing_");
        let root = tmp_root("lecture-or-nothing");
        assert_eq!(
            write_scaffold_to(&root, &root.join("AGENTS.md"), true),
            Some(true)
        );
        let agents = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
        assert!(
            agents.contains("tokenade map"),
            "no reader known: keep the lecture"
        );

        for dir in [".claude", ".codex", ".config/opencode"] {
            std::fs::create_dir_all(home.path().join(dir)).unwrap();
        }
        let _ = ensure_global_rules(Some(home.path()));
        // OpenCode reads `~/.claude/CLAUDE.md` as a fallback: no file of its own.
        let opencode = home
            .path()
            .join(".config")
            .join("opencode")
            .join("AGENTS.md");
        assert!(
            !opencode.exists(),
            "would mask the user's CLAUDE.md for OpenCode"
        );

        let claude = root.join("CLAUDE.md");
        std::fs::write(
            &claude,
            format!("# Team\n\nship on green\n\n{}", scaffold_block(false)),
        )
        .unwrap();
        // "Pristine" copies: the AGENTS.md one only carries us (our 2nd
        // write of a file we had created); the CLAUDE.md one is the user's
        // original.
        let agents_side = root.join("AGENTS.md.tokenade-original");
        std::fs::write(&agents_side, scaffold_block(true)).unwrap();
        let claude_side = root.join("CLAUDE.md.tokenade-original");
        std::fs::write(&claude_side, "# Team\n").unwrap();
        assert_eq!(
            write_scaffold_to(&root, &root.join("AGENTS.md"), true),
            Some(true)
        );
        assert_eq!(write_scaffold_to(&root, &claude, false), Some(true));
        assert!(
            !root.join("AGENTS.md").exists(),
            "wrapper + lecture only: file removed"
        );
        assert!(
            !agents_side.exists(),
            "a copy holding only our block goes with the file"
        );
        assert!(claude_side.exists(), "the user's original stays");
        assert_eq!(
            std::fs::read_to_string(&claude).unwrap(),
            "# Team\n\nship on green\n"
        );
        assert_eq!(
            write_scaffold_to(&root, &claude, false),
            Some(false),
            "second pass no-op"
        );
        assert_eq!(
            write_scaffold_to(&root, &root.join("AGENTS.md"), true),
            Some(false),
            "never created"
        );
    }

    /// "Commands" teaches `wrap` to whoever has no shell hook. The global of
    /// a hooked agent no longer carries it; a shared file does.
    #[test]
    fn commands_section_only_where_no_shell_hook_reads_it() {
        use crate::detect::Agent;
        assert!(!global_block_for(Agent::ClaudeCode, false).contains("## Commands"));
        assert!(!global_block_for(Agent::Codex, true).contains("tokenade wrap"));
        assert!(global_block_for(Agent::OpenCode, true).contains("## Commands"));
        assert!(scaffold_block_for(Agent::ClaudeCode, true).contains("## Commands"));
        let claude = global_block_for(Agent::ClaudeCode, false);
        assert!(claude.contains("## Keep output lean") && claude.contains("tokenade map"));
    }

    /// The 1.1.19 pointer in `.cursorrules` / `GEMINI.md`, and in the
    /// `CLAUDE.md` of a parent folder Claude loads while walking up: the CLI
    /// healing removes them, without touching a user's file.
    #[test]
    fn heal_reaches_agent_files_and_parent_dirs() {
        let home = crate::platform::ScratchHome::new("heal_agent_parent_");
        for dir in [".claude", ".cursor", ".gemini/antigravity", ".codex"] {
            std::fs::create_dir_all(home.path().join(dir)).unwrap();
        }
        let _ = ensure_global_rules(Some(home.path()));
        let parent = home.path().join("projects");
        let root = parent.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let pointer = format!(
            "# Project rules for AI coding agents\n\n{}\n",
            pointer_block()
        );
        for p in [
            root.join(".cursorrules"),
            root.join("GEMINI.md"),
            parent.join("CLAUDE.md"),
        ] {
            std::fs::write(&p, &pointer).unwrap();
        }
        // No marker: not provably ours, so it stays (#7 — `## Project rules`
        // is a title users write too).
        std::fs::write(parent.join("CLAUDE.local.md"), "\n\n## Project rules").unwrap();
        std::fs::write(parent.join("AGENTS.md"), "# Mine\n").unwrap();
        let healed = heal_project_lectures(&root);
        assert_eq!(healed.len(), 3, "{healed:?}");
        for p in [
            root.join(".cursorrules"),
            root.join("GEMINI.md"),
            parent.join("CLAUDE.md"),
        ] {
            assert!(!p.exists(), "{} survived", p.display());
        }
        assert_eq!(
            std::fs::read_to_string(parent.join("CLAUDE.local.md")).unwrap(),
            "\n\n## Project rules"
        );
        assert_eq!(
            std::fs::read_to_string(parent.join("AGENTS.md")).unwrap(),
            "# Mine\n"
        );
        // Orphaned copy from before the fix: the file is gone, the copy only
        // carries us.
        let orphan = root.join("QWEN.md.tokenade-original");
        std::fs::write(&orphan, pointer_block()).unwrap();
        // Same for an agent retired from the product (Windsurf).
        let retired = root.join(".windsurfrules.tokenade-original");
        std::fs::write(&retired, scaffold_block(true)).unwrap();
        let healed = heal_project_lectures(&root);
        assert!(
            healed.contains(&root.join("QWEN.md")) && healed.contains(&root.join(".windsurfrules")),
            "{healed:?}"
        );
        assert!(!orphan.exists() && !retired.exists());
        assert!(heal_project_lectures(&root).is_empty());
    }

    /// Every name in `AGENT_PROJECT_FILES` is reached by the healing: an old
    /// pointer is removed there, wherever it is stored (`.github/`, `.roo/rules/`).
    #[test]
    fn heal_walks_every_agent_project_file() {
        let home = crate::platform::ScratchHome::new("heal_every_agent_file_");
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        let _ = ensure_global_rules(Some(home.path()));
        let root = tmp_root("heal-every-agent-file");
        let pointer = format!("{}\n", pointer_block());
        for name in AGENT_PROJECT_FILES {
            let p = root.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, &pointer).unwrap();
        }
        let healed = heal_project_lectures(&root);
        for name in AGENT_PROJECT_FILES {
            let p = root.join(name);
            assert!(healed.contains(&p), "{name} not reached: {healed:?}");
            let body = std::fs::read_to_string(&p).unwrap_or_default();
            assert!(
                !body.contains("do not open another copy"),
                "{name}:\n{body}"
            );
        }
    }

    /// Reasonix left the product. What an earlier version wrote for it ALONE
    /// goes at healing: our block is removed, the file deleted if only it
    /// remained, the user's prose kept otherwise. Global included
    /// (`~/.reasonix/REASONIX.md`). `GEMINI.md` is not one: Antigravity
    /// reads it.
    #[test]
    fn files_written_for_retired_agents_are_cleaned() {
        let home = crate::platform::ScratchHome::new("heal_retired_agents_");
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        let reasonix = home.path().join(".reasonix").join("REASONIX.md");
        std::fs::create_dir_all(reasonix.parent().unwrap()).unwrap();
        std::fs::write(&reasonix, format!("{}\n", scaffold_block(true))).unwrap();
        let _ = ensure_global_rules(Some(home.path()));
        assert!(!reasonix.exists(), "~/.reasonix/REASONIX.md survived");

        let root = tmp_root("heal-retired-agents");
        let ours = format!("{}\n", scaffold_block(true));
        std::fs::write(root.join("REASONIX.md"), format!("# Mine\n\n{ours}")).unwrap();
        let healed = heal_project_lectures(&root);
        assert!(healed.contains(&root.join("REASONIX.md")), "{healed:?}");
        assert_eq!(
            std::fs::read_to_string(root.join("REASONIX.md")).unwrap(),
            "# Mine\n"
        );
        assert!(heal_project_lectures(&root).is_empty());
    }

    /// Devin Desktop, the Windsurf fork, reads `.windsurfrules`: the file is
    /// no longer a leftover of a retired agent. With Devin present, the lesson
    /// lives once in `~/.codeium/windsurf/memories/global_rules.md`, and a repo
    /// `.windsurfrules` that only carried our block goes at healing.
    #[test]
    fn devin_desktop_reads_the_lecture_from_its_global_not_windsurfrules() {
        let home = crate::platform::ScratchHome::new("devin_desktop_global_");
        std::fs::create_dir_all(home.path().join(".codeium").join("windsurf")).unwrap();
        let global = home
            .path()
            .join(".codeium")
            .join("windsurf")
            .join("memories")
            .join("global_rules.md");
        let written = ensure_global_rules(Some(home.path()));
        assert!(written.contains(&global), "{written:?}");
        let body = std::fs::read_to_string(&global).unwrap();
        assert_eq!(body.matches("tokenade map").count(), 1, "{body}");
        assert!(
            ensure_global_rules(Some(home.path())).is_empty(),
            "idempotent"
        );

        let root = tmp_root("devin-desktop-windsurfrules");
        let rules = root.join(".windsurfrules");
        std::fs::write(&rules, format!("{}\n", scaffold_block(true))).unwrap();
        let healed = heal_project_lectures(&root);
        assert!(healed.contains(&rules), "{healed:?}");
        assert!(!rules.exists(), ".windsurfrules survived a covered Devin");
    }

    /// Kilo always loads `~/.kilo/rules/*.md` AND, lacking
    /// `~/.config/kilo/AGENTS.md`, `~/.claude/CLAUDE.md`: captured ×2 on
    /// 2026-09-28. OpenCode falls back the same way. An agent reading Claude's
    /// global has no file of its own, and that global is rendered for ALL its
    /// readers: "Commands" comes back as soon as one reader has no shell hook,
    /// the Read line only promises a hook if every one has it.
    #[test]
    fn an_agent_reading_the_claude_global_gets_no_copy_of_its_own() {
        use crate::detect::Agent;
        let home = crate::platform::ScratchHome::new("fallback_readers_");
        let h = home.path();
        for dir in [".claude", ".grok", ".config/kilo", ".config/opencode"] {
            std::fs::create_dir_all(h.join(dir)).unwrap();
        }
        let kilo_rules = h.join(".kilo").join("rules").join("tokenade.md");
        std::fs::create_dir_all(kilo_rules.parent().unwrap()).unwrap();
        std::fs::write(&kilo_rules, scaffold_block(true)).unwrap();
        let _ = ensure_global_rules(Some(h));
        let claude = std::fs::read_to_string(h.join(".claude").join("CLAUDE.md")).unwrap();
        assert_eq!(claude.matches("## Explore code").count(), 1);
        assert!(
            claude.contains("## Commands"),
            "Kilo/OpenCode have no shell hook"
        );
        assert_eq!(
            claude,
            format!(
                "{}\n",
                global_block_for_readers(
                    &[
                        Agent::ClaudeCode,
                        Agent::OpenCode,
                        Agent::KiloCode,
                        Agent::Grok
                    ],
                    true
                )
                .trim_end()
            )
        );
        assert!(!kilo_rules.exists(), "Kilo would load the lecture twice");
        assert!(!h.join(".grok").join("rules").join("tokenade.md").exists());
        assert!(!h
            .join(".config")
            .join("opencode")
            .join("AGENTS.md")
            .exists());
        assert!(ensure_global_rules(Some(h)).is_empty(), "second pass no-op");

        // The user has their own global: Kilo and OpenCode no longer read
        // Claude's, the lesson goes to them.
        std::fs::write(h.join(".config").join("kilo").join("AGENTS.md"), "# mine\n").unwrap();
        let oc = h.join(".config").join("opencode").join("AGENTS.md");
        std::fs::write(&oc, "# mine too\n").unwrap();
        let _ = ensure_global_rules(Some(h));
        assert!(std::fs::read_to_string(&kilo_rules)
            .unwrap()
            .contains("tokenade map"));
        let oc_body = std::fs::read_to_string(&oc).unwrap();
        assert!(
            oc_body.contains("# mine too") && oc_body.contains("tokenade map"),
            "{oc_body}"
        );
        let claude = std::fs::read_to_string(h.join(".claude").join("CLAUDE.md")).unwrap();
        assert!(
            !claude.contains("## Commands"),
            "Claude + Grok both hook the shell"
        );
    }

    /// `.github/copilot-instructions.md`: complete for Copilot VS Code (no
    /// global), removed when only Copilot CLI — which has its own — reads it.
    #[test]
    fn copilot_project_file_follows_its_readers() {
        let home = crate::platform::ScratchHome::new("copilot_readers_");
        std::fs::create_dir_all(home.path().join(".copilot")).unwrap();
        let _ = ensure_global_rules(Some(home.path()));
        let root = tmp_root("copilot-readers");
        let gh = root.join(".github").join("copilot-instructions.md");
        std::fs::create_dir_all(gh.parent().unwrap()).unwrap();
        std::fs::write(&gh, scaffold_block(true)).unwrap();
        assert_eq!(write_scaffold_to(&root, &gh, true), Some(true));
        assert!(!gh.exists(), "Copilot CLI would load the lecture twice");

        std::fs::create_dir_all(home.path().join(".config").join("Code").join("User")).unwrap();
        assert_eq!(write_scaffold_to(&root, &gh, true), Some(true));
        assert!(
            std::fs::read_to_string(&gh)
                .unwrap()
                .contains("tokenade map"),
            "VS Code has no global"
        );
    }

    /// Without a prompt hook, the rules file is the style's only channel.
    /// Since the lesson lives in the global, the preamble must be there — the
    /// 1.1.19 compat gate saw five `sobriety` cells regress without it.
    /// `auto` resolves to empty: nothing is written.
    #[test]
    fn a_pinned_style_reaches_the_global_lecture() {
        let home = crate::platform::ScratchHome::new("global_style_");
        std::fs::create_dir_all(home.path().join(".config").join("kilo")).unwrap();
        let rules = home.path().join(".kilo").join("rules").join("tokenade.md");
        let _ = ensure_global_rules(Some(home.path()));
        assert!(
            !std::fs::read_to_string(&rules)
                .unwrap()
                .contains("[tokenade:style="),
            "auto writes nothing"
        );
        crate::style::save(home.path(), crate::style::Style::Coding).unwrap();
        let _ = ensure_global_rules(Some(home.path()));
        let body = std::fs::read_to_string(&rules).unwrap();
        assert_eq!(body.matches("[tokenade:style=coding]").count(), 1, "{body}");
        let end = body.find(SCAFFOLD_END_MARKER).unwrap();
        assert!(
            body.find("[tokenade:style=").unwrap() < end,
            "inside the block"
        );
    }

    #[test]
    fn heal_never_turns_a_global_lecture_into_a_pointer() {
        let home = crate::platform::ScratchHome::new("heal_home_is_not_a_project_");
        let h = home.path().to_path_buf();
        std::fs::create_dir_all(h.join(".claude")).unwrap();
        std::fs::create_dir_all(h.join(".codex")).unwrap();
        let _ = ensure_global_rules(Some(&h));
        let claude = h.join(".claude").join("CLAUDE.md");
        let codex = h.join(".codex").join("AGENTS.md");
        let before_claude = std::fs::read_to_string(&claude).unwrap();
        let before_codex = std::fs::read_to_string(&codex).unwrap();
        assert!(before_claude.contains("tokenade map"));

        assert!(heal_project_lectures(&h).is_empty());
        assert!(heal_project_lectures(&h.join(".codex")).is_empty());
        assert_eq!(std::fs::read_to_string(&claude).unwrap(), before_claude);
        assert_eq!(std::fs::read_to_string(&codex).unwrap(), before_codex);

        let _ = ensure(&h);
        assert!(
            !h.join("AGENTS.md").exists(),
            "ensure($HOME) wrote ~/AGENTS.md"
        );
        assert!(
            !h.join("CLAUDE.md").exists(),
            "ensure($HOME) wrote ~/CLAUDE.md"
        );
        assert_eq!(std::fs::read_to_string(&claude).unwrap(), before_claude);
    }

    #[test]
    fn heal_never_drops_user_content_in_owned_looking_section() {
        // `## Commands` is a heading users write too (#7): with a body that
        // is not ours, outside our wrapper, it is the user's and survives.
        let _home = crate::platform::ScratchHome::new("heal_user_content_");
        let root = tmp_root("heal_user_content");
        let target = root.join("AGENTS.md");
        let block = scaffold_block(false);
        let content = format!(
            "## Commands\nMy own deploy notes: make deploy-prod\n\n\
             ## Deploy notes\nkeep the pipeline green\n\n{block}"
        );
        std::fs::write(&target, &content).unwrap();
        assert_eq!(write_scaffold_to(&root, &target, false), Some(false), "nothing of ours to heal");
        let after = std::fs::read_to_string(&target).unwrap();
        assert!(
            after.contains("## Commands\nMy own deploy notes: make deploy-prod"),
            "user section under a generic heading lost:\n{after}"
        );
        assert!(
            after.contains("keep the pipeline green"),
            "user heading lost:\n{after}"
        );
        assert_eq!(after.matches(SCAFFOLD_MARKER).count(), 1);
    }


    /// #7: a project file Tokenade never wrote keeps its `## Commands`,
    /// `## Project rules`, `## Engineering principles`, `## Codebase
    /// exploration` sections — with the lesson global (strip path) or not
    /// (block path).
    #[test]
    fn ensure_keeps_user_sections_under_generic_headings() {
        let cases: &[(&str, &str, &[&str])] = &[
            (
                "CLAUDE.md",
                "# CLAUDE.md\n\nGuidance for this repo.\n\n## Commands\n\n```bash\nuv sync --extra dev\nuv run pytest\n```\n\nRun `prek run --all-files` before finishing.\n\n## Architecture\n\n- `cli.py` is the entry point.\n",
                &["## Commands", "uv sync --extra dev", "prek run --all-files", "## Architecture"],
            ),
            (
                "AGENTS.md",
                "# AGENTS.md\n\n## Project rules\n\nNever commit `.env` files.\n\n## Engineering principles\n\nPrefer the standard library.\n\n## Codebase exploration\n\nStart from `src/main.rs`.\n\n## Testing\n\n`cargo test` must pass.\n",
                &["## Project rules", "Never commit `.env` files.", "## Engineering principles", "Prefer the standard library.", "## Codebase exploration", "Start from `src/main.rs`.", "## Testing"],
            ),
        ];
        for global in [true, false] {
            let home = crate::platform::ScratchHome::new("generic_heading_");
            if global {
                for agent in [".claude", ".codex"] {
                    std::fs::create_dir_all(home.path().join(agent)).unwrap();
                }
            }
            for (name, body, kept) in cases {
                let root = tmp_root(&format!("generic_heading_{global}_{}", name.replace('.', "_")));
                let target = root.join(name);
                std::fs::write(&target, body).unwrap();
                ensure(&root);
                let after = std::fs::read_to_string(&target).unwrap();
                for k in *kept {
                    assert!(after.contains(k), "global={global} {name}: lost {k:?}:\n{after}");
                }
                if global {
                    assert_eq!(&after, body, "global={global} {name}: file touched");
                }
            }
        }
    }
    #[test]
    fn scaffold_block_plain_strips_markers_keeps_rules() {
        let plain = scaffold_block_plain();
        // HTML sentinel lines must be gone (they only matter on disk).
        assert!(
            !plain.contains(SCAFFOLD_MARKER),
            "open marker leaked: {plain}"
        );
        assert!(
            !plain.contains(SCAFFOLD_END_MARKER),
            "close marker leaked: {plain}"
        );
        // Exploration leads survive.
        assert!(plain.contains("tokenade map"));
        assert!(plain.contains("tokenade exec"));
        // v25 (bench): the "## Read several files: one turn, not N" section was
        // REMOVED — its multi-read/multi-edit nudge made the agent read+edit
        // all files one-by-one on migrations (+116% vs control) instead of one
        // script. The migration→script guidance stays in the exec section; the
        // read-batching reactive hook nudge is kept (it's the static prompt
        // section that backfired, not the hook).
        assert!(
            !plain.contains("one turn, not N"),
            "the multi-read/edit scaffold section must be gone: {plain}"
        );
        // v22: the hookless-agent manual-wrap clause. Its PURPOSE is that a
        // hooked agent must not wrap by hand what its hook already folds.
        //
        // That purpose used to be served by naming the hooked agents ("you are
        // not Claude Code or Gemini CLI"). We measured what that roster had
        // become: nine agents carry hooks, so seven read "you have none" and
        // were told to double-wrap — the assertion below was pinning the very
        // mechanism that broke the guarantee it was named after. The clause now
        // points at `tokenade detect`, which answers for whoever is reading, so
        // the check moved from the ROSTER to the PURPOSE.
        assert!(
            plain.contains("tokenade wrap"),
            "manual-wrap clause missing: {plain}"
        );
        assert!(
            plain.contains("tokenade health"),
            "the clause no longer tells a reader how to learn whether it has hooks — a hooked \
             agent could double-wrap: {plain}"
        );
        // The migration / bulk-edit lever (the regression we restored)
        // MUST be present in the injected text — that's the whole point.
        assert!(
            plain.contains("mechanical edit across many files"),
            "bulk-edit migration lever missing: {plain}"
        );
        // The read-side fan-out lever: pull facts across SEVERAL files in
        // ONE exec script (the single-document case is steered to Read —
        // that "answer several questions" phrasing caused doc-digest to
        // dump a prose file through exec, so it was deliberately dropped).
        assert!(
            plain.contains("pull facts across SEVERAL files"),
            "read-side fan-out lever missing: {plain}"
        );
    }

    #[test]
    fn flag_short_circuits_subsequent_runs() {
        let _home = crate::platform::ScratchHome::new("flag_short_circuits_su");
        let root = tmp_root("flag");
        ensure(&root); // first call writes the flag
        let second = ensure(&root);
        assert!(second.skipped);
        assert!(second.files_written.is_empty());
    }

    #[test]
    fn marker_short_circuits_when_flag_deleted() {
        let _home = crate::platform::ScratchHome::new("marker_short_circuits_");
        let root = tmp_root("marker");
        ensure(&root);
        // Simulate flag deletion (user wiped .tokenade by accident).
        let flag = root.join(".tokenade").join("scaffold.flag");
        std::fs::remove_file(&flag).unwrap();
        // Re-run: ensure() runs the full pass but marker check in
        // write_scaffold_to prevents re-append.
        let before = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
        ensure(&root);
        let after = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
        assert_eq!(before, after, "marker check must prevent duplicate append");
    }

    #[test]
    fn gitignore_appended_in_git_repo() {
        let _home = crate::platform::ScratchHome::new("gitignore_appended_in_");
        let root = tmp_root("gitignore-git");
        // Simulate a git repo.
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\nnode_modules/\n").unwrap();
        ensure(&root);
        let content = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(content.contains(".tokenade/"));
        assert!(content.contains("target/"), "user rules preserved");
        assert!(content.contains("node_modules/"));
    }

    #[test]
    fn gitignore_skipped_when_not_git_repo() {
        let _home = crate::platform::ScratchHome::new("gitignore_skipped_when");
        let root = tmp_root("gitignore-nogit");
        // No .git dir → not a repo → don't touch .gitignore.
        ensure(&root);
        assert!(!root.join(".gitignore").exists());
    }

    #[test]
    fn migrates_local_fastembed_when_global_empty() {
        let _guard = fastembed_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = tmp_root("migrate-empty");
        // Override the global cache so the test doesn't touch the
        // real ~/.cache/tokenade/fastembed/.
        let fake_global = root.join("global-cache");
        std::env::set_var("FASTEMBED_CACHE", &fake_global);
        // Set up a fake local cache with a file.
        let local = root.join(".fastembed_cache");
        std::fs::create_dir_all(local.join("models--foo")).unwrap();
        std::fs::write(local.join("models--foo").join("a.bin"), b"x").unwrap();
        let result = migrate_local_fastembed_cache(&root);
        assert!(result.is_some(), "expected migration to happen");
        let msg = result.unwrap();
        assert!(
            msg.contains("moved") || msg.contains("deleted"),
            "got: {msg}"
        );
        // Local cache should be gone.
        assert!(
            !local.exists(),
            "local cache must be removed after migration"
        );
        // Global cache should now contain the model.
        assert!(fake_global.join("models--foo").join("a.bin").exists());
        std::env::remove_var("FASTEMBED_CACHE");
    }

    #[test]
    fn deletes_local_fastembed_when_global_already_populated() {
        let _guard = fastembed_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let root = tmp_root("migrate-already");
        let fake_global = root.join("global-cache");
        std::fs::create_dir_all(fake_global.join("models--foo")).unwrap();
        std::fs::write(fake_global.join("models--foo").join("already.bin"), b"old").unwrap();
        std::env::set_var("FASTEMBED_CACHE", &fake_global);
        // Local exists too.
        let local = root.join(".fastembed_cache");
        std::fs::create_dir_all(local.join("models--foo")).unwrap();
        std::fs::write(local.join("models--foo").join("dup.bin"), b"local").unwrap();
        let result = migrate_local_fastembed_cache(&root);
        assert!(result.is_some());
        let msg = result.unwrap();
        assert!(msg.contains("deleted"), "expected delete path, got: {msg}");
        assert!(!local.exists(), "local cache must be removed");
        // Global was not overwritten.
        assert!(fake_global.join("models--foo").join("already.bin").exists());
        std::env::remove_var("FASTEMBED_CACHE");
    }

    #[test]
    fn no_op_when_no_local_cache() {
        let root = tmp_root("migrate-noop");
        let result = migrate_local_fastembed_cache(&root);
        assert!(result.is_none());
    }

    #[test]
    fn gitignore_idempotent_when_already_present() {
        let _home = crate::platform::ScratchHome::new("gitignore_idempotent_w");
        let root = tmp_root("gitignore-already");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "node_modules/\n.tokenade/\n").unwrap();
        ensure(&root);
        let content = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        // Must NOT have a duplicate entry.
        let occurrences = content.matches(".tokenade/").count();
        assert_eq!(occurrences, 1, "duplicate ignored, got:\n{content}");
    }

    #[test]
    fn append_preserves_user_content() {
        let _home = crate::platform::ScratchHome::new("append_preserves_user_");
        let root = tmp_root("append");
        let custom = "# My rules\n\n- Always use TypeScript\n";
        std::fs::write(root.join("CLAUDE.md"), custom).unwrap();
        ensure(&root);
        let content = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        assert!(content.starts_with("# My rules"));
        assert!(content.contains("Always use TypeScript"));
        assert!(content.contains(SCAFFOLD_MARKER));
    }

    #[test]
    fn legacy_flag_triggers_upgrade() {
        let _home = crate::platform::ScratchHome::new("legacy_flag_triggers_u");
        // A project scaffolded under v1 (no version line in flag,
        // old block content). When ensure() runs again with v2
        // available, the block should be SPLICED in place and the
        // user's outside-block content preserved.
        let root = tmp_root("upgrade_legacy");
        std::fs::create_dir_all(root.join(".tokenade")).unwrap();
        // Legacy flag — no `version:` line.
        std::fs::write(
            root.join(".tokenade").join("scaffold.flag"),
            "scaffolded: AGENTS.md, AGENTS.md\n",
        )
        .unwrap();
        // Legacy AGENTS.md with a v1-style block (and user text
        // around it that MUST survive the upgrade).
        let legacy = format!(
            "# Project rules\n\nUser hand-edit above the block.\n\n\
             {SCAFFOLD_MARKER}\n## Tokenade rules (v1)\n\n- old rule only\n\
             {SCAFFOLD_END_MARKER}\n\n\
             User hand-edit BELOW the block.\n"
        );
        std::fs::write(root.join("AGENTS.md"), &legacy).unwrap();

        let result = ensure(&root);
        assert!(!result.skipped, "legacy flag must trigger re-run");

        let content = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
        // Both above- and below-block hand-edits preserved verbatim.
        assert!(content.contains("User hand-edit above the block."));
        assert!(content.contains("User hand-edit BELOW the block."));
        // New block content is in place — current scaffold (hook-first
        // pivot) nudges the tokenade CLI. We anchor on a stable command
        // rather than a specific block title so refactors don't churn it.
        assert!(content.contains("tokenade map"));
        // Old rule is gone (replaced, not appended).
        assert!(!content.contains("old rule only"));

        // Flag now records v2 so a second ensure() short-circuits.
        let flag = std::fs::read_to_string(root.join(".tokenade").join("scaffold.flag")).unwrap();
        assert!(flag.contains(&format!("version: {SCAFFOLD_VERSION}")));
        let second = ensure(&root);
        assert!(second.skipped, "current-version flag should skip");
    }

    #[test]
    fn upgrade_is_idempotent_when_block_matches() {
        let _home = crate::platform::ScratchHome::new("upgrade_is_idempotent_");
        // Block ALREADY matches current scaffold_block() — even if
        // the legacy flag forces re-run, write_scaffold_to must
        // detect "no change needed" and return false.
        let root = tmp_root("upgrade_noop");
        std::fs::create_dir_all(root.join(".tokenade")).unwrap();
        std::fs::write(root.join(".tokenade").join("scaffold.flag"), "scaffolded\n").unwrap();
        // Compose the exact current-version block in the file.
        // `AGENTS.md` carries the variant with the subagent note.
        let block = scaffold_block(true);
        let file = format!("# Header\n\n{block}\nTrailing\n");
        std::fs::write(root.join("AGENTS.md"), &file).unwrap();
        let result = ensure(&root);
        // AGENTS.md should NOT appear in written list — already up
        // to date. (AGENTS.md and gitignores may still be touched.)
        assert!(
            !result.files_written.iter().any(|f| f == "AGENTS.md"),
            "should not rewrite already-current AGENTS.md, got: {:?}",
            result.files_written
        );
    }

    /// `TOKENADE_NO_WEBSEARCH=1`: the rules no longer send the agent to
    /// `tokenade search`; page fetching stays.
    #[test]
    fn web_search_off_takes_tokenade_search_out_of_the_rules() {
        let _env = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("TOKENADE_NO_WEBSEARCH").ok();
        std::env::set_var("TOKENADE_NO_WEBSEARCH", "1");
        let off = render_block(false, false, false, true);
        match prev {
            Some(v) => std::env::set_var("TOKENADE_NO_WEBSEARCH", v),
            None => std::env::remove_var("TOKENADE_NO_WEBSEARCH"),
        }
        assert!(!off.contains("tokenade search"), "{off}");
        assert!(off.contains("tokenade web <url1>"));
        assert!(render_block(false, false, false, true).contains("tokenade search"));
    }

    // ── 2.x: only a marker proves a region is Tokenade's ──────────────────

    /// Even our OWN titles, without a marker, are the user's: copied from a
    /// blog post, kept after an uninstall, or written independently.
    #[test]
    fn unmarked_tokenade_titles_are_left_alone() {
        let user = "# Mine\n\n\
            ## Explore code with the `tokenade` CLI (cheaper than reading whole files)\n\
            my own notes on tokenade map\n\n\
            ## Keep output lean\nshort answers\n\n\
            ## Tokenade rules (v3)\nkept\n\n\
            ## Commands\nmake test\n";
        let block = scaffold_block(false);
        let out = write::apply_scaffold_block(Some(user), &block).unwrap();
        assert!(out.starts_with(user.trim_end()), "user text changed:\n{out}");
        assert_eq!(out.matches(SCAFFOLD_MARKER).count(), 1);
        assert_eq!(write::apply_scaffold_block(Some(&out), &block), None, "not idempotent");
    }

    /// A refresh replaces the marked block and only it — the user's text on
    /// both sides, `## Commands` included, comes through byte for byte.
    #[test]
    fn refresh_touches_only_the_marked_block() {
        let old = format!(
            "# Mine\n\n## Commands\nuv run pytest\n\n\
             {SCAFFOLD_MARKER}\n## Explore code\nan older wording\n{SCAFFOLD_END_MARKER}\n\n\
             ## Project rules\nNever commit `.env`.\n"
        );
        let block = scaffold_block(false);
        let out = write::apply_scaffold_block(Some(&old), &block).unwrap();
        assert!(!out.contains("an older wording"), "{out}");
        assert!(out.contains(block.trim()), "{out}");
        assert!(out.starts_with("# Mine\n\n## Commands\nuv run pytest\n"), "{out}");
        assert!(out.trim_end().ends_with("## Project rules\nNever commit `.env`."), "{out}");
        assert_eq!(write::apply_scaffold_block(Some(&out), &block), None);
    }

    /// An opening marker with no closing one: only its line goes. What
    /// follows cannot be proven ours, so it is kept.
    #[test]
    fn a_lone_opening_marker_drops_only_its_line() {
        let text = format!("# Mine\n\n{SCAFFOLD_MARKER}\n## Commands\nmake deploy\n");
        let (pre, post, found) = write::take_user_sides(&text);
        assert!(found);
        assert_eq!(format!("{pre}{post}"), "# Mine\n\n## Commands\nmake deploy\n");
        let fence = format!("x\n{OLD_CLAUDE_FENCE_START}\nmine\n");
        let (pre, post, _) = write::take_user_sides(&fence);
        assert_eq!(format!("{pre}{post}"), "x\nmine\n");
    }

    /// The `<!-- tokenade:start -->` fence the 2.x install writes is a marker
    /// too: removed with its body when closed, only its line otherwise.
    #[test]
    fn old_fence_is_stripped_only_within_its_bounds() {
        let root = tmp_root("fence_bounds");
        let closed = root.join("closed.md");
        std::fs::write(
            &closed,
            format!("mine\n\n{OLD_CLAUDE_FENCE_START}\nours\n{OLD_CLAUDE_FENCE_END}\n\nafter\n"),
        )
        .unwrap();
        assert!(strip_old_claude_md_fence(&closed));
        assert_eq!(std::fs::read_to_string(&closed).unwrap(), "mine\n\nafter\n");
        let open = root.join("open.md");
        std::fs::write(&open, format!("mine\n{OLD_CLAUDE_FENCE_START}\n## Commands\nkeep\n")).unwrap();
        assert!(strip_old_claude_md_fence(&open));
        let left = std::fs::read_to_string(&open).unwrap();
        assert!(left.contains("## Commands\nkeep") && !left.contains("tokenade:start"), "{left}");
    }

    /// A file Tokenade created (intro + block) is recognised whole: retract
    /// removes it, and a user line added to it keeps it on disk.
    #[test]
    fn a_file_we_created_is_ours_whole_and_only_whole() {
        let _home = crate::platform::ScratchHome::new("created_whole_");
        let root = tmp_root("created_whole");
        ensure(&root);
        let agents = root.join("AGENTS.md");
        let text = std::fs::read_to_string(&agents).expect("ensure creates AGENTS.md");
        assert!(text.starts_with("# Project rules for AI coding agents"), "{text}");
        let (pre, post, found) = write::take_user_sides(&text);
        assert!(found && pre.trim().is_empty() && post.trim().is_empty(), "{pre}|{post}");
        let with_user = format!("{text}\n## Commands\nmake test\n");
        let (_, post, _) = write::take_user_sides(&with_user);
        assert_eq!(post.trim(), "## Commands\nmake test");
    }
}

