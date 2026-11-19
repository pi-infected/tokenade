//! Cold-`/resume` transcript compaction.
//!
//! Claude Code stores each session as a JSONL transcript
//! (`~/.claude/projects/<slug>/<uuid>.jsonl`). Resuming a session re-sends its
//! whole history to the API, so large stale `tool_result` payloads (a 300 KB
//! Bash dump, a pasted file) are re-billed on every cold resume. We mask the
//! biggest ones IN PLACE — replacing the text with a short placeholder plus a
//! recoverable stash pointer, preserving the surrounding JSON structure — so a
//! resumed session rebuilds far cheaper.
//!
//! Safety stance: only `tool_result` TEXT blocks over a token threshold are
//! touched; images / base64 are left untouched; already-masked blocks are
//! idempotent (no re-mask, no double-stash); lines with nothing to mask are
//! copied VERBATIM (their exact bytes), so the blast radius is only the lines
//! we actually shrink. The original text is always recoverable from the stash.

/// Minimum token estimate for a `tool_result` text block to be worth masking.
/// Below this the placeholder + stash overhead isn't worth the churn.
pub const MASK_MIN_TOKENS: usize = 2_000;

/// Minimum NET saving that justifies masking a block a LATER turn provably
/// superseded (see [`crate::superseded`]): a file read before that file was
/// edited, or a shell command re-run verbatim.
///
/// A different rule from [`MASK_MIN_TOKENS`], not just a lower number. The
/// 2 000 bar asks "is this big enough that masking it is worth the churn?" — a
/// judgement about a block that still says something true. For a block the
/// session has already replaced, the only thing left to weigh is what masking
/// actually saves, so that is what we measure: `tokens(original) −
/// tokens(placeholder)`.
///
/// This replaced a fixed 300-token floor. The floor was a proxy for the same
/// question and it answered it wrong in both directions — refusing a 250-token
/// stale read whose placeholder costs ~25 tokens (a 90 % saving, declined),
/// while any lower value would have been just as arbitrary. 40 tokens of net
/// gain is the point where an `expand-ref` round trip is worth risking.
pub const MASK_SUPERSEDED_MIN_NET_GAIN: usize = 40;

/// Marker prefix of a masked block — also the idempotency sentinel.
pub const MARKER: &str = "[tokenade:elided";

/// Token accounting for one compacted transcript. `tokens_before/after` are
/// the summed estimates of only the blocks that changed, so
/// `before - after` is the raw token delta the resume avoids re-sending.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CompactStats {
    pub blocks_masked: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
}

impl CompactStats {
    pub fn saved(&self) -> usize {
        self.tokens_before.saturating_sub(self.tokens_after)
    }
}

/// Mask oversized `tool_result` text blocks in ONE transcript line. Returns
/// the rewritten line plus `(before, after)` token estimates of the changed
/// blocks, or `None` when the line is unchanged (caller copies it verbatim).
///
/// `stash` receives the original text and returns a recovery-pointer string
/// (e.g. `"tokenade expand-ref <hash>"`) embedded in the placeholder. Pure
/// apart from the injected `stash` closure, so it is testable without disk.
pub fn compact_line(
    line: &str,
    stash: &mut impl FnMut(&str) -> String,
) -> Option<(String, usize, usize)> {
    compact_line_gated(line, stash, Gate::Size(MASK_MIN_TOKENS))
}

/// What has to be true before a block is worth masking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Mask anything at or above this token count. The generic rule: we know
    /// the block is heavy, we do not know that it is dead.
    Size(usize),
    /// Mask when the masking PAYS — `tokens(original) - tokens(placeholder)`
    /// at or above this many tokens.
    ///
    /// The right question for a block the session itself has already replaced.
    /// A fixed floor is a proxy for "is the placeholder overhead worth it?",
    /// and a bad one: it refused a 250-token stale read whose placeholder costs
    /// ~25, while a floor low enough to catch it would have been arbitrary in
    /// the other direction. Pricing the actual trade removes the guess.
    NetGain(usize),
}

