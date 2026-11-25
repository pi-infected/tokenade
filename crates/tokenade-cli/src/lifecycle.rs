//! Lifecycle / admin CLI commands (`upgrade|uninstall|redact|evict-stale|
//! scrub-*|calibrate|ensure-hooks`).
//! Extracted from main.rs (refactor split).
#![allow(unused_imports)]
mod browser;
mod maintenance;
use crate::wrap::*;
use crate::*;
pub(crate) use browser::*;
pub(crate) use maintenance::*;
use std::path::{Path, PathBuf};
mod uninstall;
mod upgrade;
pub(crate) use uninstall::*;
pub(crate) use upgrade::*;

/// `tokenade scan-savings` — read the user's EXISTING Claude Code transcripts
/// and estimate how much tokenade would have compacted, priced at the real
/// per-model rate card. The conversion mechanic (ported from Atelier): show a
/// prospect their own dollar number before they commit. Read-only; touches no
/// files. `--json` for machine consumption.
pub(crate) fn cmd_scan_savings(rest: &[String]) -> i32 {
    if let Err(code) = validate_simple_bool_flags(rest, "scan-savings", &["--help", "-h", "--json"])
    {
        return code;
    }
    let json = rest.iter().any(|a| a == "--json");
    let Some(home) = tokenade_core::platform::home_dir() else {
        eprintln!("scan-savings: no home dir");
        return 1;
    };
    let projects = tokenade_core::platform::claude_config_dir_for(&home).join("projects");
    let Ok(rd) = std::fs::read_dir(&projects) else {
        println!(
            "[tokenade scan-savings] no Claude Code transcripts found under {}",
            projects.display()
        );
        return 0;
    };
    // Dominant model (by session frequency) for pricing.
    let model = {
        let sms = tokenade_core::claudecode_transcript::session_models(&home, None);
        let mut counts: std::collections::HashMap<String, usize> = Default::default();
        for sm in &sms {
            *counts.entry(sm.model.clone()).or_default() += 1;
        }
        tokenade_core::agent_transcripts::dominant_key(counts.into_iter())
    };
    let mut per_project: Vec<(String, usize)> = Vec::new();
    let mut total_foldable = 0usize;
    for e in rd.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        let mut proj_tokens = 0usize;
        if let Ok(files) = std::fs::read_dir(&dir) {
            for f in files.flatten() {
                let p = f.path();
                if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                    continue;
                }
                if let Ok(body) = std::fs::read_to_string(&p) {
                    proj_tokens += tokenade_core::session_compact::foldable_tokens(&body);
                }
            }
        }
        if proj_tokens > 0 {
            total_foldable += proj_tokens;
            let label = dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("?")
                .to_string();
            per_project.push((label, proj_tokens));
        }
    }
    // Conservative single-pass fraction of foldable tokens tokenade removes.
    // Deliberately below our measured average so the pitch never over-promises.
    const SAVE_RATIO: f64 = 0.6;
    let saved = (total_foldable as f64 * SAVE_RATIO) as u64;
    let usd = model
        .as_deref()
        .and_then(tokenade_core::model_costs::cost_for_model)
        .map(|c| c.usd(saved, 0, 0, 0));
    per_project.sort_by_key(|b| std::cmp::Reverse(b.1));

    if json {
        let payload = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "foldable_tokens": total_foldable,
            "estimated_saved_tokens": saved,
            "save_ratio": SAVE_RATIO,
            "model": model,
            "estimated_usd": usd,
            "projects": per_project.iter()
                .map(|(n, t)| serde_json::json!({"project": n, "foldable_tokens": t}))
                .collect::<Vec<_>>(),
        });
        println!("{payload}");
        return 0;
    }
    if total_foldable == 0 {
        println!(
            "[tokenade scan-savings] scanned your Claude Code history — no large re-sent tool \
             outputs found yet (nothing to compact)."
        );
        return 0;
    }
    println!("[tokenade scan-savings] from your existing Claude Code sessions:");
    println!("  ~{total_foldable} tokens of large tool output tokenade would compact");
    match usd {
        Some(d) => println!(
            "  ~{saved} tokens saved (conservative, single-pass) ≈ ${d:.2} at {} input rates",
            model.as_deref().unwrap_or("your model")
        ),
        None => println!("  ~{saved} tokens saved (conservative, single-pass)"),
    }
    println!("  (real savings compound — the same context is re-sent on every later turn.)");
    println!("  top projects:");
    for (n, t) in per_project.iter().take(5) {
        println!("    {t:>10} tok  {n}");
    }
    0
}

