#![allow(unused_imports)]
//! Split out of the monolithic `hooks.rs` (mechanical module split, no
//! behaviour change). Wired back into the `hooks` namespace by mod.rs via
//! `pub(crate) use <name>::*;`.
use super::*;
use crate::*;

mod hints;
mod pending;
mod steer;
pub(crate) use hints::*;
pub(crate) use pending::*;
pub(crate) use steer::*;

/// True for specific compactors that fold *values* away, not just whitespace.
/// The log family (generic logs / access logs / container logs / curl -v)
/// replaces timestamps, ids and numbers with placeholders and folds repeated
/// lines into a count: the compacted view shows the SHAPE faithfully, but a
/// specific one-off value the agent is hunting (a single timestamp, request id,
/// status line, error) can be absent from it even though it is still in the
/// file on disk. These kinds therefore get an explicit "grep the original file"
/// steer appended to the banner — reading the whole file to find one value is
/// wasteful, and the folded view alone could hide it. Lossless kinds
/// (json/csv/yaml/toml) carry every datum into the view and need no steer.
pub(crate) fn kind_folds_values(kind: &tokenade_core::auto::Kind) -> bool {
    use tokenade_core::auto::Kind;
    matches!(
        kind,
        Kind::Logs | Kind::AccessLog | Kind::ContainerLogs | Kind::CurlVerbose
    )
}

/// Is this kind's compaction LOSSLESS enough to *substitute* for the file at
/// Read time?  Only the pure structured-serialization formats qualify: their
/// compactor restates the same data (dedups blank lines / repeated structure,
/// normalises whitespace) without dropping any value — the compacted view
/// carries every fact the raw bytes did, so handing it to the agent loses
/// nothing.
///
/// WHY THIS GATE EXISTS (bench /tmp/thol-rerun, 2026-06-20, n=10):
/// read-auto-compact fired on exactly four tasks. The split is unambiguous and
/// tracks losslessness, NOT size:
///   * pipeline-py (json+csv, lossless): $0.342 ctl → $0.302 compact = WIN,
///     and it wins on reads as small as 913 tokens.
///   * cilog (logs, lossy-fold): parity ($0.0843 vs $0.0845).
///   * log-needle-zh (logs, lossy-fold): $0.123 ctl → $0.198 compact = +60% LOSS.
///   * seo-audit (html, lossy-strip): $0.260 ctl → $0.317 compact = +23% LOSS,
///     losing on reads of 423–728 tokens (SMALLER than pipeline's winning json).
/// So size is not the signal — a 728-token html loses, a 913-token json wins.
/// The discriminator is whether the compaction is lossless. For lossy kinds the
/// substituted view is the head-slice mistake again ([[project_head_slice_wins_nowhere]]):
/// the raw file would have been cheap `cache_read` (0.1×) that a competent agent
/// greps anyway, and the small intermediate view just adds a wasted read + a
/// `cache_creation` spike. Lossy kinds therefore PASS THROUGH on the Read path
/// (their specific compactors stay fully active on the *Bash* path, where
/// command output is fresh tokens, not cheap cache_read).
///
/// Deliberately CONSERVATIVE: a kind wrongly excluded only forgoes a possible
/// win (passthrough = parity with control); a kind wrongly included risks a real
/// regression. Off (restore: compact every kind) via `TOKENADE_READ_VIEW_ALL_KINDS`.
pub(crate) fn kind_lossless_read_view(kind: &tokenade_core::auto::Kind) -> bool {
    use tokenade_core::auto::Kind;
    if std::env::var_os("TOKENADE_READ_VIEW_ALL_KINDS").is_some() {
        return true; // A/B escape: pre-0.6.2 behaviour (substitute any kind).
    }
    matches!(
        kind,
        Kind::Json | Kind::Yaml | Kind::Csv | Kind::Tsv | Kind::Ndjson | Kind::TomlConfig
    )
}

