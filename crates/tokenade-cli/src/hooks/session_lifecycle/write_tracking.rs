//! Knowing which files the turn wrote, and when.
//!
//! A write generation per session, a baseline snapshot before an edit, and
//! the targets a bash command is about to write — so a later read can tell
//! "already delivered" from "changed since".

use super::*;

/// Scan a transcript for tokenade fold-markers and extract the unique
/// 12-hex content hashes they reference. Two marker shapes appear in
/// the wild: the inline `§ref:<hash>§` emitted by the deduper and the
/// `expand_ref` hint `hash="<hash>"` that rides along with it (and with
/// disclose-stash banners). Hand-scanned rather than regex'd to keep the
/// CLI crate dependency-free; order-preserving + deduped.
pub(crate) fn extract_ref_hashes(s: &str) -> Vec<String> {
    fn is_hex12(c: &str) -> bool {
        c.len() == 12 && c.bytes().all(|b| b.is_ascii_hexdigit())
    }
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |h: &str| {
        if is_hex12(h) && seen.insert(h.to_string()) {
            out.push(h.to_string());
        }
    };
    // §ref:<hash>§  — § is the 2-byte UTF-8 sequence 0xC2 0xA7.
    for (idx, _) in s.match_indices("§ref:") {
        let rest = &s[idx + "§ref:".len()..];
        if let Some(end) = rest.find('§') {
            push(&rest[..end]);
        }
    }
    // hash="<hash>"
    for (idx, _) in s.match_indices("hash=\"") {
        let rest = &s[idx + "hash=\"".len()..];
        if let Some(end) = rest.find('"') {
            push(&rest[..end]);
        }
    }
    // `tokenade expand-ref <hash>` — the recovery pointer left behind when a
    // tool result is masked in the transcript, and the shape the two patterns
    // above BOTH miss. A masked block is the one case where the stash is the
    // only copy: the bytes are gone from the transcript. Leaving those hashes
    // out of the barrier meant the LRU could evict exactly the entry the agent
    // must be able to expand, and the manifest never named it.
    for (idx, _) in s.match_indices("expand-ref ") {
        let rest = &s[idx + "expand-ref ".len()..];
        let h: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        push(&h);
    }
    out
}

/// Read a transcript (bounded), find the live fold-refs, LRU-protect the
/// resident ones, and write a durable manifest at
/// `~/.tokenade/sessions/<session_id>-refs.md`. Returns
/// `(manifest_path, total_refs, resident_refs)` or `None` if nothing to
/// do / IO failed. Never panics — a PreCompact hook must not block the
/// summariser.
pub(crate) fn precompact_write_barrier(
    transcript_path: &str,
    session_id: &str,
) -> Option<(std::path::PathBuf, usize, usize)> {
    // Bounded read: a transcript is a finite file, but very long
    // sessions can be large — cap to avoid pulling 100s of MiB into the
    // hook process. 64 MiB covers any realistic transcript.
    const MAX_TRANSCRIPT_BYTES: u64 = 64 * 1024 * 1024;
    use std::io::Read;
    let file = std::fs::File::open(transcript_path).ok()?;
    let mut buf = String::new();
    file.take(MAX_TRANSCRIPT_BYTES)
        .read_to_string(&mut buf)
        .ok()?;

    let hashes = extract_ref_hashes(&buf);
    if hashes.is_empty() {
        return None;
    }
    let total = hashes.len();

    // Bump last_used on the residents → they survive the next size-cap
    // eviction. Returns (hash, byte_count) for those still in the store.
    let resident = tokenade_core::dedup_ref::refresh_refs(&hashes);
    let resident_set: std::collections::HashSet<&str> =
        resident.iter().map(|(h, _)| h.as_str()).collect();

    let home = tokenade_core::platform::home_dir()?;
    let dir = home.join(".tokenade").join("sessions");
    std::fs::create_dir_all(&dir).ok()?;
    let safe_id = sanitize_session_id(session_id);
    let manifest = dir.join(format!("{safe_id}-refs.md"));

    let mut md = String::with_capacity(256 + total * 48);
    md.push_str("# tokenade pre-compact ref manifest\n\n");
    md.push_str(&format!(
        "Session `{session_id}` is about to be auto-summarised (lossy). The \
         folded byte-blobs below were referenced inline in the transcript; \
         tokenade still holds them. After the summary, recover any of them \
         verbatim with `tokenade expand-ref <hash>` using the hash.\n\n"
    ));
    md.push_str(&format!(
        "{total} unique ref(s); {} resident in the dedup store (LRU-protected), \
         the rest expandable via the disclose stash.\n\n",
        resident.len()
    ));
    md.push_str("| hash | bytes | source |\n|------|------:|--------|\n");
    for h in &hashes {
        if let Some((_, n)) = resident.iter().find(|(rh, _)| rh == h) {
            md.push_str(&format!("| `{h}` | {n} | dedup store |\n"));
        } else if !resident_set.contains(h.as_str()) {
            md.push_str(&format!("| `{h}` | ? | stash (if retained) |\n"));
        }
    }
    std::fs::write(&manifest, md).ok()?;
    Some((manifest, total, resident.len()))
}

