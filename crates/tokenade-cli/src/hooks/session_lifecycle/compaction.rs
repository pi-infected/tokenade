//! **What the host's compaction takes away, and what we save from it.**
//!
//! When the agent compacts its own session, it throws away the context we
//! had folded — and the recovery pointers go with it. What is here puts back
//! in front of it the references still worth offering, and only those:
//! recalling too many is noise in the one turn where it has no room left.

use super::*;

/// `tokenade hookwrap-precompact` — Claude Code `PreCompact` hook
/// entry point. Fires JUST BEFORE Claude Code's built-in
/// conversation auto-summariser runs (when context fills enough to
/// trigger it). We emit a one-line `additionalContext` reminding
/// the agent to PROACTIVELY save key facts to tokenade's structured
/// stores (memory, wiki) BEFORE summarisation drops them.
///
/// Why this matters: Claude Code's summariser is generic and lossy
/// — multi-step debugging chains, file-content snippets, and tool
/// outputs all get squashed into prose. Tokenade's memory + wiki
/// give the agent a typed escape hatch.
///
/// Cost: ~30 tokens once per compaction event (rare, maybe 1-2
/// times per very-long session). Reward: agent saves what matters
/// before it's compressed away.
///
/// Bypass:
///   - `TOKENADE_HOOK_DISABLED=1` (or `STS_HOOK_DISABLED=1`).
///   - Malformed stdin → silent (no crash, no compaction-blocking).
/// The context of session `session_id` was emptied — compacted, or cleared by
/// a `/clear` that keeps the session id: what tokenade folded out of it is no
/// longer re-read (the credit stops), nor is what it injected (the charge
/// stops), and pending replays are void. The rules file, reloaded into the
/// next window, stays charged.
pub(crate) fn note_context_reset(session_id: &str) {
    note_session_compaction_reset(session_id);
    let skey: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    if skey.is_empty() {
        return;
    }
    crate::hooks::replay::mark_compaction(&skey);
    super::credit::note_overhead_at_compaction(&skey);
}