impl Gate {
    /// Smallest token count that could possibly pass, for the cheap pre-filter.
    fn min_tokens(self) -> usize {
        match self {
            Gate::Size(n) => n,
            // Placeholder cost is ~25 tokens whatever the block; being a few
            // tokens low here only means we parse a line we then reject.
            Gate::NetGain(n) => n,
        }
    }

    /// Does a block of `before` tokens clear this gate?
    fn admits(self, before: usize) -> bool {
        match self {
            Gate::Size(n) => before >= n,
            Gate::NetGain(n) => before.saturating_sub(placeholder_cost(before)) >= n,
        }
    }
}

/// [`compact_line`] with the gate supplied by the caller, so a line whose result
/// a later turn superseded can be judged on what masking actually SAVES rather
/// than on the generic size bar.
pub fn compact_line_gated(
    line: &str,
    stash: &mut impl FnMut(&str) -> String,
    gate: Gate,
) -> Option<(String, usize, usize)> {
    compact_line_gated_keeping_head(line, stash, gate, false)
}

/// [`compact_line_gated`], with the option to keep the block's opening.
///
/// `keep_head` is for a result nothing can produce again — a delegated
/// report. See [`crate::superseded::delegated_report_ids`].
pub fn compact_line_gated_keeping_head(
    line: &str,
    stash: &mut impl FnMut(&str) -> String,
    gate: Gate,
    keep_head: bool,
) -> Option<(String, usize, usize)> {
    // Cheap pre-filter: only a user `tool_result` line big enough to hold a
    // maskable block is worth parsing. ~3 bytes/token is a conservative floor.
    if line.len() < gate.min_tokens() * 3 || !line.contains("\"tool_result\"") {
        return None;
    }
    let mut v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("user") {
        return None;
    }
    let content = v.pointer_mut("/message/content")?.as_array_mut()?;
    let mut before = 0usize;
    let mut after = 0usize;
    let mut masked = 0usize;
    for block in content.iter_mut() {
        if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
            continue;
        }
        // A tool_result `content` is either a bare string or a list of typed
        // parts (`{type:"text",text}`, `{type:"image",...}`). Mask text only.
        match block.get_mut("content") {
            Some(serde_json::Value::String(s)) => {
                if let Some((ns, b, a)) = mask_result_text(s, stash, gate, keep_head) {
                    *s = ns;
                    before += b;
                    after += a;
                    masked += 1;
                }
            }
            Some(serde_json::Value::Array(items)) => {
                for it in items.iter_mut() {
                    if it.get("type").and_then(|t| t.as_str()) != Some("text") {
                        continue; // leave images / other parts alone
                    }
                    if let Some(serde_json::Value::String(s)) = it.get_mut("text") {
                        if let Some((ns, b, a)) = mask_result_text(s, stash, gate, keep_head) {
                            *s = ns;
                            before += b;
                            after += a;
                            masked += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if masked == 0 {
        return None;
    }
    let out = serde_json::to_string(&v).ok()?;
    Some((out, before, after))
}

/// Mask the INPUT of a tool call the session has already superseded.
///
/// A `tool_use` block carries whatever the agent typed to make the call, and
/// for the calls that cost real bytes — an inline script, a long patch, a
/// generated query — that input is the same order of magnitude as the result
/// it produced. On a measured 6 MB transcript the `tool_use` lines held
/// 1.16 MB against 1.84 MB for every `tool_result` combined, and none of it
/// was ever a candidate for masking.
///
/// Only DEAD ids qualify: if the call still explains something live, its text
/// stays. And the net-gain gate does the rest of the work — masking
/// `{"command":"cargo test"}` would cost more than it saves, so it doesn't
/// happen. What gets masked is the 4 KB script whose output is already gone.
pub fn compact_tool_use_line(
    line: &str,
    stash: &mut impl FnMut(&str) -> String,
    dead: &std::collections::HashSet<String>,
) -> Option<(String, usize, usize)> {
    if dead.is_empty() || !line.contains("\"tool_use\"") {
        return None;
    }
    let mut v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
        return None;
    }
    let gate = Gate::NetGain(MASK_SUPERSEDED_MIN_NET_GAIN);
    let content = v.pointer_mut("/message/content")?.as_array_mut()?;
    let mut before = 0usize;
    let mut after = 0usize;
    let mut masked = 0usize;
    for block in content.iter_mut() {
        if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
            continue;
        }
        // The id is what ties this call to a result the session has replaced.
        // No id, or a live one, and the call keeps every byte.
        match block.get("id").and_then(|i| i.as_str()) {
            Some(id) if dead.contains(id) => {}
            _ => continue,
        }
        let Some(serde_json::Value::Object(input)) = block.get_mut("input") else {
            continue;
        };
        // The HEAD of every value stays — see `mask_tool_use_value`. Keeping
        // only the JSON keys was not enough: `{"command":"[tokenade:elided …]"}`
        // is a readable key in front of a call nobody can identify.
        for value in input.values_mut() {
            if let serde_json::Value::String(s) = value {
                if let Some((ns, b, a)) = mask_tool_use_value(s, stash, gate) {
                    *s = ns;
                    before += b;
                    after += a;
                    masked += 1;
                }
            }
        }
    }
    if masked == 0 {
        return None;
    }
    Some((serde_json::to_string(&v).ok()?, before, after))
}

/// The placeholder that replaces a masked block.
fn placeholder_for(before: usize, ptr: &str) -> String {
    format!("{MARKER} {before} tokens on cold resume — recover: {ptr}]")
}

/// What the placeholder will cost, priced BEFORE we pay for a stash write.
///
/// The stash pointer is `tokenade expand-ref ` plus a fixed-width hash, so a
/// dummy of the right shape prices the real thing exactly. Gating first means
/// a block we are going to refuse never touches the disk.
fn placeholder_cost(before: usize) -> usize {
    crate::tokenizer::estimate(&placeholder_for(before, "tokenade expand-ref 0123456789ab"))
}

/// How much of a `tool_use` input value survives masking.
///
/// A dead call's RESULT is already gone, so the only thing the block still
/// carries is WHAT WAS ATTEMPTED — and that is exactly what a resumed session
/// needs in order not to redo it. Replacing the whole value with a pointer
/// turned "I already ran `cargo test -p x`" into "I already ran [something]".
/// MEASURED 2026-09-05 across this machine's 805 transcripts: 540 masked
/// values landed in `input.command`, at 90, 108, 133 tokens — commands, not
/// the "4 KB script whose output is already gone" this masker was written for,
/// and several of them re-encountered by the agent more than once.
///
/// So the head stays and the bulk goes: `cd repo && python3 - <<'PY'` still
/// reads as itself, the 4 KB heredoc after it still leaves. The gate then
/// settles the rest — with 180 characters retained, a short command cannot
/// clear the net-gain bar at all and is never touched.
const TOOL_USE_HEAD_CHARS: usize = 180;

/// How much of a DELEGATED report survives masking.
///
/// Wider than a tool call's head because what has to survive is different: a
/// call is identified by its first line, a report is read for its conclusion,
/// which takes a sentence or two. On the field cases (2 173–2 676 tokens) 400
/// characters is about 5% of the block, so the fold still pays.
const REPORT_HEAD_CHARS: usize = 400;

/// Cut `s` to at most `max` characters, on a char boundary, preferring the end
/// of a line inside the window. Shared by the tool-call head and the report
/// head: two cutters would drift, and the boundary rule is the delicate part.
fn head_of(s: &str, max: usize) -> &str {
    let mut cut = max.min(s.len());
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    if let Some(nl) = s[..cut].rfind('\n') {
        if nl > 0 {
            cut = nl;
        }
    }
    &s[..cut]
}

/// Mask a `tool_use` input value, keeping its first [`TOOL_USE_HEAD_CHARS`]
/// characters so the call is still legible.
fn mask_tool_use_value(
    s: &str,
    stash: &mut impl FnMut(&str) -> String,
    gate: Gate,
) -> Option<(String, usize, usize)> {
    if s.contains(MARKER) {
        return None; // idempotent — never re-mask or re-stash
    }
    let before = crate::tokenizer::estimate(s);
    if !gate.admits(before) {
        return None;
    }
    // Cut on a char boundary, and prefer the end of the first line when it
    // falls inside the window: a command's first line is its identity.
    let mut cut = TOOL_USE_HEAD_CHARS.min(s.len());
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    if let Some(nl) = s[..cut].find('\n') {
        cut = nl;
    }
    let head = &s[..cut];
    if head.trim().is_empty() {
        return None; // nothing legible to keep — leave the call alone
    }
    let ptr = stash(s);
    let tail_tokens = crate::tokenizer::estimate(&s[cut..]);
    let masked = format!("{head}\n{}", placeholder_for(tail_tokens, &ptr));
    let after = crate::tokenizer::estimate(&masked);
    // Never let masking GROW the value.
    if after >= before {
        return None;
    }
    Some((masked, before, after))
}

/// Mask one text string if it clears `gate` and is not already masked.
/// Mask a `tool_result` text block, optionally keeping its opening.
///
/// `keep_head` marks a result nothing can produce again — a delegated report.
/// Everything else a transcript holds names something recoverable: a file is
/// on disk, a command can be re-run, a page re-fetched. A subagent's report is
/// the distilled output of a run that will not happen the same way twice, so
/// replacing it with a bare pointer removes the deliverable and leaves the
/// reader with a hash where a conclusion was.
fn mask_result_text(
    s: &str,
    stash: &mut impl FnMut(&str) -> String,
    gate: Gate,
    keep_head: bool,
) -> Option<(String, usize, usize)> {
    if !keep_head {
        return mask_text(s, stash, gate);
    }
    if s.starts_with(MARKER) || s.contains(MARKER) {
        return None; // idempotent — never re-mask or re-stash
    }
    let before = crate::tokenizer::estimate(s);
    if !gate.admits(before) {
        return None;
    }
    let head = head_of(s, REPORT_HEAD_CHARS);
    if head.trim().is_empty() {
        return mask_text(s, stash, gate); // nothing legible to keep
    }
    let ptr = stash(s);
    let placeholder = format!("{head}\n{}", placeholder_for(before, &ptr));
    let after = crate::tokenizer::estimate(&placeholder);
    // Never let masking GROW a block, head or no head.
    if after >= before {
        return None;
    }
    Some((placeholder, before, after))
}

fn mask_text(
    s: &str,
    stash: &mut impl FnMut(&str) -> String,
    gate: Gate,
) -> Option<(String, usize, usize)> {
    if s.starts_with(MARKER) {
        return None; // idempotent — never re-mask or re-stash
    }
    let before = crate::tokenizer::estimate(s);
    if !gate.admits(before) {
        return None;
    }
    let ptr = stash(s);
    let placeholder = placeholder_for(before, &ptr);
    let after = crate::tokenizer::estimate(&placeholder);
    // Never let masking GROW a block (pathological tiny threshold / long ptr).
    if after >= before {
        return None;
    }
    Some((placeholder, before, after))
}

/// Compact a whole transcript in place ([`crate::line_patch`]): mask oversized
/// `tool_result` text blocks, stashing each original so it stays recoverable.
/// **Every line keeps its byte length** (JSON-whitespace padding; a line the mask
/// would lengthen is kept): readers follow it by byte offset (lost compactions, 2026-10-15).
/// Returns the token accounting; a no-op file returns a zero [`CompactStats`].
pub fn compact_transcript(path: &std::path::Path) -> std::io::Result<CompactStats> {
    let content = std::fs::read_to_string(path)?;
    // One extra pass over the parsed lines to learn which results the session
    // itself has already replaced — a file read before that file was edited, a
    // command re-run verbatim. Those get the lower floor below: they are not
    // merely heavy, they are answers to questions the session has since
    // re-asked. Cheap enough to be unconditional, and it fails safe: an
    // unparseable transcript yields an empty set and the generic rule alone.
    let parsed_lines = crate::superseded::parse_lines(&content);
    let dead = crate::superseded::superseded_tool_use_ids(&parsed_lines);
    // A delegated report is the one result nothing can produce again, so it
    // keeps its opening when the bulk goes. Same pass, same cost.
    let delegated = crate::superseded::delegated_report_ids(&parsed_lines);
    let mut patches = Vec::new();
    let mut gains = Vec::new();
    let mut offset = 0u64;
    for line in content.split_inclusive('\n') {
        let at = offset;
        offset += line.len() as u64;
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if trimmed.is_empty() {
            continue;
        }
        // `stash_sole_copy`, not `stash_for_reference`: masking REPLACES the
        // bytes in the transcript, so this stash entry is the last copy of them.
        // A TTL sweep taking it would leave a pointer to nothing.
        let mut stash = |text: &str| match crate::disclose::stash_sole_copy(
            text,
            &crate::disclose::SourceHint::web(),
        ) {
            Some((hash, _)) => format!("tokenade expand-ref {hash}"),
            None => "(original not stashed)".to_string(),
        };
        // A line is judged as superseded only when EVERY result it carries is —
        // one line can hold several, and the gate applies to the whole line.
        let gate = superseded_line_gate(trimmed, &dead).unwrap_or(Gate::Size(MASK_MIN_TOKENS));
        let keep_head = !delegated.is_empty() && {
            let ids = serde_json::from_str::<serde_json::Value>(trimmed)
                .map(|v| crate::superseded::result_ids_in_line(&v))
                .unwrap_or_default();
            !ids.is_empty() && ids.iter().all(|id| delegated.contains(id))
        };
        let folded = compact_line_gated_keeping_head(trimmed, &mut stash, gate, keep_head)
            .or_else(|| compact_tool_use_line(trimmed, &mut stash, &dead));
        if let Some((new_line, b, a)) = folded.filter(|(n, _, _)| n.len() <= trimmed.len()) {
            let mut new = new_line.into_bytes();
            new.resize(trimmed.len(), b' ');
            patches.push(crate::line_patch::Patch {
                offset: at,
                old: trimmed.into(),
                new,
            });
            gains.push((b, a));
        }
    }
    // In place, never temp + rename: the session may append while this runs,
    // and a rename would drop those lines (#12). A line changed since the read
    // keeps its new bytes; only the folds that landed are counted.
    let mut stats = CompactStats::default();
    for (landed, (b, a)) in crate::line_patch::apply(path, &patches)?
        .into_iter()
        .zip(gains)
    {
        if landed {
            stats.blocks_masked += 1;
            stats.tokens_before += b;
            stats.tokens_after += a;
        }
    }
    Ok(stats)
}

/// The net-gain gate when every `tool_result` on this line was superseded,
/// `None` otherwise.
///
/// All-or-nothing on purpose: the gate is a property of the LINE (one call
/// masks whatever blocks it holds), so a line mixing a superseded result with a
/// live one keeps the conservative bar. Mixed lines are rare; getting them wrong
/// would mask something the agent still needs at a far lower bar.
fn superseded_line_gate(line: &str, dead: &std::collections::HashSet<String>) -> Option<Gate> {
    if dead.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let ids = crate::superseded::result_ids_in_line(&v);
    (!ids.is_empty() && ids.iter().all(|id| dead.contains(id)))
        .then_some(Gate::NetGain(MASK_SUPERSEDED_MIN_NET_GAIN))
}

/// The SHORT Anthropic prompt-cache TTL — the floor of the trim gate, not
/// the assumed ceiling. The API offers exactly two TTLs — 5 min (default)
/// and 1 h (extended, per-block `cache_control:{ttl:"1h"}`) — both *sliding*
/// windows (each cache hit refreshes them). The 2026-07-11 assumption that
/// Claude Code never sends the extended TTL was FALSIFIED by the 2026-07-12
/// transcript audit: `usage.cache_creation.ephemeral_1h_input_tokens`
/// dominates every Claude Code session since at least 2026-05-29 (headless
/// `-p` runs included, verified live). Trimming a transcript whose 1 h cache
/// is still warm DESTROYS a guaranteed cheap resume: a plain `--resume`
/// rebuilds a byte-identical prefix (verified live 2026-07-12 — full
/// cache_read hit across processes), so the untrimmed resume costs 0.1×
/// while the trimmed one re-writes at full price. The trim gate is therefore
/// evidence-based per transcript: see [`idle_beyond_cache_window`].
/// `TOKENADE_MAX_CACHE_TTL_SECS` still force-overrides both TTLs.
pub const ANTHROPIC_MAX_CACHE_TTL_SECS: u64 = 300;

/// The EXTENDED Anthropic prompt-cache TTL (1 h), the conservative ceiling
/// used when a transcript's tail carries 1 h-TTL evidence or no evidence at
/// all.
pub const ANTHROPIC_EXTENDED_CACHE_TTL_SECS: u64 = 3_600;

/// Safety slack added on top of the cache TTL before a transcript is
/// considered trim-safe. Covers clock skew and the window where a long
/// streaming response hasn't reached the transcript yet (mtime is a lower
/// bound on the last cache touch, refreshed at every appended event). One
/// full TTL of slack: trimming too early is a bounded one-time loss (the
/// next resume re-writes the trimmed context at full price instead of
/// reading the warm cache), trimming late forfeits the saving — 2×TTL keeps
/// the guarantee while landing ~10 min after idle instead of 2 h.
pub const TRIM_SAFETY_MARGIN_SECS: u64 = 300;

/// Idle time (since last transcript activity) after which the session's prompt
/// cache is provably expired and the transcript is safe to trim: the max cache
/// TTL plus a safety margin. `TOKENADE_MAX_CACHE_TTL_SECS` overrides the TTL
/// term (settings hook for a raised/known ceiling); the margin is always added.
/// Because a transcript is written AFTER each turn (after the request that
/// refreshed the cache), its mtime never predates the last cache touch, so an
/// mtime idle beyond this bound guarantees the cache window has closed.
pub fn trim_idle_threshold_secs() -> u64 {
    let ttl = std::env::var("TOKENADE_MAX_CACHE_TTL_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(ANTHROPIC_MAX_CACHE_TTL_SECS);
    ttl.saturating_add(TRIM_SAFETY_MARGIN_SECS)
}

/// Detect the prompt-cache TTL a transcript's requests actually use, from
/// the LAST assistant usage record in its tail: a positive
/// `ephemeral_1h_input_tokens` after the last positive
/// `ephemeral_5m_input_tokens` → 1 h; the converse → 5 min; no usage split
/// (pre-2026 transcript, non-Anthropic backend) → `None`. Bounded tail read
/// (128 KiB), never errors.
pub fn detect_cache_ttl_secs(path: &std::path::Path) -> Option<u64> {
    use std::io::{Read, Seek};
    const TAIL: u64 = 128 * 1024;
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL);
    f.seek(std::io::SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf).ok()?;
    let s = String::from_utf8_lossy(&buf);
    // Position of the last occurrence of `key` whose numeric value is > 0.
    let last_positive = |key: &str| -> Option<usize> {
        let mut best = None;
        let mut from = 0;
        while let Some(i) = s[from..].find(key) {
            let at = from + i;
            let digits: String = s[at + key.len()..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if digits.parse::<u64>().map(|v| v > 0).unwrap_or(false) {
                best = Some(at);
            }
            from = at + key.len();
        }
        best
    };
    let p1h = last_positive("\"ephemeral_1h_input_tokens\":");
    let p5m = last_positive("\"ephemeral_5m_input_tokens\":");
    match (p1h, p5m) {
        (Some(a), Some(b)) => Some(if a > b {
            ANTHROPIC_EXTENDED_CACHE_TTL_SECS
        } else {
            ANTHROPIC_MAX_CACHE_TTL_SECS
        }),
        (Some(_), None) => Some(ANTHROPIC_EXTENDED_CACHE_TTL_SECS),
        (None, Some(_)) => Some(ANTHROPIC_MAX_CACHE_TTL_SECS),
        (None, None) => None,
    }
}

/// Evidence-based trim gate (2026-07-12): true iff `path` has been idle
/// beyond the cache window its OWN tail proves it uses. Decision ladder:
/// 1. `TOKENADE_MAX_CACHE_TTL_SECS` set → trust it outright (+ margin).
/// 2. Idle beyond the 1 h ceiling (+ margin) → safe under either TTL.
/// 3. Inside the ambiguous band: safe only with explicit 5 min-TTL evidence
///    AND idle beyond 5 min (+ margin). 1 h evidence or NO evidence → wait
///    for the ceiling (never sacrifice a possibly-warm cache).
pub fn idle_beyond_cache_window(path: &std::path::Path) -> bool {
    if let Some(ttl) = std::env::var("TOKENADE_MAX_CACHE_TTL_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&v| v > 0)
    {
        return is_idle_beyond(path, ttl.saturating_add(TRIM_SAFETY_MARGIN_SECS));
    }
    if is_idle_beyond(
        path,
        ANTHROPIC_EXTENDED_CACHE_TTL_SECS + TRIM_SAFETY_MARGIN_SECS,
    ) {
        return true;
    }
    match detect_cache_ttl_secs(path) {
        Some(ttl) if ttl <= ANTHROPIC_MAX_CACHE_TTL_SECS => {
            is_idle_beyond(path, ttl + TRIM_SAFETY_MARGIN_SECS)
        }
        _ => false,
    }
}

/// True iff `path` has been idle (unmodified) for longer than `idle_secs` — the
/// provable-cache-dead test. Unknown/zero mtime → `false` (treat as possibly
/// live: never trim on missing evidence). Used to gate trimming the ACTIVE
/// transcript on resume, where mtime is the last-activity-before-resume stamp.
pub fn is_idle_beyond(path: &std::path::Path, idle_secs: u64) -> bool {
    let now = std::time::SystemTime::now();
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|mt| now.duration_since(mt).ok())
        .map(|age| age.as_secs() > idle_secs)
        .unwrap_or(false)
}

/// Back-compat default idle threshold (max cache TTL + margin). Prefer
/// [`trim_idle_threshold_secs`] at call sites so a `TOKENADE_MAX_CACHE_TTL_SECS`
/// override is honored; this const is the compile-time equivalent for the
/// default (no override) and matches the historical 2 h freshness gate.
pub const TRIM_FRESHNESS_SECS: u64 = ANTHROPIC_MAX_CACHE_TTL_SECS + TRIM_SAFETY_MARGIN_SECS;

/// Compact every `*.jsonl` transcript in `dir` EXCEPT the active session file
/// and any modified within `freshness_secs`. This is the auto trigger: it
/// shrinks a project's stale, resumable transcripts without ever touching the
/// live one. Records NO gain — the saving is credited only when a session is
/// actually resumed (see [`scan_masked`]). Returns the aggregate stats.
pub fn compact_stale_transcripts(
    dir: &std::path::Path,
    active: &std::path::Path,
    freshness_secs: u64,
) -> std::io::Result<CompactStats> {
    let mut total = CompactStats::default();
    let now = std::time::SystemTime::now();
    let active_name = active.file_name();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(total);
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        if p.file_name() == active_name {
            continue; // never the live session
        }
        let fresh = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|mt| now.duration_since(mt).ok())
            .map(|age| age.as_secs() < freshness_secs)
            .unwrap_or(true); // unknown mtime → treat as fresh, skip
        if fresh {
            continue;
        }
        // Past the fast mtime pre-filter, the binding gate is the
        // evidence-based cache window: a transcript idle 10 min whose tail
        // shows 1 h-TTL writes still has a WARM cache — trimming it would
        // turn a guaranteed 0.1× resume read into a full-price rewrite.
        if !idle_beyond_cache_window(&p) {
            continue;
        }
        if let Ok(st) = compact_transcript(&p) {
            total.blocks_masked += st.blocks_masked;
            total.tokens_before += st.tokens_before;
            total.tokens_after += st.tokens_after;
        }
    }
    Ok(total)
}

/// Sum the savings a prior trim pass baked into a transcript — the token
/// count each masked placeholder still avoids re-sending. Used to credit the
/// REAL resume once it happens (`before` = elided tokens, `after` = the tiny
/// placeholders). Read-only.
pub fn scan_masked(transcript: &str) -> CompactStats {
    let mut st = CompactStats::default();
    for line in transcript.lines() {
        if !line.contains(MARKER) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) else {
            continue;
        };
        for block in content {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                continue;
            }
            match block.get("content") {
                Some(serde_json::Value::String(s)) => accum_masked(s, &mut st),
                Some(serde_json::Value::Array(items)) => {
                    for it in items {
                        if it.get("type").and_then(|t| t.as_str()) == Some("text") {
                            if let Some(s) = it.get("text").and_then(|x| x.as_str()) {
                                accum_masked(s, &mut st);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    st
}

/// Sum the tokens in oversized `tool_result` text blocks that tokenade WOULD
/// fold — used by the pre-install savings scanner to estimate, from a
/// prospect's existing transcripts, how much context tokenade could have
/// compacted. Counts only not-yet-masked blocks over [`MASK_MIN_TOKENS`], so
/// it's a conservative single-pass figure (the real saving compounds as the
/// same blocks are re-sent on every later turn). Read-only.
pub fn foldable_tokens(transcript: &str) -> usize {
    let mut total = 0usize;
    for line in transcript.lines() {
        if line.len() < MASK_MIN_TOKENS * 3 || !line.contains("\"tool_result\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("user") {
            continue;
        }
        let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) else {
            continue;
        };
        for block in content {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                continue;
            }
            match block.get("content") {
                Some(serde_json::Value::String(s)) => total += foldable_of(s),
                Some(serde_json::Value::Array(items)) => {
                    for it in items {
                        if it.get("type").and_then(|t| t.as_str()) == Some("text") {
                            if let Some(s) = it.get("text").and_then(|x| x.as_str()) {
                                total += foldable_of(s);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    total
}

/// Tokens of `s` if it's an unmasked block over threshold, else 0.
fn foldable_of(s: &str) -> usize {
    if s.starts_with(MARKER) {
        return 0;
    }
    let t = crate::tokenizer::estimate(s);
    if t >= MASK_MIN_TOKENS {
        t
    } else {
        0
    }
}

/// Add one masked block's encoded token counts to `st` if `s` is a placeholder.
fn accum_masked(s: &str, st: &mut CompactStats) {
    let Some(rest) = s.strip_prefix(MARKER) else {
        return;
    };
    // placeholder shape: "[tokenade:elided <N> tokens on cold resume — recover: …]"
    let Some(before) = rest
        .split_whitespace()
        .next()
        .and_then(|w| w.parse::<usize>().ok())
    else {
        return;
    };
    st.blocks_masked += 1;
    st.tokens_before += before;
    st.tokens_after += crate::tokenizer::estimate(s);
}
#[cfg(test)]
include!("session_compact_tests.rs");
