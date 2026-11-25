//! One API response must be counted once, however many times the agent wrote
//! it to disk.
//!
//! Claude Code writes a single assistant response as SEVERAL transcript
//! entries — text, more text, tool_use — and stamps the SAME `message.usage`
//! object on every one of them. Summing per entry counts one billed response
//! two, three, four times.
//!
//! Measured 2026-07-31 over 129 real transcripts in `~/.claude/projects`:
//! 5560 of 18143 assistant entries (31%) repeated an identical
//! `(message.id, message.usage)` pair, and naive summation reported
//! 1_852_166_819 tokens where the deduplicated figure is 609_869_980 —
//! **+203.7%**. Every view built on those numbers was affected: the projects
//! table, the per-day model panel, the cost column, and the aggregates the
//! client reports upstream.
//!
//! The key is `(scope, id, usage-fingerprint)`, and the fingerprint is
//! load-bearing in BOTH directions:
//!   - Same id AND identical usage → the same billed response, written twice.
//!     Drop it.
//!   - Same id but DIFFERENT usage → an agent that rewrites a running total
//!     under one id as the response streams (the cumulative shape Codex uses).
//!     Those must all pass through, because the caller converts them to deltas
//!     itself; deduping on the id alone would silently delete the increments.
//!
//! Records with no id are always admitted. There is no honest way to tell a
//! duplicate from two identical-looking responses without one, and inventing a
//! key out of a timestamp would quietly delete real usage — the opposite
//! failure, and the harder one to notice.
//!
//! ## Why not dedupe on the id alone (the obvious simplification)
//!
//! Re-measured 2026-07-31 over the same 131 transcripts, comparing every policy
//! on the same input (reference = the exact figure, per-field max per id):
//!
//! | policy                                   | total tokens   | error     |
//! |------------------------------------------|----------------|-----------|
//! | sum every entry (the bug)                | 34_709_457_109 | +118.29 % |
//! | **this module** — `(id, usage)` first    | 15_902_674_605 | **+0.011 %** |
//! | id alone, first entry wins               | 15_900_909_924 |  −0.000 2 % |
//! | id alone, LAST entry wins                | 15_895_977_671 |  −0.031 %  |
//! | per-field max per id (reference)         | 15_900_946_535 |   —        |
//!
//! Same id with a DIFFERENT usage payload is rare but real: 50 of 59_551 repeat
//! pairs, in exactly two shapes.
//!
//!   * a streaming placeholder followed by the final figure
//!     (`output_tokens: 5` then `3557`) — summing both overshoots by the
//!     placeholder, which is where the +0.011 % comes from;
//!   * the final figure followed by a ZEROED copy (`4505` then `0`, the nested
//!     `cache_creation` block still intact) — summing both is exact, because the
//!     duplicate contributes nothing.
//!
//! That second shape is why last-entry-wins must not be adopted here: it keeps
//! the zeroed copy and throws the whole response away. It is the policy the
//! obvious reading of the bug suggests, and it is measurably worse than what
//! this module does.
//!
//! ## History is not rewritten (owner decision, 2026-07-31)
//!
//! Every local view re-derives its figures from the transcripts on each render,
//! so applying this guard fixed the PAST as well as the future: the projects
//! table, the per-day model panel and the cost column are correct for old
//! sessions the moment the deduplicated build runs.
//!
//! What is NOT corrected is what already left the machine — reports POSTed
//! upstream before the fix carry the inflated numbers, and they stay as they
//! are. Correcting them would mean re-deriving and re-sending history we can no
//! longer audit line by line, and a silent restatement of past figures is worse
//! than a known break in the series.

use std::collections::HashSet;

/// Tracks which `(scope, id, usage)` triples have already been counted.
///
/// Scope should be whatever bounds the id's uniqueness — usually the transcript
/// file path. Two different sessions may legitimately reuse an id; scoping
/// keeps one session's response from masking another's. An Anthropic `msg_…`
/// id is globally unique and ignores the scope (see `fingerprint_canonical`).
#[derive(Default)]
pub struct UsageDedup {
    seen: HashSet<u64>,
}

