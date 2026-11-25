//! The command catalogue: what tokenade answers to, the one-line usage of
//! each, and what to say when the user typed something else.
//!
//! `help_routing` decides which help to print; this decides what a command
//! IS. Both read from `KNOWN_COMMANDS`, so a new command is spelled once.

use super::*;

// The commands themselves, moved out of the dispatcher. `commands.rs` already
// held the name table and the typo suggestion; the bodies move down next to
// it, in the same module family.
pub(crate) mod argv_helpers;
pub(crate) mod filter_cmd;
pub(crate) mod formatting;
pub(crate) mod index_cmds;

/// Commands whose `-h`/`--help` belongs to them, not to tokenade's global help
/// interception.
///
/// `shim-exec` stays listed even though it no longer forwards to a real tool:
/// a stale shim invokes it as `shim-exec df -- -h`, and without this the
/// global banner answers instead of the removal message (field report
/// 2026-07-13 — a disk widget parsed that banner as empty output).
pub(super) fn cmd_forwards_help(cmd: &str) -> bool {
    // PRIMARY names only. The `exec` comment below tells what the other
    // method cost: `exec` and `run-script` dispatch to the SAME handler as
    // `execute`, only `execute` was listed, and `tokenade exec --help`
    // answered "this command has no dedicated help yet" about a command that
    // has help — under the name the scaffold tells every agent to use. Fixed
    // by hand that day; fixed by CONSTRUCTION since the alias is resolved
    // here.
    matches!(
        crate::command_table::primary_of(cmd),
        // `llm-proxy` parses subcommands and flags the one-line global text
        // could not describe, and its own help carries the honest limit of
        // rank 4 — so the handler is the help.
        "llm-proxy"
            | "proxy"
            | "raw"
            | "wrap"
            | "filter"
            | "execute"
            // `exec` and `run-script` dispatch to the SAME handler as
            // `execute`, and only `execute` was listed here — so
            // `tokenade execute --help` printed the real help while
            // `tokenade exec --help` answered "this command has no dedicated
            // help yet". `exec` is the spelling the scaffold and the agent
            // rules tell every agent to use, so the documented name was the
            // broken one.
            | "shim-exec"
            // Each of these handlers ALREADY answers `--help` with a fuller
            // usage than the global one-liner — and none was listed, so the
            // global handler intercepted first and printed "this command has
            // no dedicated help yet" about a command that has help.
            //
            // Verified command by command against the BINARY, not by reading
            // this list: the handler must call `asks_for_help`, or forwarding
            // would send `--help` into running the command.
            | "pack"
            | "receipt"
            | "read-mode"
            | "mcp-stats"
            | "turn-summary"
            | "cache-note"
            | "shellwrap"
            // Handler usage strictly RICHER than the global one-liner (adds
            // examples, `--diagnose`, the saved-page mode). Deliberately NOT
            // here: `semantic`, `mistake`, `disclose` — their internal usage is
            // narrower or subcommand-specific, so forwarding would DOWNGRADE
            // what the user already gets. They need a real help entry instead.
            | "web"
            | "search"
            // Found by the guard, not by me: my hand-rolled census restricted
            // command names to [a-z0-9-], so the UNDERSCORE alias never
            // matched and the pair stayed invisible.
            | "shell-init"
            // Same defect as the block above, three commands further on, and
            // the reason the sweep below now derives this list instead of
            // trusting it: each of these three prints a full dedicated help
            // from inside its handler, behind an `asks_for_help` guard, and
            // none was listed — so the global interception fired first and
            // dumped the ENTIRE command catalogue instead. Measured on the
            // 1.1.9 binary: `review-context --help` answered with 100 lines
            // of unrelated commands, `judge --help` with "unknown command
            // 'judge' — did you mean `ddg`?" about a command that runs.
            | "judge"
            | "review-context"
            | "bench"
    )
}

