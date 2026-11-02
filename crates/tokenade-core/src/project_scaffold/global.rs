//! **The CLI lesson lives on the machine, not in the repo.**
//!
//! Tokenade is an installed binary. Pasting `tokenade map` back into every
//! project `AGENTS.md` paid for the same lesson on every clone, and twice
//! when the agent also loaded a global file. Here: one copy **per agent**,
//! in the file THAT agent loads outside the repo. The project only carries a
//! pointer.
//!
//! An agent with no global address (Aider, Odysseus, …) keeps the block in
//! the project file — a pointer to a file the agent does not open would be a
//! blank page.

use super::*;
use crate::detect::Agent;

/// Cursor frontmatter: its glob is `*.mdc`, `alwaysApply` so the lesson
/// arrives without a file glob.
const CURSOR_MDC_PREFIX: &str = "\
---
description: \"tokenade CLI — prefer these commands over reading files whole\"
alwaysApply: true
---

";

/// One global file per agent that has one, and only that agent.
///
/// Paths read in the package / shipped docs of EACH agent (2026-09-27), not
/// in our old table:
/// - Claude Code: `~/.claude/CLAUDE.md`
/// - Cursor: `~/.cursor/rules/**/*.mdc` (`alwaysApply`)
/// - Grok: `$GROK_HOME/rules/*.md`; Claude/Cursor compat ALSO loads
///   `~/.claude/CLAUDE.md` and `~/.cursor/rules/*.md` (not `.mdc`)
/// - Qwen Code: `~/.qwen/QWEN.md` (Gemini fork)
/// - Antigravity: `~/.gemini/GEMINI.md` (`rules.go` of the agy /
///   language_server binaries, and `<RULE[user_global]>` from a real
///   conversation)
/// - Codex: `$CODEX_HOME/AGENTS.md` (default `~/.codex/AGENTS.md`)
/// - Pi: `~/.pi/agent/AGENTS.md` (shipped README)
/// - Droid: `~/.factory/` (factory.ai docs "Personal directory")
/// - Cline: `~/.cline/rules/` (cline.bot docs; not `~/.clinerules`)
///
/// - OpenCode: `~/.config/opencode/AGENTS.md` (lean-ctx already wrote there)
/// - Copilot CLI: `$HOME/.copilot/copilot-instructions.md` (package
///   `@github/copilot`, source `home-copilot`)
/// - Kilo CLI: `~/.kilo/rules/` and `~/.kilocode/rules/` (`globalRulesDirs`
///   of the `@kilocode/cli` package)
/// - Devin Desktop: `~/.codeium/windsurf/memories/global_rules.md`
///   (`globalMemoriesPathSegments` under `~/.codeium/<app>` in its bundle;
///   a Windsurf fork that kept its paths)
///
/// Hermes: `SOUL.md` is the identity, not the project rules; a
/// `~/.hermes/AGENTS.md` only loads if cwd = that folder. No global
/// address. Aider / Copilot VS Code / Odysseus: same.
pub fn global_rule_file(agent: Agent, home: &Path) -> Option<PathBuf> {
    Some(match agent {
        Agent::ClaudeCode | Agent::T3Code | Agent::OpenClaw => {
            crate::platform::claude_config_dir_for(home).join("CLAUDE.md")
        }
        Agent::Antigravity => home.join(".gemini").join("GEMINI.md"),
        Agent::Cursor => home.join(".cursor").join("rules").join("tokenade.mdc"),
        Agent::Grok => home.join(".grok").join("rules").join("tokenade.md"),
        Agent::Qwen => home.join(".qwen").join("QWEN.md"),
        Agent::Codex => home.join(".codex").join("AGENTS.md"),
        Agent::Pi => home.join(".pi").join("agent").join("AGENTS.md"),
        Agent::Droid => home.join(".factory").join("AGENTS.md"),
        Agent::Cline => home.join(".cline").join("rules").join("tokenade.md"),
        Agent::OpenCode => home.join(".config").join("opencode").join("AGENTS.md"),
        Agent::Copilot => home.join(".copilot").join("copilot-instructions.md"),
        Agent::KiloCode => home.join(".kilo").join("rules").join("tokenade.md"),
        Agent::DevinDesktop => home
            .join(".codeium")
            .join("windsurf")
            .join("memories")
            .join("global_rules.md"),
        Agent::Hermes | Agent::Aider | Agent::CopilotVsCode | Agent::Odysseus | Agent::Unknown => {
            return None
        }
    })
}

