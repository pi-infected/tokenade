//! **Which files does the agent REALLY load?**
//!
//! The cascade is not a convention: each agent has its own, and what the
//! user thinks gets loaded is not what is loaded. Auditing the wrong
//! set of files returns a verdict on a prefix that does not exist.
//!
//! Hence a table per agent rather than a name heuristic: a heuristic
//! that is wrong is wrong SILENTLY, and a mute audit reads as a clean
//! audit.

use super::*;

/// Compute the candidate cascade file paths for a working directory,
/// in load order: global user file first, then ancestor directories
/// from the filesystem root down to `start`. Pure path arithmetic — does
/// not touch the filesystem, so callers must filter to existing files.
///
/// For each directory we consider both `CLAUDE.md` and `.claude/CLAUDE.md`.
pub fn cascade_paths(start: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    // Absolutise FIRST. The cascade is the ancestor chain, and `Path::ancestors`
    // is textual: given a relative `start` it stops at the relative root, so the
    // walk both misses everything above it and emits two spellings of the same
    // file. Measured 2026-09-02 from this repo:
    //
    //   tokenade audit-claude-md            → 6 files, 11 810 tokens  (correct)
    //   tokenade audit-claude-md .          → 5 rows, `CLAUDE.md` AND `./CLAUDE.md`
    //   tokenade audit-claude-md ../tokenade → 6 files,  3 783 tokens
    //
    // The relative forms under-report the always-resident prefix by ~8 000
    // tokens (the 9 745-token `~/CLAUDE.md` is simply never visited) while the
    // duplicated spellings inflate the "duplicated across files" figure with a
    // file compared against itself — the audit then advises hoisting content
    // that already lives in exactly one place. The command documents an
    // optional positional directory, so a relative one is a supported input.
    //
    // `canonicalize` resolves `.`, `..` and symlinks; it needs the directory to
    // exist, and a non-existent one keeps the caller's path rather than
    // silently auditing somewhere else.
    let start_owned;
    let start: &Path = match start.canonicalize() {
        Ok(p) => {
            start_owned = p;
            &start_owned
        }
        Err(_) => start,
    };
    let mut out: Vec<PathBuf> = Vec::new();
    let push = |p: PathBuf, out: &mut Vec<PathBuf>| {
        if !out.contains(&p) {
            out.push(p);
        }
    };

    // 1. Global user instructions.
    if let Some(h) = home {
        push(
            crate::platform::claude_config_dir_for(h).join("CLAUDE.md"),
            &mut out,
        );
    }

    // 2. Ancestor chain, root -> start (nearest last).
    let ancestors: Vec<&Path> = start.ancestors().collect();
    for dir in ancestors.into_iter().rev() {
        push(dir.join("CLAUDE.md"), &mut out);
        push(dir.join(".claude").join("CLAUDE.md"), &mut out);
    }

    out
}