/// A/B gate for retiring head-slice as a *compacted-view delivery channel*.
///
/// The bench is unambiguous: on every large-Read task (log-needle, log-forensics,
/// data-*, doc-digest, config-peek, html-extract) head-slice LOSES vs control
/// (+0.9 % … +8.2 %), and `tokenade-acoff` (additionalContext OFF) loses almost
/// as much — so the cost is the *channel itself*, not the injection: a ~100-token
/// `limit:20` tool_result can't anchor a prompt-cache breakpoint, so the read
/// block is re-created at the first Edit (one cache invalidation per run, never
/// amortised). The json/csv/yaml compactors already escaped this via the temp-file
/// redirect (`from_specific_compactor` in `emit_read_compaction`, the ONLY arm
/// that wins). This gate (default ON) extends that channel to every branch that
/// delivers a real compacted VIEW — code-strip and the generic dedup fallback.
/// Off with `TOKENADE_NO_READ_VIEW_REDIRECT` for A/B / escape.
pub(crate) fn read_view_redirect_enabled() -> bool {
    std::env::var_os("TOKENADE_NO_READ_VIEW_REDIRECT").is_none()
}

/// Run the L64 disclose stash on a compacted VIEW, then deliver it:
///   * a full view (below the stash threshold) → cache-stable temp-redirect
///     (the `from_specific_compactor=true` channel) — retires the losing
///     head-slice for these branches;
///   * a stashed marker (huge view) → legacy head-slice, because a marker is a
///     pointer, not readable content — redirecting it would hand the agent the
///     pointer instead of the file.
/// `maybe_stash_with_threshold` returns its input verbatim below threshold and a
/// strictly shorter marker when it stashes, so length is the reliable signal.
pub(crate) fn emit_view_compaction(
    parsed: &serde_json::Value,
    file_path: &str,
    offset: u64,
    limit: u64,
    reason: String,
) -> Option<tokenade_core::Savings> {
    let pre_stash_len = reason.len();
    let reason = tokenade_core::disclose::maybe_stash_with_threshold(
        reason,
        tokenade_core::disclose::SourceHint::File(std::path::PathBuf::from(file_path)),
        tokenade_core::disclose::read_threshold_tokens(),
    );
    let full_view = reason.len() >= pre_stash_len && read_view_redirect_enabled();
    emit_read_compaction(parsed, file_path, offset, limit, reason, full_view)
}

pub(crate) fn emit_read_compaction(
    parsed: &serde_json::Value,
    file_path: &str,
    offset: u64,
    limit: u64,
    reason: String,
    from_specific_compactor: bool,
) -> Option<tokenade_core::Savings> {
    let sav = emit_read_compaction_inner(
        parsed,
        file_path,
        offset,
        limit,
        reason,
        from_specific_compactor,
    );
    // The ledger row the caller writes for this fold banks it for the
    // re-read credit (`gain::bank`), bounded by the Read delivery cap.
    sav
}