/// Copies that older installs left in files Grok (compat) loads IN ADDITION
/// to the native file, plus `~/AGENTS.md` that a wrong table wrote for
/// Codex/Pi/Droid (they read `~/.codex/`, `~/.pi/agent/`, `~/.factory/`).
/// Remove them, do not point them: a pointer here is still a copy.
///
/// And the global of an agent RETIRED from the product (Reasonix): no reader
/// left, our block would stay there forever. `~/.gemini/GEMINI.md`, Gemini
/// CLI's, is NOT in it: it is also Antigravity's global.
fn leftover_global_files(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".reasonix").join("REASONIX.md"),
        crate::platform::claude_config_dir_for(home).join("AGENTS.md"),
        home.join(".cursor").join("rules").join("tokenade.md"),
        home.join("AGENTS.md"),
        home.join("CLAUDE.md"),
        home.join(".hermes").join("rules.md"),
        home.join(".clinerules"),
    ]
}

/// Project files an agent does NOT load. A lesson here is never read, and
/// if the agent loaded it by mistake it would be a duplicate.
///
/// Including those of agents RETIRED from the product (Reasonix): an earlier
/// install wrote them, nothing reads them for us any more. We remove our
/// block; the file goes if it was all that remained. (`GEMINI.md` is not one:
/// Antigravity reads it; `.windsurfrules` neither: Devin Desktop, the
/// Windsurf fork, reads it — it is in [`AGENT_PROJECT_FILES`].)
pub(crate) fn leftover_project_files(root: &Path) -> Vec<PathBuf> {
    [".hermes_rules.md", "REASONIX.md"]
        .iter()
        .map(|n| root.join(n))
        .collect()
}

/// Agent-specific project files. Healing takes them over like `CLAUDE.md`:
/// otherwise a pointer or a lesson from a previous version stayed there
/// until the next MCP scaffold.
pub(crate) const AGENT_PROJECT_FILES: &[&str] = &[
    ".cursorrules",
    ".grokrules",
    ".windsurfrules",
    "GEMINI.md",
    "QWEN.md",
    ".clinerules",
    ".hermes.md",
    "HERMES.md",
    ".github/copilot-instructions.md",
    ".roo/rules/tokenade.md",
    ".kilocode/rules/tokenade.md",
];

/// `CLAUDE.md` / `AGENTS.md` that Claude concatenates with the global lesson
/// when cwd is the repo or a crate. Not research or `benchmarks/` directories.
pub(crate) fn concatenating_project_rule_files(root: &Path) -> Vec<PathBuf> {
    const NAMES: &[&str] = &["CLAUDE.md", "AGENTS.md", "CLAUDE.local.md"];
    let mut out: Vec<PathBuf> = NAMES.iter().map(|n| root.join(n)).collect();
    out.push(root.join(".claude").join("CLAUDE.md"));
    if let Ok(rd) = std::fs::read_dir(root.join("crates")) {
        for ent in rd.flatten() {
            let p = ent.path();
            if !p.is_dir() {
                continue;
            }
            for name in NAMES {
                out.push(p.join(name));
            }
        }
    }
    out
}