pub(super) const KNOWN_COMMANDS: &[&str] = &[
    "read",
    "exec-compact",
    "self-update",
    "selfupdate",
    "upgrade",
    "ask",
    "audit-claude-md",
    "audit-claudemd",
    "claude-md-audit",
    "claudemd",
    "bench",
    "benchmark",
    "bing",
    "bypass",
    "cache-note",
    "cachenote",
    "competitors",
    "dash",
    "dashboard",
    "ddg",
    "detect",
    "compact-sessions",
    "scan-savings",
    "scan_savings",
    "detect-competitors",
    "disclose",
    "discover",
    "doctor",
    "evict-stale",
    "evict_stale",
    "expand-fold",
    // A subagent's brief, as TEXT: the six bridges ask the binary for it
    // instead of carrying a copy.
    "subagent-brief",
    // A tool call seen by a plugin bridge: the session clock.
    "session-tick",
    // Rank 4 without a proxy: a provider payload on stdin.
    "fold-payload",
    "expand-ref",
    "scrub-secret",
    "scrub_secret",
    "scrub-cache",
    "scrub_cache",
    "redact",
    "calibrate",
    "replay",
    "trust-filters",
    "trust_filters",
    "list-commands",
    "list_commands",
    "ensure-hooks",
    "ensure_hooks",
    "exec",
    "execute",
    "filter",
    "find",
    "gain",
    "google",
    "health",
    "healthcheck",
    "help",
    "gemini-hook",
    "geminihook",
    "hooks",
    "hookwrap",
    "hookwrap-postread",
    "hookwrap-postwebfetch",
    "hookwrap-precompact",
    "hookwrap-pre-edit",
    "hookwrap-pretool",
    "hookwrap-sessionstart",
    "hookwrap-userprompt",
    "impact",
    "incompat",
    "incompatibilities",
    "index",
    "index-touch",
    "install",
    "llm-proxy",
    "llm_proxy",
    "map",
    "mcp-wrap",
    "mcp-proxy",
    "mcp-stats",
    "mcp_wrap",
    "mcp_proxy",
    "mcpstats",
    "measure",
    "mistake",
    "mistakes",
    "noproxy",
    "pack",
    "pre-edit-snapshot",
    "proxy",
    "query",
    "raw",
    "read-mode",
    "readmode",
    "receipt",
    "review",
    "review-context",
    "run-script",
    "scan",
    "sem",
    "semantic",
    "semantic-status",
    "semantic_status",
    "search",
    "serp",
    "shell-init",
    "shell_init",
    "shellwrap",
    "shim-exec",
    "skeleton",
    "snapshot",
    "stats",
    "statusline",
    "style",
    "summarize-turn",
    "add-compactor",
    "add-filter",
    "turn-summary",
    "turnsummary",
    "uninstall",
    "unwrap-mcps",
    "unwrap_mcps",
    "version",
    "watch",
    "web",
    "wrap",
    // Dispatched names that this list did not know about. `respond` asks
    // KNOWN_COMMANDS whether a name is a command, so every one of these
    // answered `--help` with "unknown command '<itself>'" — including
    // `xref` and `xfold`, which `tokenade --help` documents as aliases on
    // the very same run, and whose "did you mean" then pointed at `read`
    // and `find`. Half of each family was already here (`expand-ref` but
    // not `expand_ref`, `hookwrap-postread` but not
    // `hookwrap-subagentstop`, `compact-sessions` but not
    // `compact_sessions`): drift, not a design. The sweep below derives
    // this set from the dispatch, so the next arm either lands here or
    // fails there.
    //
    // None of these carries an `oneline_help_for` entry, so `list-commands`
    // and the README docs guard are unchanged — this is about the binary
    // not denying a command it runs.
    "audit-gains",
    "bench",
    "codex-trust",
    "codex_trust",
    "compact_sessions",
    "crawl",
    "expand_fold",
    "expand_ref",
    "fold_payload",
    "gemini-after-tool",
    "gemini-afterhook",
    "gemini-lifecycle",
    "groundedness",
    "hookwrap-copilot",
    "hookwrap-subagentstart",
    "hookwrap-subagentstop",
    "hookwrap-websearch-post",
    "hookwrap-mcp-post",
    "hookwrap-cursor-mcp-post",
    "judge",
    "real-savings-refresh",
    "reporter-run",
    "savings-receipt",
    "subagent_brief",
    "xfold",
    "xref",
];

pub(super) fn warn_format_not_a_command(name: &str) -> bool {
    if !FORMAT_NAMES.contains(&name) {
        return false;
    }
    eprintln!(
        "tokenade: '{name}' is a FORMAT, not a command — formats are detected \
         from the content. Use `tokenade read <file>` (or `tokenade read -` for \
         piped input) and the right compactor is chosen for you."
    );
    true
}

pub(super) fn nearest_command(typed: &str, max_distance: usize) -> Option<&'static str> {
    use tokenade_core::edit_distance::bounded;
    let mut best: Option<(&'static str, usize)> = None;
    for &cmd in KNOWN_COMMANDS {
        if let Some(d) = bounded(typed, cmd, max_distance) {
            match best {
                Some((_, prev)) if prev <= d => {}
                _ => best = Some((cmd, d)),
            }
        }
    }
    best.map(|(cmd, _)| cmd)
}

#[cfg(test)]
mod a_command_the_binary_runs_is_a_command_it_knows {
    //! Two lists decide what `tokenade <cmd> --help` prints, and neither is
    //! the dispatch. `respond` asks KNOWN_COMMANDS whether the name exists;
    //! `cmd_forwards_help` decides whether the handler gets to answer at all.
    //! Both were hand-written, both had drifted, and the failure is silent in
    //! both directions: a command that runs answers "unknown command", or a
    //! handler holding a page of help never sees the argument.
    //!
    //! Every roster here is DERIVED from the source that actually decides.

