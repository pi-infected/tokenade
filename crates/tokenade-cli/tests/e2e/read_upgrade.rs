//! **The upgrade heals the lesson for every agent, whether Claude Code is
//! present or not.**
//!
//! A HOME in the state 1.1.19 left behind (globals topped with the project
//! header, pointers in the repo, `~/.kilo/rules/tokenade.md` next to the Claude
//! fallback, a full `.github/copilot-instructions.md` next to Copilot's
//! global). A single `tokenade --version` — what the updater runs after the
//! swap, and every hook — then, for each agent, the set of files it LOADS
//! (read from its package on 2026-09-28, docs/PLAN-lecon-agents.md) must carry
//! the lesson exactly once, with no leftovers.
use super::*;

const TITLE: &str = "## Explore code with the `tokenade` CLI";
const STALE: &[&str] = &["do not open another copy", "Auto-scaffolded by tokenade"];

fn old_block() -> String {
    format!(
        "<!-- tokenade-scaffold -->\n{TITLE} (old)\n`tokenade map` old body\n\n## Keep output lean\nold\n<!-- /tokenade-scaffold -->\n"
    )
}
fn header(body: &str) -> String {
    // The intro 2.x writes, word for word: since 2.0 only markers and this
    // exact intro identify what is ours.
    format!("# Project rules for AI coding agents\n\nAuto-scaffolded by tokenade on first MCP session. Safe to edit; the tokenade block below is identified by the HTML marker and will be updated in-place on future tokenade upgrades.\n\n{body}")
}
fn pointer() -> String {
    header("<!-- tokenade-scaffold -->\n## Project rules\nThe `tokenade` CLI is available globally. Its usage rules are loaded from this machine's agent config — if the Tokenade instruction is already in the context, do not open another copy.\n<!-- /tokenade-scaffold -->\n")
}

fn write(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}
fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}
fn dir_md(d: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(d)
        .map(|r| {
            r.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == ext))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}
fn first(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .find(|p| p.exists())
        .cloned()
        .into_iter()
        .collect()
}

/// What each agent loads, per agent (the plan's P0 census).
fn load_sets(h: &Path, r: &Path) -> Vec<(&'static str, Vec<PathBuf>)> {
    let claude = h.join(".claude/CLAUDE.md");
    let mut kilo = first(&[h.join(".config/kilo/AGENTS.md"), claude.clone()]);
    kilo.extend(dir_md(&h.join(".kilo/rules"), "md"));
    kilo.extend(dir_md(&h.join(".kilocode/rules"), "md"));
    kilo.extend(first(&[r.join("AGENTS.md"), r.join("CLAUDE.md")]));
    let mut opencode = first(&[h.join(".config/opencode/AGENTS.md"), claude.clone()]);
    opencode.extend(first(&[r.join("AGENTS.md"), r.join("CLAUDE.md")]));
    let mut grok = vec![
        h.join(".grok/AGENTS.md"),
        claude.clone(),
        r.join("AGENTS.md"),
        r.join("CLAUDE.md"),
    ];
    grok.extend(dir_md(&h.join(".grok/rules"), "md"));
    grok.extend(dir_md(&h.join(".cursor/rules"), "md"));
    let mut cursor = vec![
        r.join(".cursorrules"),
        r.join("AGENTS.md"),
        r.join("CLAUDE.md"),
    ];
    cursor.extend(dir_md(&h.join(".cursor/rules"), "mdc"));
    let mut cline = vec![
        h.join(".agents/AGENTS.md"),
        r.join("AGENTS.md"),
        r.join(".clinerules"),
    ];
    cline.extend(dir_md(&h.join(".cline/rules"), "md"));
    let mut pi = first(&[h.join(".pi/agent/AGENTS.md"), h.join(".pi/agent/CLAUDE.md")]);
    pi.extend(first(&[r.join("AGENTS.md"), r.join("CLAUDE.md")]));
    vec![
        ("claude-code", vec![claude.clone(), r.join("CLAUDE.md")]),
        (
            "codex",
            vec![h.join(".codex/AGENTS.md"), r.join("AGENTS.md")],
        ),
        (
            "antigravity",
            vec![
                h.join(".gemini/GEMINI.md"),
                r.join("GEMINI.md"),
                r.join("AGENTS.md"),
            ],
        ),
        (
            "qwen",
            vec![
                h.join(".qwen/QWEN.md"),
                h.join(".qwen/AGENTS.md"),
                r.join("QWEN.md"),
                r.join("AGENTS.md"),
            ],
        ),
        ("cursor", cursor),
        ("grok", grok),
        ("opencode", opencode),
        ("kilo", kilo),
        (
            "copilot-cli",
            vec![
                h.join(".copilot/copilot-instructions.md"),
                r.join(".github/copilot-instructions.md"),
                r.join("AGENTS.md"),
                r.join("CLAUDE.md"),
                r.join("GEMINI.md"),
            ],
        ),
        (
            "droid",
            vec![
                h.join(".factory/AGENTS.md"),
                h.join(".factory/CLAUDE.md"),
                r.join("AGENTS.md"),
                r.join("CLAUDE.md"),
            ],
        ),
        ("pi", pi),
        ("cline", cline),
        (
            "devin-desktop",
            vec![
                h.join(".codeium/windsurf/memories/global_rules.md"),
                r.join(".windsurfrules"),
                r.join("AGENTS.md"),
            ],
        ),
    ]
}

