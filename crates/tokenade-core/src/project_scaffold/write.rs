//! **Writing the rules block into an agent's file.**
//!
//! Update = remove everything that is ours, then write the current block.
//! "Ours" is ONLY what a marker proves: the `<!-- tokenade-scaffold -->` block,
//! the `<!-- tokenade:start -->` fence, and the exact intro of a file we
//! created. Headings and wordings are never matched — `## Commands` is a title
//! users write too (#7). Files from before 2.0 are not healed.

use super::*;

/// Closing marker for the scaffold block. Used by the in-place
/// upgrade path so we can replace just the rules block without
/// touching the user's surrounding edits.
pub(crate) const SCAFFOLD_END_MARKER: &str = "<!-- /tokenade-scaffold -->";

/// Parse the `version: N` line from a `.tokenade/scaffold.flag`
/// file. Returns `Some(n)` when a numeric version is present, `None`
/// otherwise (legacy flag files written before v2 had no version
/// line — treated as version 1 by the caller).
/// The style the block carries, as written in the flag. `None` on a flag
/// from before this field, which forces a pass — the right answer, since we
/// then do not know what the file contains.
pub(super) fn parse_flag_style(s: &str) -> Option<String> {
    for line in s.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("style:") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

pub(super) fn parse_flag_version(s: &str) -> Option<u32> {
    for line in s.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("version:") {
            if let Ok(n) = rest.trim().parse::<u32>() {
                return Some(n);
            }
        }
    }
    None
}

/// The intro [`fresh_rules_file`] writes above the block, line for line.
fn wrapper_line(t: &str) -> bool {
    t == WRAPPER_TITLE || t == WRAPPER_INTRO
}

const WRAPPER_TITLE: &str = "# Project rules for AI coding agents";
const WRAPPER_INTRO: &str = "Auto-scaffolded by tokenade on first MCP session. Safe to edit; the \
tokenade block below is identified by the HTML marker and will be \
updated in-place on future tokenade upgrades.";

/// Opening marker → its closing marker.
fn region_end(t: &str) -> Option<&'static str> {
    if t == SCAFFOLD_MARKER {
        Some(SCAFFOLD_END_MARKER)
    } else if t == OLD_CLAUDE_FENCE_START {
        Some(OLD_CLAUDE_FENCE_END)
    } else {
        None
    }
}

fn is_marker(t: &str) -> bool {
    t == SCAFFOLD_MARKER
        || t == SCAFFOLD_END_MARKER
        || t == OLD_CLAUDE_FENCE_START
        || t == OLD_CLAUDE_FENCE_END
}

/// User prose on either side of what is provably Tokenade's: marked regions,
/// stray marker lines, our intro. `found` is true as soon as one went. An
/// opening marker with no closing one loses only its own line — without the
/// bound, what follows cannot be proven ours.
pub(super) fn take_user_sides(text: &str) -> (String, String, bool) {
    let lines: Vec<&str> = text.lines().collect();
    let mut prefix: Vec<&str> = Vec::new();
    let mut suffix: Vec<&str> = Vec::new();
    let mut found = false;
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if let Some(end) = region_end(t) {
            found = true;
            if let Some(j) = lines[i + 1..].iter().position(|l| l.trim() == end) {
                i += j + 2;
            } else {
                i += 1;
            }
            continue;
        }
        if is_marker(t) || wrapper_line(t) {
            found = true;
            i += 1;
            continue;
        }
        if found {
            suffix.push(lines[i]);
        } else {
            prefix.push(lines[i]);
        }
        i += 1;
    }
    (join_lines(&prefix), join_lines(&suffix), found)
}

fn join_lines(lines: &[&str]) -> String {
    let mut cleaned = String::new();
    let mut blanks = 0usize;
    for l in lines {
        if l.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        cleaned.push_str(l);
        cleaned.push('\n');
    }
    cleaned
}

fn fresh_rules_file(block: &str) -> String {
    format!("{WRAPPER_TITLE}\n\n{WRAPPER_INTRO}\n\n{block}")
}

