//! **A delivered fold stays out of the context for the rest of the session.**
//!
//! Every token a fold removes would have been re-read on every following call,
//! until compaction. `fold_rereads` counts those re-reads from the session's
//! compaction journal (`{key}-compactions.jsonl`); this module is the ONLY one
//! that deposits a measured fold there. The deposit used to be made by hand in
//! `wrap` and in the Read hook, and nowhere else: MEASURED 2026-10-08 over 14
//! days, ≈ 7.7 M tokens folded by `tokenade read`/`web`/`skeleton`, WebFetch,
//! the Read dedup, images and documents earned no re-read at all. It now
//! happens when the row is written.

use super::*;

/// Classes whose row must not be banked: a HEURISTIC baseline
/// (`ci_heuristic`, no bytes the agent would have held) or a row whose path
/// already banks the fold itself (`session_compact`, the resume).
const NOT_BANKED_CLASSES: &[&str] = &["ci_heuristic", "session_compact"];

/// Sources that count their re-reads themselves: the call-by-call proxy
/// (exact), `fold-payload` which has neither usage nor context size.
const OWN_REREADS_SOURCES: &[&str] = &["llm-proxy", "fold-payload"];

/// Called AFTER `record_inner` has released its process lock: banking a
/// sub-agent fold writes an estimated row, which takes it again.
pub(super) fn bank_measured_row(
    file_name: &str,
    source: &str,
    op: &str,
    class: Option<&str>,
    agent: &str,
    s: &Savings,
) {
    if file_name != "gain.jsonl" {
        return;
    }
    let Some(skey) = session_key() else {
        return;
    };
    bank_fold(&skey, banked_delta(source, op, class, agent, s) as usize);
}

/// What a MEASURED row removes from the context from now on: what the agent
/// would really have held (`deliverable`, otherwise its channel's delivery
/// cap) minus what it received — the Read hook's bound, generalised.
///
/// A row with no KNOWN cap is not banked. Those are the self-bounded
/// baselines (`html-to-markdown`, `extract-*`, images, search): their
/// "before" is a modelled credit, not bytes the agent would have held — for
/// Claude Code, a web page is worth 0.6 × its raw HTML, up to 435 k tokens
/// (MEASURED 2026-10-08), whereas WebFetch only puts a summary in the
/// context. Banked as is, that credit would have been re-read ~200 times.
pub(super) fn banked_delta(
    source: &str,
    op: &str,
    class: Option<&str>,
    agent: &str,
    s: &Savings,
) -> u64 {
    if OWN_REREADS_SOURCES.contains(&source)
        || class.is_some_and(|c| NOT_BANKED_CLASSES.contains(&c))
    {
        return 0;
    }
    let shown = match s.deliverable_tokens {
        Some(d) => d,
        None => match crate::coefficients::delivery_cap_tokens(agent, source, op) {
            u64::MAX => return 0,
            cap => cap,
        },
    };
    (s.tokens_before as u64)
        .min(shown)
        .saturating_sub(s.tokens_after as u64)
}

/// Banks `saved` folded tokens into the journal of session `safe` (key
/// already normalised).
pub fn bank_fold(safe: &str, saved: usize) {
    if saved == 0 || safe.is_empty() {
        return;
    }
    // A fold made while a subagent runs is in the SUBAGENT's context, which
    // shares the parent's session id but not its context: banked here, it
    // was credited on every parent call until the parent compacted (~217
    // re-reads) for output the parent never held. MEASURED 2026-10-05, 30
    // days: 1 122 folds and 21.5 M tokens folded inside subagents (≈ 30 % of
    // all folding). Credited here instead, for the re-reads a subagent's tool
    // result gets ([`SUBAGENT_EVENT_REREADS`], the same count its injections
    // are charged). The
    // marker (`sidechain-<key>`, ≥ 1 while a subagent runs) cannot tell a
    // parent fold made while a background subagent runs: it is then credited
    // as a subagent's, the lower of the two.
    if crate::session::sidechain_generation(safe) > 0 {
        record_estimated(
            "hookwrap",
            "compounding",
            PROXY_LABEL,
            "compounding",
            Savings::new(saved.saturating_mul(SUBAGENT_EVENT_REREADS as usize), 0),
        );
        return;
    }
    let dir = crate::session::sessions_dir();
    let turn = fs::read_to_string(dir.join(format!("{safe}-turn")))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let _ = fs::create_dir_all(&dir);
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{safe}-compactions.jsonl")))
    {
        // Single-write append: see debug_log.rs on cross-process interleaving.
        let ts = crate::platform::now_unix_secs();
        let _ =
            f.write_all(format!("{{\"turn\":{turn},\"delta\":{saved},\"ts\":{ts}}}\n").as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_the_agent_would_have_held_is_banked() {
        let page = Savings::new(435_000, 14_000);
        assert_eq!(
            banked_delta("cli", "html-to-markdown", None, "claude-code", &page),
            0,
            "modelled web credit"
        );
        let mut shell = Savings::new(50_000, 1_000);
        shell.deliverable_tokens = Some(8_000);
        assert_eq!(
            banked_delta("wrap", "auto:json", None, "claude-code", &shell),
            7_000
        );
        assert_eq!(
            banked_delta(
                "cli",
                "impact_of",
                Some("ci_heuristic"),
                "claude-code",
                &shell
            ),
            0
        );
        assert_eq!(
            banked_delta("llm-proxy", "llm-proxy:fold", None, "codex", &shell),
            0
        );
        let read = Savings::new(3_000, 500);
        assert_eq!(
            banked_delta("hook", "read_auto_compact", None, "claude-code", &read),
            2_500
        );
    }
}