/// Session-scoped write counter under read-pending/: `writegen-<skey>`.
/// Incremented on every Edit/Write; consumed by `loop_guard_nudge`.
/// Read-modify-write, and deliberately left that way. Audited 2026-08-03 with
/// the rest of the shared-state counters: this one is safe by SEMANTICS, not by
/// atomicity. What the generation must do is *change* after a write, not count
/// exactly — two concurrent writes that both read 5 and both store 6 still
/// invalidate every tag taken at 5, which is the whole contract (tags are
/// compared for equality, never for distance). Do not "fix" it into an
/// append-only counter; the fix would buy nothing and the equality comparison
/// is what makes it correct. Contrast `read_pretool::breaker_bump`, where a
/// lost increment genuinely breaks the mechanism.
pub(crate) fn bump_write_generation(stdin_json: Option<&str>) {
    let skey = session_key_from_stdin(stdin_json);
    let dir = crate::hooks::read_staging::read_pending_dir();
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join(format!("writegen-{skey}"));
    let g = std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
        .wrapping_add(1);
    let _ = std::fs::write(&p, g.to_string());
}

/// **A write made BY Bash is a write.** The generation only moved on
/// `Edit`/`Write`; an agent editing through `sed -i`, a Python heredoc or
/// `cat > f <<` passed for having touched nothing. Surveyed over 514 loop-guard
/// firings in local transcripts (2026-10-02): **206** followed such a write
/// since the previous pass, with no `Edit` in between — `cargo test` re-run
/// after a fix, called a "loop". And the replay (`replay.rs`, guard 1) served
/// back a `git diff` from before the write.
///
/// A false positive costs one hint or one replay less; a false negative, a
/// wrong answer. The rule therefore leans toward "wrote".
///
/// Word scan, no `regex`: this hook runs before EVERY command, in a fresh
/// process, and `main.rs` already quantifies what `Regex::new` costs at
/// startup.
pub(crate) fn bash_writes_files(cmd: &str) -> bool {
    let words: Vec<&str> = cmd
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')' | '`'))
        .filter(|w| !w.is_empty())
        .collect();
    let base = |w: &str| w.rsplit('/').next().unwrap_or(w).to_string();
    let in_place_flag = |w: &&str| {
        *w == "--in-place"
            || (w.starts_with('-')
                && !w.starts_with("--")
                && w[1..].chars().all(|c| c.is_ascii_alphabetic())
                && w.contains('i'))
    };
    for (i, w) in words.iter().enumerate() {
        let rest = &words[i + 1..];
        match base(w).as_str() {
            "rm" | "mv" | "cp" | "touch" | "mkdir" | "rmdir" | "ln" | "truncate" | "install"
            | "patch" => return true,
            "sed" | "perl"
                if rest
                    .iter()
                    .take_while(|a| a.starts_with('-'))
                    .any(&in_place_flag) =>
            {
                return true
            }
            "tee" if rest.iter().any(|a| !a.starts_with('-')) => return true,
            "git"
                if rest.first().is_some_and(|s| {
                    matches!(
                        *s,
                        "apply"
                            | "checkout"
                            | "restore"
                            | "stash"
                            | "am"
                            | "cherry-pick"
                            | "merge"
                            | "rebase"
                            | "reset"
                            | "switch"
                    )
                }) =>
            {
                return true
            }
            "cargo"
                if rest.first().is_some_and(|s| matches!(*s, "fmt" | "fix"))
                    && !cmd.contains("--check") =>
            {
                return true
            }
            _ => {}
        }
    }
    let interpreter = ["python", "node", "ruby", "perl"]
        .iter()
        .any(|p| cmd.contains(p));
    let writes_in_script = [
        "write_text(",
        "write_bytes(",
        "writeFileSync",
        ".write(",
        "'w')",
        "\"w\")",
        "'a')",
        "\"a\")",
        "'wb')",
    ]
    .iter()
    .any(|k| cmd.contains(k));
    if interpreter && writes_in_script {
        return true;
    }
    // A redirect to anything but /dev/null or a descriptor writes a file.
    let b = cmd.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'>' && (i == 0 || !matches!(b[i - 1], b'-' | b'=')) {
            let mut j = i + 1;
            while j < b.len() && (b[j] == b'>' || b[j] == b' ') {
                j += 1;
            }
            let end = cmd[j..]
                .find(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | ')'))
                .map_or(cmd.len(), |e| j + e);
            let target = &cmd[j..end];
            if !target.is_empty() && !target.starts_with('&') && target != "/dev/null" {
                return true;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    false
}