/// Serve the view a SPECIFIC read compactor already produced.
///
/// `Some(savings)` = served here and the hook is done. `None` = this path did
/// not apply (unreadable file, net loss, IO error) and the caller falls through
/// to the inline head-slice / deny flow — never a silent drop.
fn serve_specific_compactor(
    parsed: &serde_json::Value,
    file_path: &str,
    offset: u64,
    limit: u64,
    reason: &str,
) -> Option<tokenade_core::Savings> {
    if let Ok(raw) = std::fs::read_to_string(file_path) {
        let lines: Vec<&str> = raw.lines().collect();
        let start = if offset > 0 {
            (offset as usize).saturating_sub(1).min(lines.len())
        } else {
            0
        };
        let raw_tokens = tokenade_core::tokenizer::estimate(&lines[start..].join("\n"));
        let compacted_tokens = tokenade_core::tokenizer::estimate(reason);
        // Net guard: only redirect when the view is genuinely smaller than
        // the raw read it replaces (it almost always is for these formats).
        if compacted_tokens < raw_tokens {
            let session_id = crate::hooks::session_id_of(parsed).unwrap_or("");
            if let Some(temp_path) =
                stage_read_temp_file(session_id, file_path, offset, limit, reason)
            {
                if let Some(temp_str) = temp_path.to_str() {
                    // Redirect file_path → temp; drop limit/offset so the
                    // agent reads the whole compacted view (it already
                    // reflects the requested slice). Native Read truncation
                    // bounds it if it is large — same cache-stable path the
                    // control arm hits.
                    let mut updated = parsed
                        .get("tool_input")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({}));
                    if let Some(obj) = updated.as_object_mut() {
                        obj.insert("file_path".into(), serde_json::json!(temp_str));
                        obj.remove("limit");
                        obj.remove("offset");
                    }
                    let decision = serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": "allow",
                            "permissionDecisionReason":
                                "tokenade: serving a compacted view of this file directly (cache-stable)",
                            "updatedInput": updated,
                        }
                    });
                    println!("{decision}");
                    tokenade_core::debug_log::log(
                        "hookwrap-pretool",
                        "read_compaction_temp_redirect",
                        serde_json::json!({
                            "file": file_path,
                            "temp": temp_str,
                            "raw_tokens": raw_tokens,
                            "compacted_tokens": compacted_tokens,
                        }),
                    );
                    return Some(tokenade_core::Savings::new(raw_tokens, compacted_tokens));
                }
            }
        }
    }
    // Any failure (unreadable file, net loss, IO error) falls through to
    // the inline head-slice / deny flow below — never a silent drop.
    None
}