/// Append-or-create the scaffold block in one rules file. Returns
/// true if we touched the file; false on no-op or error.
///
/// Upgrade: drop every Tokenade-owned region, then write `block`
/// once. Prose outside those regions is kept.
pub(super) fn write_scaffold_to(root: &Path, target: &Path, subagent_note: bool) -> Option<bool> {
    // `root` is the REPOSITORY, and every write below goes through the
    // repo-scoped guard. MEASURED 2026-08-07: a repo shipping `CLAUDE.md` as
    // a symlink got ~4 KB of scaffold written straight THROUGH the link, into
    // whatever path the repo chose. Cloning a repo must not let it pick a file
    // on your machine for us to write to.
    // `CLAUDE.md` is Claude Code's file, and Claude propagates the brevity
    // rules to subagents on its own (CLAUDE.md inheritance + the Explore/Plan
    // pretool preamble), so it omits the static subagent note. Every other
    // rule file targets an agent with no per-subagent injection channel, so it
    // carries the note.
    // Which variant to write is the CALLER's call, not this file name's. A
    // user who keeps `CLAUDE.md` and `AGENTS.md` in sync with a symlink has
    // ONE file reachable under both names, and it has to carry the note the
    // non-Claude agents need — see `resolve_rule_targets`.
    let is_claude = !subagent_note;
    // The style line goes INSIDE, before the end marker. Placed after, it
    // falls into the user's territory: the splice only replaces
    // `[start..end]`, so a line beyond the marker is never refreshed nor
    // removed — checked on 2026-08-21, switching from `coding` to `auto` left
    // it in place — and `uninstall` would not know it is ours.
    // A pointer rather than the block, when the agent already reads
    // `AGENTS.md`. The middle guard exists for a repo that keeps its rule
    // files in sync through a LINK: if `AGENTS.md` and the target are the same
    // file, the pointer would point to itself and the project would have no
    // rules at all.
    if lecture_covered_elsewhere(root, target) {
        return strip_project_lecture(root, target);
    }
    let block = with_style_line(&scaffold_block(!is_claude), root);
    let existing = if target.exists() {
        match std::fs::read_to_string(target) {
            Ok(s) => Some(s),
            Err(_) => return Some(false),
        }
    } else {
        None
    };
    let Some(new_content) = apply_scaffold_block(existing.as_deref(), &block) else {
        return Some(false);
    };
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    crate::paths::write_atomic_repo_scoped(root, target, &new_content)
        .map(|_| true)
        .ok()
}

/// Nothing rather than a lesson: every agent that reads THIS file already has
/// the lesson outside the repo. A pointer ("the rules are elsewhere") cost
/// tokens every session without teaching anything, and lied to whoever
/// clones the repo without Tokenade.
fn lecture_covered_elsewhere(root: &Path, target: &Path) -> bool {
    let Some(home) = crate::platform::home_dir() else {
        return false;
    };
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let dedicated = dedicated_project_file_readers(target);
    // For its OWN file, Hermes is only covered by a global — otherwise
    // "covered by its file" would empty that file.
    let own_file_counts = dedicated.is_none();
    let Some(mut readers) = dedicated
        .or_else(|| shared_project_file_readers(name))
        .map(<[_]>::to_vec)
    else {
        return false;
    };
    // A `CLAUDE.md` linked to `AGENTS.md` is ONE file under two names.
    let same_as_agents_md = name != "AGENTS.md"
        && matches!(
            (std::fs::canonicalize(root.join("AGENTS.md")), std::fs::canonicalize(target)),
            (Ok(a), Ok(b)) if a == b
        );
    if same_as_agents_md {
        readers.extend_from_slice(AGENTS_MD_READERS);
    }
    readers_covered(&readers, &home, own_file_counts)
}

