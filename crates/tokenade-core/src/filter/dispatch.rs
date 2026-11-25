//! **The big switch: which compactor for this command.**
//!
//! A single `match`, six hundred lines, and on purpose — the ORDER of the arms
//! carries meaning, every exception is commented in place, and `build.rs` reads
//! this file to extract `FILTER_ARM_PROGS`, the exact set of programs an arm
//! names. Splitting this match into scattered tables would lose both.

use super::*;

/// The per-family dispatch match, moved verbatim out of
/// `filter_command_inner`. Routes by `(prog, sub)` to the right
/// compactor, returning `Ok((name, compacted))` for the arms that
/// produce a `(name, String)` pair, or `Err(result)` for the arms that
/// early-return a fully-built `FilterResult` (passthrough/auto paths).
/// Every arm is identical to the original; the only change is wrapping
/// the two terminal forms in `Ok`/`Err`.
pub(super) fn dispatch_by_command(
    prog: &str,
    sub: &str,
    argv: &[String],
    output: &str,
    before: usize,
) -> Result<(&'static str, String), FilterResult> {
    // Re-own `prog` so the arm bodies keep their verbatim `prog.as_str()`
    // / `prog.to_string()` calls (and the build-script scan still finds
    // the literal `match (prog.as_str(), sub)` anchor it keys on).
    let prog = prog.to_string();
    let pair: (&'static str, String) = match (prog.as_str(), sub) {
        ("git", "status") => ("git-status", crate::cmd::linux::git::status::dispatch(argv, output)),
        ("git", "diff") => ("git-diff", crate::cmd::linux::git::diff::dispatch(argv, &stat_view(argv, output))),
        ("git", "log") => ("git-log", crate::cmd::linux::git::log::dispatch(argv, &stat_view(argv, output))),
        ("git", "branch") => ("git-branch", crate::cmd::linux::git::branch::dispatch(argv, output)),
        ("git", "stash") => ("git-stash", crate::cmd::linux::git::stash::dispatch(argv, output)),
        ("git", "tag") => ("git-tag", crate::cmd::linux::git::tag::compact(output)),
        ("git", "remote") => ("git-remote", crate::cmd::linux::git::remote::dispatch(argv, output)),
        // `git show <rev>:<path>` prints a FILE, not a commit: the commit
        // compactor sampled it into scattered lines (measured 2026-10-03 on a
        // 523-line `.rs`: header, then 92 runs of 5 lines out of context).
        // Read it as `cat <path>` reads the same file.
        ("git", "show") => match git_show_blob_path(argv, output) {
            Some(blob) => read_of_a_file("git-show", &["git".to_string(), blob], output, before)?,
            None => ("git-show", crate::cmd::linux::git::show::compact(&stat_view(argv, output))),
        },
        ("git", "blame") => ("git-blame", crate::cmd::linux::git::blame::compact(output)),
        // `git grep` prints exactly what `grep -rn` prints (`path:line:text`).
        // It went to a generic table row — head 80, no file census, and the
        // failure rescue that reads `error` in a RESULT as a failure: measured
        // 2026-10-03 on `git grep -n 'fn '` (683 hits, 50 files), 38 files
        // were named nowhere. Same compactor as every other grep.
        ("git", "grep") => ("git-grep", crate::cmd::linux::grep_dispatcher::dispatch(&argv[1..], output)),
        ("git", "config") => ("git-config", crate::cmd::linux::git::config::compact(output)),
        ("git", "reflog") => ("git-reflog", crate::cmd::linux::git::reflog::compact(output)),
        ("git", "cherry-pick") | ("git", "revert") | ("git", "merge") | ("git", "rebase") => {
            ("git-rebase-like", crate::cmd::linux::git::rebase_like::compact(output))
        }
        ("git", "fetch") | ("git", "pull") | ("git", "push") => {
            ("git-net", crate::cmd::linux::git::net::compact(output))
        }
        ("git", "clone") => ("git-clone", crate::cmd::linux::git::net::compact(output)),
        // Git's LISTING subcommands. Each of these already had a compactor
        // written for it — they were simply never reached, because the
        // dispatcher had no arm and the output fell through to the generic
        // pass. `git ls-files` alone is 23 k tokens on this repo, and it is
        // what an agent runs to see a project, so it runs often.
        //
        // All of them fold a path list into a directory census, and all of
        // them return `None` on a listing short enough to print whole — hence
        // the fall-through instead of an unconditional fold.
        ("git", "ls-files") => folded_or_fallthrough("git-ls-files", crate::cmd::linux::git_ls_files::try_compact(output), output, before)?,
        ("git", "ls-tree") => folded_or_fallthrough("git-ls-tree", crate::cmd::linux::git_ls_tree::try_compact(output), output, before)?,
        ("git", "rev-list") => folded_or_fallthrough("git-rev-list", crate::cmd::linux::git_rev_list::try_compact(output), output, before)?,
        ("git", "worktree") => folded_or_fallthrough("git-worktree", crate::cmd::linux::git_worktree::try_compact(output), output, before)?,
        // `git lfs ls-files` — the subcommand is `lfs`, the shape is a path
        // list with an object id per line, so it needs its own reader.
        ("git", "lfs") if argv.iter().any(|a| a == "ls-files") => {
            folded_or_fallthrough(
                "git-lfs-ls-files",
                crate::cmd::linux::git_lfs_ls_files::try_compact(output),
                output,
                before,
            )?
        }
        // `git submodule` splits by its own second word: `foreach` prints one
        // block per submodule, `status` one line per submodule.
        ("git", "submodule") => {
            let foreach = argv.iter().any(|a| a == "foreach");
            let folded = if foreach {
                crate::cmd::linux::git_submodule_foreach::try_compact(output)
            } else {
                crate::cmd::linux::git_submodule_status::try_compact(output)
            };
            let name = if foreach {
                "git-submodule-foreach"
            } else {
                "git-submodule-status"
            };
            folded_or_fallthrough(name, folded, output, before)?
        }
        ("cargo", "build") | ("cargo", "check") | ("cargo", "clippy") => {
            ("cargo-build", crate::cmd::linux::cargo::build::dispatch(argv, output))
        }
        ("cargo", "test") => ("cargo-test", crate::cmd::linux::cargo::test::dispatch(argv, output)),
        ("cargo", "install") => ("cargo-install", crate::cmd::linux::cargo::install::compact(output)),
        ("cargo", "run") => ("cargo-run", crate::cmd::linux::cargo::run::compact(output)),
        ("cargo", "bench") => ("cargo-bench", crate::cmd::linux::cargo::bench::compact(output)),
        ("cargo", "doc") => ("cargo-doc", crate::cmd::linux::cargo::build::compact(output)),
        ("cargo", "fmt") => ("cargo-fmt", crate::cmd::linux::cargo::fmt::compact(output)),
        ("cargo", "update") => ("cargo-update", crate::cmd::linux::cargo::update::compact(output)),
        ("cargo", "clean") | ("cargo", "new") | ("cargo", "init") => {
            let s = strip_ansi(output);
            ("cargo-misc", s.trim().to_string())
        }
        ("cargo", "tree") => ("cargo-tree", crate::cmd::linux::cargo::tree::dispatch(argv, output)),
        ("cargo", "search") => ("cargo-search", crate::cmd::linux::cargo::search::compact(output)),
        ("pytest", _) | ("py.test", _) => ("pytest", crate::cmd::linux::pytest_dispatcher::dispatch(argv, output)),
        // Node package managers + Bun (JS runtime + pkg mgr). bun's
        // install output is npm-shaped enough (`installed N packages`,
        // `[XXms]` timing) that npm_dispatch's dedup + log compaction
        // applies cleanly; on bun-specific patterns it degrades to
        // passthrough rather than mangling output.
        ("npm", _) | ("pnpm", _) | ("yarn", _) | ("bun", _) => {
            ("npm", crate::cmd::linux::npm_dispatcher::dispatch(argv, output))
        }
        // JVM / non-cargo build systems. Each has its own quirks but
        // they all converge on the same "Compiling X" + diagnostic
        // shape — variants handled per build tool.
        ("make", _) | ("gmake", _) => ("make", crate::cmd::linux::make_dispatcher::dispatch(argv, output)),
        ("gradle", _) | ("gradlew", _) | ("./gradlew", _) => {
            ("gradle", crate::cmd::linux::gradle_dispatcher::dispatch(argv, output))
        }
        ("mvn", _) | ("maven", _) | ("./mvnw", _) | ("mvnw", _) => {
            ("mvn", crate::cmd::linux::mvn_dispatcher::dispatch(argv, output))
        }
        ("dotnet", _) => ("dotnet", crate::cmd::linux::dotnet_dispatcher::dispatch(argv, output)),
        // `wt` (worktrunk) relays its hooks' output whole: each hook goes
        // through its OWN command's compactor, which this layer owns and passes
        // down (the formats layer must not name it).
        ("wt", _) | ("git-wt", _) if crate::cmd::linux::worktrunk::detect(output) => (
            "wt",
            crate::cmd::linux::worktrunk::compact_with(output, &|cmd, body| {
                let argv: Vec<String> =
                    crate::command_rewrite::meaningful_segment(cmd).split_whitespace().map(String::from).collect();
                filter_command_inner(&argv, body).output
            }),
        ),
        // Linters / formatters — variant-aware: --json / --format=junit /
        // --quiet / --output-format change shape across the family.
        // Modern additions (2026): biome (eslint+prettier replacement),
        // oxlint (Rust-based ESLint alt, growing fast), actionlint
        // (GitHub Actions yaml linter), cspell (spell checker).
        ("ruff", _) | ("eslint", _) | ("tslint", _) | ("prettier", _) | ("black", _)
        | ("isort", _) | ("flake8", _) | ("mypy", _) | ("pylint", _) | ("rubocop", _)
        | ("golangci-lint", _) | ("clippy-driver", _) | ("rustfmt", _)
        | ("shellcheck", _) | ("yamllint", _) | ("markdownlint", _)
        | ("biome", _) | ("oxlint", _) | ("actionlint", _) | ("cspell", _) => {
            ("linter", crate::cmd::linux::linter_dispatcher::dispatch(argv, output))
        }
        // Text utilities — tend to be Claude's most-used Bash class
        // (per transcript mining: tail 13k×, head 12k×). Already cheap
        // by nature, but generic's dedup+truncate still wins on huge
        // greps/sorts. The label helps the dashboard split them out.
        ("ls", _) => ("ls", crate::cmd::linux::ls_dispatcher::dispatch(argv, output)),
        // File / path search — all share grep-like `path:[line]:content`
        // or pure path-list output, so the same find_dispatch handles them.
        // `locate`/`mlocate`/`plocate` produce one path per line.
        ("find", _)
        | ("fd", _)
        | ("fdfind", _)
        | ("locate", _)
        | ("mlocate", _)
        | ("plocate", _) => ("find", crate::cmd::linux::find_dispatcher::dispatch(argv, output)),
        // Content search — same output shape (one match per line,
        // optionally `file:line:content`). Variants of grep all route
        // through grep_dispatch.
        ("grep", _)
        | ("egrep", _)
        | ("fgrep", _)
        | ("zgrep", _)
        | ("rg", _)
        | ("ripgrep", _)
        | ("ug", _)
        | ("ugrep", _)
        | ("ag", _)
        | ("ack", _)
        | ("ack-grep", _) => ("grep", crate::cmd::linux::grep_dispatcher::dispatch(argv, output)),
        // Process search — `pgrep` lists PIDs (optionally with `-a`
        // for argv). Goes through its own compactor (already exists at
        // cmd/linux/pgrep.rs but isn't auto-routed yet).
        ("pgrep", _) | ("pkill", _) => ("pgrep", crate::cmd::linux::pgrep::compact(output)),
        // Stream filters: their output is the *transformed content* of an
        // arbitrary input — they impose no command-specific shape. So the
        // most specific thing we can do is sniff the CONTENT's format and
        // hand it to that format's compactor (JSON / log / table / diff /
        // csv / …), exactly like `cat`/`bat` already do (#502). When auto
        // detects nothing it falls back to the same dedup+truncate the old
        // `text_util` pass did, so this is strictly ≥ the previous routing.
        //
        // These only reach here as argv[0] DIRECT (`awk '…' file`,
        // `sed '…' file`): the `user_wants_raw` SAFETY NET 0 above already
        // passthroughs the pipe / downstream-transform shapes. The
        // head/tail blind-slice advisory is emitted separately on stderr
        // (`emit_head_slice_advisory`), so rerouting the stdout here does
        // not affect it.
        // A LARGE ranged read of a source file — `sed -n '200,600p'
        // src/foo.rs`, `head -400 src/foo.rs` — is an exploratory read
        // wearing a stream filter's clothes. It fell through to the generic
        // pass, which has nothing to say about code: field data showed ~12 %
        // saved on 300-line slices that the Read hook would have stripped to
        // a third. Route it through the SAME `code_strip(Comments)` that
        // `cat` (#502) and the Read pretool already apply.
        //
        // Gated on size, and that gate is the whole point: a 20-line slice is
        // an EDIT TARGET, and an edit target must come back byte-exact. Only
        // above [`RANGED_READ_MIN_TOKENS`] is the read unambiguously a browse.
        // A byte-precise `head -c` / `tail --bytes` is not a ranged read and
        // not a browse: it is a request for exact bytes. See
        // [`head_tail_byte_precise`].
        (prog @ ("sed" | "head" | "tail" | "awk"), _)
            if cat_argv_has_source_extension(argv)
                && !head_tail_byte_precise(argv)
                && (prog != "awk" || awk_only_selects_lines(argv)) =>
        {
            // Below the threshold the slice is a SURGICAL read and comes back
            // byte-exact — `RANGED_READ_MIN_TOKENS` said so, and the guard of
            // this arm did not keep the promise: a small slice fell through
            // to the `sed` arm below, i.e. to content detection. Spotted in
            // the registry (`sed -n '1540,1570p;…' src/store/schema.rs`, op
            // `auto:sql-source`) and reproduced: 68 lines of a `.rs` file
            // delivered as `[tokenade:sql] comments stripped`, the six `--`
            // lines removed being the INSIDE of a Rust string literal — the
            // bytes an agent then quotes in its next `Edit`.
            // Returned as `passthrough`, like the normal `diff` below: under a
            // command name, the "folded nothing" nets hand an unchanged
            // answer to `generic`, which rewrites even a small input.
            if before <= RANGED_READ_MIN_TOKENS {
                return Err(FilterResult {
                    filter: "passthrough".to_string(),
                    output: strip_ansi(output),
                    savings: Savings::new(before, before),
                });
            }
            let name = match prog {
                "sed" => "sed",
                "head" => "head",
                "awk" => "awk",
                _ => "tail",
            };
            match cat_try_code_strip(argv, output) {
                Some(stripped) => (name, stripped),
                // No rules for this language, nothing to strip, or a source
                // read that fits and keeps its comments: the bytes
                // are still source. Returned as `passthrough` — an unchanged
                // answer under a command name is handed to `generic`, which
                // dropped 66 `///` lines of `sed -n 42,420p detect.rs` silently.
                None => {
                    return Err(FilterResult {
                        filter: "passthrough".to_string(),
                        output: strip_ansi(output),
                        savings: Savings::new(before, before),
                    })
                }
            }
        }
        // A NORMAL diff (`diff a b`) is nothing but changes. Not named, it went
        // to content detection, which saw the `///` comments of the files
        // compared and folded it as MARKDOWN: measured 2026-10-02, a 185-line
        // diff delivered with 31 changed lines elided (`auto:markdown`), and
        // under 180 lines `cmd::linux::diff` cut its middle (12 of 64). A
        // passthrough has to be an arm: `dispatch_named` refuses any result
        // that is not SMALLER than its input. And it carries the `passthrough`
        // label, like the byte-precise `head`/`tail` below: under a command
        // name, `prefer_the_fallback_when_the_name_folded_nothing` would hand
        // an unchanged answer straight back to the markdown fold.
        // `diff -y` is the whole file side by side: fold the unchanged rows,
        // keep every marked one. See `fold_side_by_side`.
        ("diff", _) if crate::cmd::linux::diff::asks_side_by_side(argv) => {
            if let Some(out) = crate::cmd::linux::diff::fold_side_by_side(output) {
                let after = tokenizer::estimate(&out);
                if after < before {
                    return Err(FilterResult {
                        filter: "diff".into(),
                        output: out,
                        savings: Savings::new(before, after),
                    });
                }
            }
            return Err(FilterResult {
                filter: "passthrough".into(),
                output: output.to_string(),
                savings: Savings::new(before, before),
            });
        }
        ("diff", _) if crate::cmd::linux::diff::served_whole(output) => {
            return Err(FilterResult {
                filter: "passthrough".into(),
                output: output.to_string(),
                savings: Savings::new(before, before),
            });
        }
        // Byte-precise first, and BEFORE the generic arm below: the fold there
        // is recoverable but still not the bytes that were asked for.
        ("head" | "tail", _) if head_tail_byte_precise(argv) => {
            return Err(FilterResult {
                filter: "passthrough".into(),
                output: output.to_string(),
                savings: Savings::new(before, before),
            });
        }
        ("tail", _) | ("head", _) | ("sort", _) | ("uniq", _) | ("wc", _)
        | ("cut", _) | ("tr", _) | ("awk", _) | ("sed", _) | ("xargs", _) => {
            return Err(auto_or_generic(output, before));
        }
        // Content-transparent commands — the bytes they emit are NOT their
        // own diagnostic shape but the *content flowing through them*: `tee`
        // copies stdin verbatim, `perl -pe/-ne` is a line filter, `base64`
        // en/decodes, `pdftotext` dumps a PDF's prose. Interpreters belong
        // here too: `python`/`python3`, `npx` and `fish` print whatever the
        // *script they run* emits — there is no "python-shaped" output to
        // key on, only the JSON / log / table / traceback the program itself
        // produced. A command-specific compactor here can only be wrong (it
        // would impose tee/perl/python semantics on the content passing
        // through). The correct command-specific behaviour is therefore to
        // compact by the content's OWN detected format — exactly what
        // `auto_or_generic` does (JSON→json, log→log, table→table, else
        // dedup/passthrough). OS-common: covers Linux, macOS and Windows in
        // one place.
        // A test runner launched through a WRAPPER that masks it from the
        // program-name routing: `python -m unittest`, `npx jest`, `bundle exec
        // rspec`, `poetry run pytest`, `pnpm dlx vitest`, … In each case `prog`
        // is the wrapper (interpreter / package-runner), which has no output
        // shape of its own — but the runner does, and `auto_or_generic` would
        // never reach a test compactor (unittest in particular gets mis-routed
        // to the stacktrace folder and comes out disordered). Resolve the real
        // runner and route to its dedicated compactor. Must precede the generic
        // interpreter arm AND the pkgmgr arm (`bundle`/`poetry`/`uv`/…) below.
        // Generalises the original `python -m unittest` fix across every
        // language's wrapper, not just Python.
        (_, _) if wrapped_test_runner(argv).is_some() => {
            route_wrapped_runner(wrapped_test_runner(argv).unwrap(), argv, output)
        }
        // `-layout` prints a blank line under every table row (cell padding)
        // and aligns columns with spaces the content route later collapses:
        // padding dropped, columns made `| a | b |`, then the content route.
        ("pdftotext", _) => {
            let (tight, _) = tk_formats::pdf_text::tighten_layout(output);
            if tight != output {
                let mut r = auto_or_generic(&tight, before);
                r.savings = crate::Savings::new(before, crate::tokenizer::estimate(&r.output));
                return Err(r);
            }
            return Err(auto_or_generic(output, before));
        }
        ("tee", _) | ("perl", _) | ("base64", _)
        | ("python", _) | ("python3", _) | ("npx", _) | ("fish", _) => {
            return Err(auto_or_generic(output, before));
        }
        // `gh` (GitHub CLI) — uncovered yet high tokens/event in the wild
        // (`gh api` returns raw GitHub REST JSON; `gh pr/issue/run list|view`
        // returns tables/text). Both are content-shaped, so for `gh api`
        // strip the REST url-zoo chrome first (same lever as the GitHub MCP),
        // then content-fold whatever shape is left. Other subcommands fall
        // straight through to the content-aware fold.
        ("gh", _) => {
            if let Some(stripped) = crate::mcp_compact::rest_api::compact(output) {
                return Err(auto_or_generic(&stripped, before));
            }
            // 58 `gh_*` subcommand specs sat behind this unconditional return.
            let sub = argv
                .iter()
                .skip(1)
                .find(|a| !a.starts_with('-'))
                .map(String::as_str)
                .unwrap_or("");
            if let Some(hit) = subcommand_spec("gh", sub, output, before) {
                return Err(hit);
            }
            return Err(auto_or_generic(output, before));
        }
        // `ssh [opts] host <remote command…>` run via the Bash tool is
        // NON-interactive (a bare `ssh host` would need a TTY and produces no
        // captured output here). The remote command's stdout is exactly what
        // tokenade compacts — route it to THAT command's own compactor, so
        // `ssh vps 'docker ps'` gets the docker fold, `ssh vps journalctl …`
        // the systemd/log fold, `ssh vps df -h` the df fold, etc. Heavy lever
        // for ops/deploy users (a VPS-deploy session showed ~300 ssh calls all
        // passing through uncompacted). Bare host (no remote command) → the
        // generic path is a safe no-op on the empty output.
        ("ssh", _) => {
            return Err(match ssh_remote_argv(argv) {
                Some(remote) if !remote.is_empty() => filter_command_inner(&remote, output),
                _ => auto_or_generic(output, before),
            });
        }
        // #502 — `cat`/`bat`/`batcat` of a source file. Apply
        // `code_strip(Comments)` when argv carries a code_strip-known
        // extension AND it actually shrinks the output. Otherwise
        // fall through to `auto_or_generic` so JSON / YAML / diff /
        // … still get their format-specific compactors.
        //
        // #590 — extension: when argv has a known SOURCE-CODE
        // extension (e.g. `.ps1`, `.lua`, `.pl`, `.r`, `.fish`,
        // `.zig`) that `code_strip` doesn't have rules for, return
        // raw bytes instead of falling through to auto::compact.
        // This mirrors the #570 Read pretool fix on the Bash path:
        // PowerShell scripts no longer get mis-routed to the cmake/
        // env/ini compactors when none of the code_strip languages
        // matched. The text-shaped formats (JSON / YAML / diff /
        // log) still hit `auto_or_generic` because their extensions
        // are NOT in `is_source_code_extension`.
        // PowerShell / cmd.exe file dumps route EXACTLY like `cat`: the
        // dominant uncovered Windows op in field data was `Get-Content`
        // (112 events for a single user, all through the weak generic
        // fallback). `gc`/`type` are its stock aliases; PowerShell is
        // case-insensitive so the prog is matched lowercased here (the
        // dispatch scrutinee is the raw token — normalize in the guard).
        (ps_cat, _) if matches!(ps_cat.to_ascii_lowercase().as_str(), "get-content" | "gc" | "type") => {
            let content: String = match cat_try_code_strip(argv, output) {
                Some(stripped) => stripped,
                None => {
                    if cat_argv_has_source_extension(argv) {
                        strip_ansi(output)
                    } else {
                        return Err(auto_or_generic(output, before));
                    }
                }
            };
            let ct = tokenizer::estimate(&content);
            ("get-content", cat_truncate_keeping_banner(argv, content, ct))
        }
        // `Select-String` = PowerShell grep; same folding as grep/rg.
        (ps_grep, _) if ps_grep.eq_ignore_ascii_case("select-string") || ps_grep.eq_ignore_ascii_case("sls") => {
            ("grep", crate::cmd::linux::grep_dispatcher::dispatch(argv, output))
        }
        // `nl FILE` is `cat -n FILE`: a numbered read of a file. It went to the
        // `nl` spec — a TABLE fold that put "problem" lines first (code lines
        // that merely contain `error`) and kept 50 lines of head and 5 of tail.
        // Measured 2026-10-02 on `filter/fallback.rs`: 485 lines → 3 234
        // bytes with the whole middle gone, where `cat -n` on the same file
        // keeps every definition (5 737 bytes, comments stripped and said).
        ("cat", _) | ("bat", _) | ("batcat", _) | ("nl", _) => {
            let label = if prog == "nl" { "nl" } else { "cat" };
            read_of_a_file(label, argv, output, before)?
        }
        // `env` is two commands sharing a name, and only one of them is a
        // wrapper. See [`env_is_the_dump`]: this arm gives the dump the exact
        // route `printenv` takes — its own compactor when that is smaller, the
        // content detector otherwise.
        ("env", _) if env_is_the_dump(argv) => {
            if let Some(c) = crate::cmd::dispatch_named("env", output) {
                let after = tokenizer::estimate(&c);
                if after < before {
                    return Err(FilterResult {
                        filter: "env".to_string(),
                        output: c,
                        savings: Savings::new(before, after),
                    });
                }
            }
            return Err(auto_or_generic(output, before));
        }
        // `corepack pnpm install` IS pnpm — corepack resolves the pinned
        // version and execs it. Only when it names a package manager:
        // `corepack enable` / `prepare` are corepack's own commands, which is
        // why it is an arm and not a `WRAPPER_PROGS` entry (that list would
        // label `corepack enable` as a program called `enable`). Fleet
        // 2026-10-02: `corepack.js` 37 events reported uncovered.
        ("corepack", "pnpm" | "pnpx" | "yarn" | "yarnpkg" | "npm" | "npx") => {
            return Err(filter_wrapped(argv, output, before));
        }
        // Wrappers — look at the WRAPPED command's first word. `sudo
        // cargo build` should route as `cargo`, not as `sudo`.
        ("sudo", _) | ("sshpass", _) | ("timeout", _) | ("nice", _) | ("ionice", _)
        | ("chrt", _) | ("env", _) | ("nohup", _) | ("setsid", _) | ("eatmydata", _)
        // `command CMD args` (and `command -p CMD`) is a shell builtin that
        // runs CMD bypassing functions/aliases — its output IS that of the
        // wrapped command. Route by the wrapped word, same as `sudo`/`env`.
        | ("command", _) => {
            return Err(filter_wrapped(argv, output, before));
        }
        // System info utilities — variant aware.
        ("df", _) | ("du", _) | ("ps", _) | ("stat", _) | ("free", _) | ("uptime", _)
        | ("lsof", _) | ("netstat", _) | ("ss", _) | ("ip", _) | ("ifconfig", _) => {
            ("sysinfo", crate::cmd::linux::sysinfo_dispatcher::dispatch(prog.as_str(), argv, output))
        }
        // VCS beyond git.
        ("hg", _) | ("svn", _) | ("fossil", _) => {
            return Err(auto_or_generic_labelled(output, before, "vcs"));
        }
        // Docker / podman / buildah / skopeo — handled by docker_dispatch
        // which routes by subcommand (ps / images / logs / inspect / …).
        // `docker exec <ctr> CMD` / `docker compose exec <svc> CMD` /
        // `kubectl exec <pod> -- CMD`: the OUTPUT belongs to the INNER
        // command, not to docker/kubectl — route it to that command's own
        // compactor, exactly like the `ssh host 'cmd'` lever (same field
        // pattern: one ops user ran 193 remote-exec calls, all landing in
        // the generic fallback).
        ("docker", "exec") | ("podman", "exec") => {
            return Err(match container_exec_remote_argv(argv, 1) {
                Some(remote) if !remote.is_empty() => filter_command_inner(&remote, output),
                _ => auto_or_generic(output, before),
            });
        }
        ("docker", "compose") if argv.iter().any(|a| a == "exec" || a == "run") => {
            return Err(match compose_exec_remote_argv(argv) {
                Some(remote) if !remote.is_empty() => filter_command_inner(&remote, output),
                _ => auto_or_generic(output, before),
            });
        }
        ("kubectl", "exec") => {
            // kubectl exec pod [-c ctr] -- CMD…  (everything after `--`)
            return Err(match argv.iter().position(|a| a == "--") {
                Some(dd) if argv.len() > dd + 1 => {
                    filter_command_inner(&argv[dd + 1..], output)
                }
                _ => auto_or_generic(output, before),
            });
        }
        ("docker", _) | ("podman", _) | ("buildah", _) | ("skopeo", _) => {
            ("docker", crate::cmd::linux::docker::dispatch(argv, output))
        }
        // Kubernetes runtimes — handled by kubectl_dispatch.
        ("kubectl", _) | ("k9s", _) | ("minikube", _) | ("kind", _) | ("k3s", _)
        | ("helm", _) | ("nomad", _) | ("consul", _) | ("vault", _) => {
            ("kubectl", crate::cmd::linux::kubectl::dispatch(argv, output))
        }
        // Cloud CLIs — variant aware (most accept --output json/text/table).
        ("aws", _) | ("az", _) | ("gcloud", _) | ("gsutil", _)
        | ("terraform", _) | ("tofu", _) | ("pulumi", _) | ("cdk", _) | ("serverless", _) | ("sam", _) => {
            ("cloud", crate::cmd::linux::cloud_dispatcher::dispatch(argv, output))
        }
        // Databases — variant aware.
        ("psql", _) | ("mysql", _) | ("sqlite3", _) | ("mongo", _) | ("mongosh", _)
        | ("redis-cli", _) | ("influx", _) | ("clickhouse-client", _) => {
            ("db", crate::cmd::linux::db_dispatcher::dispatch(prog.as_str(), argv, output))
        }
        // JSON/YAML processors — variant aware: jq's --compact / --raw
        // / --slurp change behaviour without changing output SHAPE
        // much, so most paths route to auto (JSON-detection). The
        // `--null-input` + missing-key combo can yield "null" which
        // we want to surface rather than swallow as empty.
        ("jq", _) | ("yq", _) | ("fx", _) | ("jp", _) => {
            ("jsonproc", crate::cmd::linux::jq_dispatcher::dispatch(argv, output))
        }
        // Network tools — variant aware: curl -v adds verbose
        // transcript, dig +short shrinks to one IP, etc.
        ("curl", _) | ("wget", _) => ("network", crate::cmd::linux::curl_dispatcher::dispatch(argv, output)),
        ("dig", _) | ("nslookup", _) | ("host", _) => {
            ("network", crate::cmd::linux::dig_dispatcher::dispatch(argv, output))
        }
        ("ping", _) | ("ping6", _) | ("traceroute", _) | ("tracepath", _) | ("mtr", _) => {
            return Err(named_or_generic(prog.as_str(), output, before, "network"));
        }
        // L88 + L89 — mount / lsblk (no JSON flags, straight per-cmd).
        ("mount", _) | ("lsblk", _) => {
            return Err(named_or_generic(prog.as_str(), output, before, "sysinfo"));
        }
        // Windows utilities — same per-command dispatch.
        ("ipconfig", _) | ("tasklist", _) => {
            return Err(named_or_generic(prog.as_str(), output, before, "sysinfo"));
        }
        // Other build / tooling commands users invoke from RTK's list.
        ("just", _) | ("task", _) | ("bazel", _) | ("buck", _) | ("ninja", _)
        | ("cmake", _) | ("ant", _) | ("sbt", _) | ("lein", _) | ("mix", _) => {
            return Err(auto_or_generic_labelled(output, before, "build-other"));
        }
        // Test runners from the JS/Ruby/PHP world — variant aware. These are
        // the same standalone runners `WRAPPED_RUNNERS` resolves behind a
        // wrapper (`npx jasmine`, `bundle exec rspec`); listing them here too
        // means a DIRECT invocation (`jasmine`, `ava`, `tap`) routes
        // identically rather than falling to `auto_or_generic` — same runner,
        // same compaction, whether or not a wrapper masked it.
        ("jest", _) | ("mocha", _) | ("vitest", _) | ("cypress", _) | ("playwright", _)
        | ("jasmine", _) | ("ava", _) | ("tap", _)
        | ("rspec", _) | ("minitest", _) | ("phpunit", _) | ("pest", _) => {
            ("test-runner", crate::cmd::linux::test_runner_dispatcher::dispatch(prog.as_str(), argv, output))
        }
        // Native test subcommands the program-name routing can't reach on its
        // own: `go test`'s prog is `go` (not the `go_test` compactor module),
        // and `cargo nextest` has no `("cargo","nextest")` arm — both would
        // fall through to `auto_or_generic` and get mis-routed (stacktrace /
        // generic) instead of their dedicated block parser. Route them
        // explicitly. The rest (deno/node/swift/dart/flutter) have no dedicated
        // compactor, so they go through the line-based test engine
        // (`test_runner_dispatch` → pytest): it preserves order (folds passing
        // lines, keeps failures + summary), strictly better than the generic
        // deduper. `-v`/`--verbose` already passthrough via `user_wants_raw`.
        ("go", "test") => ("go-test", crate::cmd::linux::go_test::compact(output)),
        ("cargo", "nextest") => ("cargo-nextest", crate::cmd::linux::cargo_nextest::compact(output)),
        ("deno", "test") => ("test-runner", crate::cmd::linux::test_runner_dispatcher::dispatch("deno", argv, output)),
        ("swift", "test") => ("test-runner", crate::cmd::linux::test_runner_dispatcher::dispatch("swift", argv, output)),
        // The Dart analyzer (529 diagnostics, 121 KB on a real app) and
        // `pub get`'s dependency list: `cmd::linux::dart_dispatcher`.
        ("flutter", "analyze") | ("dart", "analyze") => match crate::cmd::linux::dart_dispatcher::compact_analyze(output) {
            Some(c) => ("dart-analyze", c),
            None => return Err(auto_or_generic(output, before)),
        },
        ("flutter", "pub") | ("dart", "pub") => match crate::cmd::linux::dart_dispatcher::compact_pub(output) {
            Some(c) => ("dart-pub", c),
            None => return Err(auto_or_generic(output, before)),
        },
        ("dart", "test") | ("flutter", "test") => {
            ("test-runner", crate::cmd::linux::test_runner_dispatcher::dispatch(prog.as_str(), argv, output))
        }
        // Node's built-in runner (`node --test`) emits TAP; route it the same
        // way. Bare `node script.js` (no `--test`) falls through to `_`.
        ("node", _) if has_flag(argv, "--test") => {
            ("test-runner", crate::cmd::linux::test_runner_dispatcher::dispatch("node", argv, output))
        }
        // Remaining interpreters / runtimes — same content-transparent class
        // as `python`/`npx` above, and for the same reason: what they print is
        // the *script's* output (JSON / log / table / stack trace), never a
        // shape of their own, so the only correct command-specific behaviour
        // is to compact by the content's OWN detected format. They sit HERE
        // rather than in that arm because their test subcommands must match
        // first: `deno test` and `node --test` are routed just above, and
        // `php artisan test` / `php vendor/bin/phpunit` via the
        // `wrapped_test_runner` guard. `bun` is absent on purpose — it is a
        // package manager here (npm arm), not an interpreter.
        // `php artisan <cmd>`: Laravel's dot leaders (`test` is routed above).
        ("php", _) if argv.get(1).map(|s| basename(s)).as_deref() == Some("artisan") => {
            ("artisan", crate::cmd::linux::artisan::compact(output))
        }
        // A crashed script prints a JavaScript stack whose runtime frames
        // (`at Module._compile (node:internal/…)`, seven of ten on a real
        // `node app.js` ENOENT) say nothing about the program: counted before
        // the content route sees the rest. The `node` ROW of the spec table is
        // never reached from here — this arm is the one production runs.
        ("node", _) | ("deno", _) => {
            let (folded, dropped) = crate::stacktrace::fold_js_frame_runs(output);
            if dropped > 0 {
                let mut r = auto_or_generic(&folded, before);
                r.savings = crate::Savings::new(before, crate::tokenizer::estimate(&r.output));
                return Err(r);
            }
            return Err(auto_or_generic(output, before));
        }
        ("php", _) | ("ruby", _) => {
            return Err(auto_or_generic(output, before));
        }
        ("ansible-playbook", _) | ("ansible", _) => {
            ("ansible", crate::cmd::linux::ansible_dispatcher::dispatch(argv, output))
        }
        // systemd: per-subcommand. `find_subcommand` returns the verbatim
        // argv[1], which for systemctl is often a FLAG (`systemctl
        // --type=service`, `systemctl --failed`) rather than the
        // subcommand — so we re-scan argv here for the first non-flag
        // token. `status` keeps its bespoke compactor; bare `systemctl`
        // (and flag-only forms) defaults to `list-units`, which gets its
        // own fold; everything else sniffs content.
        ("systemctl", _) => {
            let sub = argv.iter().skip(1).find(|a| !a.starts_with('-')).map(String::as_str);
            match sub {
                Some("status") => ("systemctl-status", crate::systemd::compact(output)),
                None | Some("list-units") => {
                    ("systemctl-list", systemctl_list_units(output))
                }
                // Ask the same question the generic path asks before giving
                // up: `systemctl list-dependencies` has a spec of its own, and
                // returning here is what made it unreachable.
                other => {
                    if let Some(hit) =
                        subcommand_spec("systemctl", other.unwrap_or(""), output, before)
                    {
                        return Err(hit);
                    }
                    return Err(auto_or_generic_labelled(output, before, "systemd"));
                }
            }
        }
        ("journalctl", _) => {
            return Err(auto_or_generic_labelled(output, before, "systemd"));
        }
        ("pre-commit", _) => ("pre-commit", crate::cmd::linux::linter_dispatcher::dispatch(argv, output)),
        // PowerShell: route the CMDLET, not the host.
        //
        // `Get-Process` on its own already compacted, because an unknown
        // program falls through to name dispatch and `get_process` is in the
        // table. `powershell Get-Process` did not: `powershell` IS a known
        // name, so it reached the generic tail and the cmdlet was never asked
        // about. Measured 2026-09-02 on a 220-row Get-Service table: bare
        // `Get-Process` 76.7%, `pwsh Get-Process` 76.7%, `foo Get-Process`
        // 76.7%, `powershell Get-Process` 0.0%. The host name is scaffolding
        // — the cmdlet is what shaped the bytes.
        ("powershell", cmdlet) | ("pwsh", cmdlet) | ("powershell.exe", cmdlet)
            if !cmdlet.is_empty() =>
        {
            folded_or_fallthrough(
                "powershell-cmdlet",
                crate::cmd::dispatch_named(cmdlet, output),
                output,
                before,
            )?
        }
        // Package managers — variant-aware (most accept --json / --yaml
        // / --quiet that change shape).
        ("apt", _) | ("apt-get", _) | ("dpkg", _) | ("yum", _) | ("dnf", _) | ("rpm", _)
        | ("pacman", _) | ("brew", _) | ("port", _) | ("snap", _) | ("flatpak", _)
        | ("nix-env", _) | ("nix", _) | ("guix", _) | ("poetry", _) | ("pipenv", _)
        | ("conda", _) | ("gem", _) | ("bundle", _) | ("composer", _) | ("pip", _)
        | ("pip3", _) | ("uv", _) => {
            ("pkgmgr", crate::cmd::linux::pkgmgr_dispatcher::dispatch(argv, output))
        }
        // Unknown command: try the format-detecting `auto` compactor first.
        // It can route terraform/kubectl/jq/jvmbuild/etc. by *content*, so a
        // CLI user invoking `tokenade proxy terraform plan` gets the same compression
        // that MCP tools provide. If `auto` detects nothing useful, fall back
        // to the generic ANSI-strip + dedup pass.
        _ => {
            // Try program-name dispatch first (handles any program with a
            // dedicated compactor module but no explicit arm above).
            if let Some(c) = crate::cmd::dispatch_named(prog.as_str(), output) {
                let after = tokenizer::estimate(&c);
                // Guard against inflation: if dispatch_named returns a result
                // larger than or equal to the input, fall through to the
                // format-detecting auto path — it may do better.
                if after < before {
                    return Err(FilterResult {
                        filter: prog.to_string(),
                        output: c,
                        savings: Savings::new(before, after),
                    });
                }
            }
            // Two-token subcommand dispatch. A dedicated compactor module
            // named `<prog>_<sub>` (e.g. `go test`→`go_test`, `zfs list`→
            // `zfs_list`, `zpool status`→`zpool_status`, `argocd app list`→
            // `argocd_app_list`) is keyed `prog_sub`, but the bare-program
            // dispatch above only ever looks up `prog`, so every such module
            // for a program with no explicit arm or family dispatcher is
            // unreachable — exactly the gap that hid `go_test`/`cargo_nextest`.
            // Rebuild the composite key from the resolved subcommand and retry
            // once. Each `try_compact` is content-defensive (`if !detect {
            // return None }`), so a wrong guess folds nothing and we fall
            // through to the generic pass unchanged.
            if let Some(hit) = subcommand_spec(prog.as_str(), sub, output, before) {
                return Err(hit);
            }
            return Err(auto_or_generic(output, before));
        }
    };
    Ok(pair)
}

