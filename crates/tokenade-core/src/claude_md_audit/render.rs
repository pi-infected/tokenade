//! **What the audit REPORTS.**
//!
//! A report names the file, the line and the reason. A report that says
//! only « 12 duplicates » forces redoing the work by hand, and
//! nobody does it.
//!
//! Stale references are the half that gets forgotten: a `CLAUDE.md` that
//! points to a vanished path sends the agent looking for what no longer exists,
//! every session.

use super::*;

/// Per-agent label for the rules cascade header in `render_with_label`.
pub fn rules_label_for_agent(agent: &crate::detect::Agent) -> &'static str {
    use crate::detect::Agent;
    match agent {
        Agent::ClaudeCode | Agent::T3Code | Agent::Unknown => "CLAUDE.md cascade",
        Agent::Cursor => ".cursorrules cascade",
        Agent::Cline => ".clinerules cascade",
        Agent::DevinDesktop => ".windsurfrules / AGENTS.md cascade",
        Agent::Copilot | Agent::CopilotVsCode => ".github/copilot-instructions.md cascade",
        Agent::Codex => "AGENTS.md cascade",
        Agent::Droid => "AGENTS.md cascade",
        Agent::Pi => "AGENTS.md cascade",
        Agent::OpenCode => "AGENTS.md / opencode.md cascade",
        Agent::KiloCode => ".kilocoderules cascade",
        Agent::Aider => "CONVENTIONS.md cascade",
        Agent::Hermes => ".hermes.md then cwd AGENTS.md (first match)",
        Agent::Qwen => "QWEN.md cascade",
        Agent::Grok => ".grokrules cascade",
        Agent::OpenClaw | Agent::Antigravity => "agent rules cascade",
        Agent::Odysseus => "Odysseus soul.md",
    }
}

/// Render an audit as a compact markdown report, labelled with `cascade_label`
/// (e.g. "CLAUDE.md cascade", ".cursorrules cascade").
pub fn render_with_label(audit: &CascadeAudit, cascade_label: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!("[tokenade:claude-md-audit] {cascade_label}\n\n"));

    if audit.files.is_empty() {
        s.push_str(&format!("No {cascade_label} files found.\n"));
        return s;
    }

    s.push_str(&format!(
        "{} file(s), {} tokens always-resident in the prefix.\n\n",
        audit.files.len(),
        audit.total_tokens
    ));

    s.push_str("| tokens | lines | file |\n|---:|---:|---|\n");
    for f in &audit.files {
        s.push_str(&format!("| {} | {} | {} |\n", f.tokens, f.lines, f.path));
    }

    if audit.duplicates.is_empty() {
        s.push_str("\nNo cross-file duplication detected.\n");
        return s;
    }

    s.push_str(&format!(
        "\n~{} tokens duplicated across files (recoverable by hoisting each line to one place):\n\n",
        audit.duplicated_tokens
    ));
    // Cap the listing so the report itself stays small.
    let cap = 15usize;
    for d in audit.duplicates.iter().take(cap) {
        let preview: String = d.text.chars().take(72).collect();
        let ellipsis = if d.text.chars().count() > 72 {
            "…"
        } else {
            ""
        };
        s.push_str(&format!(
            "- {} tok ×{} files: \"{}{}\"\n",
            d.wasted_tokens(),
            d.files.len(),
            preview,
            ellipsis
        ));
    }
    if audit.duplicates.len() > cap {
        // Name the tokens as well as the count. The header promises a total
        // ("~404 tokens duplicated") and the visible list sums to something
        // smaller; without the remainder's weight a reader cannot tell whether
        // the tail is worth hoisting, or reconcile the two numbers at all.
        // Measured on a real six-file cascade: 404 claimed, 262 shown, and the
        // line said only "and 11 more duplicated line(s)". Same shape as the
        // `pack` manifest cap — counting is not the same as saying nothing.
        let rest: usize = audit
            .duplicates
            .iter()
            .skip(cap)
            .map(DuplicateLine::wasted_tokens)
            .sum();
        s.push_str(&format!(
            "- … and {} more duplicated line(s), {} tok.\n",
            audit.duplicates.len() - cap,
            rest
        ));
    }

    s
}