    /// `name -> was the dispatch arm behind a `#[cfg(...)]``
    fn dispatched_names() -> Vec<(String, bool)> {
        let src = include_str!("main.rs");
        let lines: Vec<&str> = src.lines().collect();
        let mut out = Vec::new();
        for (n, line) in lines.iter().enumerate() {
            let t = line.trim();
            let Some((pats, body)) = t.split_once("=>") else {
                continue;
            };
            let pats = pats.trim();
            if !pats.starts_with('"') || !body.contains("cmd_") {
                continue;
            }
            let gated = lines[..n]
                .iter()
                .rev()
                .find(|l| !l.trim().is_empty())
                .is_some_and(|l| l.trim().starts_with("#[cfg("));
            for p in pats.split('|') {
                let p = p.trim();
                if let Some(name) = p.strip_prefix('"').and_then(|p| p.strip_suffix('"')) {
                    if name.starts_with('-') {
                        continue; // `-h`/`-v` typo arms, not commands
                    }
                    out.push((name.to_string(), gated));
                }
            }
        }
        out
    }

    /// A name the dispatcher answers to must be a name the binary admits to.
    ///
    /// `xref` and `xfold` are the sharpest case this caught: `tokenade --help`
    /// documents both as aliases, `tokenade xref` runs, and
    /// `tokenade xref --help` answered "unknown command 'xref' — did you mean
    /// `read`?" — the tool contradicting its own help page in the same breath.
    #[test]
    fn every_dispatched_name_is_a_known_command() {
        let names = dispatched_names();
        assert!(
            names.len() >= 100,
            "the dispatch parser found only {} names — it stopped matching \
             the source it reads",
            names.len()
        );
        let missing: Vec<&str> = names
            .iter()
            .filter(|(_, gated)| !gated)
            .map(|(n, _)| n.as_str())
            .filter(|n| !super::KNOWN_COMMANDS.contains(n))
            .collect();
        assert!(
            missing.is_empty(),
            "these names dispatch but KNOWN_COMMANDS does not carry them, so \
             `tokenade <name> --help` answers \"unknown command\" about a \
             command that runs: {missing:?}"
        );
    }

    /// Names routed to a dedicated help by `help_routing`, read from its
    /// tables rather than restated.
    fn routed_for_help() -> std::collections::HashSet<String> {
        let src = include_str!("help_routing.rs");
        let from = src
            .find("static HELP_TEXTS")
            .expect("HELP_TEXTS table moved");
        let to = src
            .find("/// Answer `tokenade")
            .expect("respond() moved above the tables");
        let mut out = std::collections::HashSet::new();
        let mut rest = &src[from..to];
        while let Some(at) = rest.find('"') {
            rest = &rest[at + 1..];
            let Some(end) = rest.find('"') else { break };
            let lit = &rest[..end];
            if !lit.is_empty()
                && lit
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            {
                out.insert(lit.to_string());
            }
            rest = &rest[end + 1..];
        }
        out
    }