impl UsageDedup {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when this record's usage should be counted.
    ///
    /// `id` is the agent's own per-response identifier (`message.id` for Claude
    /// Code). `None` means the format carries none — always counted.
    pub fn admit(&mut self, scope: &str, id: Option<&str>, usage: &serde_json::Value) -> bool {
        let Some(id) = id.filter(|s| !s.is_empty()) else {
            return true;
        };
        self.seen.insert(fingerprint(scope, id, usage))
    }

    /// Same decision as [`Self::admit`], taking a fingerprint computed
    /// earlier by [`fingerprint_for`].
    ///
    /// The transcript cache stores that one `u64` instead of the message id
    /// and the canonical usage string it was made from — those two strings
    /// were the bulk of the cached bytes. `None` means the record carried no
    /// id, which `admit` has always treated as "cannot be a duplicate".
    pub fn admit_key(&mut self, key: Option<u64>) -> bool {
        match key {
            None => true,
            Some(k) => self.seen.insert(k),
        }
    }

    /// How many duplicate records have been rejected so far — for the debug log
    /// and for tests that need to prove the guard actually fired.
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

/// Stable 64-bit digest of the triple. `DefaultHasher` is SipHash with a FIXED
/// key in Rust, so this is reproducible across runs — a randomly seeded hash
/// here would make dedup itself non-deterministic, and a compactor that wobbles
/// between runs is a bug this repo already has a test for
/// (`tests/determinism.rs`).
fn fingerprint(scope: &str, id: &str, usage: &serde_json::Value) -> u64 {
    fingerprint_canonical(scope, id, &canonical_usage(usage))
}

/// The key [`UsageDedup::admit_key`] consumes, computed where the record is
/// still available. `None` when the record has no usable id — the same case
/// `admit` short-circuits as always-admit.
pub fn fingerprint_for(scope: &str, id: Option<&str>, usage: &serde_json::Value) -> Option<u64> {
    let id = id.filter(|s| !s.is_empty())?;
    Some(fingerprint_canonical(scope, id, &canonical_usage(usage)))
}

/// The hasher itself, over the canonical form. Both entry points end here so
/// the Value path and the cached-string path cannot drift.
fn fingerprint_canonical(scope: &str, id: &str, canonical: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // An Anthropic response id (`msg_…`) is unique across the whole API, so
    // the file is no part of its identity: Claude Code COPIES earlier
    // responses into a resumed or forked session's transcript, and scoping by
    // file counted them once per copy. Measured 2026-10-03 over this machine's
    // 1 578 transcripts: 58 files carry copies, up to +144 % of a session's
    // own usage (ccusage #1765 found the same). Other agents keep their scope:
    // their ids are not promised unique across sessions.
    if !id.starts_with("msg_") {
        scope.hash(&mut h);
    }
    id.hash(&mut h);
    // Serialising the usage object canonicalises key order (serde_json
    // preserves insertion order without the `preserve_order` feature, and the
    // agents that write these files emit a stable field order), so identical
    // usage hashes identically.
    canonical.hash(&mut h);
    h.finish()
}

/// Order-independent rendering of a usage object, so two records that differ
/// only in key order are still recognised as the same response.
pub fn canonical_usage(usage: &serde_json::Value) -> String {
    match usage {
        serde_json::Value::Object(map) => {
            let mut parts: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{k}={}", canonical_usage(v)))
                .collect();
            parts.sort();
            format!("{{{}}}", parts.join(","))
        }
        serde_json::Value::Array(a) => {
            let parts: Vec<String> = a.iter().map(canonical_usage).collect();
            format!("[{}]", parts.join(","))
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_same_response_written_twice_is_counted_once() {
        let mut d = UsageDedup::new();
        let usage = json!({"input_tokens": 100, "output_tokens": 20});
        assert!(d.admit("sess.jsonl", Some("msg_1"), &usage));
        assert!(!d.admit("sess.jsonl", Some("msg_1"), &usage));
        assert!(!d.admit("sess.jsonl", Some("msg_1"), &usage));
    }

    #[test]
    fn a_running_total_under_one_id_still_passes() {
        // The cumulative shape: an agent rewrites the same id with growing
        // numbers. Deduping on the id alone would delete every increment.
        let mut d = UsageDedup::new();
        assert!(d.admit("s", Some("msg_1"), &json!({"input_tokens": 10})));
        assert!(d.admit("s", Some("msg_1"), &json!({"input_tokens": 25})));
        assert!(d.admit("s", Some("msg_1"), &json!({"input_tokens": 60})));
    }

    #[test]
    fn key_order_does_not_make_a_duplicate_look_new() {
        let mut d = UsageDedup::new();
        assert!(d.admit("s", Some("m"), &json!({"a": 1, "b": 2})));
        assert!(!d.admit("s", Some("m"), &json!({"b": 2, "a": 1})));
    }

    #[test]
    fn nested_cache_blocks_are_compared_too() {
        // Claude's usage nests `cache_creation` and an `iterations` array;
        // two records identical down to those must still collapse.
        let mut d = UsageDedup::new();
        let u = json!({
            "input_tokens": 4621,
            "cache_read_input_tokens": 27191,
            "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 0},
            "iterations": [{"input_tokens": 4621, "output_tokens": 228}]
        });
        assert!(d.admit("s", Some("m"), &u));
        assert!(!d.admit("s", Some("m"), &u));
    }

    #[test]
    fn different_sessions_do_not_mask_each_other() {
        // An id with no promise of global uniqueness (not an Anthropic `msg_…`).
        let mut d = UsageDedup::new();
        let usage = json!({"input_tokens": 5});
        assert!(d.admit("a.jsonl", Some("turn_1"), &usage));
        assert!(d.admit("b.jsonl", Some("turn_1"), &usage));
    }

    #[test]
    fn a_zeroed_second_copy_of_one_response_is_not_the_one_we_keep() {
        // MEASURED shape, 2026-07-31: Claude Code sometimes writes the response
        // once with its real figures and again with the top-level counters
        // zeroed (the nested `cache_creation` block survives). Both are admitted
        // and summed, which is EXACT — the zeroed copy adds nothing.
        //
        // This test exists to stop the "just keep the last entry per id"
        // simplification: it would keep the zeros and drop 4505 output tokens.
        let mut d = UsageDedup::new();
        let real = json!({
            "input_tokens": 2,
            "output_tokens": 4505,
            "cache_creation_input_tokens": 4965,
            "cache_read_input_tokens": 975_061,
            "cache_creation": {"ephemeral_1h_input_tokens": 4965, "ephemeral_5m_input_tokens": 0}
        });
        let zeroed = json!({
            "input_tokens": 0,
            "output_tokens": 0,
            "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0,
            "cache_creation": {"ephemeral_1h_input_tokens": 4965, "ephemeral_5m_input_tokens": 0}
        });
        assert!(d.admit("s", Some("msg_1"), &real));
        assert!(d.admit("s", Some("msg_1"), &zeroed));
        // …and the exact duplicate of either still collapses.
        assert!(!d.admit("s", Some("msg_1"), &real));
        assert!(!d.admit("s", Some("msg_1"), &zeroed));
    }

    #[test]
    fn a_record_without_an_id_is_always_counted() {
        // No id means no honest way to tell a duplicate from two identical
        // responses. Counting twice overstates; dropping understates and hides
        // real usage. We take the side that never deletes data.
        let mut d = UsageDedup::new();
        let usage = json!({"input_tokens": 7});
        assert!(d.admit("s", None, &usage));
        assert!(d.admit("s", None, &usage));
        assert!(d.admit("s", Some(""), &usage));
    }

    /// A response copied into another session's transcript (resume, fork) is
    /// the same billed response: counted once. An id from an agent that does
    /// not promise global uniqueness keeps its per-file scope.
    #[test]
    fn a_response_copied_into_another_session_is_counted_once() {
        let usage = json!({"input_tokens": 10, "output_tokens": 3});
        let mut d = UsageDedup::new();
        assert!(d.admit("a.jsonl", Some("msg_01ABC"), &usage));
        assert!(!d.admit("b.jsonl", Some("msg_01ABC"), &usage));
        assert!(d.admit("a.jsonl", Some("resp_7"), &usage));
        assert!(d.admit("b.jsonl", Some("resp_7"), &usage));
    }
}