pub(crate) fn cmd_hookwrap_precompact() -> i32 {
    if hook_disabled_by_env() {
        return 0;
    }
    let started = std::time::Instant::now();
    let Some(input) = crate::hooks::stdin::read_event("hookwrap-sessionstart") else {
        return 0;
    };
    // Parse for telemetry + to locate the transcript; tolerate malformed.
    let parsed = crate::hooks::stdin::parse_event(&input);
    let session_id = parsed
        .as_ref()
        .and_then(crate::hooks::session_id_of)
        .unwrap_or("");
    let transcript_path = parsed
        .as_ref()
        .and_then(|p| p.get("transcript_path"))
        .and_then(|s| s.as_str())
        .unwrap_or("");

    // Re-arm the once-per-session hint throttle: the summariser about to
    // run squashes our previously-injected hint lines out of the live
    // context, so without this they'd stay suppressed forever despite the
    // agent no longer seeing them. Resetting here lets each hint fire once
    // more when its condition next reappears post-compaction.
    reset_session_hints(session_id);

    // Invalidate range-coverage dedup for this session: the summariser is
    // about to squash earlier turns, so the verbatim bytes we registered as
    // "still in context" may no longer be. Dropping the session's ranges
    // means a post-compaction re-read re-ships in full rather than serving a
    // §ref to content that got summarised away. (expand-ref still recovers
    // it, but correctness-first: never assert "in context" once it isn't.)
    tokenade_core::dedup_ref::clear_session_ranges(session_id);
    // Same invariant for the WebSearch seen-set: results served before the
    // summary may be squashed out of context, so post-compaction searches
    // must re-serve rather than omit "already seen" URLs.
    crate::hooks::websearch_post::clear_websearch_seen(session_id);
    // Same reason for the auto-style marker: the squashed preamble must be
    // re-stated on the next turn.
    clear_session_style_marker(session_id);

    // Compounding horizon: the summariser squashes the history segments our
    // folds would otherwise have kept getting re-sent in — their compounding
    // credit stops HERE (owner audit 2026-07-06: a saved token stops paying
    // once the segment that contained it is compacted).
    // Also voids pending replays and fixes the injected-context base: their
    // outputs leave the live context with the summary about to run. MUST
    // happen even without a transcript path (some agent dialects omit it).
    note_context_reset(session_id);

    // P3 write-barrier: the summary about to run will drop the inline
    // `§ref:<hash>§` / `expand_ref hash="…"` markers that point at bytes
    // tokenade folded away earlier in the session. Two failure modes to
    // prevent: (1) the LRU size-cap (`evict_to_size_cap`) could pick one
    // of those very refs as a victim right when the agent is about to
    // need it post-summary; (2) the agent forgets the hash existed at
    // all. So we scan the transcript for live refs, bump their last_used
    // (protecting them from eviction), and write a durable per-session
    // manifest the agent can consult to `expand_ref` after the summary.
    let mut barrier_total = 0usize;
    let mut barrier_resident = 0usize;
    // Prune what the summariser is about to read. This is the ONE moment where
    // rewriting the transcript is free: Claude Code is about to rebuild the
    // whole prefix anyway, so the cache invalidation that makes this expensive
    // mid-session costs nothing here. Everything the session has already
    // superseded — a file read before that file was edited, a command re-run
    // verbatim — is dead weight the summary would otherwise have to carry, and
    // squeezing it out leaves the summariser more room for live facts.
    //
    // Runs BEFORE the write-barrier on purpose: masking creates new recovery
    // pointers, and the barrier is what protects them from LRU eviction.
    let mut masked_blocks = 0usize;
    let mut masked_saved = 0usize;
    // Its own switch: `TOKENADE_NO_SESSION_COMPACT` is the SessionStart trim of
    // idle transcripts, a different moment with a different cost (#12).
    if !transcript_path.is_empty()
        && !tokenade_core::env_compat::is_truthy_one("TOKENADE_NO_PRECOMPACT_MASK")
    {
        if let Ok(st) = tokenade_core::session_compact::compact_transcript(std::path::Path::new(
            transcript_path,
        )) {
            masked_blocks = st.blocks_masked;
            masked_saved = st.tokens_before.saturating_sub(st.tokens_after);
        }
    }
    if !transcript_path.is_empty() {
        if let Some((_manifest, total, resident)) =
            precompact_write_barrier(transcript_path, session_id)
        {
            barrier_total = total;
            barrier_resident = resident;
        }
    }

    // Emit NOTHING. Claude Code's PreCompact hook schema accepts no
    // `hookSpecificOutput` variant — an `additionalContext` payload here
    // fails validation, which both surfaces a hook error to the user on
    // every compaction and silently discards the message. All model-facing
    // guidance lives in the post-compaction SessionStart hook instead
    // (`source: "compact"` → recovery note + freshest stashed refs); this
    // handler's job is purely the side effects above (state resets + the
    // ref write-barrier and its manifest).
    tokenade_core::debug_log::log(
        "hookwrap-precompact",
        "state_reset",
        serde_json::json!({
            "input_chars": input.len(),
            "refs_total": barrier_total,
            "refs_resident": barrier_resident,
            "masked_blocks": masked_blocks,
            "masked_saved": masked_saved,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

/// Bound every store tokenade keeps on the user's disk, at most once every 24 h.
///
/// Four sweeps, all discovered the same way — a retention policy that existed on
/// paper and that nothing invoked:
///   * `dedup.db` against its 150 MiB cap, and the stash against its 2 GiB one:
///     both caps were documented, tested, and reachable ONLY through the manual
///     `evict-stale` command;
///   * `read-pending`, whose 10-minute TTL lived inside staging functions that
///     only run when a Read or Grep happens to be staged;
///   * tokenade's own transient backups, which had no horizon at all.
///
/// It was named `enforce_size_caps_daily` while it grew to cover the last two —
/// renamed so the name stops under-describing the blast radius. See the call
/// site in [`cmd_hookwrap_sessionstart`] for why this lives on SessionStart.
/// Never prints: SessionStart's stdout is the agent's context.
pub(crate) const LOCAL_STATE_SWEEP_TTL_SECS: u64 = 24 * 3600;

/// Stamp name. Changing it from the older `size-caps` costs one extra sweep on
/// upgrade, which is idempotent maintenance — cheaper than a stamp that names
/// a fraction of what it gates.
pub(crate) const LOCAL_STATE_STAMP: &str = "local-state-sweep";

pub(crate) fn daily_local_state_sweep() {
    let Some(home) = tokenade_core::platform::home_dir() else {
        return;
    };
    if !tokenade_core::stores::stamp_due(&home, LOCAL_STATE_STAMP, LOCAL_STATE_SWEEP_TTL_SECS) {
        return;
    }
    // Stamp BEFORE the work, like `ensure_hooks`: two sessions opening at once
    // must not both run the sweep.
    tokenade_core::stores::mark_stamp(&home, LOCAL_STATE_STAMP);

    let (dedup_rows, dedup_freed) = tokenade_core::dedup_ref::evict_to_size_cap();
    let (stash_files, stash_freed) = tokenade_core::disclose::evict_to_size_cap().unwrap_or((0, 0));
    // `read-pending` has a 10-minute TTL and two opportunistic GCs, but both sit
    // inside staging functions and only run when a Read or Grep happens to be
    // staged — so on a real install the directory still held 12 228 files,
    // 5 692 of them between one and seven days old. Sweeping it from here does
    // not depend on any hot path firing. The cost is inodes and 4 KiB blocks,
    // not bytes: `pending_freed` reads ~0 by design (see `COUNTER_TTL_SECS`).
    let (pending_files, pending_freed) = crate::hooks::read_staging::sweep_read_pending_in(
        &crate::hooks::read_staging::read_pending_dir(),
    );
    // Our own transient backups (log rotations, ledger purge copies, recalc
    // safety dirs). Measured 2026-08-03: ~38 MB aged 13-63 days, with nothing
    // anywhere that removed them. 30-day horizon; `last-report.zip` is
    // deliberately out of scope — the user made that one.
    let (bak_files, bak_freed) =
        tokenade_core::platform::sweep_own_backups_in(&home.join(".tokenade"));
    // SQLite hands freed pages to its own freelist, never back to the
    // filesystem. Fine for a hot store; wrong for these, which churn — measured
    // on a real install: index.db 36 % dead (19.4 MB), fold_store.db 49 %,
    // file_search.db 97 %. `dedup.db` sat at 0 % because its eviction already
    // ends in a VACUUM, which is the whole idea, just applied to one store out
    // of five. Done HERE and nowhere else: a VACUUM rewrites the file and takes
    // an exclusive lock, so it belongs on the once-a-day path and never on a
    // per-command one.
    let (vacuumed, vacuum_freed) =
        tokenade_core::platform::vacuum_bloated_stores(&home.join(".tokenade"));
    if dedup_rows > 0 || stash_files > 0 || pending_files > 0 || bak_files > 0 || vacuumed > 0 {
        tokenade_core::debug_log::log(
            "hookwrap-sessionstart",
            "local_state_swept",
            serde_json::json!({
                "dedup_rows": dedup_rows,
                "dedup_freed": dedup_freed,
                "stash_files": stash_files,
                "stash_freed": stash_freed,
                "pending_files": pending_files,
                "pending_freed": pending_freed,
                "backup_files": bak_files,
                "backup_freed": bak_freed,
                "vacuumed_stores": vacuumed,
                "vacuum_freed": vacuum_freed,
            }),
        );
    }
}

/// Re-arm recovery after an auto-compaction ate the §ref hashes.
///
/// The post-compaction re-entry is the ONLY documented hook channel that can
/// inject context AFTER an auto-summary (PreCompact is block-only), so this is
/// where the freshest stashed refs get named again. Returns the process exit
/// code, like every other terminal branch of the hook.
/// Which stashed refs to put in front of an agent whose context was just
/// compacted, and in what order.
///
/// Ranked by SIZE, not by recency. What a compaction costs is the DETAIL of
/// the big folded outputs — the ones the agent would have to re-run a command
/// or re-Read a file to get back. A 240-byte fragment was never a loss.
///
/// Measured 2026-08-27 on this machine, at the moment a real compaction fired:
/// the eight refs offered were `ef01278ce0ee (0k), 969130fc6b74 (0k), …` —
/// **eight zeros in a row**, 240 to 343 bytes each — while the same six-hour
/// window held 5.8 MB, 5.6 MB and three of 1.5 MB, none older than nine
/// minutes. Recency bought nothing (every large ref was inside 48 minutes) and
/// cost everything. The offer advertised its own uselessness, and an agent
/// reading eight `(0k)` learns not to call `expand-ref` at all.
///
/// The floor is where the corpus puts it, not where it felt right: of the 92
/// refs in that window, **53 are at least 512 bytes and the other 39 are all
/// below it**, so any cut between 512 and 8 192 selects the same 53. 1 KiB sits
/// in that gap. Below it a ref cannot return more than the ~15 tokens spent
/// naming it.
pub(crate) fn refs_worth_offering(
    dir: &std::path::Path,
    session: Option<&str>,
) -> Vec<(std::time::SystemTime, String, u64)> {
    refs_worth_offering_in(dir, &tokenade_core::session::sessions_dir(), session)
}

pub(crate) fn refs_worth_offering_in(
    dir: &std::path::Path,
    sessions_dir: &std::path::Path,
    session: Option<&str>,
) -> Vec<(std::time::SystemTime, String, u64)> {
    // Scope to what THIS session folded. The stash is one directory shared by
    // every agent on the machine, and the six-hour window below cannot tell
    // them apart: measured over this machine's ledger, 1 828 of 3 676 moments
    // (50 %) had more than one session active in the preceding six hours, and
    // 2026-08-28 alone held fifteen. The digest says "raw bytes that tokenade
    // folded earlier" — a claim about this agent's own lost detail — so
    // offering another session's work is not a near miss, it is the wrong
    // answer wearing the right sentence.
    //
    // `gain`'s session module already drew this line for the statusline
    // ticker; the digest kept the window.
    let mine: Option<std::collections::HashSet<String>> = session.map(|sid| {
        tokenade_core::disclose::session_ref_hashes_in(sessions_dir, sid)
            .into_iter()
            .collect()
    });
    /// A ref smaller than this costs more to advertise than it can give back.
    const MIN_WORTH_OFFERING: u64 = 1024;
    const WINDOW_SECS: u64 = 6 * 3600;
    const AT_MOST: usize = 8;

    let mut entries: Vec<(std::time::SystemTime, String, u64)> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return entries;
    };
    let now = std::time::SystemTime::now();
    for e in rd.flatten() {
        let Ok(md) = e.metadata() else { continue };
        let Ok(mtime) = md.modified() else { continue };
        // Only refs fresh enough to plausibly belong to THIS session.
        if now
            .duration_since(mtime)
            .map(|d| d.as_secs() > WINDOW_SECS)
            .unwrap_or(true)
        {
            continue;
        }
        if md.len() < MIN_WORTH_OFFERING {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        let hash = name.split('.').next().unwrap_or("").to_string();
        if hash.len() < 8 {
            continue;
        }
        // No record of this session having folded it → not ours to offer.
        if mine.as_ref().is_some_and(|m| !m.contains(&hash)) {
            continue;
        }
        entries.push((mtime, hash, md.len()));
    }
    entries.sort_by_key(|b| std::cmp::Reverse(b.2));
    entries.truncate(AT_MOST);
    entries
}

pub(crate) fn emit_compact_digest(started: std::time::Instant, session: Option<&str>) -> i32 {
    // Post-compaction re-entry — the ONLY documented hook channel that
    // can inject context AFTER an auto-summary (PreCompact is
    // block-only). The summary routinely drops the §ref hashes that
    // make folded bytes recoverable, leaving the agent to re-run
    // commands / re-Read files it already paid for. Re-arm it with a
    // tiny digest of the freshest stashed refs (~15 tokens each,
    // capped at 8) + the recovery affordance. Opt out:
    // TOKENADE_NO_COMPACT_DIGEST=1.
    if std::env::var("TOKENADE_NO_COMPACT_DIGEST").is_ok() {
        return 0;
    }
    let entries = refs_worth_offering(&tokenade_core::disclose::stash_dir(), session);
    let mut line = String::from(
        "[tokenade] Context was just auto-compacted. Raw bytes that tokenade folded              earlier are STILL recoverable without re-running anything:              `tokenade expand-ref <hash>` (add `--grep <pattern>` to filter).",
    );
    if !entries.is_empty() {
        // "Freshest" was the label AND the ranking, and both were wrong. Say
        // what these are now: the biggest things this session folded, which is
        // what a compaction actually costs.
        line.push_str(" Biggest folds still recoverable: ");
        let parts: Vec<String> = entries
            .iter()
            .map(|(_, h, sz)| format!("{h} ({}k)", sz / 1024))
            .collect();
        line.push_str(&parts.join(", "));
        line.push('.');
    }
    line.push_str(
        " Files read before the compaction re-serve as cheap diffs on re-Read —              prefer re-Reading over pasting from memory.",
    );
    // Collapse the multi-line string-literal indentation into single spaces.
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    log_emitted_additional_context("hookwrap-sessionstart-compact", "SessionStart", &line);
    println!(
        "{}",
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": line,
            }
        })
    );
    // Recovery-credit marker: an expand-ref within the next 30 min is
    // (statistically) the digest doing its job — the alternative was
    // re-running the original command. cmd_expand_ref credits it.
    if let Some(h) = tokenade_core::platform::home_dir() {
        let _ = std::fs::write(
            h.join(".tokenade").join("compact-digest-ts"),
            tokenade_core::platform::now_unix_secs().to_string(),
        );
    }
    tokenade_core::debug_log::log(
        "hookwrap-sessionstart",
        "compact_digest_emitted",
        serde_json::json!({"refs": entries.len(),
                           "elapsed_ms": started.elapsed().as_millis() as u64}),
    );
    0
}

/// Trim transcripts whose prompt cache has provably expired, and book what
/// that saved.
///
/// Both halves are gated on the same three conditions (a transcript to work
/// on, the opt-out unset, quota not paused), which is why they live in one
/// function rather than two called from the hook body.
pub(crate) fn compact_session_transcripts(
    parsed: &serde_json::Value,
    transcript_path: &str,
    source: &str,
    home: &Option<std::path::PathBuf>,
) {
    if !transcript_path.is_empty()
        && !tokenade_core::env_compat::is_truthy_one("TOKENADE_NO_SESSION_COMPACT")
    {
        // Idle threshold = max prompt-cache TTL (1 h ceiling, overridable via
        // TOKENADE_MAX_CACHE_TTL_SECS) + safety margin. A transcript untouched
        // longer than this has a provably-expired cache, so trimming it can
        // only shrink a future full-price re-send — never sacrifice a live
        // cache-read.
        let idle_threshold = tokenade_core::session_compact::trim_idle_threshold_secs();
        let active = std::path::Path::new(transcript_path);
        if let Some(dir) = active.parent() {
            let st = tokenade_core::session_compact::compact_stale_transcripts(
                dir,
                active,
                idle_threshold,
            )
            .unwrap_or_default();
            if st.blocks_masked > 0 {
                tokenade_core::debug_log::log(
                    "hookwrap-sessionstart",
                    "session_compact_trim",
                    serde_json::json!({"blocks": st.blocks_masked, "saved": st.saved()}),
                );
            }
        }
        // Resume credit (MEASURED): this resumed session genuinely re-sends its
        // (now trimmed) history, so the masked blocks are really avoided now.
        // A cold resume re-bills the changed prefix as INPUT, so the delta is
        // an input-class saving. Credited once per session_id via a sentinel
        // (which the sessions/ TTL sweep later reclaims).
        if source == "resume" {
            // Cache-window trim of the ACTIVE file being resumed. The stale
            // pass above skips the active transcript (a live session must never
            // be mutated), but on `resume` the file's mtime is the
            // last-activity-BEFORE-resume stamp: if that exceeds the max cache
            // TTL, this session's server-side prompt cache has already expired
            // by idle, so the very first resume request is a full-input miss
            // regardless. Trimming it now (in place, recoverable via
            // stash) only makes that unavoidable miss cheaper — zero risk to a
            // live cache. Gated strictly on provable idleness against the TTL
            // the transcript's own tail proves it uses (2026-07-12: Claude
            // Code writes 1 h-TTL cache entries, and a plain --resume rebuilds
            // a byte-identical prefix, so a resume within the window is a
            // guaranteed cache hit the trim must never sacrifice).
            if tokenade_core::session_compact::idle_beyond_cache_window(active) {
                if let Ok(st) = tokenade_core::session_compact::compact_transcript(active) {
                    if st.blocks_masked > 0 {
                        tokenade_core::debug_log::log(
                            "hookwrap-sessionstart",
                            "session_compact_resume_active",
                            serde_json::json!({"blocks": st.blocks_masked, "saved": st.saved()}),
                        );
                    }
                }
            }
            if let Some(h) = home.as_ref() {
                book_resume_credit(h, parsed, transcript_path);
            }
        }
    }
}

/// Credit the resumed session for the blocks its trimmed transcript no longer
/// re-sends.
///
/// Delta-based: the sentinel stores the total already credited, so a session
/// trimmed and resumed many times books each new saving exactly once.
pub(crate) fn book_resume_credit(
    h: &std::path::Path,
    parsed: &serde_json::Value,
    transcript_path: &str,
) {
    let skey: String = crate::hooks::session_id_of(parsed)
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    let sentinel = h
        .join(".tokenade")
        .join("sessions")
        .join(format!("{skey}-resume-credited"));
    if !skey.is_empty() {
        if let Ok(body) = std::fs::read_to_string(transcript_path) {
            let st = tokenade_core::session_compact::scan_masked(&body);
            // Delta-based credit (2026-07-11): with the trim
            // threshold at ~10 min a session gets trimmed and
            // resumed MANY times; the old once-per-session
            // sentinel credited only the first resume and dropped
            // every later trim. The sentinel now stores the total
            // saved already credited; each resume books only the
            // NEW masked delta. Legacy "1" content (old one-shot
            // format) seeds at the current total so history is
            // never double-credited.
            let total = st.saved();
            let prev: usize = match std::fs::read_to_string(&sentinel) {
                Ok(s) => {
                    let t = s.trim();
                    if t == "1" {
                        total
                    } else {
                        t.parse::<usize>().unwrap_or(0)
                    }
                }
                Err(_) => 0,
            };
            let delta = total.saturating_sub(prev);
            if delta > 0 {
                tokenade_core::gain::record_classed(
                    "hook",
                    "session_resume_saved",
                    transcript_path,
                    "session_compact",
                    tokenade_core::Savings::new(delta, 0),
                );
                // Feed the trim into the session's compounding log: the
                // masked blocks are also NOT re-read on every later call of
                // the resumed session. Through the one writer every fold
                // goes through: it dates the line, and the exact re-read
                // count (`gain::fold_rereads`) skips an undated one — this
                // path wrote its own line without `ts`, so on Claude Code a
                // resume trim earned no re-read credit at all.
                note_session_compaction(parsed, delta);
            }
            if let Some(p) = sentinel.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let _ = std::fs::write(&sentinel, total.to_string());
        }
    }
}
