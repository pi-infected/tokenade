//! #8 — git inside a worktree-isolated Claude Code agent.
//!
//! Claude Code refuses, in an agent isolated in `<repo>/.claude/worktrees/…`,
//! any command where git is an OPERAND of another program: it cannot show that
//! git targets the agent's own worktree. `tokenade wrap 'git status'` is such a
//! command, so the usual rewrite broke every git call of those agents.
//!
//! Probed live in an isolated agent (Claude Code 2.1.x): refused are
//! `tokenade wrap 'git …'` and `git … | tokenade filter git …`; accepted are
//! plain `git …` and `git … 2>&1 | tokenade filter <no git word>`. So git still
//! runs as written — what it runs stays readable — and its output is piped into
//! `filter`, which gets the original command through a staged file rather than
//! its argv. Compaction is kept; only the plumbing changes.

use super::*;

const STAGE_PREFIX: &str = "filterargv-";

/// Is this hook call made from inside a Claude Code isolated worktree?
pub(crate) fn in_isolated_worktree(parsed: &serde_json::Value) -> bool {
    parsed
        .get("cwd")
        .and_then(|c| c.as_str())
        .is_some_and(|c| c.replace('\\', "/").contains("/.claude/worktrees/"))
}

/// Does any word of `cmd` run git (`git`, `/usr/bin/git`, `git.exe`)?
/// Over-matching (`grep git f`) only costs the compaction of that call.
pub(crate) fn mentions_git(cmd: &str) -> bool {
    cmd.split(|c: char| c.is_whitespace() || ";&|()`$<>'\"{}".contains(c))
        .filter_map(|w| w.rsplit(['/', '\\']).next())
        .any(|w| w == "git" || w.eq_ignore_ascii_case("git.exe"))
}

/// The pipe form for ONE plain git command, or `None` when the command is
/// anything else (chain, pipe, redirection, substitution): those run unwrapped.
pub(crate) fn git_pipe_form(cmd: &str, exe: &str, parsed: &serde_json::Value) -> Option<String> {
    if extract_first_meaningful_word(cmd) != "git"
        || cmd.contains(['|', '>', '<', '`', '$', ';', '&', '\n', '(', ')'])
    {
        return None;
    }
    // Unique per call: two agents running the same git command at once must
    // not consume each other's staged argv.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let key = format!(
        "{:016x}",
        fnv1a(&format!("{cmd}\0{nanos}\0{}", std::process::id()))
    );
    let path = crate::hooks::read_staging::read_pending_dir().join(format!("{STAGE_PREFIX}{key}"));
    tokenade_core::platform::ensure_parent_dir(&path);
    std::fs::write(&path, cmd).ok()?;
    let env = session_env_prefix(parsed);
    // `pipefail`: the call must still fail when git does, as under `wrap`.
    Some(format!(
        "set -o pipefail; {cmd} 2>&1 | {env}{exe} filter --deliver --staged {key}"
    ))
}

/// `filter --staged <key>`: the argv staged by [`git_pipe_form`], consumed once.
pub(crate) fn take_staged_argv(key: &str) -> Option<Vec<String>> {
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let path = crate::hooks::read_staging::read_pending_dir().join(format!("{STAGE_PREFIX}{key}"));
    let cmd = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    Some(cmd.split_whitespace().map(str::to_string).collect())
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}