/// Turns a complete project lesson into a pointer, removes the 1.1.18 fence
/// in the repo's `.claude/CLAUDE.md`, and the project leftovers. Does not
/// write a file that did not exist. Idempotent.
///
/// cwd = `$HOME` (or `~/.codex`, `~/.factory`…): `root/.claude/CLAUDE.md`
/// IS Claude's global, `root/AGENTS.md` Codex's. Rewriting them as a pointer
/// erased the lesson `ensure_global_rules` had just placed — a lesson ↔
/// pointer flip-flop on every command, measured on 2026-09-28 on `cd ~`. No
/// global file is a project file.
pub fn heal_project_lectures(root: &Path) -> Vec<PathBuf> {
    let mut touched = Vec::new();
    let home = crate::platform::home_dir();
    if home.as_deref().is_some_and(|h| h.starts_with(root)) {
        return touched;
    }
    let globals: Vec<PathBuf> = home
        .as_deref()
        .map(|h| {
            Agent::all()
                .iter()
                .filter_map(|a| global_rule_file(*a, h))
                .chain(leftover_global_files(h))
                .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
                .collect()
        })
        .unwrap_or_default();
    let is_global = |p: &Path| {
        let c = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        globals.iter().any(|g| *g == c)
    };
    let fence = root.join(".claude").join("CLAUDE.md");
    if !is_global(&fence) && strip_old_claude_md_fence(&fence) {
        touched.push(fence);
    }
    // The folders between the repo and `$HOME`: Claude loads their
    // `CLAUDE.md` walking up, and an earlier scaffold may have written there
    // (a `projects/` folder opened once in a session).
    let ancestors: Vec<&Path> = match home.as_deref() {
        Some(h) => root
            .ancestors()
            .skip(1)
            .take_while(|a| a.starts_with(h) && *a != h)
            .collect(),
        None => Vec::new(),
    };
    let leftovers = std::iter::once(root)
        .chain(ancestors.iter().copied())
        .flat_map(leftover_project_files);
    for path in leftovers {
        if is_global(&path) {
            continue;
        }
        if strip_leftover(&path) || (!path.exists() && drop_our_sidecar(&path)) {
            touched.push(path);
        }
    }
    let candidates = concatenating_project_rule_files(root)
        .into_iter()
        .chain(AGENT_PROJECT_FILES.iter().map(|n| root.join(n)))
        .map(|p| (root, p))
        .chain(ancestors.iter().flat_map(|a| {
            ["CLAUDE.md", "AGENTS.md", "CLAUDE.local.md"]
                .iter()
                .chain(AGENT_PROJECT_FILES)
                .map(move |n| (*a, a.join(n)))
        }));
    for (base, path) in candidates {
        if is_global(&path) {
            continue;
        }
        if !path.exists() {
            if drop_our_sidecar(&path) {
                touched.push(path);
            }
            continue;
        }
        let Ok(existing) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !existing.contains(SCAFFOLD_MARKER) && !existing.contains(OLD_CLAUDE_FENCE_START) {
            continue;
        }
        let note = path.file_name().and_then(|n| n.to_str()) != Some("CLAUDE.md");
        if write_scaffold_to(base, &path, note) == Some(true) && !touched.iter().any(|p| p == &path)
        {
            touched.push(path);
        }
    }
    touched
}

pub(crate) fn strip_old_claude_md_fence(path: &Path) -> bool {
    let Ok(existing) = std::fs::read_to_string(path) else {
        return false;
    };
    let Some(start) = existing.find(OLD_CLAUDE_FENCE_START) else {
        return false;
    };
    // Without its closing marker, only the opening line is provably ours.
    let end = match existing.find(OLD_CLAUDE_FENCE_END) {
        Some(end_at) if end_at >= start => end_at + OLD_CLAUDE_FENCE_END.len(),
        _ => start + OLD_CLAUDE_FENCE_START.len(),
    };
    let prefix = existing[..start].trim_end();
    let suffix = existing[end..].trim_start_matches('\n');
    let mut out = String::with_capacity(prefix.len() + suffix.len() + 2);
    out.push_str(prefix);
    if !prefix.is_empty() && !suffix.is_empty() {
        out.push_str("\n\n");
    } else if !prefix.is_empty() {
        out.push('\n');
    }
    out.push_str(suffix);
    if out.trim().is_empty() {
        return remove_with_sidecar(path);
    }
    crate::paths::write_atomic(path, &out).is_ok()
}

/// The lesson is already in what THIS agent loads outside the repo (see
/// [`effective_global_file`]).
pub(crate) fn agent_has_global_lecture(agent: Option<Agent>, home: &Path) -> bool {
    let Some(agent) = agent else {
        return false;
    };
    effective_global_file(agent, home).is_some_and(|p| file_has_lecture(&p))
}

/// A global copy already exists on this machine.
pub(crate) fn file_has_lecture(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .is_some_and(|s| s.contains(SCAFFOLD_MARKER))
}

