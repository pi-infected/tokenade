//! **A command word, and the other names that name IT.**
//!
//! This table exists because per-command lists used to read `args[0]` as-is
//! and were maintained by hand. The day they were checked against the
//! dispatcher's arms, **twenty-seven commands had an alias classified
//! differently from themselves** — and the check runs BEFORE the `match`, so
//! the alias arrived unresolved:
//!
//! - `expand_ref` and `expand_fold` were treated differently from
//!   `expand-ref` and `expand-fold`, although this is the retrieval path:
//!   tokenade replaced content with a pointer, and failing to honour the
//!   pointer would lose data the user already had.
//! - `self-update` and `selfupdate` were treated differently from `upgrade`,
//!   under two names out of three.
//! - `mcp-wrap`, the CANONICAL name, was missing where its deprecated alias
//!   `mcp-proxy` was listed — and it speaks JSON-RPC, so prose could end up
//!   in a protocol that does not read any.
//! - and twenty-odd others, nearly all underscore twins.
//!
//! The table is DERIVED from the arms of the `match` in `main.rs` — that is
//! what decides what a command is — and a test checks it both ways. The
//! first name that is not a flag is the PRIMARY name; the others are its
//! aliases and inherit everything said about it.

/// `(primary name, aliases)`, one tuple per dispatcher arm.
pub(crate) const COMMAND_ALIASES: &[(&str, &[&str])] = &[
    ("version", &["--version", "-V"]),
    ("proxy", &[]),
    ("shim-exec", &[]),
    ("raw", &["bypass", "noproxy"]),
    ("filter", &[]),
    ("index", &[]),
    ("index-touch", &[]),
    ("query", &["find"]),
    ("impact", &[]),
    ("map", &[]),
    ("skeleton", &[]),
    ("web", &[]),
    ("crawl", &[]),
    ("search", &["serp", "google", "bing", "ddg"]),
    ("snapshot", &[]),
    ("read", &[]),
    ("ask", &["semantic", "sem"]),
    ("subagent-brief", &["subagent_brief"]),
    ("session-tick", &[]),
    ("fold-payload", &["fold_payload"]),
    ("expand-ref", &["expand_ref", "xref"]),
    ("expand-fold", &["expand_fold", "xfold"]),
    ("style", &[]),
    ("watch", &[]),
    ("dashboard", &["dash", "stats"]),
    ("gain", &[]),
    ("audit-gains", &[]),
    ("statusline", &[]),
    ("hookwrap-copilot", &[]),
    ("gemini-after-tool", &[]),
    ("gemini-lifecycle", &[]),
    ("real-savings-refresh", &[]),
    ("reporter-run", &[]),
    ("measure", &[]),
    ("judge", &["groundedness"]),
    ("read-mode", &["readmode"]),
    ("pack", &[]),
    ("review-context", &["review"]),
    ("mcp-stats", &["mcpstats"]),
    ("mistake", &["mistakes"]),
    ("disclose", &[]),
    ("turn-summary", &["summarize-turn", "turnsummary"]),
    ("add-compactor", &["add-filter"]),
    ("receipt", &["savings-receipt"]),
    ("cache-note", &["cachenote"]),
    ("execute", &["exec", "run-script"]),
    ("detect", &[]),
    (
        "incompatibilities",
        &["incompat", "detect-competitors", "competitors", "scan"],
    ),
    ("discover", &[]),
    ("healthcheck", &["health", "doctor"]),
    (
        "audit-claude-md",
        &["audit-claudemd", "claude-md-audit", "claudemd"],
    ),
    ("hooks", &[]),
    ("install", &[]),
    ("hookwrap", &[]),
    ("gemini-hook", &["geminihook"]),
    ("gemini-afterhook", &[]),
    ("shell-init", &["shell_init"]),
    ("shellwrap", &[]),
    ("hookwrap-pretool", &[]),
    ("hookwrap-postread", &[]),
    ("hookwrap-postwebfetch", &[]),
    ("hookwrap-websearch-post", &[]),
    ("hookwrap-mcp-post", &[]),
    ("hookwrap-cursor-mcp-post", &[]),
    ("hookwrap-userprompt", &[]),
    ("hookwrap-sessionstart", &[]),
    ("hookwrap-subagentstart", &[]),
    ("hookwrap-precompact", &[]),
    ("hookwrap-subagentstop", &[]),
    ("pre-edit-snapshot", &["hookwrap-pre-edit"]),
    ("wrap", &["exec-compact"]),
    ("upgrade", &["self-update", "selfupdate"]),
    ("semantic-status", &["semantic_status"]),
    ("evict-stale", &["evict_stale"]),
    ("compact-sessions", &["compact_sessions"]),
    ("scan-savings", &["scan_savings"]),
    ("scrub-secret", &["scrub_secret"]),
    ("scrub-cache", &["scrub_cache"]),
    ("redact", &[]),
    ("calibrate", &[]),
    ("bench", &["benchmark"]),
    ("replay", &[]),
    ("trust-filters", &["trust_filters"]),
    ("list-commands", &["list_commands"]),
    ("ensure-hooks", &["ensure_hooks"]),
    ("codex-trust", &["codex_trust"]),
    ("mcp-wrap", &["mcp_wrap", "mcp-proxy", "mcp_proxy"]),
    ("llm-proxy", &["llm_proxy"]),
    ("unwrap-mcps", &["unwrap_mcps"]),
    ("uninstall", &[]),
    ("help", &["--help", "-h"]),
];