    /// A handler that writes its own help must be able to receive `--help`.
    ///
    /// The global interception in `run()` fires BEFORE dispatch, so a handler
    /// whose command is in neither `cmd_forwards_help` nor a `help_routing`
    /// table never sees the flag: its help is dead code. Found this way on the
    /// 1.1.9 binary — `review-context`, `judge` and `bench` each hold a full
    /// dedicated usage behind an `asks_for_help` guard, and all three answered
    /// with the 100-line global catalogue or "unknown command" instead.
    #[test]
    fn a_handler_that_writes_its_own_help_can_receive_the_flag() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sources: Vec<(String, String)> = Vec::new();
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs")
                    && p.file_name().is_some_and(|f| f != "help_routing.rs")
                {
                    if let Ok(body) = std::fs::read_to_string(&p) {
                        sources.push((p.display().to_string(), body));
                    }
                }
            }
        }
        assert!(
            sources.len() >= 20,
            "walked only {} source files — the walk stopped finding the crate",
            sources.len()
        );

        // handler fn name -> the names it is dispatched under
        let mut fn_names: std::collections::HashMap<String, Vec<String>> = Default::default();
        {
            let src = include_str!("main.rs");
            for line in src.lines() {
                let t = line.trim();
                let Some((pats, body)) = t.split_once("=>") else {
                    continue;
                };
                if !pats.trim().starts_with('"') {
                    continue;
                }
                let Some(handler) = body
                    .rsplit("::")
                    .next()
                    .and_then(|b| b.split('(').next())
                    .map(str::trim)
                else {
                    continue;
                };
                if !handler.starts_with("cmd_") {
                    continue;
                }
                let names: Vec<String> = pats
                    .split('|')
                    .filter_map(|p| {
                        p.trim()
                            .strip_prefix('"')
                            .and_then(|p| p.strip_suffix('"'))
                            .map(str::to_string)
                    })
                    .collect();
                fn_names
                    .entry(handler.to_string())
                    .or_default()
                    .extend(names);
            }
        }
        assert!(
            fn_names.len() >= 50,
            "the handler census found only {} functions",
            fn_names.len()
        );

        let routed = routed_for_help();
        let gated: std::collections::HashSet<String> = dispatched_names()
            .into_iter()
            .filter(|(_, g)| *g)
            .map(|(n, _)| n)
            .collect();

        let mut unreachable: Vec<String> = Vec::new();
        for (path, body) in &sources {
            let bytes = body.as_bytes();
            for (at, _) in body.match_indices("fn cmd_") {
                let after = &body[at + 3..];
                let Some(open) = after.find('(') else {
                    continue;
                };
                let name = after[..open].trim();
                if !name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                {
                    continue;
                }
                // The handler body: up to the NEAREST next top-level `fn`. An
                // `or_else` chain took the first KIND found anywhere below: a
                // private `fn` added far down a file of `pub(crate) fn`
                // handlers stretched one handler over every later one.
                let start = at + 3 + open;
                let end = ["\nfn ", "\npub fn ", "\npub(crate) fn "]
                    .iter()
                    .filter_map(|d| body[start..].find(d))
                    .min()
                    .map(|o| start + o)
                    .unwrap_or(bytes.len());
                if !body[start..end].contains("asks_for_help") {
                    continue;
                }
                let Some(names) = fn_names.get(name) else {
                    continue;
                };
                if names.iter().any(|n| gated.contains(n)) {
                    continue;
                }
                // EVERY name, not any: the flag is asked of the name the
                // user typed. A first pass accepted the handler as soon as one
                // alias was reachable, and survived the mutation that drops
                // `judge` while keeping `groundedness` — which is precisely
                // `tokenade judge --help` staying broken.
                for n in names {
                    if !super::cmd_forwards_help(n) && !routed.contains(n) {
                        unreachable.push(format!("`{n}` -> {name}() in {path}"));
                    }
                }
            }
        }
        unreachable.sort();
        unreachable.dedup();
        assert!(
            unreachable.is_empty(),
            "these handlers print their own --help, but the global \
             interception answers first and they never see the flag — their \
             help is dead code:\n  {}",
            unreachable.join("\n  ")
        );
    }
}

#[cfg(test)]
mod help_forwarding_tests {
    /// Every alias that dispatches to the SAME handler must agree about
    /// forwarding `--help`, and the roster is DERIVED from the dispatch match
    /// rather than restated here.
    ///
    /// A hand-written list is how the defect happened: `cmd_forwards_help`
    /// named `execute` and not its two aliases, so `tokenade exec --help`
    /// claimed no help existed while `tokenade execute --help` printed it in
    /// full. A list that has to be updated by hand when a sibling appears will
    /// be wrong again; reading the match means adding an alias tomorrow either
    /// works or fails HERE.
    #[test]
    fn every_alias_of_one_handler_agrees_about_forwarding_help() {
        let src = include_str!("main.rs");
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        for line in src.lines() {
            let t = line.trim();
            // `"a" | "b" | "c" => cmd_x(rest),` — only multi-alias arms.
            let Some((pats, body)) = t.split_once("=>") else {
                continue;
            };
            if !pats.contains('|') || !pats.trim_start().starts_with('"') {
                continue;
            }
            let Some(handler) = body.split('(').next().map(str::trim) else {
                continue;
            };
            if !handler.starts_with("cmd_") {
                continue;
            }
            let names: Vec<String> = pats
                .split('|')
                .filter_map(|p| {
                    let p = p.trim();
                    p.strip_prefix('"')
                        .and_then(|p| p.strip_suffix('"'))
                        .map(str::to_string)
                })
                .collect();
            if names.len() > 1 {
                groups.push((handler.to_string(), names));
            }
        }
        // Anti-vacuity: a parser that finds nothing would pass silently.
        assert!(
            groups.len() >= 5,
            "the dispatch parser found only {} alias groups — it stopped \
             matching the source it reads",
            groups.len()
        );

        for (handler, names) in groups {
            let forwards: Vec<&String> = names
                .iter()
                .filter(|n| super::cmd_forwards_help(n))
                .collect();
            assert!(
                forwards.is_empty() || forwards.len() == names.len(),
                "{handler}: aliases disagree about forwarding --help. \
                 forwarding={forwards:?} all={names:?} — the ones left out \
                 answer \"no dedicated help yet\" for a command that has help"
            );
        }
    }
}