/// Presence: a global is only written if the agent IS on the machine.
/// T3 / OpenClaw share the Claude file: a single write.
///
/// `presence_markers` is strict (Qwen requires `settings.json`). A `~/.qwen`
/// directory without that file still counts: it is the same signal as
/// [`detect_installed_agents`].
fn agents_with_global(home: &Path) -> Vec<Agent> {
    Agent::all()
        .iter()
        .copied()
        .filter(|a| global_rule_file(*a, home).is_some() && agent_home_present(*a, home))
        .collect()
}

pub(crate) fn agent_home_present(agent: Agent, home: &Path) -> bool {
    if agent
        .presence_markers(Some(home))
        .iter()
        .any(|p| p.exists())
    {
        return true;
    }
    match agent {
        Agent::Qwen => home.join(".qwen").is_dir(),
        Agent::Grok => home.join(".grok").is_dir(),
        Agent::Cursor => home.join(".cursor").is_dir(),
        Agent::Hermes => home.join(".hermes").is_dir(),
        Agent::Cline => home.join(".cline").is_dir() || home.join(".clinerules").exists(),
        Agent::Codex => home.join(".codex").is_dir(),
        Agent::Droid => home.join(".factory").is_dir(),
        Agent::Pi => home.join(".pi").join("agent").is_dir(),
        Agent::ClaudeCode => crate::platform::claude_config_dir_for(home).exists(),
        Agent::T3Code => home.join(".t3").is_dir() || home.join(".t3code").is_dir(),
        Agent::OpenClaw => {
            home.join(".openclaw").is_dir() || home.join(".config").join("openclaw").is_dir()
        }
        Agent::Antigravity => home.join(".gemini").join("antigravity").exists(),
        _ => false,
    }
}

/// Writes (or splices) the CLI block into the global of every present agent.
/// Idempotent. Outside the repo: [`crate::paths::write_atomic`].
///
/// Returns the paths touched (files already up to date are not listed).
pub fn ensure_global_rules(home: Option<&Path>) -> Vec<PathBuf> {
    let Some(home) = home else {
        return Vec::new();
    };
    let mut written = Vec::new();
    // Before writing the lesson: the short 1.1.18 fence would stay next to it.
    let claude_md = crate::platform::claude_config_dir_for(home).join("CLAUDE.md");
    if strip_old_claude_md_fence(&claude_md) {
        written.push(claude_md.clone());
    }
    let present = agents_with_global(home);
    let mut groups: std::collections::BTreeMap<PathBuf, Vec<Agent>> = Default::default();
    for agent in &present {
        if let Some(file) = effective_global_file(*agent, home) {
            groups.entry(file).or_default().push(*agent);
        }
    }
    for (path, readers) in &groups {
        let prefix = if path.extension().and_then(|e| e.to_str()) == Some("mdc") {
            CURSOR_MDC_PREFIX
        } else {
            ""
        };
        // Claude propagates CLAUDE.md to subagents; the others lack that channel.
        let note = readers
            .iter()
            .any(|a| !matches!(a, Agent::ClaudeCode | Agent::T3Code | Agent::OpenClaw));
        if write_global_for_readers(path, readers, note, prefix) == Some(true) {
            written.push(path.clone());
        }
    }
    // The OWN file of an agent that reads the lesson elsewhere: a second
    // copy (Kilo always loads `~/.kilo/rules/*.md` AND, as a fallback,
    // `~/.claude/CLAUDE.md` — captured ×2 on 2026-09-28).
    for agent in &present {
        let (Some(own), Some(eff)) = (
            global_rule_file(*agent, home),
            effective_global_file(*agent, home),
        ) else {
            continue;
        };
        if own != eff && !groups.contains_key(&own) && strip_leftover(&own) {
            written.push(own);
        }
    }
    for path in leftover_global_files(home) {
        if strip_leftover(&path) {
            written.push(path);
        }
    }
    written
}