/// The PRIMARY name of this command: `mcp_proxy` → `mcp-wrap`.
///
/// Returns the word unchanged when it is nobody's alias — a word the
/// dispatcher does not know stays itself, so the caller decides.
pub(crate) fn primary_of(cmd: &str) -> &str {
    for (primary, aliases) in COMMAND_ALIASES {
        if *primary == cmd {
            return primary;
        }
        if aliases.contains(&cmd) {
            return primary;
        }
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The arms of the `match` in `main.rs`, read from the SOURCE.
    ///
    /// The dispatcher decides what a command is; this table is a copy of it,
    /// and a copy gets checked. Both ways: an arm missing from the table would
    /// make `primary_of` silent about it (it would return itself, and the
    /// per-command lists would classify it apart from its siblings), a table
    /// row without an arm would name a command that does not exist.
    fn dispatch_groups() -> Vec<Vec<String>> {
        let src = include_str!("main.rs");
        let production = tokenade_core::srcsweep::production_source(src);
        // BOUNDED to the dispatcher's `match cmd {`. Without the bound, the
        // sweep picked up the `"-h" | "--h" => Some("--help")` of a typo
        // suggestion `match` twenty lines further down, and declared `-h` as
        // belonging to two commands.
        let at = production
            .find("match cmd {")
            .expect("the dispatcher: `match cmd {`");
        let body = &production[at..];
        let mut depth = 0i32;
        let mut out = Vec::new();
        for line in body.lines() {
            let before = depth;
            depth += line.chars().filter(|c| *c == '{').count() as i32;
            depth -= line.chars().filter(|c| *c == '}').count() as i32;
            if before == 0 && depth == 0 {
                continue;
            }
            if depth <= 0 {
                break; // the `match` is closed
            }
            if before != 1 {
                continue; // not at arm level
            }
            let t = line.trim_start();
            if !t.starts_with('"') {
                continue;
            }
            let Some(arrow) = t.find("=>") else { continue };
            let mut names = Vec::new();
            let mut ok = true;
            for part in t[..arrow].split('|') {
                let p = part.trim();
                if p.len() >= 2
                    && p.starts_with('"')
                    && p.ends_with('"')
                    && !p[1..p.len() - 1].contains('"')
                {
                    names.push(p[1..p.len() - 1].to_string());
                } else {
                    ok = false;
                    break;
                }
            }
            if ok && !names.is_empty() {
                out.push(names);
            }
        }
        out
    }

    #[test]
    fn the_table_is_the_dispatcher_neither_more_nor_less() {
        let groups = dispatch_groups();
        assert!(
            groups.len() >= 90,
            "only {} arms read — the sweep got lost",
            groups.len()
        );
        let from_src: std::collections::BTreeSet<Vec<String>> = groups.into_iter().collect();
        let from_table: std::collections::BTreeSet<Vec<String>> = COMMAND_ALIASES
            .iter()
            .map(|(p, al)| {
                let mut v: Vec<String> = vec![p.to_string()];
                v.extend(al.iter().map(|a| a.to_string()));
                v.sort();
                v
            })
            .collect();
        let from_src: std::collections::BTreeSet<Vec<String>> = from_src
            .into_iter()
            .map(|mut v| {
                v.sort();
                v
            })
            .collect();
        let missing: Vec<_> = from_src.difference(&from_table).collect();
        let extra: Vec<_> = from_table.difference(&from_src).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "the table and the dispatcher diverged:\n  arms without a row: {missing:?}\n  \
             rows without an arm: {extra:?}"
        );
    }

    /// A name can belong to only ONE command.
    #[test]
    fn no_name_belongs_to_two_commands() {
        let mut seen: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
        let mut dup = Vec::new();
        for (primary, aliases) in COMMAND_ALIASES {
            for n in std::iter::once(primary).chain(aliases.iter()) {
                if let Some(other) = seen.insert(n, primary) {
                    dup.push(format!("`{n}`: {other} and {primary}"));
                }
            }
        }
        assert!(
            dup.is_empty(),
            "one name for two commands:\n  {}",
            dup.join("\n  ")
        );
    }

    /// **What the per-command lists classify is a PRIMARY name.**
    ///
    /// For both lists in `hook_health`. The one that bit was `PROTOCOL_QUIET`:
    /// `mcp-wrap`, the canonical name, was not in it — only its deprecated
    /// alias `mcp-proxy` — so the « ✓ refreshed … » line of the post-upgrade
    /// refresh could go out into JSON-RPC, the class of stdout pollution that
    /// list's comment describes.
    #[test]
    fn the_per_command_lists_classify_primaries_only() {
        let src = include_str!("hook_health.rs");
        let mut wrong = Vec::new();
        for name in ["USER_FACING", "PROTOCOL_QUIET"] {
            let at = src
                .find(&format!("const {name}: &[&str] = &["))
                .unwrap_or_else(|| panic!("`{name}` disappeared from `hook_health.rs`"));
            let body = &src[at..];
            let end = body.find("];").expect("the table closes");
            let mut count = 0;
            for piece in body[..end].split('"').skip(1).step_by(2) {
                if piece.is_empty() || piece.contains(' ') {
                    continue;
                }
                count += 1;
                if primary_of(piece) != piece {
                    wrong.push(format!(
                        "    {name}: `{piece}` is an alias of `{}`",
                        primary_of(piece)
                    ));
                }
            }
            assert!(count >= 10, "{name}: only {count} entries read");
        }
        assert!(
            wrong.is_empty(),
            "these lists classify ALIASES; their siblings stay unclassified:\n{}",
            wrong.join("\n")
        );
    }

    /// Healing the global lectures is not an opt-in: it runs outside the
    /// version gate and outside TOKENADE_NO_AUTO_UPDATE, and `main` calls it
    /// before the post-upgrade refresh (including wrap / hooks).
    #[test]
    fn lecture_heal_is_not_an_opt_in() {
        let health = include_str!("hook_health.rs");
        let heal = health
            .split("fn maybe_heal_global_lectures")
            .nth(1)
            .expect("maybe_heal_global_lectures");
        let heal_body = &heal[..heal.find("\nfn ").expect("next fn after heal")];
        assert!(
            heal_body.contains("ensure_global_rules"),
            "heal must rewrite stacked lectures"
        );
        assert!(
            heal_body.contains("heal_project_lectures"),
            "heal must splice nested crate lectures to pointers"
        );
        assert!(
            !heal_body.contains("needs_post_upgrade_refresh"),
            "heal must not wait for a version bump"
        );
        assert!(
            !heal_body.contains("is_opted_out") && !heal_body.contains("TOKENADE_NO_AUTO_UPDATE"),
            "heal must ignore the auto-update opt-out"
        );
        assert!(
            !heal_body.contains("tokenade install"),
            "heal body must not mention install as a prerequisite"
        );

        let refresh = health
            .split("fn maybe_run_post_upgrade_refresh")
            .nth(1)
            .expect("maybe_run_post_upgrade_refresh");
        let refresh_fn = &refresh[..refresh.find("\npub(super) fn ").expect("next fn")];
        assert!(
            !refresh_fn.contains("ensure_global_rules"),
            "lecture heal must not live behind the upgrade gate:\n{refresh_fn}"
        );

        let main = include_str!("main.rs");
        let heal_at = main
            .find("maybe_heal_global_lectures()")
            .expect("main must call the heal");
        let refresh_at = main
            .find("maybe_run_post_upgrade_refresh()")
            .expect("main still runs the versioned refresh");
        assert!(
            heal_at < refresh_at,
            "heal must run even when the upgrade refresh returns early"
        );

        let swap = include_str!("../../tokenade-core/src/updater/swap.rs");
        let after_rename = swap
            .split("fs::rename(&tmp_path, exe_path)")
            .nth(1)
            .expect("atomic rename");
        assert!(
            after_rename.contains("--no-auto-update")
                // Split: otherwise the spawn scanner would read this needle as a spawn.
                && after_rename.contains(concat!("Command", "::new(exe_path)")),
            "upgrade + daily auto must exec the new live binary after swap"
        );
    }
}