pub(crate) fn emit_read_compaction_inner(
    parsed: &serde_json::Value,
    file_path: &str,
    offset: u64,
    limit: u64,
    reason: String,
    from_specific_compactor: bool,
) -> Option<tokenade_core::Savings> {
    // CENTRAL net anti-inflation guard + authoritative savings accounting —
    // covers EVERY read-compaction branch (auto-compact, skeleton,
    // json/yaml/csv/lockfile/…) in one place, so no caller can re-introduce
    // the leak OR over-report the gain.
    //
    // The classic guard (compacted < raw, see filter.rs SAFETY NET 3) is NOT
    // sufficient on the Read path: the compacted body is delivered as a
    // PostToolUse `additionalContext` ON TOP OF the head-slice that
    // `updatedInput` leaves in the tool_result — it is an ADD, not a
    // substitution. So the true cost is `head_slice + injection`, which can
    // exceed the raw read even when the compacted body alone is smaller (e.g.
    // a 476-token HTML page compacted to 379 but re-injected whole: head≈full
    // page + 379 + banner > 476). Measured: seo-audit, 9 small reads, +4.7k
    // tokens injected, cache_creation x1.83, +44% cost. Refuse unless the
    // COMBINED cost is genuinely below the raw read.
    //
    // The SAME `head_slice + injection` figure is the TRUE post-compaction
    // size, so we return it as the realised `Savings` (raw → head+injection).
    // Call sites log THIS instead of their optimistic `compacted-vs-raw` view,
    // which ignored the residual head slice the additionalContext rides on top
    // of. Returns `None` on passthrough (net loss) or when the file can't be
    // measured (fail-open) — callers then skip the gain_log entry.
    // S1 — Don't engage on a Read carrying a param we'd have to strip. Both
    // exits below either rewrite `updatedInput` to {file_path,limit,offset}
    // (staged path) or substitute the whole output (legacy deny); either way
    // a param we don't understand (`pages` on a PDF today, any future param
    // tomorrow) would be silently dropped, mutilating the agent's call and
    // sending it into a re-Read spiral. Passthrough on ANY unknown key — this
    // generalises `reads_pdf_natively` to any param / format / agent instead
    // of patching format by format.
    if let Some(extra) = parsed.get("tool_input").and_then(unknown_read_param) {
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "read_compaction_unknown_param_passthrough",
            serde_json::json!({ "file": file_path, "param": extra }),
        );
        return None;
    }
    // Paul's rule (2026-06-19) — a SPECIFIC Read compactor (json/csv/yaml/…)
    // delivers its compacted view DIRECTLY as the Read result, by redirecting
    // `file_path` to a temp file holding that view. This replaces the old
    // head-slice + PostToolUse `additionalContext` delivery, whose ~100-token
    // tool_result was too small to anchor a prompt-cache breakpoint → the read
    // phase was never durably cached → Claude Code re-created the whole Read
    // block at the first Edit (measured: one cache_read→base invalidation per
    // run, wiping the compaction gain). A single redirected read returns the
    // WHOLE compacted body as one ordinary tool_result, exactly like the
    // control arm's full reads — cache-stable AND still smaller than raw.
    // Scoped to specific compactors only; the generic disclose fallback keeps
    // the existing flow.
    if from_specific_compactor {
        if let Some(s) = serve_specific_compactor(parsed, file_path, offset, limit, &reason) {
            return Some(s);
        }
    }
    // Paul's rule (2026-06-17) — a compacted view that is itself large is not
    // inlined. Above READ_INLINE_TOKEN_CAP we replace the content with a steer
    // BEFORE staging, so every downstream consumer (the staged head+inject, the
    // net-savings accounting, the PostToolUse delivery, and the legacy-deny
    // substitution) carries the small steer rather than thousands of tokens of
    // digest. Central here ⇒ covers every read-compaction branch at once.
    let compacted_view_tokens = tokenade_core::tokenizer::estimate(&reason);
    let too_large_steered = compacted_view_tokens > READ_INLINE_TOKEN_CAP;
    let mut reason = if too_large_steered {
        tokenade_core::debug_log::log(
            "hookwrap-pretool",
            "read_compaction_too_large_steer",
            serde_json::json!({ "file": file_path, "compacted_tokens": compacted_view_tokens }),
        );
        read_too_large_steer(file_path, compacted_view_tokens)
    } else {
        reason
    };
    let mut net_savings: Option<tokenade_core::Savings> = None;
    if let Ok(raw) = std::fs::read_to_string(file_path) {
        let lines: Vec<&str> = raw.lines().collect();
        let start = if offset > 0 {
            (offset as usize).saturating_sub(1).min(lines.len())
        } else {
            0
        };
        // Respect `limit`: the net guard used to compare against the WHOLE
        // remainder of the file even for `Read(offset, limit=100)`, letting
        // the injection exceed what the native slice would cost (and gain
        // rows over-credit savings).
        let end = if limit > 0 {
            (start + limit as usize).min(lines.len())
        } else {
            lines.len()
        };
        let visible = &lines[start..end];
        // Register the RAW visible content as the diff baseline. The staged
        // paths (code-strip / auto-compact / temp-redirect) never registered
        // a prior, so `try_exact_slice_diff` had nothing to diff against on
        // the NEXT read — big code files re-sent a full compacted view after
        // every edit (545 re-reads of one file in a real session; 3 diff
        // hits total in prod). With the baseline in place, a post-edit
        // re-read serves a few-line diff instead.
        {
            let joined = visible.join("\n");
            if joined.len() >= tokenade_core::dedup_ref::MIN_BYTES_TO_DEDUPE {
                let vhash = tokenade_core::dedup_ref::hash_content(joined.as_bytes());
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                let pk = tokenade_core::dedup_ref::path_key(file_path, offset, limit);
                tokenade_core::dedup_ref::register(&cwd, &vhash, &joined);
                tokenade_core::dedup_ref::register_path_pointer(&cwd, &pk, &vhash);
                if offset == 0 && limit == 0 {
                    // canonical key == pk here; nothing more to do
                } else {
                    // ALSO refresh the canonical (0,0) baseline so paginated
                    // diffs of other windows can anchor on the same file.
                    let full = lines.join("\n");
                    let fhash = tokenade_core::dedup_ref::hash_content(full.as_bytes());
                    let cpk = tokenade_core::dedup_ref::path_key(file_path, 0, 0);
                    tokenade_core::dedup_ref::register(&cwd, &fhash, &full);
                    tokenade_core::dedup_ref::register_path_pointer(&cwd, &cpk, &fhash);
                }
            }
        }
        // Generic large-file Read steer (mirrors the Bash `emit_head_slice_advisory`
        // for the Read head-slice path). Appended BEFORE the net-savings estimate
        // so the footer's own cost is counted by the guard below — a tiny file
        // where the footer would tip `effective_after` past `raw` falls through
        // to passthrough, never inflates.
        if should_search_steer(from_specific_compactor, too_large_steered, visible.len()) {
            reason.push_str(&read_search_steer(file_path, visible.len()));
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "read_compaction_search_steer",
                serde_json::json!({ "file": file_path, "lines": visible.len() }),
            );
        }
        let raw_tokens = tokenade_core::tokenizer::estimate(&visible.join("\n"));
        let inject_tokens = tokenade_core::tokenizer::estimate(&reason);
        // Residual tool_result after the rewrite: a head slice on the staged
        // path, nothing on the legacy-deny path (it substitutes the Read).
        let head_slice_tokens = if read_flow_wants_legacy_deny() {
            0
        } else {
            let head_n = if limit > 0 {
                limit.min(READ_HEAD_LINES)
            } else {
                READ_HEAD_LINES
            } as usize;
            tokenade_core::tokenizer::estimate(
                &visible
                    .iter()
                    .take(head_n)
                    .copied()
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        };
        let effective_after = head_slice_tokens + inject_tokens;
        if effective_after >= raw_tokens {
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "read_compaction_net_loss_skipped",
                serde_json::json!({
                    "file": file_path,
                    "raw_tokens": raw_tokens,
                    "head_slice_tokens": head_slice_tokens,
                    "inject_tokens": inject_tokens,
                }),
            );
            // Passthrough: emit nothing — Claude Code runs the Read normally.
            return None;
        }
        net_savings = Some(tokenade_core::Savings::new(raw_tokens, effective_after));
    }
    if !read_flow_wants_legacy_deny() {
        let session_id = crate::hooks::session_id_of(parsed).unwrap_or("");
        // Respect an explicit smaller limit — a tiny slice request
        // must not grow into a 20-line head.
        let head = if limit > 0 {
            limit.min(READ_HEAD_LINES)
        } else {
            READ_HEAD_LINES
        };
        // #6 — staged under the limit PostToolUse will SEE: Claude Code hands
        // that hook the rewritten input (`limit: head`), not the caller's. Keyed
        // on the caller's `limit`, every shrunk Read (`limit: 60` → 20, no limit
        // → 20) missed its entry: the agent got the head slice and nothing
        // else — a silent cut, while the reason promised content to follow.
        if stage_read_pending(session_id, file_path, offset, head, &reason) {
            // S2 — Merge, don't reconstruct: start from the original
            // tool_input and override only limit/offset, so no param is ever
            // dropped in the rewrite. Defence in depth — S1 above already
            // passes through on unknown keys, so today only the known three
            // can reach here, but this keeps the rewrite lossless if that
            // whitelist ever widens.
            let mut updated = parsed
                .get("tool_input")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({ "file_path": file_path }));
            updated["limit"] = serde_json::json!(head);
            if offset > 0 {
                updated["offset"] = serde_json::json!(offset);
            }
            let decision = serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "allow",
                    "permissionDecisionReason":
                        "tokenade: Read shrunk to a head slice; compacted content follows as additionalContext",
                    "updatedInput": updated,
                }
            });
            println!("{decision}");
            tokenade_core::debug_log::log(
                "hookwrap-pretool",
                "read_compaction_staged",
                serde_json::json!({
                    "file": file_path,
                    "head_lines": head,
                    "context_bytes": reason.len(),
                }),
            );
            return net_savings;
        }
        // Staging failed (no home dir, disk full, …) — fall through to
        // the deny so the compacted content is never silently dropped.
    }
    let reason = format!(
        "{reason}\n\n\
         (tokenade: this Read was substituted — Claude Code did NOT mark the \
         file as read. Before Editing this file, do a short Read with \
         offset/limit first.)"
    );
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    });
    println!("{decision}");
    net_savings
}