/// Returns rules-file paths for `agent`, from global to project-local.
/// Mirrors the CLAUDE.md cascade for Claude Code; other agents use their
/// own per-agent convention (.cursorrules, .clinerules, AGENTS.md, etc.).
/// **DECLARED RESERVE.** No production caller: the audit only walks
/// Claude Code's cascade today. The other agents' table is
/// kept because it is the SURVEY of their conventions — redoing it costs
/// a read of each agent, keeping it costs 200 lines.
#[allow(dead_code)]
pub(crate) fn rules_files_for_agent(
    agent: &crate::detect::Agent,
    start: &Path,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    use crate::detect::Agent;

    fn push_unique(p: PathBuf, out: &mut Vec<PathBuf>) {
        if !out.contains(&p) {
            out.push(p);
        }
    }

    match agent {
        // T3 drives Claude with user+project settingSources — same cascade.
        Agent::ClaudeCode | Agent::T3Code | Agent::Unknown => cascade_paths(start, home),

        Agent::Cursor => {
            let mut out = Vec::new();
            if let Some(h) = home {
                push_unique(h.join(".cursor").join("rules"), &mut out);
                push_unique(h.join(".cursorrules"), &mut out);
            }
            for dir in start.ancestors() {
                push_unique(dir.join(".cursorrules"), &mut out);
                push_unique(dir.join("cursor.md"), &mut out);
            }
            out
        }

        Agent::Cline => {
            // cline.bot docs: global `~/.cline/rules/` (+ `~/Documents/Cline/Rules`,
            // `~/.agents/AGENTS.md`). Workspace `.clinerules/` / `.cline/rules/`.
            // `~/.clinerules` is not the global location.
            let mut out = Vec::new();
            if let Some(h) = home {
                push_unique(h.join(".cline").join("rules"), &mut out);
                push_unique(h.join(".agents").join("AGENTS.md"), &mut out);
            }
            for dir in start.ancestors() {
                push_unique(dir.join(".clinerules"), &mut out);
                push_unique(dir.join(".cline").join("rules"), &mut out);
                push_unique(dir.join("AGENTS.md"), &mut out);
            }
            out
        }

        // Same file, read by both halves of the product: the CLI and the VS
        // Code extension each look for `.github/copilot-instructions.md`.
        // Splitting the agent at rank 4 does not split its rules cascade.
        Agent::Copilot | Agent::CopilotVsCode => {
            let mut out = Vec::new();
            for dir in start.ancestors() {
                push_unique(
                    dir.join(".github").join("copilot-instructions.md"),
                    &mut out,
                );
            }
            out
        }

        // Devin Desktop (Windsurf fork): global
        // `~/.codeium/windsurf/memories/global_rules.md`, project `.windsurfrules`
        // and `AGENTS.md` (nested included) — read in its installed bundle.
        Agent::DevinDesktop => {
            let mut out = Vec::new();
            if let Some(h) = home {
                push_unique(
                    h.join(".codeium")
                        .join("windsurf")
                        .join("memories")
                        .join("global_rules.md"),
                    &mut out,
                );
            }
            for dir in start.ancestors() {
                push_unique(dir.join(".windsurfrules"), &mut out);
                push_unique(dir.join("AGENTS.md"), &mut out);
            }
            out
        }

        Agent::Codex => {
            // Global is `$CODEX_HOME/AGENTS.md` (default `~/.codex/AGENTS.md`),
            // not `~/AGENTS.md`. Binary 0.147.0: « Failed to read global
            // AGENTS.md instructions from `..` » via `codex-home`.
            let mut out = Vec::new();
            if let Some(h) = home {
                push_unique(h.join(".codex").join("AGENTS.md"), &mut out);
            }
            for dir in start.ancestors() {
                push_unique(dir.join("AGENTS.md"), &mut out);
            }
            out
        }

        // Factory Droid: personal dir `~/.factory/` (docs.factory.ai), plus
        // AGENTS.md / CLAUDE.md from cwd up to git root.
        Agent::Droid => {
            let mut out = Vec::new();
            if let Some(h) = home {
                push_unique(h.join(".factory").join("AGENTS.md"), &mut out);
            }
            for dir in start.ancestors() {
                push_unique(dir.join("AGENTS.md"), &mut out);
                push_unique(dir.join("CLAUDE.md"), &mut out);
            }
            out
        }

        Agent::OpenCode => {
            // Binary 1.18.16: project `AGENTS.md` / `CLAUDE.md` / `CONTEXT.md`,
            // plus `~/.claude/CLAUDE.md` as long as `disableClaudeCodePrompt` is
            // false (default). No global location of its own.
            let mut out = Vec::new();
            if let Some(h) = home {
                push_unique(h.join(".claude").join("CLAUDE.md"), &mut out);
            }
            for dir in start.ancestors() {
                push_unique(dir.join("AGENTS.md"), &mut out);
                push_unique(dir.join("CLAUDE.md"), &mut out);
                push_unique(dir.join("opencode.md"), &mut out);
            }
            out
        }

        // Pi reads AGENTS.md (home + project cascade) — the same channel the
        // tokenade scaffold writes into. Project-local `.pi/` resources are
        // trust-gated and not part of the bloat-prone rules cascade.
        Agent::Pi => {
            // Shipped README: `~/.pi/agent/AGENTS.md` (global), not `~/AGENTS.md`.
            let mut out = Vec::new();
            if let Some(h) = home {
                push_unique(h.join(".pi").join("agent").join("AGENTS.md"), &mut out);
            }
            for dir in start.ancestors() {
                push_unique(dir.join("AGENTS.md"), &mut out);
                push_unique(dir.join("CLAUDE.md"), &mut out);
            }
            out
        }

        Agent::KiloCode => {
            let mut out = Vec::new();
            for dir in start.ancestors() {
                push_unique(dir.join(".kilocoderules"), &mut out);
            }
            out
        }

        Agent::Aider => {
            let mut out = Vec::new();
            if let Some(h) = home {
                push_unique(h.join(".aider.conf.yml"), &mut out);
            }
            for dir in start.ancestors() {
                push_unique(dir.join("CONVENTIONS.md"), &mut out);
                push_unique(dir.join(".aider.conf.yml"), &mut out);
            }
            out
        }

        Agent::Hermes => {
            // Shipped skill: first match wins. `.hermes.md`/`HERMES.md` walks
            // up to the git root; `AGENTS.md`/`CLAUDE.md` cwd only.
            // `SOUL.md` in `$HERMES_HOME` is the identity, not the rules.
            // No `~/.hermes/rules.md`.
            let mut out = Vec::new();
            if let Some(h) = home {
                push_unique(h.join(".hermes").join("SOUL.md"), &mut out);
            }
            for dir in start.ancestors() {
                push_unique(dir.join(".hermes.md"), &mut out);
                push_unique(dir.join("HERMES.md"), &mut out);
                push_unique(dir.join("AGENTS.md"), &mut out);
            }
            out
        }

        // Qwen Code mirrors Gemini CLI's cascade under ~/.qwen/ + QWEN.md.
        Agent::Qwen => {
            let mut out = Vec::new();
            if let Some(h) = home {
                push_unique(h.join(".qwen").join("QWEN.md"), &mut out);
            }
            for dir in start.ancestors() {
                push_unique(dir.join("QWEN.md"), &mut out);
            }
            out
        }

        Agent::Grok => {
            // User-guide 12-project-rules.md (Grok 1.0.41): `$GROK_HOME/rules/*.md`
            // (not a `rules.md` file), plus named files (AGENTS.md, CLAUDE.md, …)
            // and, compat on, `~/.claude/` + `~/.cursor/rules/*.md`.
            let mut out = Vec::new();
            if let Some(h) = home {
                push_unique(h.join(".grok").join("rules").join("tokenade.md"), &mut out);
                push_unique(h.join(".claude").join("CLAUDE.md"), &mut out);
                push_unique(h.join(".claude").join("AGENTS.md"), &mut out);
                push_unique(
                    h.join(".cursor").join("rules").join("tokenade.md"),
                    &mut out,
                );
            }
            for dir in start.ancestors() {
                push_unique(dir.join("AGENTS.md"), &mut out);
                push_unique(dir.join("CLAUDE.md"), &mut out);
                push_unique(dir.join(".grokrules"), &mut out);
            }
            out
        }

        Agent::OpenClaw | Agent::Antigravity => cascade_paths(start, home),

        // Odysseus has no formal rules cascade. The proposed convention is
        // `data/soul.md` relative to the workspace root — check for it only;
        // fall back to empty (no CLAUDE.md-style inheritance).
        Agent::Odysseus => {
            let mut out = Vec::new();
            push_unique(start.join("data").join("soul.md"), &mut out);
            out
        }
    }
}