fn upgrade_heals_every_agent(with_claude: bool) {
    let h = TempHome::new(if with_claude {
        "read-up-claude"
    } else {
        "read-up-noclaude"
    });
    let home = h.path();
    for d in [
        ".codex",
        ".gemini/antigravity",
        ".qwen",
        ".cursor",
        ".grok",
        ".config/opencode",
        ".config/kilo",
        ".copilot",
        ".factory",
        ".pi/agent",
        ".cline",
        ".codeium/windsurf",
    ] {
        std::fs::create_dir_all(home.join(d)).unwrap();
    }
    if with_claude {
        write(
            &home.join(".claude/CLAUDE.md"),
            &format!("@RTK.md\n\n{}", old_block()),
        );
    }
    // 1.1.19 state: globals topped, Kilo / OpenCode / Copilot duplicates.
    for g in [
        ".codex/AGENTS.md",
        ".gemini/GEMINI.md",
        ".qwen/QWEN.md",
        ".factory/AGENTS.md",
        ".pi/agent/AGENTS.md",
        ".cline/rules/tokenade.md",
        ".kilo/rules/tokenade.md",
        ".config/opencode/AGENTS.md",
        ".copilot/copilot-instructions.md",
    ] {
        write(&home.join(g), &header(&old_block()));
    }
    write(
        &home.join(".cursor/rules/tokenade.mdc"),
        &format!("---\nalwaysApply: true\n---\n\n{}", header(&old_block())),
    );
    let repo = home.join("work/repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    for f in [
        "AGENTS.md",
        "CLAUDE.md",
        "GEMINI.md",
        "QWEN.md",
        ".cursorrules",
        ".grokrules",
        ".clinerules",
    ] {
        write(&repo.join(f), &pointer());
    }
    write(
        &repo.join(".github/copilot-instructions.md"),
        &header(&old_block()),
    );
    // `.windsurfrules` that 1.1.19 left in full: Devin Desktop reads it, and
    // its lesson must move into its global.
    write(&repo.join(".windsurfrules"), &header(&old_block()));
    write(&home.join("work/CLAUDE.md"), &pointer());

    let dst = home.join(".tokenade-testbin").join("tokenade");
    std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
    if std::fs::hard_link(shared_binary(), &dst).is_err() {
        std::fs::copy(binary(), &dst).unwrap();
    }
    let out = tokenade_core::process_util::retry_while_text_file_busy(|| {
        Command::new(&dst)
            .args(["--version", "--no-auto-update"])
            .current_dir(&repo)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", home)
            .env("USERPROFILE", home)
            .output()
    })
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let mut bad = Vec::new();
    for (agent, files) in load_sets(home, &repo) {
        if agent == "claude-code" && !with_claude {
            continue;
        }
        let text: String = files.iter().map(|p| read(p)).collect::<Vec<_>>().join("\n");
        let n = text.matches(TITLE).count();
        let stale: Vec<_> = STALE.iter().filter(|s| text.contains(**s)).collect();
        if n != 1 || !stale.is_empty() || text.contains("(old)") {
            bad.push(format!(
                "{agent}: lesson ×{n}, stale {stale:?}, old body {}",
                text.contains("(old)")
            ));
        }
    }
    assert!(!read(&home.join(".claude/CLAUDE.md")).contains("@RTK.md") || with_claude);
    assert!(
        !home.join("work/CLAUDE.md").exists(),
        "parent-dir pointer survived"
    );
    assert!(
        bad.is_empty(),
        "after one `tokenade --version`:\n{}",
        bad.join("\n")
    );
}

#[test]
fn upgrade_heals_every_agent_without_claude_code() {
    upgrade_heals_every_agent(false);
}

#[test]
fn upgrade_heals_every_agent_with_claude_code() {
    upgrade_heals_every_agent(true);
}
