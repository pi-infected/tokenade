// The tests of `crate::claude_md_audit`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `claude_md_audit`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_compress_is_lossless_and_collapses_blanks() {
        let input = "# H  \n\n\n\nline one   \n\n\nline two\n\n\n";
        let out = whitespace_compress(input);
        assert_eq!(out, "# H\n\nline one\n\nline two\n");
        // every non-blank content line survives verbatim.
        for l in ["# H", "line one", "line two"] {
            assert!(out.contains(l));
        }
    }

    #[test]
    fn stale_refs_flags_missing_paths_only() {
        let dir = std::env::temp_dir().join(format!("tok-stale-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join("src"));
        std::fs::write(dir.join("src/here.rs"), "fn x(){}").unwrap();
        let cascade = vec![(
            "CLAUDE.md".to_string(),
            "Use `src/here.rs` for X and `src/missing.rs` for Y. Also see https://e/a/b.rs."
                .to_string(),
        )];
        let stale = stale_refs(&cascade, &dir);
        let refs: Vec<&str> = stale.iter().map(|s| s.reference.as_str()).collect();
        assert!(
            refs.contains(&"src/missing.rs"),
            "missing flagged: {refs:?}"
        );
        assert!(!refs.contains(&"src/here.rs"), "existing not flagged");
        assert!(refs.iter().all(|r| !r.starts_with("http")), "URLs ignored");
        let _ = std::fs::remove_dir_all(&dir);
    }




    #[test]
    fn empty_cascade_is_empty() {
        let a = audit(&[]);
        assert_eq!(a.total_tokens, 0);
        assert!(a.files.is_empty());
        assert!(a.duplicates.is_empty());
        assert_eq!(a.duplicated_tokens, 0);
    }

    #[test]
    fn counts_tokens_and_files() {
        let files = vec![
            (
                "a/CLAUDE.md".to_string(),
                "alpha bravo charlie delta\n".to_string(),
            ),
            (
                "b/CLAUDE.md".to_string(),
                "echo foxtrot golf hotel india\n".to_string(),
            ),
        ];
        let a = audit(&files);
        assert_eq!(a.files.len(), 2);
        assert!(a.total_tokens > 0);
        assert_eq!(
            a.total_tokens,
            a.files.iter().map(|f| f.tokens).sum::<usize>()
        );
    }

    #[test]
    fn detects_cross_file_duplicate_line() {
        let shared = "Always prefer tokenade MCP tools over raw grep and Read.";
        let files = vec![
            (
                "global/CLAUDE.md".to_string(),
                format!("# Global\n{shared}\nunique global content line here ok\n"),
            ),
            (
                "project/CLAUDE.md".to_string(),
                format!("# Project\n{shared}\nunique project content line here ok\n"),
            ),
        ];
        let a = audit(&files);
        assert_eq!(a.duplicates.len(), 1, "exactly one shared line");
        let d = &a.duplicates[0];
        assert_eq!(d.files.len(), 2);
        assert!(d.tokens > 0);
        assert_eq!(d.wasted_tokens(), d.tokens); // 2 files -> one redundant copy
        assert_eq!(a.duplicated_tokens, d.tokens);
    }

    #[test]
    fn whitespace_normalization_treats_lines_as_equal() {
        let files = vec![
            (
                "a.md".to_string(),
                "The quick brown fox jumps over the lazy dog.\n".to_string(),
            ),
            (
                "b.md".to_string(),
                "The   quick  brown   fox jumps over the lazy dog.\n".to_string(),
            ),
        ];
        let a = audit(&files);
        assert_eq!(a.duplicates.len(), 1, "whitespace differences collapse");
    }

    #[test]
    fn short_and_structural_lines_are_ignored() {
        let files = vec![
            ("a.md".to_string(), "---\n```\n# Hi\nshort\n".to_string()),
            ("b.md".to_string(), "---\n```\n# Hi\nshort\n".to_string()),
        ];
        let a = audit(&files);
        assert!(
            a.duplicates.is_empty(),
            "rules/fences/short lines must not be flagged, got {:?}",
            a.duplicates
        );
    }

    #[test]
    fn same_line_repeated_within_one_file_is_not_cross_file() {
        let files = vec![(
            "a.md".to_string(),
            "This sentence is long enough to be considered.\nThis sentence is long enough to be considered.\n".to_string(),
        )];
        let a = audit(&files);
        assert!(
            a.duplicates.is_empty(),
            "intra-file repetition is not cross-file duplication"
        );
    }

    #[test]
    fn duplicates_sorted_by_wasted_tokens_desc() {
        let long = "This is a fairly long shared instruction line that costs more tokens than the others do.";
        let short = "Shorter shared line but still over threshold here.";
        let files = vec![
            ("a.md".to_string(), format!("{long}\n{short}\n")),
            ("b.md".to_string(), format!("{long}\n{short}\n")),
            ("c.md".to_string(), format!("{long}\n")), // long appears in 3 files
        ];
        let a = audit(&files);
        assert_eq!(a.duplicates.len(), 2);
        assert!(
            a.duplicates[0].wasted_tokens() >= a.duplicates[1].wasted_tokens(),
            "sorted descending by wasted tokens"
        );
        // `long` is in 3 files (2 redundant copies), so it must rank first.
        assert_eq!(a.duplicates[0].files.len(), 3);
    }

    #[test]
    fn a_relative_start_audits_the_same_cascade_as_an_absolute_one() {
        // This test moves the process CWD, which is shared by every test in the
        // crate: `platform::cwd_discipline` fails the build for any test that
        // reads or writes it without serialising on ENV_LOCK. It caught this
        // one on the first workspace run.
        let _g = crate::platform::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // `Path::ancestors` is textual: a relative `start` stopped at the
        // relative root, so the walk missed every ancestor above it and emitted
        // two spellings of the same file. Measured before the fix on this repo:
        // absolute → 6 files / 11 810 tokens, `../tokenade` → 3 783.
        let tmp = std::env::temp_dir().join(format!(
            "tokenade-cascade-rel-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let deep = tmp.join("a").join("b");
        std::fs::create_dir_all(&deep).expect("dirs");
        let home = tmp.join("home");
        std::fs::create_dir_all(&home).expect("home");

        let abs = cascade_paths(&deep, Some(&home));
        let cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&deep).expect("cd");
        let rel = cascade_paths(Path::new("."), Some(&home));
        let rel_dotdot = cascade_paths(Path::new("../b"), Some(&home));
        std::env::set_current_dir(&cwd).expect("cd back");

        assert_eq!(
            rel, abs,
            "`.` must audit the same cascade as the absolute path"
        );
        assert_eq!(rel_dotdot, abs, "`../b` must audit the same cascade too");

        // No path may appear twice under any spelling — that double count is
        // what made the audit report a file as a duplicate of itself.
        let mut seen = std::collections::HashSet::new();
        for p in &rel {
            assert!(
                seen.insert(p.clone()),
                "duplicate entry in the cascade: {p:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn cascade_paths_load_order_global_first_then_root_to_cwd() {
        let home = PathBuf::from("/home/u");
        let start = PathBuf::from("/home/u/proj/sub");
        let paths = cascade_paths(&start, Some(&home));
        // Global first.
        assert_eq!(paths[0], PathBuf::from("/home/u/.claude/CLAUDE.md"));
        // Nearest (cwd) CLAUDE.md must come after an ancestor's.
        let cwd_idx = paths
            .iter()
            .position(|p| p == &PathBuf::from("/home/u/proj/sub/CLAUDE.md"))
            .unwrap();
        let root_idx = paths
            .iter()
            .position(|p| p == &PathBuf::from("/CLAUDE.md"))
            .unwrap();
        assert!(root_idx < cwd_idx, "root before cwd (nearest last)");
    }

    #[test]
    fn cascade_paths_no_home_skips_global() {
        let start = PathBuf::from("/tmp/x");
        let paths = cascade_paths(&start, None);
        assert!(!paths
            .iter()
            .any(|p| p.ends_with(".claude/CLAUDE.md")
                && crate::platform::path_has_segment(p, "home")));
        assert!(paths.contains(&PathBuf::from("/tmp/x/CLAUDE.md")));
    }

    #[test]
    fn render_smoke_no_duplicates() {
        let a = audit(&[(
            "a.md".to_string(),
            "one two three four five six seven\n".to_string(),
        )]);
        let r = render(&a);
        assert!(r.contains("always-resident"));
        assert!(r.contains("No cross-file duplication"));
    }

    #[test]
    fn duplication_is_what_hoisting_would_recover_not_what_is_written() {
        // "recoverable by hoisting each line to one place": a line in THREE
        // files costs three copies and hoisting recovers TWO of them. Both
        // off-by-one readings — counting every copy, or counting the line once
        // — survive a test that only checks the report adds up to itself,
        // because a two-file fixture makes n-1 and 1 the same number.
        let line = "one shared rule line that appears in every file\n";
        let files: Vec<(String, String)> = (0..3)
            .map(|i| (format!("{i}/CLAUDE.md"), format!("{line}unique to {i}\n")))
            .collect();
        let a = audit(&files);
        assert_eq!(
            a.duplicates.len(),
            1,
            "the fixture should have one duplicate"
        );
        let d = &a.duplicates[0];
        assert_eq!(d.files.len(), 3, "the line is not seen in all three files");
        assert_eq!(
            d.wasted_tokens(),
            d.tokens * 2,
            "a line in three files wastes two copies, not {} — hoisting leaves one",
            d.wasted_tokens() / d.tokens.max(1)
        );
        assert_eq!(
            a.duplicated_tokens,
            d.tokens * 2,
            "the header total is not what hoisting would recover"
        );
    }

    #[test]
    fn the_duplicate_listing_reconciles_with_the_total_it_promises() {
        // The header promises a total; the listing is capped at 15. Whatever
        // the cap hides has to be counted IN TOKENS, or the two numbers cannot
        // be reconciled and a reader cannot tell whether the tail is worth
        // hoisting. Measured on a real six-file cascade before this: 404
        // promised, 262 shown, and the remainder line said only "and 11 more
        // duplicated line(s)".
        //
        // 26 duplicated lines, deliberately more than the cap of 15, each of a
        // different length so the tail is not a multiple of the head.
        let mut a = String::new();
        let mut b = String::new();
        for i in 0..26 {
            let line = format!(
                "shared rule number {i} {}\n",
                "padding word ".repeat(i % 5 + 1)
            );
            a.push_str(&line);
            b.push_str(&line);
        }
        a.push_str("only in a\n");
        b.push_str("only in b\n");
        let audit = audit(&[("a/CLAUDE.md".into(), a), ("b/CLAUDE.md".into(), b)]);
        assert!(
            audit.duplicates.len() > 15,
            "the fixture does not exceed the listing cap, so it measures nothing"
        );
        let out = render(&audit);

        let promised: usize = out
            .split("~")
            .nth(1)
            .and_then(|t| t.split_whitespace().next())
            .and_then(|t| t.parse().ok())
            .expect("the header names a total");
        let shown: usize = out
            .lines()
            .filter(|l| l.starts_with("- ") && l.contains(" tok ×"))
            .filter_map(|l| {
                l.trim_start_matches("- ")
                    .split(' ')
                    .next()?
                    .parse::<usize>()
                    .ok()
            })
            .sum();
        let rest: usize = out
            .lines()
            .find(|l| l.contains("more duplicated line(s)"))
            .and_then(|l| {
                let t = l.rsplit(',').next()?.trim();
                t.trim_end_matches(" tok.").parse().ok()
            })
            .expect("the remainder line names its tokens");

        assert!(shown > 0 && rest > 0, "one side of the sum is empty: {out}");
        assert_eq!(
            shown + rest,
            promised,
            "the listing does not add up to the total the header promises: \
             {shown} shown + {rest} hidden != {promised}\n{out}"
        );
    }

    #[test]
    fn render_lists_duplicates() {
        let shared = "This shared guideline line is duplicated across two files here.";
        let a = audit(&[
            ("a.md".to_string(), format!("{shared}\n")),
            ("b.md".to_string(), format!("{shared}\n")),
        ]);
        let r = render(&a);
        assert!(r.contains("tokens duplicated across files"));
        assert!(r.contains("files:"));
    }

    /// Owns its directory: the old helper left every fixture behind in the
    /// system temp dir (a RAM-backed tmpfs on this host), relying on the NEXT
    /// run of the same test to wipe it. Cleanup now happens at end of test.
    struct TmpDir(std::path::PathBuf);
    impl TmpDir {
        fn join(&self, p: &str) -> std::path::PathBuf {
            self.0.join(p)
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp_dir(name: &str) -> TmpDir {
        // The path used to be `tokenade-test-audit-{name}` — FIXED, shared by
        // every process on the host, and wiped with `remove_dir_all` on entry.
        // Two concurrent `cargo test` runs (routine on this machine) therefore
        // deleted each other's fixtures mid-test. pid + a process-wide counter
        // make it unique per invocation; `create_dir` fails loudly rather than
        // silently adopting someone else's directory.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!(
            "tokenade-test-audit-{name}-{}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir(&d).expect("test tempdir name must be unique");
        TmpDir(d)
    }

    #[test]
    fn apply_dedup_removes_shared_line_from_less_specific_file() {
        let dir = tmp_dir("remove-shared");
        let global = dir.join("global.md");
        let project = dir.join("project.md");

        let shared = "Always prefer tokenade MCP tools over raw grep and Read for queries.";
        std::fs::write(
            &global,
            format!("# Global\n{shared}\nglobal only line here\n"),
        )
        .unwrap();
        std::fs::write(
            &project,
            format!("# Project\n{shared}\nproject only line here\n"),
        )
        .unwrap();

        // global = index 0 (less specific), project = index 1 (more specific)
        let result = apply_dedup_entries(vec![
            (global.clone(), std::fs::read_to_string(&global).unwrap()),
            (project.clone(), std::fs::read_to_string(&project).unwrap()),
        ]);

        assert_eq!(
            result.files_modified.len(),
            1,
            "only global should be modified"
        );
        assert_eq!(result.files_modified[0], global.display().to_string());
        assert!(result.tokens_after < result.tokens_before, "tokens reduced");

        let global_after = std::fs::read_to_string(&global).unwrap();
        assert!(
            !global_after.contains(shared),
            "shared line removed from global"
        );
        assert!(
            global_after.contains("global only line"),
            "unique line preserved"
        );

        let project_after = std::fs::read_to_string(&project).unwrap();
        assert!(
            project_after.contains(shared),
            "shared line kept in project"
        );
    }

    /// #9: one file reached through two cascade spellings (a symlink) must
    /// not be deduplicated against itself — that stripped every user line.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_cascade_file_is_never_deduped_against_itself() {
        // Under /tmp, not $TMPDIR: a TMPDIR inside $HOME puts the developer's
        // real ~/.claude/CLAUDE.md in the fixture's ancestor chain.
        let dir = TmpDir(std::path::Path::new("/tmp").join(format!(
            "tokenade-test-audit-symlink-{}",
            std::process::id()
        )));
        std::fs::create_dir(&dir.0).unwrap();
        let home = dir.join("home");
        let proj = dir.join("proj");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(&proj).unwrap();
        let global = home.join(".claude/CLAUDE.md");
        let body = "# My rules\n\n## Testing\nAlways run the full test suite before committing.\n\n\n\n## Style\nPrefer early returns over nested conditionals.\n";
        std::fs::write(&global, body).unwrap();
        let link = proj.join("CLAUDE.md");
        std::os::unix::fs::symlink(&global, &link).unwrap();

        let r = apply_dedup_exact(&proj, Some(&home));
        assert!(r.files_modified.is_empty(), "{:?}", r.files_modified);
        assert_eq!(std::fs::read_to_string(&global).unwrap(), body);
        assert_eq!(read_cascade(&proj, Some(&home)).len(), 1);

        // Compaction writes THROUGH the link, never over it.
        apply_compress(&proj, Some(&home));
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        let after = std::fs::read_to_string(&global).unwrap();
        assert!(after.contains("Always run the full test suite") && !after.contains("\n\n\n"));
    }

    #[test]
    fn apply_dedup_is_idempotent() {
        let dir = tmp_dir("idempotent");
        let global = dir.join("global.md");
        let project = dir.join("project.md");

        let shared = "Always prefer tokenade MCP tools over raw grep and Read for queries.";
        std::fs::write(
            &global,
            format!("# Global\n{shared}\nglobal only line here\n"),
        )
        .unwrap();
        std::fs::write(
            &project,
            format!("# Project\n{shared}\nproject only line here\n"),
        )
        .unwrap();

        // First pass — cleans up.
        apply_dedup_entries(vec![
            (global.clone(), std::fs::read_to_string(&global).unwrap()),
            (project.clone(), std::fs::read_to_string(&project).unwrap()),
        ]);

        // Second pass — must be a no-op.
        let result2 = apply_dedup_entries(vec![
            (global.clone(), std::fs::read_to_string(&global).unwrap()),
            (project.clone(), std::fs::read_to_string(&project).unwrap()),
        ]);
        assert!(
            result2.files_modified.is_empty(),
            "idempotent: no rewrites on second pass"
        );
        assert_eq!(
            result2.tokens_before, result2.tokens_after,
            "no change in token count"
        );
    }

    #[test]
    fn apply_dedup_single_file_is_noop() {
        let dir = tmp_dir("single-file");
        let f = dir.join("only.md");
        std::fs::write(
            &f,
            "This is a line that appears only once in the cascade.\n",
        )
        .unwrap();
        let result = apply_dedup_entries(vec![(f.clone(), std::fs::read_to_string(&f).unwrap())]);
        assert!(result.files_modified.is_empty());
        assert_eq!(result.tokens_before, result.tokens_after);
    }

    /// Semantic dedup: two files carrying the same instruction written
    /// differently. Run with `cargo test -- --ignored` (embedding init ~200 ms).
    #[test]
    #[ignore = "semantic dedup: initialises the embedding (~200 ms)"]
    fn apply_dedup_removes_semantic_paraphrase_from_less_specific_file() {
        let dir = tmp_dir("semantic-paraphrase");
        let global = dir.join("global.md");
        let project = dir.join("project.md");

        // Same rule, different phrasing — embedding cosine should be ≥ 0.85.
        let global_line =
            "Always use tokenade MCP tools instead of raw grep or Read for codebase searches.";
        let project_line =
            "Prefer tokenade MCP tools over raw grep and Read when searching the codebase.";

        std::fs::write(
            &global,
            format!("# Global\n{global_line}\nglobal only line here\n"),
        )
        .unwrap();
        std::fs::write(
            &project,
            format!("# Project\n{project_line}\nproject only line here\n"),
        )
        .unwrap();

        let result = apply_dedup_entries(vec![
            (global.clone(), std::fs::read_to_string(&global).unwrap()),
            (project.clone(), std::fs::read_to_string(&project).unwrap()),
        ]);

        // At least one file must be modified.
        assert!(
            !result.files_modified.is_empty(),
            "expected semantic dedup to modify at least one file"
        );
        assert!(
            result.tokens_after < result.tokens_before,
            "tokens must decrease after semantic dedup"
        );

        // The global (less-specific) file must have lost its line.
        let global_after = std::fs::read_to_string(&global).unwrap();
        assert!(
            !global_after.contains(global_line),
            "paraphrase must be removed from less-specific file"
        );
        assert!(
            global_after.contains("global only line"),
            "unique line must be preserved"
        );
    }

    #[test]
    fn rules_files_for_agent_claude_code_matches_cascade() {
        let home = PathBuf::from("/home/u");
        let start = PathBuf::from("/home/u/proj");
        let cc = rules_files_for_agent(&crate::detect::Agent::ClaudeCode, &start, Some(&home));
        let expected = cascade_paths(&start, Some(&home));
        assert_eq!(cc, expected);
    }

    #[test]
    fn rules_files_for_agent_cursor_uses_cursorrules() {
        let home = PathBuf::from("/home/u");
        let start = PathBuf::from("/home/u/proj");
        let paths = rules_files_for_agent(&crate::detect::Agent::Cursor, &start, Some(&home));
        assert!(paths.iter().any(|p| p.ends_with(".cursorrules")));
        assert!(!paths.iter().any(|p| p.ends_with("CLAUDE.md")));
    }

    #[test]
    fn rules_files_for_agent_copilot_uses_github_dir() {
        let start = PathBuf::from("/home/u/proj");
        let paths = rules_files_for_agent(&crate::detect::Agent::Copilot, &start, None);
        assert!(paths
            .iter()
            .any(|p| { p.ends_with(".github/copilot-instructions.md") }));
    }

    #[test]
    fn rules_files_for_agent_codex_uses_agents_md() {
        let home = PathBuf::from("/home/u");
        let start = PathBuf::from("/home/u/proj");
        let paths = rules_files_for_agent(&crate::detect::Agent::Codex, &start, Some(&home));
        assert!(paths.iter().any(|p| p.ends_with("AGENTS.md")));
    }

    #[test]
    fn rules_files_for_agent_no_duplicates_across_ancestors() {
        let home = PathBuf::from("/home/u");
        let start = PathBuf::from("/home/u/proj/sub");
        let paths = rules_files_for_agent(&crate::detect::Agent::Cline, &start, Some(&home));
        // All paths are unique.
        let mut seen = std::collections::HashSet::new();
        for p in &paths {
            assert!(seen.insert(p.clone()), "duplicate path: {}", p.display());
        }
    }
}
