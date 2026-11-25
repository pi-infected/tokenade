//! **Rewrite what is already written** — the log, and the cache.
//!
//! `redact` goes over what tokenade has recorded; `scrub-cache` over what it
//! has set aside to serve again. Two stores, two reasons to walk them, a
//! single rule: never announce a cleanup that did not happen.
//!
//! The report is therefore drawn from what was ACTUALLY rewritten, not from
//! what was asked — a locked line, a read-only file, a missing store: each
//! would yield a "done" that is not.

use super::*;

/// `tokenade redact <file…> [--in-place] [--dry-run]` — apply the
/// built-in secret-redaction patterns (the same set used before any byte
/// enters a log or a `report` archive) to ARBITRARY files. Use it to
/// scrub third-party data — e.g. a client's shared transcripts — before
/// reading it. Streams line-by-line (lossy UTF-8) so multi-GB JSONL files
/// never load into memory; idempotent.
///
/// Default: writes `<file>.redacted` beside each input, original intact.
/// `--in-place`: overwrite the original (atomic — temp file + rename).
/// `--dry-run`: report how many lines WOULD change; write nothing.
pub(crate) fn cmd_redact(rest: &[String]) -> i32 {
    use std::io::{BufRead, Write};
    if crate::help_routing::asks_for_help(rest) {
        println!("usage: tokenade redact <file…> [--in-place] [--dry-run]");
        println!();
        println!("Applies tokenade's built-in secret-redaction patterns (Bearer/Basic,");
        println!("Authorization, api_key=, *_TOKEN=/_KEY=/_SECRET=, AWS/GitHub keys, …),");
        println!("replacing every secret-shaped value with `<redacted>`.");
        println!();
        println!("  (default)    write `<file>.redacted`, leave the original intact");
        println!("  --in-place   overwrite the original (atomic: temp file + rename)");
        println!("  --dry-run    report how many lines would change; write nothing");
        println!();
        println!("Streams line-by-line — safe on multi-GB files. Idempotent.");
        println!("Best-effort: rotate any credential that may have leaked regardless.");
        return 0;
    }
    let in_place = rest.iter().any(|a| a == "--in-place");
    let dry_run = rest.iter().any(|a| a == "--dry-run");
    if in_place && dry_run {
        eprintln!("tokenade redact: --in-place and --dry-run are mutually exclusive");
        return 2;
    }
    for a in rest.iter().filter(|a| a.starts_with('-')) {
        if !matches!(a.as_str(), "--in-place" | "--dry-run") {
            eprintln!("tokenade redact: unknown flag '{a}' (try `tokenade redact --help`)");
            return 2;
        }
    }
    let files: Vec<&String> = rest.iter().filter(|a| !a.starts_with('-')).collect();
    if files.is_empty() {
        eprintln!("usage: tokenade redact <file…> [--in-place] [--dry-run]");
        eprintln!("       (run with --help for details)");
        return 2;
    }
    let glyph = if dry_run { "◐" } else { "✓" };
    let mut rc = 0;
    let mut grand_changed = 0usize;
    for f in &files {
        let path = std::path::PathBuf::from(f.as_str());
        let input = match fs::File::open(&path) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("  ✗ {}: open failed: {e}", path.display());
                rc = 1;
                continue;
            }
        };
        let mut reader = std::io::BufReader::new(input);
        let tmp_path = std::path::PathBuf::from(format!("{f}.tok-redact.tmp"));
        let mut writer: Option<std::io::BufWriter<fs::File>> = if dry_run {
            None
        } else {
            match fs::File::create(&tmp_path) {
                Ok(x) => Some(std::io::BufWriter::new(x)),
                Err(e) => {
                    eprintln!("  ✗ {}: temp create failed: {e}", tmp_path.display());
                    rc = 1;
                    continue;
                }
            }
        };
        let mut lines = 0usize;
        let mut changed = 0usize;
        let mut line_redactor = tokenade_core::redact::LineRedactor::new();
        let mut io_err: Option<String> = None;
        let mut buf: Vec<u8> = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => {
                    io_err = Some(format!("read failed: {e}"));
                    break;
                }
            }
            let had_nl = buf.last() == Some(&b'\n');
            let s = String::from_utf8_lossy(&buf);
            let line = s.strip_suffix('\n').unwrap_or(&s);
            let line = line.strip_suffix('\r').unwrap_or(line);
            lines += 1;
            // A secret can span lines; this loop sees one at a time. The
            // redactor's streaming form carries the little state that needs —
            // see `redact::LineRedactor`.
            let Some(red) = line_redactor.line(line) else {
                changed += 1;
                continue;
            };
            if red != line {
                changed += 1;
            }
            if let Some(w) = writer.as_mut() {
                let res = if had_nl {
                    writeln!(w, "{red}")
                } else {
                    write!(w, "{red}")
                };
                if let Err(e) = res {
                    io_err = Some(format!("write failed: {e}"));
                    break;
                }
            }
        }
        if let Some(w) = writer.as_mut() {
            let _ = w.flush();
        }
        drop(writer);
        if let Some(e) = io_err {
            eprintln!("  ✗ {}: {e}", path.display());
            let _ = fs::remove_file(&tmp_path);
            rc = 1;
            continue;
        }
        if dry_run {
            grand_changed += changed;
            println!(
                "  {glyph} {} — would redact {changed}/{lines} line(s)",
                path.display()
            );
            continue;
        }
        let dest = if in_place {
            path.clone()
        } else {
            std::path::PathBuf::from(format!("{f}.redacted"))
        };
        if let Err(e) = fs::rename(&tmp_path, &dest) {
            eprintln!("  ✗ {}: finalize failed: {e}", path.display());
            let _ = fs::remove_file(&tmp_path);
            rc = 1;
            continue;
        }
        grand_changed += changed;
        println!(
            "  {glyph} {} → {} — {changed}/{lines} line(s) redacted",
            path.display(),
            dest.display()
        );
    }
    let verb = if dry_run { "would redact" } else { "redacted" };
    println!(
        "[redact] {verb} {grand_changed} line(s) across {} file(s)",
        files.len()
    );
    rc
}