/// The global file where THIS agent reads the lesson. Grok always loads
/// `~/.claude/CLAUDE.md` (compat). OpenCode and Kilo only fall back to it if
/// their own global `AGENTS.md` does not exist: creating one to put the
/// lesson in would MASK the user's `CLAUDE.md` — so they read Claude's,
/// unless the user already has their own (then we write there).
pub(crate) fn effective_global_file(agent: Agent, home: &Path) -> Option<PathBuf> {
    let own = global_rule_file(agent, home)?;
    let falls_back = match agent {
        Agent::Grok => true,
        Agent::OpenCode => !has_user_content(&own),
        Agent::KiloCode => !has_user_content(&home.join(".config").join("kilo").join("AGENTS.md")),
        _ => false,
    };
    if falls_back && agent_home_present(Agent::ClaudeCode, home) {
        return global_rule_file(Agent::ClaudeCode, home);
    }
    Some(own)
}

/// The file carries something other than our block and our header.
fn has_user_content(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let rest = without_marked_block(&text).unwrap_or(text);
    !rest.trim().is_empty() && !leftover_is_wrapper_only(&rest)
}

/// Removes the marked block from every known global. A file left empty →
/// deleted (like the `~/.claude/CLAUDE.md` fence on uninstall).
pub fn retract_global_rules(home: Option<&Path>) -> Vec<PathBuf> {
    let Some(home) = home else {
        return Vec::new();
    };
    let mut touched = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let claude_md = crate::platform::claude_config_dir_for(home).join("CLAUDE.md");
    if strip_old_claude_md_fence(&claude_md) {
        touched.push(claude_md);
    }
    for agent in Agent::all() {
        let Some(path) = global_rule_file(*agent, home) else {
            continue;
        };
        if !seen.insert(path.clone()) {
            continue;
        }
        if strip_leftover(&path) {
            touched.push(path);
        }
    }
    for path in leftover_global_files(home) {
        if strip_leftover(&path) {
            touched.push(path);
        }
    }
    touched
}

/// Paths Grok loads in addition to the native file. A `tokenade map` here is
/// a duplicate, with or without a marker. An "Auto-scaffolded" header with
/// no lesson is still one more file in the prompt: we remove it.
pub(crate) fn strip_leftover(path: &Path) -> bool {
    let stripped = strip_marked_block(path);
    let Ok(existing) = std::fs::read_to_string(path) else {
        return stripped;
    };
    if leftover_is_wrapper_only(&existing) {
        return remove_with_sidecar(path) || stripped;
    }
    stripped
}

/// Only our markers and intro remain — nothing of the user's.
fn leftover_is_wrapper_only(s: &str) -> bool {
    let (pre, post, found) = super::write::take_user_sides(s);
    found && pre.trim().is_empty() && post.trim().is_empty()
}

fn strip_marked_block(path: &Path) -> bool {
    let Ok(existing) = std::fs::read_to_string(path) else {
        return false;
    };
    let Some(out) = without_marked_block(&existing) else {
        return false;
    };
    if out.trim().is_empty() {
        return remove_with_sidecar(path);
    }
    crate::paths::write_atomic(path, &out).is_ok()
}

/// The file without our marked block; `None` if it does not carry one.
fn without_marked_block(existing: &str) -> Option<String> {
    let start = existing.find(SCAFFOLD_MARKER)?;
    let end_at = existing.find(SCAFFOLD_END_MARKER)?;
    if end_at < start {
        return None;
    }
    let end = end_at + SCAFFOLD_END_MARKER.len();
    let prefix = existing[..start].trim_end();
    let suffix = existing[end..].trim_start_matches('\n');
    let mut out = String::with_capacity(prefix.len() + suffix.len() + 2);
    out.push_str(prefix);
    if !prefix.is_empty() && !suffix.is_empty() {
        out.push_str("\n\n");
    } else if !prefix.is_empty() {
        out.push('\n');
    }
    out.push_str(suffix);
    Some(out)
}