/// PostToolUse handler for `Read` — delivers the compacted content
/// staged by `emit_read_compaction` when it shrank the Read to a head
/// slice. Silent on every failure path (no staged entry, stale entry,
/// unparseable stdin): the head slice the agent already got is genuine
/// file content, so the worst case is a plain small Read.
/// #1066 — Claude Code spills a large PostToolUse `additionalContext` to a
/// readable `tool-results/<tool_use_id>-N-additionalContext.txt` file and
/// surfaces its path. Agents then `Read` that file in full, pulling the entire
/// compacted view back into context — undoing the very compaction it carried
/// (observed: a 145k→20k log re-read as a 25k-capped, paginated file, ~+90%
/// session cost, ~30% of needle-search runs). The read-auto-compact view sits
/// under [`READ_DEFAULT_THRESHOLD_TOKENS`] (25k) so it is NOT stashed, and is
/// delivered whole. Keep the delivered `additionalContext` comfortably inline
/// so Claude Code never spills it; above the cap, hand over a bounded head plus
/// an explicit steer to the cheap expand path instead of a re-readable file.
pub(crate) const POSTREAD_INLINE_CAP_BYTES: usize = 8_000;

pub(crate) fn cap_postread_additional_context(context: &str, file_path: &str) -> String {
    if context.len() <= POSTREAD_INLINE_CAP_BYTES {
        return context.to_string();
    }
    // Over the inline byte budget. Don't deliver a truncated digest head — for
    // an extraction task it is noise (the wanted line may be past the cut) and
    // it still costs every turn. Replace it wholesale with the steer. Most
    // large views are already swapped for a steer at staging by
    // `read_too_large_steer` (token gate); this is the spill-safety net for the
    // byte-large / token-small and stale-entry cases, and guarantees the
    // delivered payload stays small enough that Claude Code never spills it.
    read_too_large_steer(file_path, tokenade_core::tokenizer::estimate(context))
}