/// Who reads these shared project files — read in each package on
/// 2026-09-28 (docs/PLAN-lecon-agents.md, P0).
fn shared_project_file_readers(name: &str) -> Option<&'static [crate::detect::Agent]> {
    use crate::detect::Agent::*;
    Some(match name {
        "AGENTS.md" => AGENTS_MD_READERS,
        "CLAUDE.md" | "CLAUDE.local.md" => &[
            ClaudeCode, Cursor, Grok, Copilot, Droid, Pi, OpenCode, KiloCode, Hermes,
        ],
        // Antigravity (agy's rules.go / language_server); does not read CLAUDE.md.
        "GEMINI.md" => &[Antigravity, Copilot],
        "QWEN.md" => &[Qwen],
        ".cursorrules" => &[Cursor, Hermes],
        ".grokrules" => &[Grok],
        ".clinerules" => &[Cline],
        _ => return None,
    })
}

const AGENTS_MD_READERS: &[crate::detect::Agent] = {
    use crate::detect::Agent::*;
    &[
        Codex,
        Pi,
        Droid,
        OpenCode,
        KiloCode,
        Copilot,
        Cursor,
        Cline,
        Grok,
        Qwen,
        Hermes,
        Antigravity,
        DevinDesktop,
    ]
};

/// Every PRESENT reader has the lesson elsewhere: its global, or a complete
/// project file it reads first (Hermes `.hermes.md`, first found). No
/// recognised reader: we write — an agent we do not detect may read it, and
/// one lesson too many costs less than a missing one.
fn readers_covered(readers: &[crate::detect::Agent], home: &Path, own_file_counts: bool) -> bool {
    use crate::detect::Agent::*;
    let present: Vec<_> = readers
        .iter()
        .copied()
        .filter(|a| agent_home_present(*a, home))
        .collect();
    !present.is_empty()
        && present.iter().all(|a| {
            (own_file_counts && matches!(a, Hermes)) || agent_has_global_lecture(Some(*a), home)
        })
}

/// Same splice as [`write_scaffold_to`], outside the repo, for a global its
/// `readers` read. `write_atomic` follows a link the user chose (chezmoi);
/// the repo-scoped guard would refuse.
pub(crate) fn write_global_for_readers(
    target: &Path,
    readers: &[crate::detect::Agent],
    subagent_note: bool,
    prefix: &str,
) -> Option<bool> {
    // The style preamble ALSO travels in the global: for an agent with no
    // prompt hook (OpenCode, Kilo, Hermes, Pi, Cursor), the rules file is the
    // only channel, and it lived only in the PROJECT file — removed when the
    // lesson moved to the global (compat gate 1.1.19: 5 `sobriety` cells ok →
    // partial). Global style (`~/.tokenade/style.json`); `auto` resolves to
    // empty, so nothing is written in the common case.
    let block = with_style_line(&global_block_for_readers(readers, subagent_note), target);
    splice_and_write(target, &block, prefix, false)
}

fn splice_and_write(target: &Path, block: &str, prefix: &str, repo_scoped: bool) -> Option<bool> {
    let _ = repo_scoped;
    let existing = if target.exists() {
        match std::fs::read_to_string(target) {
            Ok(s) => Some(s),
            Err(_) => return Some(false),
        }
    } else {
        None
    };
    // Outside a repo, no "Project rules for AI coding agents" header: this is
    // not a project file, and the header was re-read every session.
    // `None` from the splice = already up to date (or refused): we start from
    // the existing content, otherwise the header of an up-to-date global
    // would never go.
    let mut new_content = match existing.as_deref() {
        None => format!("{}\n", block.trim_end()),
        Some(e) => apply_scaffold_block(Some(e), block).unwrap_or_else(|| e.to_string()),
    };
    let kept: Vec<&str> = new_content
        .lines()
        .filter(|l| !wrapper_line(l.trim()))
        .collect();
    new_content = join_lines(&kept).trim_start().to_string();
    if existing.as_deref() == Some(new_content.as_str()) {
        return Some(false);
    }
    if !prefix.is_empty() && !new_content.starts_with("---") {
        new_content = format!("{prefix}{new_content}");
    }
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    crate::paths::write_atomic(target, &new_content)
        .map(|_| true)
        .ok()
}

fn has_owned_region(text: &str) -> bool {
    text.lines().any(|l| is_marker(l.trim()))
}