/// Backward-compat wrapper — always uses the "CLAUDE.md cascade" label.
/// New callers should prefer `render_with_label` with `rules_label_for_agent`.
pub fn render(audit: &CascadeAudit) -> String {
    render_with_label(audit, "CLAUDE.md cascade")
}

/// A reference in the rules cascade that points at a project path no longer
/// present on disk — stale guidance the agent will still read and act on (#8).
#[derive(Debug, Clone)]
pub struct StaleRef {
    pub source_file: String,
    pub reference: String,
}

/// Whether a token looks like an in-repo source path worth existence-checking.
/// Conservative: must contain a `/`, end in a known source extension, and not
/// be a URL / glob / absolute path — so we only ever flag unambiguous file
/// references, never prose that happens to contain a slash.
pub(crate) fn looks_like_repo_path(tok: &str) -> bool {
    if tok.len() < 5 || !tok.contains('/') || tok.contains(' ') || tok.contains('*') {
        return false;
    }
    if tok.starts_with('/') || tok.starts_with("http") || tok.starts_with("~") {
        return false;
    }
    const EXTS: &[&str] = &[
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".java", ".rb", ".c", ".h", ".cpp",
        ".json", ".toml", ".yaml", ".yml", ".md", ".sh", ".sql",
    ];
    EXTS.iter().any(|e| tok.ends_with(e))
}

/// #8 — flag file references in the rules cascade that no longer resolve under
/// `root`: a CLAUDE.md still pointing at a renamed/deleted file is stale
/// guidance the agent acts on. Deliberately scoped to *file paths* (an
/// unambiguous signal) rather than symbol names (which need a full index and
/// are false-positive-prone). Reporter only — never mutates.
pub fn stale_refs(cascade: &[(String, String)], root: &Path) -> Vec<StaleRef> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (src, content) in cascade {
        for raw in content.split(|c: char| c.is_whitespace() || "`\"'()[]{}<>,;:!?".contains(c)) {
            let tok = raw.trim_matches(|c: char| c == '.' || c == ',');
            if !looks_like_repo_path(tok) {
                continue;
            }
            if !seen.insert((src.clone(), tok.to_string())) {
                continue;
            }
            if !root.join(tok).exists() {
                out.push(StaleRef {
                    source_file: src.clone(),
                    reference: tok.to_string(),
                });
            }
        }
    }
    out
}

/// #9 — lossless whitespace compaction of a rules file: trailing whitespace
/// stripped, runs of blank lines collapsed to one, leading/trailing blanks
/// dropped. Every non-blank line (section numbers, cross-references, code
/// fences) survives verbatim, so it is safe to apply unattended — unlike the
/// lossy "shorten the prose" passes competitor tools run.
pub fn whitespace_compress(content: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for line in content.lines() {
        let t = line.trim_end();
        if t.is_empty() && lines.last().map(|l| l.is_empty()).unwrap_or(true) {
            continue; // collapse consecutive blanks; drop leading blanks
        }
        lines.push(t);
    }
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

/// Apply [`whitespace_compress`] to each cascade file in place, with the usual
/// before/after token accounting.
pub fn apply_compress(start: &Path, home: Option<&Path>) -> ApplyResult {
    let mut files_modified = Vec::new();
    let mut tokens_before = 0usize;
    let mut tokens_after = 0usize;
    for (p, content) in existing_cascade(start, home) {
        // Write through a symlink, not over it: `write_atomic` renames a temp
        // file onto the path, which would replace the link with a copy.
        let target = p.canonicalize().unwrap_or_else(|_| p.clone());
        tokens_before += tokenizer::estimate(&content);
        let compressed = whitespace_compress(&content);
        tokens_after += tokenizer::estimate(&compressed);
        if compressed != content && crate::paths::write_atomic(&target, &compressed).is_ok() {
            files_modified.push(p.display().to_string());
        }
    }
    ApplyResult {
        files_modified,
        tokens_before,
        tokens_after,
    }
}