/// What [`retract_global_rules`] would touch, without writing anything: path,
/// and `true` when the file would go (only we remained). The `uninstall`
/// dry-run announces it — silent, it let people believe no global file would
/// be touched.
pub fn plan_retract_global_rules(home: Option<&Path>) -> Vec<(PathBuf, bool)> {
    let Some(home) = home else {
        return Vec::new();
    };
    let mut seen = std::collections::BTreeSet::new();
    let candidates =
        std::iter::once(crate::platform::claude_config_dir_for(home).join("CLAUDE.md"))
            .chain(
                Agent::all()
                    .iter()
                    .filter_map(|a| global_rule_file(*a, home)),
            )
            .chain(leftover_global_files(home));
    let mut plan = Vec::new();
    for path in candidates {
        if !seen.insert(path.clone()) {
            continue;
        }
        let Ok(existing) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(rest) = without_marked_block(&existing) else {
            continue;
        };
        let removes = rest.trim().is_empty() || leftover_is_wrapper_only(&rest);
        plan.push((path, removes));
    }
    plan
}

/// Deletes a file that only carried us, and its `<file>.tokenade-original`
/// copy when it too only carries us: taken at our second write of a file WE
/// had created, it keeps no user state, and stayed in the repo until an
/// `uninstall` (ten copies at the root of this repo, 2026-09-28). A copy that
/// carries anything other than us is the user's original: it stays.
pub(crate) fn remove_with_sidecar(path: &Path) -> bool {
    let removed = std::fs::remove_file(path).is_ok();
    if removed {
        drop_our_sidecar(path);
    }
    removed
}

/// The `<path>.tokenade-original` copy goes if it only carries us. Also
/// called when `path` no longer exists: the orphaned copies from before this
/// fix.
fn drop_our_sidecar(path: &Path) -> bool {
    let mut side = path.as_os_str().to_owned();
    side.push(crate::paths::PRISTINE_SUFFIX);
    let side = PathBuf::from(side);
    let Ok(text) = std::fs::read_to_string(&side) else {
        return false;
    };
    let rest = without_marked_block(&text).unwrap_or(text);
    (rest.trim().is_empty() || leftover_is_wrapper_only(&rest))
        && std::fs::remove_file(&side).is_ok()
}

/// Tokens of tokenade's block this agent loads into EVERY session: the marked
/// section of its global rules file (or the whole file, where the file is
/// ours — `tokenade.md`, `tokenade.mdc`), plus the marked section of the
/// project's `AGENTS.md` (`CLAUDE.md` for Claude Code). What the files hold, not what we would write: a
/// block the user removed costs nothing.
pub fn loaded_block_tokens(agent: Agent, home: &Path, cwd: Option<&Path>) -> usize {
    let section = |p: &Path| -> usize {
        let Ok(text) = std::fs::read_to_string(p) else {
            return 0;
        };
        let ours = p.file_stem().and_then(|s| s.to_str()) == Some("tokenade");
        match (text.find(SCAFFOLD_MARKER), text.find(SCAFFOLD_END_MARKER)) {
            (Some(a), Some(b)) if b > a => {
                crate::tokenizer::estimate(&text[a..b + SCAFFOLD_END_MARKER.len()])
            }
            _ if ours => crate::tokenizer::estimate(&text),
            _ => 0,
        }
    };
    let global = global_rule_file(agent, home);
    let mut n = global.as_deref().map(section).unwrap_or(0);
    let project_file = match agent {
        Agent::ClaudeCode => "CLAUDE.md",
        _ => "AGENTS.md",
    };
    if let Some(project) = cwd.map(|c| c.join(project_file)) {
        if global.as_deref() != Some(project.as_path()) {
            n += section(&project);
        }
    }
    n
}

#[cfg(test)]
mod loaded_block_tests {
    use super::*;

    #[test]
    fn only_what_the_files_hold_is_counted() {
        let home = std::env::temp_dir().join(format!("tk-loaded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let rules = home.join(".factory").join("AGENTS.md");
        std::fs::create_dir_all(rules.parent().unwrap()).unwrap();
        std::fs::write(
            &rules,
            format!(
                "# mine\n{SCAFFOLD_MARKER}\n{}\n{SCAFFOLD_END_MARKER}\nmore of mine\n",
                "rule ".repeat(200)
            ),
        )
        .unwrap();
        let with = loaded_block_tokens(Agent::Droid, &home, None);
        assert!(with > 100, "{with}");
        std::fs::write(&rules, "# mine, no block\n").unwrap();
        assert_eq!(loaded_block_tokens(Agent::Droid, &home, None), 0);
        let _ = std::fs::remove_dir_all(&home);
    }
}