pub(crate) fn cmd_scrub_cache(rest: &[String]) -> i32 {
    if crate::help_routing::asks_for_help(rest) {
        println!("usage: tokenade scrub-cache [--dry-run]");
        println!();
        println!("Re-applies the built-in redact regex pass to every persisted byte in:");
        println!("  • ~/.tokenade/dedup.db   (every row in dedup_content)");
        println!("  • ~/.cache/tokenade/stash/* (text files only — binaries left alone)");
        println!();
        println!("Use after upgrading tokenade or after rotating credentials, to clean");
        println!("up any creds that landed on disk before the in-pipeline redact fixes");
        println!("(#475/#477/#478) shipped. Idempotent — running twice is harmless.");
        println!();
        println!("Flags:");
        println!("  --dry-run   count rows/files that WOULD change, modify nothing");
        println!();
        println!("Different from `tokenade scrub-secret <substring>`, which targets");
        println!("ONE specific string the user knows leaked. This sweeps every known");
        println!("credential pattern (AWS, GitHub, Stripe, Twilio, SendGrid, …).");
        return 0;
    }
    // #657 — reject unknown flags + positionals up front. Before
    // this, `scrub-cache --dryrun` (typo) silently performed a real
    // scrub, and `scrub-cache foo` dropped the positional without
    // warning. This command takes no positional arguments.
    if let Err(code) = validate_simple_bool_flags(rest, "scrub-cache", SCRUB_CACHE_FLAGS) {
        return code;
    }
    // #512 — `--dry-run` parity with the sibling scrub-secret. Counts
    // rows/files that would change without modifying any persisted state,
    // so users can audit the blast radius before committing.
    let dry_run = rest.iter().any(|a| a == "--dry-run");
    if dry_run {
        println!("[tokenade scrub-cache --dry-run] reporting what WOULD change; no writes");
    } else {
        println!("[tokenade scrub-cache] re-applying redact patterns to persisted state");
    }

    // 1. dedup.db
    let (mut touched, before, after) = if dry_run {
        tokenade_core::dedup_ref::scrub_via_redact_dry_run()
    } else {
        tokenade_core::dedup_ref::scrub_via_redact()
    };
    if touched > 0 {
        if dry_run {
            println!(
                "  ◐  dedup.db — {touched} row(s) WOULD be redacted; db size currently {before} bytes"
            );
        } else {
            println!("  ✓ dedup.db — redacted {touched} row(s); db size {before} → {after} bytes");
        }
    } else {
        println!("  --  dedup.db — nothing to redact");
    }

    // 2. ~/.cache/tokenade/stash/*
    let stash_result = if dry_run {
        tokenade_core::disclose::scrub_via_redact_dry_run()
    } else {
        tokenade_core::disclose::scrub_via_redact()
    };
    match stash_result {
        Ok((files, bytes)) if files > 0 => {
            if dry_run {
                println!(
                    "  ◐  stash — {files} file(s) WOULD be redacted; {bytes} byte(s) WOULD be freed"
                );
            } else {
                println!("  ✓ stash — redacted {files} file(s); freed {bytes} byte(s)");
            }
        }
        Ok(_) => println!("  --  stash — nothing to redact"),
        Err(e) => eprintln!("  ✗ stash — scan failed: {e}"),
    }

    // 3. semantic.db — DELETE, never patch, for the reasons `scrub-secret`
    // gives. A blob written since the comment redaction landed cannot match;
    // this is for the ones written before it.
    let sem = tokenade_core::semantic_db::purge_redactable(dry_run);
    if sem > 0 {
        if dry_run {
            println!("  ◐  semantic.db — {sem} project index(es) WOULD be dropped");
        } else {
            println!(
                "  ✓ semantic.db — dropped {sem} project index(es) (they rebuild on the next query)"
            );
        }
        touched += sem;
    } else {
        println!("  --  semantic.db — nothing to redact");
    }

    // 3b. file_search.db — same remedy, same reason, one store over. Nothing
    // writes there any more; only chunks from before redaction can match.
    let fsr = tokenade_core::file_search::purge_redactable(dry_run);
    if fsr > 0 {
        if dry_run {
            println!("  ◐  file_search.db — {fsr} indexed file(s) WOULD be dropped");
        } else {
            println!(
                "  ✓ file_search.db — dropped {fsr} indexed file(s) (they re-index on the next question)"
            );
        }
        touched += fsr;
    } else {
        println!("  --  file_search.db — nothing to redact");
    }

    println!();
    if touched == 0 {
        println!("  no historic credentials found in the cache.");
    } else if dry_run {
        println!(
            "  dry-run complete. Re-run without --dry-run to apply, or audit \
             the matching rows with `tokenade scrub-secret <substring> --dry-run`."
        );
    } else {
        println!(
            "  scrub complete. Reminder: rotate any credential you confirmed leaked — \
             scrubbing the local cache does not undo prior exfiltration to the model."
        );
    }
    0
}
