# Tokenade — what's new
## 2.0.7
- Windows: a status line Tokenade wraps (`tokenade statusline --wrap-b64`) runs again when its command quotes a path, such as `node "d:\x\statusline.mjs"`. It was run through `cmd /C` with Rust's argument quoting, which cmd does not parse, so nothing ran and only the ticker showed. The command now runs through Git Bash, as Claude Code runs it, or through `cmd` with the line passed verbatim when Git Bash is missing. `tokenade wrap --shell cmd` had the same quoting bug and is fixed too (#10).
- A wrapped status line that fails with no output now shows `⚠ status line `…` failed: <first stderr line>` instead of disappearing.
- Git Bash is found the way Claude Code finds it: `CLAUDE_CODE_GIT_BASH_PATH`, then the Git whose `git.exe` is on PATH, then the default and per-user install directories.

## 2.0.6
- Claude Code: `git` works again in subagents isolated in a worktree (`.claude/worktrees/…`). Claude Code refuses a command where git is an argument of another program, which `tokenade wrap 'git …'` was. There, a plain git command now runs as written and its output is piped into `tokenade filter`, so it is still compacted; a chain or pipe that involves git runs unwrapped (#8).
- The SessionStart `CLAUDE.md` dedup no longer compares a file with itself when two cascade paths lead to it (a symlinked `$CLAUDE_CONFIG_DIR/CLAUDE.md`, a dotfile repo, a symlinked project), which stripped every user line but the headings. Files are now identified by their real path. `audit-claude-md --compress` also writes through a symlink instead of replacing it with a copy (#9).

## 2.0.5
- opencode 2: the plugin bridge now resets the savings ledger on the 2.x compaction event (`session.compaction.ended`, `event.data.sessionID`) and folds a structured shell `output` when the host's `result.content` is empty. Both were silent no-ops: the bridge listened for 1.x's removed `session.compacted` event, and accepted only a string `output`.

## 2.0.4
- A repeated `Read` of an unchanged file, and any `Read` Tokenade shrinks to a head slice, now gets its note or compacted view delivered. It was staged under the line limit the agent asked for, while the delivery hook looks it up under the shortened one, so the agent got the first 20 lines and nothing else (#6).
- The rules file scaffold only touches what a marker proves is Tokenade's: the `<!-- tokenade-scaffold -->` block, the `<!-- tokenade:start -->` fence, and the intro of a file Tokenade created. Sections titled `## Commands`, `## Project rules`, `## Engineering principles` or `## Codebase exploration` in your `CLAUDE.md` / `AGENTS.md` are no longer removed (#7). Leftovers from 1.x outside those markers are no longer cleaned up automatically.
- opencode 2: the plugin bridge loads again (opencode 2 expects an object default export), and folding keeps a shell result's structured `output` and its exit notice intact (#1).

## 2.0.3
- After a successful `tokenade install` (or, if that run was not in a terminal, the first `tokenade dashboard` with saved tokens to show), one line says Tokenade is open source and links the GitHub repository. Shown once per machine, never when stdout is not a terminal or under CI, and `TOKENADE_NO_STAR_INVITE=1` turns it off. It makes no network call.

## 2.0.2
- `tokenade gain` no longer prints a `metered:` line (and `gain --json` no longer has `operations_as_metered` / `saved_as_metered`): they described the metering of the former paid service.
- `tokenade read` with a relative path from a subdirectory of a git repository now recognises the repository, so a source file you are about to edit comes back unchanged instead of with its comments stripped.
- `tokenade read --help` now describes what happens to source code: unchanged when it is likely to be edited (inside a git repository, under ~20k tokens), comments stripped for other source files over 3 KB, `TOKENADE_NO_CODE_STRIP=1` to turn the stripping off.
- `tokenade expand-fold --help` shows its own help instead of the `expand-ref` one, and `expand-fold <template_id> --max N` no longer reads `N` as the content hash.
- Help texts and the one-time open-source notice trimmed.

## 2.0.1
- The 1.x → 2.0 cleanup now also removes `last-report.zip` (the support bundle of the removed `tokenade report`, which held a copy of agent transcripts) and a stray `reporter.lock`, including on machines already migrated to 2.0.0.


## 2.0.0
- **Tokenade is open source** (MIT OR Apache-2.0). No account, no login, no licence key, no trial, no quota: every feature works out of the box.
- **No telemetry.** Nothing is sent anywhere; your savings ledger and dashboard stay on your machine. The only network calls left are the ones you ask for (web fetch/search) and the daily signed update check, which now reads GitHub Releases (`TOKENADE_NO_AUTO_UPDATE=1` turns it off).
- `tokenade login`, `activate` and `report` are gone. On first run, leftover licence and usage-report files under `~/.tokenade` are removed once; your savings history is kept.


## Earlier releases
Release notes for 1.x and before: https://tokenade.net/en/docs/changelog