/// Read the existing members of a cascade from disk, returning each as
/// `(display_path, content)` in load order. Silently skips files that do
/// not exist or cannot be read.
pub fn read_cascade(start: &Path, home: Option<&Path>) -> Vec<(String, String)> {
    existing_cascade(start, home)
        .into_iter()
        .map(|(p, c)| (p.display().to_string(), c))
        .collect()
}

/// The existing cascade members, ONE entry per file on disk (#9).
///
/// Two spellings can name one file: `$CLAUDE_CONFIG_DIR/CLAUDE.md` symlinked
/// to `~/.claude/CLAUDE.md` (also reached as the home ancestor's
/// `.claude/CLAUDE.md`), a dotfile repo checkout, a symlinked project dir.
/// Kept as two entries, dedup compared the file with itself, found every line
/// "duplicated" and stripped it from one spelling — that is, from the file.
/// Identity is the canonical path; the LAST spelling (most-specific load
/// position) is the one kept.
pub(crate) fn existing_cascade(start: &Path, home: Option<&Path>) -> Vec<(PathBuf, String)> {
    let mut out: Vec<(PathBuf, PathBuf, String)> = Vec::new();
    for p in cascade_paths(start, home) {
        let Ok(c) = std::fs::read_to_string(&p) else {
            continue;
        };
        let id = p.canonicalize().unwrap_or_else(|_| p.clone());
        out.retain(|(seen, _, _)| *seen != id);
        out.push((id, p, c));
    }
    out.into_iter().map(|(_, p, c)| (p, c)).collect()
}