/// `None` = already up to date. `Some` = bytes to write.
///
/// Everything a marker proves Tokenade's goes; the current block is placed
/// once. Everything else stays (see [`take_user_sides`]).
pub(super) fn apply_scaffold_block(existing: Option<&str>, block: &str) -> Option<String> {
    let Some(existing) = existing else {
        return Some(fresh_rules_file(block));
    };
    let start = existing.find(SCAFFOLD_MARKER);
    let end_at = existing.find(SCAFFOLD_END_MARKER);
    if let (Some(s), Some(e)) = (start, end_at) {
        if e < s {
            return None;
        }
    }
    if let (Some(start), Some(end_at)) = (start, end_at) {
        if end_at > start {
            let end = end_at + SCAFFOLD_END_MARKER.len();
            let prefix = &existing[..start];
            let suffix = &existing[end..];
            let current = &existing[start..end];
            let dirty_sides = has_owned_region(prefix) || has_owned_region(suffix);
            if !dirty_sides && current.trim() == block.trim() {
                return None;
            }
            let (pre, pre_after, _) = take_user_sides(prefix);
            let (suf_before, suf, _) = take_user_sides(suffix);
            return Some(rebuild(
                &merge_user(&pre, &pre_after),
                block,
                &merge_user(&suf_before, &suf),
            ));
        }
    }
    let (prefix, suffix, found) = take_user_sides(existing);
    if found {
        let assembled = rebuild(&prefix, block, &suffix);
        if assembled.trim() == existing.trim() {
            return None;
        }
        return Some(assembled);
    }
    if existing.trim().is_empty() {
        return Some(fresh_rules_file(block));
    }
    if existing.contains(block.trim()) {
        return None;
    }
    Some(rebuild(existing, block, ""))
}

fn merge_user(a: &str, b: &str) -> String {
    let a = a.trim_end_matches('\n').trim_end();
    let b = b.trim_start_matches('\n').trim_start();
    if a.is_empty() {
        return b.to_string();
    }
    if b.is_empty() {
        return a.to_string();
    }
    format!("{a}\n\n{b}")
}

fn rebuild(prefix: &str, block: &str, suffix: &str) -> String {
    if prefix.trim().is_empty() && suffix.trim().is_empty() {
        return fresh_rules_file(block);
    }
    let p = prefix.trim_end_matches('\n').trim_end();
    let s = suffix.trim_start_matches('\n').trim_start();
    let b = block.trim_end();
    if p.is_empty() {
        return format!("{b}\n\n{s}");
    }
    if s.is_empty() {
        return format!("{p}\n\n{b}\n");
    }
    format!("{p}\n\n{b}\n\n{s}")
}

/// Removes everything that is Tokenade's from a project file; deletes the
/// file if nothing else remains. Never creates a file.
fn strip_project_lecture(root: &Path, target: &Path) -> Option<bool> {
    let Ok(existing) = std::fs::read_to_string(target) else {
        return Some(false);
    };
    if !has_owned_region(&existing) && !existing.lines().any(|l| wrapper_line(l.trim())) {
        return Some(false);
    }
    let stripped = apply_scaffold_block(Some(&existing), "").unwrap_or_else(|| existing.clone());
    let kept: Vec<&str> = stripped
        .lines()
        .filter(|l| {
            !wrapper_line(l.trim())
                && l.trim() != SCAFFOLD_MARKER
                && l.trim() != SCAFFOLD_END_MARKER
        })
        .collect();
    let body = join_lines(&kept);
    let body = body.trim();
    if body.is_empty() {
        return Some(remove_with_sidecar(target));
    }
    let out = format!("{body}\n");
    if out == existing {
        return Some(false);
    }
    crate::paths::write_atomic_repo_scoped(root, target, &out)
        .map(|_| true)
        .ok()
}

/// Places the style line INSIDE the block, before the end marker: beyond it,
/// it would fall on the user's side and never be refreshed.
fn with_style_line(base: &str, root: &Path) -> String {
    let line = style_line(root);
    match (line.is_empty(), base.rfind(SCAFFOLD_END_MARKER)) {
        (false, Some(at)) => format!("{}{line}{}", &base[..at], &base[at..]),
        _ => base.to_string(),
    }
}