/// A read of one file's content: comments stripped by its extension, then the
/// large-read truncation. Shared by `cat`/`nl` and `git show <rev>:<path>`.
fn read_of_a_file(
    label: &'static str,
    argv: &[String],
    output: &str,
    before: usize,
) -> Result<(&'static str, String), FilterResult> {
    let clause = read_clause(argv);
    let argv = clause.as_slice();
    let content: String = match cat_try_code_strip(argv, output) {
        Some(stripped) => stripped,
        None => {
            if cat_argv_has_source_extension(argv) {
                strip_ansi(output)
            } else {
                return Err(auto_or_generic(output, before));
            }
        }
    };
    let ct = tokenizer::estimate(&content);
    let out = cat_truncate_keeping_banner(argv, content, ct);
    // Unchanged source (a read that fits keeps its comments): say so as
    // `passthrough`, or the "folded nothing" nets hand it to `generic`.
    if out == strip_ansi(output) && cat_argv_has_source_extension(argv) {
        return Err(FilterResult {
            filter: "passthrough".to_string(),
            output: out,
            savings: Savings::new(before, before),
        });
    }
    Ok((label, out))
}

/// The read's own clause, operands as the shell sees them. Inside a loop the
/// argv runs on (`cat -n "src/$f.rs"; done`), and the operand carries its quotes
/// and the `;` that ends it — so `.rs` was never recognised and the read fell
/// to the generic fold. Measured 2026-10-05 on a real `for f in …; do echo
/// "═══ $f"; cat -n "src/commands/$f.rs"; done` (4 files, 48 KB): 2 of the 4
/// file headers and 8 definitions survived, where `cat -n` on the same four
/// files keeps 35 definitions at the same size.
pub(super) fn read_clause(argv: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for tok in argv {
        if matches!(tok.as_str(), "|" | "||" | "&&" | ";" | "done" | "fi") || tok.starts_with('>') {
            break;
        }
        let ends = tok.ends_with(';');
        let t = tok
            .trim_end_matches(';')
            .trim_matches(|c| c == '"' || c == '\'');
        if !t.is_empty() {
            out.push(t.to_string());
        }
        if ends {
            break;
        }
    }
    out
}

/// The `<rev>:<path>` operand of `git show` when what it printed is that blob:
/// one operand, a non-empty path after the colon, output not a commit/tag.
fn git_show_blob_path(argv: &[String], output: &str) -> Option<String> {
    if output.starts_with("commit ") || output.starts_with("tag ") {
        return None;
    }
    let mut ops = argv.iter().skip(2).filter(|a| !a.starts_with('-'));
    let (one, None) = (ops.next()?, ops.next()) else {
        return None;
    };
    let (_, path) = one.split_once(':')?;
    (!path.is_empty() && !path.ends_with('/')).then(|| one.clone())
}

/// `--stat` rows with git's drawn bar reduced to its sign (`23 +-`).
fn stat_view<'a>(argv: &[String], output: &'a str) -> std::borrow::Cow<'a, str> {
    if argv
        .iter()
        .any(|a| a == "--stat" || a.starts_with("--stat=") || a == "--patch-with-stat")
    {
        std::borrow::Cow::Owned(crate::cmd::linux::git::diff::squeeze_stat_bars(output))
    } else {
        std::borrow::Cow::Borrowed(output)
    }
}