/// Bash `PreToolUse`: bump the generation BEFORE a command that writes runs,
/// so every command after it counts — and replays — against the new state.
pub(crate) fn bump_write_generation_if_bash_writes(parsed: &serde_json::Value, cmd: &str) {
    if bash_writes_files(cmd) {
        bump_write_generation(Some(&parsed.to_string()));
    }
}

pub(crate) fn current_write_generation(skey: &str) -> u64 {
    let p = crate::hooks::read_staging::read_pending_dir().join(format!("writegen-{skey}"));
    std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

/// `tokenade pre-edit-snapshot` — Claude Code `PreToolUse` hook that
/// fires BEFORE Edit/Write/MultiEdit. Reads the target file and stores
/// it in the dedup_ref content store so that PostToolUse
/// `cmd_index_touch` → `compose_edit_diff_hint` can emit a unified
/// diff instead of "no snapshot available".
///
/// Without this hook, `skipped_no_snapshot` fires ~740 times (every
/// Edit on a file the agent never Read beforehand). With it, the
/// file_diff hint fires even for those edits, saving the agent a full
/// re-Read after every blind write.
///
/// Cost: one `fs::read_to_string` + one SQLite upsert per Edit, bounded
/// by `MIN_BYTES_TO_DEDUPE` (≤1500 bytes skipped). For a 50 kB file
/// that's ~1–2 ms. Worth every microsecond given the alternative is
/// ~50k tokens of redundant re-Read.
pub(crate) fn cmd_pre_edit_snapshot() -> i32 {
    if hook_disabled_by_env() {
        return 0;
    }
    let started = std::time::Instant::now();
    let stdin_json = crate::hooks::stdin::read_event("pre-edit-snapshot");

    // D2 adoption credit: if THIS file carries a fresh write-nudge marker
    // (the agent was told "prefer Edit over a near-identical full Write")
    // and the current tool IS an Edit, the stored re-emission estimate is
    // realized — the agent generated a hunk instead of the whole file.
    // Credited once (marker zeroed), class=output, est:true.
    if let Some(json) = stdin_json.as_deref() {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json) {
            let tool = parsed
                .get("tool_name")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            if matches!(tool, "Edit" | "MultiEdit") {
                if let Some(fp) = parsed
                    .pointer("/tool_input/file_path")
                    .and_then(|p| p.as_str())
                {
                    let session = crate::hooks::session_id_of(&parsed).unwrap_or("nosess");
                    // Writers audit: this hook process never set the session
                    // key, so its write_to_edit credit was booked sess=null —
                    // invisible to the session ticker.
                    {
                        let skey: String = session
                            .chars()
                            .filter(|c| c.is_ascii_alphanumeric())
                            .take(40)
                            .collect();
                        if !skey.is_empty() {
                            tokenade_core::session::set_session_key_authoritative(&skey);
                        }
                    }
                    let skey: String = session
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .take(40)
                        .collect();
                    let marker = crate::hooks::read_staging::read_pending_dir().join(format!(
                        "writenudge-{skey}-{}",
                        tokenade_core::dedup_ref::hash_content(fp.as_bytes())
                    ));
                    if let Ok(txt) = std::fs::read_to_string(&marker) {
                        if let Ok(chars) = txt.trim().parse::<usize>() {
                            if chars > 0 {
                                gain_log::record_estimated(
                                    "hookwrap-pre-edit",
                                    "write_to_edit",
                                    fp,
                                    "output",
                                    tokenade_core::Savings::new(chars / 4, 0),
                                );
                                let _ = std::fs::write(&marker, "0");
                            }
                        }
                    }
                }
            }
        }
    }
    let Some(file_path) = edited_file_path(stdin_json.as_deref()) else {
        return 0;
    };
    // Bump the session's write-generation BEFORE any size/binary gate: the
    // loop-guard keys its "same command re-run" counter on this, so any
    // edit — however small — resets the count. Without it the normal
    // edit→test→edit→test cycle was nudged with "re-running won't change
    // the result" at every 3rd run (counter was cumulative, never reset).
    bump_write_generation(stdin_json.as_deref());

    let path = std::path::PathBuf::from(&file_path);
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    if !meta.is_file() {
        return 0;
    }
    let size = meta.len() as usize;
    if size < tokenade_core::dedup_ref::MIN_BYTES_TO_DEDUPE {
        return 0;
    }
    // Skip binary files — read_to_string will error on non-UTF-8.
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return 0,
    };

    let root = root();
    // #SECURITY — redact secrets before storing in dedup.db. Same pass as
    // the Read hook. Without this, editing a .env / kubeconfig
    // persists cleartext credentials and re-exposes them via expand_ref.
    let content = tokenade_core::redact::redact(&content);
    let hash = tokenade_core::dedup_ref::hash_content(content.as_bytes());
    tokenade_core::dedup_ref::register(&root, &hash, &content);
    let pk = tokenade_core::dedup_ref::path_key(&file_path, 0, 0);
    tokenade_core::dedup_ref::register_path_pointer(&root, &pk, &hash);

    tokenade_core::debug_log::log(
        "pre-edit-snapshot",
        "snapshotted",
        serde_json::json!({
            "file_path": file_path,
            "size": size,
            "hash": &hash[..12],
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

/// Extract file paths likely written by a Bash command.
///
/// Detects the most common write patterns an agent uses:
///   `cmd > file`, `cmd >> file`, `>file`, `>>file`  (output redirect)
///   `tee [-a] file`                                  (tee)
///   `cp src dst`                                     (copy destination)
///   `mv src dst`                                     (move destination)
///
/// Shell quoting and complex expressions are not parsed — this is a
/// best-effort heuristic, not a full shell interpreter. False negatives
/// (missed files) are safe: we just lose the diff baseline for that
/// Read. False positives (non-existent paths) fail silently via the
/// `read_to_string` error path.
pub(crate) fn extract_bash_write_targets(cmd: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let n = tokens.len();
    let mut i = 0;
    while i < n {
        let t = tokens[i];
        // `> file` or `>> file`
        if (t == ">" || t == ">>") && i + 1 < n {
            let p = tokens[i + 1].trim_matches(|c| c == '"' || c == '\'');
            if !p.is_empty() && !p.starts_with('-') && !p.starts_with('&') {
                targets.push(p.to_string());
            }
            i += 2;
            continue;
        }
        // `>file` or `>>file` without space
        let rest = t
            .strip_prefix(">>")
            .or_else(|| t.strip_prefix('>'))
            .map(|r| r.trim_matches(|c| c == '"' || c == '\''));
        if let Some(p) = rest {
            if !p.is_empty() && !p.starts_with('-') && !p.starts_with('&') {
                targets.push(p.to_string());
                i += 1;
                continue;
            }
        }
        // `tee [-a|--append] file ...`
        if t == "tee" {
            let mut j = i + 1;
            while j < n {
                let a = tokens[j];
                if a == "-a" || a == "--append" {
                    j += 1;
                    continue;
                }
                if a.starts_with('-') {
                    break;
                }
                targets.push(a.trim_matches(|c| c == '"' || c == '\'').to_string());
                j += 1;
            }
        }
        // `cp src dst` or `mv src dst` — last non-flag argument
        if (t == "cp" || t == "mv") && i + 2 < n {
            // walk to last non-flag token
            let mut last = None;
            for tok in &tokens[(i + 1)..n] {
                if !tok.starts_with('-') {
                    last = Some(*tok);
                }
            }
            if let Some(p) = last {
                targets.push(p.trim_matches(|c| c == '"' || c == '\'').to_string());
            }
        }
        i += 1;
    }
    targets
}

/// Read each file written by a Bash command and register it as a
/// canonical (file, 0, 0) path baseline so the next `Read` of that
/// file hits L31 diff instead of passthrough. Redacts secrets before
/// storing — consistent with the pre-edit-snapshot and Read hooks.
pub(crate) fn register_bash_write_baselines(cmd: &str) {
    let paths = extract_bash_write_targets(cmd);
    if paths.is_empty() {
        return;
    }
    let root = root();
    for file_path in &paths {
        let p = std::path::Path::new(file_path);
        let content = match std::fs::read_to_string(p) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if content.len() < tokenade_core::dedup_ref::MIN_BYTES_TO_DEDUPE {
            continue;
        }
        let content = tokenade_core::redact::redact(&content);
        let hash = tokenade_core::dedup_ref::hash_content(content.as_bytes());
        tokenade_core::dedup_ref::register(&root, &hash, &content);
        let pk = tokenade_core::dedup_ref::path_key(file_path, 0, 0);
        tokenade_core::dedup_ref::register_path_pointer(&root, &pk, &hash);
        tokenade_core::debug_log::log(
            "hookwrap",
            "bash_write_baseline",
            serde_json::json!({
                "file": file_path,
                "hash": &hash[..12],
            }),
        );
    }
}