/// `tokenade ensure-hooks [--quiet] [--force]` — re-assert tokenade's agent
/// hooks + shell proxy if another tool or an agent upgrade dropped/overwrote
/// them. Idempotent; acts only on a machine that installed tokenade (the
/// install sentinel). Throttled to ~6h unless `--force`. Runs automatically
/// (SessionStart, `tokenade wrap`, the shell proxy); run by hand to repair now.
pub(crate) fn cmd_ensure_hooks(rest: &[String]) -> i32 {
    if crate::help_routing::asks_for_help(rest) {
        println!("usage: tokenade ensure-hooks [--quiet] [--force]");
        println!();
        println!("Re-assert tokenade's agent hooks + shell proxy if another tool or an");
        println!("agent upgrade dropped or overwrote them. Idempotent; acts only on a");
        println!("machine that has installed tokenade. Throttled to ~6h unless --force.");
        println!("Mostly automatic (SessionStart / `tokenade wrap`); run it to repair now.");
        return 0;
    }
    let quiet = rest.iter().any(|a| a == "--quiet");
    let force = rest.iter().any(|a| a == "--force");
    for a in rest.iter().filter(|a| a.starts_with('-')) {
        if !matches!(a.as_str(), "--quiet" | "--force") {
            eprintln!(
                "tokenade ensure-hooks: unknown flag '{a}' (try `tokenade ensure-hooks --help`)"
            );
            return 2;
        }
    }
    let home = tokenade_core::platform::home_dir();
    let installed = home
        .as_deref()
        .map(tokenade_core::install_state::installed_ever)
        .unwrap_or(false);
    let repaired = ensure_hooks(home.as_deref(), !force);
    if !quiet {
        if !installed {
            println!("tokenade ensure-hooks: not installed on this machine — run `tokenade install` first.");
        } else if repaired > 0 {
            let plural = if repaired == 1 { "entry" } else { "entries" };
            println!("✓ re-asserted {repaired} drifted hook/proxy {plural}.");
        } else {
            println!("✓ hooks healthy — nothing to repair.");
        }
    }
    0
}

/// `tokenade list-commands` — print every user-facing CLI command, one
/// per line. The source of truth is KNOWN_COMMANDS filtered to PRIMARY
/// commands (not aliases) that have a help-body line — i.e. exactly the
/// set a user can type and that the release pipeline expects to be
/// documented in the public README. Hidden/internal commands (hookwrap*,
/// index-touch, …) have no help line and are excluded; aliases collapse
/// to their primary. Machine-readable (no banner) so the release guard
/// can diff it against the README.
/// `tokenade calibrate --db <results.sqlite>` — HIDDEN / dev maintenance
/// command. Recomputes the approximate-savings coefficients (currently the
/// output-lean ratio) from a THOL benchmark DB and persists them to
/// `~/.tokenade/coefficients.json`. Deliberately has no `oneline_help_for`
/// entry, so it stays out of `list-commands` and the README docs guard
/// doesn't require it (it isn't an end-user command).
pub(crate) fn cmd_calibrate(rest: &[String]) -> i32 {
    let Some(db) = get_flag(rest, "--db") else {
        eprintln!("usage: tokenade calibrate --db <results.sqlite>");
        return 2;
    };
    let db = std::path::PathBuf::from(db);
    match tokenade_core::coefficients::calibrate_output_lean_from_db(&db) {
        Ok(ratio) => match tokenade_core::coefficients::write_output_lean_ratio(ratio) {
            Ok(()) => {
                println!(
                    "calibrated output_lean_ratio = {ratio:.4} → ~/.tokenade/coefficients.json"
                );
                0
            }
            Err(e) => {
                eprintln!("calibrate: write failed: {e}");
                1
            }
        },
        Err(e) => {
            eprintln!("calibrate: {e}");
            1
        }
    }
}

pub(crate) fn cmd_list_commands(rest: &[String]) -> i32 {
    if crate::help_routing::asks_for_help(rest) {
        println!("usage: tokenade list-commands");
        println!();
        println!("Print every user-facing CLI command, one per line (primary");
        println!("commands with a help entry; aliases + hidden internals excluded).");
        println!("Used by the release pipeline to verify the public README documents");
        println!("every command.");
        return 0;
    }
    if let Err(c) =
        crate::code_intel::refuse_all_positionals(rest, "list-commands", &["--help", "-h"], &[])
    {
        return c;
    }
    let mut cmds: Vec<&str> = KNOWN_COMMANDS
        .iter()
        .copied()
        .filter(|c| alias_to_primary(c).is_none() && oneline_help_for(c).is_some())
        .collect();
    cmds.sort_unstable();
    cmds.dedup();
    for c in cmds {
        println!("{c}");
    }
    0
}