#[cfg(test)]
mod sweep_tests {
    use super::{sweep_read_pending_in, COUNTER_TTL_SECS, READ_PENDING_TTL_SECS};
    use std::time::{Duration, SystemTime};

    fn seed(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tk-sweep-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_aged(dir: &std::path::Path, name: &str, secs_old: u64) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"x").unwrap();
        let when = SystemTime::now() - Duration::from_secs(secs_old);
        // `set_times` is the only way to age a file without sleeping.
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(when))
            .unwrap();
        p
    }

    /// The whole point: a counter must outlive a long think gap, but not a week.
    #[test]
    fn counters_survive_a_think_gap_and_die_after_a_day() {
        let d = seed("counters");
        let fresh = write_aged(&d, "loop-abc-123", READ_PENDING_TTL_SECS + 60);
        let ancient = write_aged(&d, "loop-abc-456", COUNTER_TTL_SECS + 60);
        let (n, _) = sweep_read_pending_in(&d);
        assert!(
            fresh.exists(),
            "a counter 11 minutes old was swept — that resets the loop guard \
             whenever the agent thinks for a while, which is why they were made \
             immortal in the first place"
        );
        assert!(!ancient.exists(), "a counter older than a day must go");
        assert_eq!(n, 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Staged payloads keep the short horizon they always had.
    #[test]
    fn staged_payloads_still_die_after_ten_minutes() {
        let d = seed("payloads");
        let old = write_aged(&d, "bashseen-deadbeef-01", READ_PENDING_TTL_SECS + 60);
        let young = write_aged(&d, "bashseen-deadbeef-02", 5);
        let (n, _) = sweep_read_pending_in(&d);
        assert!(!old.exists() && young.exists());
        assert_eq!(n, 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_directory_is_not_an_error() {
        let d = std::env::temp_dir().join("tk-sweep-absent-does-not-exist");
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(sweep_read_pending_in(&d), (0, 0));
    }
}
