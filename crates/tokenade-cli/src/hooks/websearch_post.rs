#![allow(unused_imports)]
//! PostToolUse compactor for Claude Code's `WebSearch` tool
//! (`tokenade hookwrap-websearch-post`).
//!
//! Claude Code >= 2.1.121 lets a PostToolUse hook REPLACE the tool output
//! before the model sees it via
//! `hookSpecificOutput.updatedToolOutput` (shape-preserving: if the tool
//! defines an outputSchema the replacement must still parse, else it is
//! silently dropped — which for us is plain fail-open: the native output
//! stands). WebSearch results are the single largest web-lever we have:
//! measured web-heavy sessions show ~5.7k chars/call, dozens-to-155
//! calls/session, 82-96% of all tool-result chars.
//!
//! Claude Code renders `tool_response.results` to the model as, per item:
//!   string          → the string verbatim
//!   {content:[…]}   → `Links: ${JSON.stringify(content)}` (non-empty)
//!   {content:[]}    → `No links found.`
//! so shrinking `content` arrays and string items shrinks exactly what the
//! model pays.
//!
//! Levers, in order of impact:
//!   1. CROSS-CALL DEDUP — a URL already served by a previous WebSearch of
//!      the SAME session is dropped (searches overlap heavily); a one-line
//!      note counts the omissions. Per-session seen-set persisted next to
//!      the webfetch session cache (`~/.tokenade/sessions/<sid>-websearch
//!      .json`, same best-effort idiom — no locking, an I/O error just
//!      skips dedup on that call).
//!   2. Link slimming — content entries keep ONLY {title, url} (page_age /
//!      metadata / encrypted payload fields dropped), titles trimmed to
//!      ~120 chars.
//!   3. Prose dedup + whitespace collapse on string items.
//!
//! The ORIGINAL tool_response is stashed via the standard disclose stash so
//! `tokenade expand-ref <hash>` recovers it byte-exact; a one-line banner
//! item carries the hash. Everything is fail-open: any gate/parse problem
//! prints NOTHING and exits 0, leaving the native output untouched.
//! Bypass per-call with `TOKENADE_NO_WEBSEARCH=1` (or the broader
//! `TOKENADE_HOOK_DISABLED=1`).
use super::*;
use crate::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Title cap for link entries (chars). Real captured titles run to ~140+
/// chars of SEO suffix ("… · GitHub · Releases · tokio-rs/tokio"); 120
/// keeps them identifying without the tail.
pub(crate) const WEBSEARCH_TITLE_MAX_CHARS: usize = 120;

/// Net-savings gate: don't replace for crumbs — the banner itself costs,
/// and a replacement that saves less than this is noise. Both legs must
/// pass (absolute chars saved AND percent).
pub(crate) const WEBSEARCH_MIN_SAVED_CHARS: usize = 600;
pub(crate) const WEBSEARCH_MIN_SAVED_PCT: usize = 15;

/// Entry point for `tokenade hookwrap-websearch-post`. Reads the
/// PostToolUse JSON on stdin; on success prints the `updatedToolOutput`
/// replacement JSON; on ANY gate/parse failure prints nothing. Always
/// exits 0 (fail-open — build_hook_command's `; exit 0` tail backs this
/// up at the shell level too).
pub(crate) fn cmd_hookwrap_websearch_post() -> i32 {
    let started = std::time::Instant::now();
    if hook_disabled_by_env() {
        return 0;
    }
    let Some(input) = crate::hooks::stdin::read_event("hookwrap-websearch-post") else {
        return 0;
    };
    let Some(parsed) = crate::hooks::stdin::parse_event(&input) else {
        tokenade_core::debug_log::log(
            "hookwrap-websearch-post",
            "parse_err",
            serde_json::json!({"elapsed_ms": started.elapsed().as_millis() as u64}),
        );
        return 0;
    };
    // Session-scope the gain rows this hook books (statusline ticker) —
    // same idiom as cmd_hookwrap_pretool.
    if let Some(sid) = crate::hooks::session_id_of(&parsed) {
        let skey: String = sid
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        tokenade_core::session::set_session_key_authoritative(&skey);
    }
    // #4-style FAIL-OPEN when not activated (same gate as the webfetch
    // hook): with the recovery commands gated off, serving a compacted
    // view + an expand-ref banner the agent cannot expand is strictly
    // worse than the native output. Silent variant — the quota-message
    // emitter speaks the PreToolUse protocol, not PostToolUse.
    let Some(outcome) = websearch_post_decision(&parsed, &websearch_state_dir()) else {
        return 0;
    };
    println!("{}", outcome.json_line);
    // Gain accounting — bounded by construction: `before` is the chars the
    // model ACTUALLY pays for the native rendering (string items + the
    // `Links: […]` JSON), never a raw-bytes assiette; `after` is the same
    // measure of what we served. No multiplier, no compounding (matches
    // the 0.8.11 bounded-accounting rules; the untied web channel keeps
    // realistic == naive because the assiette is already delivery-bounded).
    let sav = tokenade_core::Savings::new(outcome.before_tokens, outcome.after_tokens);
    let label: String = outcome.query.chars().take(120).collect();
    gain_log::record("hook", "websearch_compact", &label, sav);
    tokenade_core::debug_log::log(
        "hookwrap-websearch-post",
        "websearch_compact_fired",
        serde_json::json!({
            "query": outcome.query,
            "before_chars": outcome.before_chars,
            "after_chars": outcome.after_chars,
            "before_tokens": outcome.before_tokens,
            "after_tokens": outcome.after_tokens,
            "dropped_urls": outcome.dropped,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

/// Everything the caller needs after a successful compaction decision.
pub(crate) struct WebsearchOutcome {
    /// The exact line to print on stdout (the full hookSpecificOutput
    /// envelope with `updatedToolOutput`).
    pub(crate) json_line: String,
    pub(crate) query: String,
    pub(crate) before_chars: usize,
    pub(crate) after_chars: usize,
    pub(crate) before_tokens: usize,
    pub(crate) after_tokens: usize,
    /// Content entries + prose items dropped as already-served.
    pub(crate) dropped: usize,
}

/// The full decision pipeline, minus stdin/gain (kept in
/// [`cmd_hookwrap_websearch_post`] so this stays unit-testable with a
/// tempdir `state_dir`). Returns None on every skip path — the hook then
/// prints nothing and the native output stands.
pub(crate) fn websearch_post_decision(
    parsed: &serde_json::Value,
    state_dir: &Path,
) -> Option<WebsearchOutcome> {
    if tokenade_core::env_compat::is_truthy_one("TOKENADE_NO_WEBSEARCH") {
        return None;
    }
    if parsed.get("hook_event_name").and_then(|s| s.as_str()) != Some("PostToolUse")
        || parsed.get("tool_name").and_then(|s| s.as_str()) != Some("WebSearch")
    {
        return None;
    }
    let resp = parsed.get("tool_response")?;
    if !resp.is_object() {
        return None;
    }
    let results = resp.get("results").and_then(|r| r.as_array())?;
    if results.is_empty() {
        return None;
    }
    let query = resp
        .get("query")
        .and_then(|q| q.as_str())
        .unwrap_or("")
        .to_string();
    let session_id = crate::hooks::session_id_of(parsed).unwrap_or("");

    let before_text = rendered_results_text(results);
    let before_chars = before_text.len();

    // Cross-call seen-set (best-effort; an unreadable file = empty set,
    // dedup simply starts fresh — never an error surface).
    let seen_path = websearch_seen_path(state_dir, session_id);
    let mut seen = load_websearch_seen(&seen_path);
    let compaction = compact_websearch_results(results, &mut seen);
    // Persist REGARDLESS of the gate below: whether we replace or pass
    // through, every URL/prose item in this response reaches the model
    // (natively on passthrough), so it counts as served for later calls.
    save_websearch_seen(&seen_path, &seen);

    let mut new_results = compaction.results;
    // Stash the ORIGINAL tool_response byte-exact for `expand-ref`
    // recovery, and reference it in a one-line banner item.
    let orig = serde_json::to_string(resp).unwrap_or_default();
    if let Some((hash, _path)) = tokenade_core::disclose::stash_for_reference(
        &orig,
        &tokenade_core::disclose::SourceHint::web(),
    ) {
        new_results.push(serde_json::Value::String(format!(
            "[tokenade:websearch-compact] full result set folded — recover with \
             `tokenade expand-ref {hash}`"
        )));
    }

    let after_text = rendered_results_text(&new_results);
    let after_chars = after_text.len();
    // Net-savings gate (banner + omitted note already included in `after`).
    let saved = before_chars.saturating_sub(after_chars);
    let pct = (saved * 100).checked_div(before_chars).unwrap_or(0);
    if saved < WEBSEARCH_MIN_SAVED_CHARS || pct < WEBSEARCH_MIN_SAVED_PCT {
        tokenade_core::debug_log::log(
            "hookwrap-websearch-post",
            "websearch_skip_low_gain",
            serde_json::json!({
                "query": query,
                "before_chars": before_chars,
                "after_chars": after_chars,
                "pct": pct,
            }),
        );
        return None;
    }

    // Shape-preserving replacement: clone the original tool_response and
    // swap ONLY `results` — query/durationSeconds/searchCount (and any
    // future top-level field) ride through untouched. If Claude Code's
    // outputSchema ever rejects this, the replacement is silently dropped
    // and the native output stands (fail-open by protocol design).
    let mut updated = resp.clone();
    updated
        .as_object_mut()
        .expect("checked is_object above")
        .insert("results".to_string(), serde_json::Value::Array(new_results));
    let decision = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "updatedToolOutput": updated,
        }
    });
    Some(WebsearchOutcome {
        json_line: decision.to_string(),
        query,
        before_chars,
        after_chars,
        before_tokens: tokenade_core::tokenizer::estimate(&before_text),
        after_tokens: tokenade_core::tokenizer::estimate(&after_text),
        dropped: compaction.dropped,
    })
}

/// Per-session seen-set: URLs (normalized) + prose items (content hash of
/// the collapsed string) already served by earlier WebSearch calls.
#[derive(Default)]
pub(crate) struct WebsearchSeen {
    pub(crate) urls: HashSet<String>,
    pub(crate) prose: HashSet<String>,
}

/// Directory for the per-session websearch seen-set. Lives next to the
/// webfetch session cache (`~/.tokenade/sessions`). Env override
/// `TOKENADE_WEBSEARCH_STATE_DIR` for test isolation, mirroring
/// `TOKENADE_WEBFETCH_CACHE_DIR`.
pub(crate) fn websearch_state_dir() -> PathBuf {
    if let Some(p) = tokenade_core::env_compat::path_from_env("TOKENADE_WEBSEARCH_STATE_DIR") {
        return p;
    }
    tokenade_core::platform::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tokenade")
        .join("sessions")
}

pub(crate) fn websearch_seen_path(state_dir: &Path, session_id: &str) -> PathBuf {
    // session_id is a UUID from Claude Code but sanitize anyway (same as
    // webfetch_cache_path).
    let safe: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    state_dir.join(format!("{safe}-websearch.json"))
}

/// Load the seen-set. Any read/parse failure = empty set (fail-open).
pub(crate) fn load_websearch_seen(path: &Path) -> WebsearchSeen {
    let mut seen = WebsearchSeen::default();
    let Ok(raw) = std::fs::read_to_string(path) else {
        return seen;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return seen;
    };
    for (key, set) in [("urls", &mut seen.urls), ("prose", &mut seen.prose)] {
        if let Some(arr) = v.get(key).and_then(|a| a.as_array()) {
            for e in arr {
                if let Some(s) = e.as_str() {
                    set.insert(s.to_string());
                }
            }
        }
    }
    seen
}

/// Persist the seen-set. Best-effort: any I/O error silently skips (the
/// next call just re-serves — never an error surface). Same idiom as
/// `webfetch_cache_record`.
pub(crate) fn save_websearch_seen(path: &Path, seen: &WebsearchSeen) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut urls: Vec<&String> = seen.urls.iter().collect();
    let mut prose: Vec<&String> = seen.prose.iter().collect();
    urls.sort();
    prose.sort();
    let doc = serde_json::json!({"urls": urls, "prose": prose});
    let _ = std::fs::write(path, serde_json::to_string(&doc).unwrap_or_default());
}

/// Drop the session's seen-set (PreCompact): the summariser is about to
/// squash the turns that contained the previously-served results, so
/// "already served" no longer implies "still in context". Post-compaction
/// searches must re-serve in full rather than omit URLs the model can no
/// longer see. Mirrors `dedup_ref::clear_session_ranges`.
pub(crate) fn clear_websearch_seen(session_id: &str) {
    if session_id.is_empty() {
        return;
    }
    let path = websearch_seen_path(&websearch_state_dir(), session_id);
    let _ = std::fs::remove_file(path);
}

pub(crate) struct WebsearchCompaction {
    pub(crate) results: Vec<serde_json::Value>,
    pub(crate) dropped: usize,
}

/// The pure compaction: slims link entries to {title, url}, dedups URLs
/// (within the call AND against `seen` from earlier calls — first
/// occurrence wins and is marked seen), collapses/dedups prose items, and
/// appends the one-line omitted note when anything was dropped. `seen` is
/// updated in place with everything served.
pub(crate) fn compact_websearch_results(
    results: &[serde_json::Value],
    seen: &mut WebsearchSeen,
) -> WebsearchCompaction {
    let mut out: Vec<serde_json::Value> = Vec::with_capacity(results.len());
    let mut dropped = 0usize;
    for item in results {
        match item {
            serde_json::Value::String(s) => {
                let collapsed = collapse_whitespace(s);
                if collapsed.is_empty() {
                    continue;
                }
                let h = tokenade_core::dedup_ref::hash_content(collapsed.as_bytes());
                if seen.prose.insert(h) {
                    out.push(serde_json::Value::String(collapsed));
                } else {
                    dropped += 1;
                }
            }
            serde_json::Value::Object(obj) => {
                let entries = obj.get("content").and_then(|c| c.as_array());
                let Some(entries) = entries else {
                    // Unknown object shape — pass through untouched rather
                    // than guess (shape-preserving bias).
                    out.push(item.clone());
                    continue;
                };
                let mut kept: Vec<serde_json::Value> = Vec::with_capacity(entries.len());
                for e in entries {
                    let title = e.get("title").and_then(|t| t.as_str()).unwrap_or("");
                    let url = e.get("url").and_then(|u| u.as_str()).unwrap_or("");
                    if !url.is_empty() {
                        let norm = normalize_url(url);
                        if !seen.urls.insert(norm) {
                            // Already served — by an earlier call of this
                            // session or earlier in THIS call.
                            dropped += 1;
                            continue;
                        }
                    }
                    // Keep ONLY {title, url} — page_age / metadata /
                    // encrypted payload fields are dead weight the model
                    // never needs.
                    kept.push(serde_json::json!({
                        "title": trim_title(title),
                        "url": url,
                    }));
                }
                if kept.is_empty() {
                    // Every link was a dup: drop the whole item — an empty
                    // content array renders as a misleading "No links
                    // found."; the omitted note below explains instead.
                    continue;
                }
                let mut slim = obj.clone();
                slim.insert("content".to_string(), serde_json::Value::Array(kept));
                out.push(serde_json::Value::Object(slim));
            }
            other => out.push(other.clone()),
        }
    }
    if dropped > 0 {
        out.push(serde_json::Value::String(format!(
            "[tokenade] {dropped} result(s) omitted — already listed in earlier searches \
             this session."
        )));
    }
    WebsearchCompaction {
        results: out,
        dropped,
    }
}

/// Model-visible rendering of a `results` array, mirroring Claude Code:
/// string → verbatim; object with non-empty content → `Links: <JSON>`;
/// else `No links found.`. Items joined with blank lines (the constant
/// header line is identical before/after, so it cancels out of savings).
pub(crate) fn rendered_results_text(results: &[serde_json::Value]) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(results.len());
    for item in results {
        match item {
            serde_json::Value::String(s) => parts.push(s.clone()),
            obj => {
                let content = obj.get("content").and_then(|c| c.as_array());
                match content {
                    Some(c) if !c.is_empty() => parts.push(format!(
                        "Links: {}",
                        serde_json::to_string(c).unwrap_or_default()
                    )),
                    _ => parts.push("No links found.".to_string()),
                }
            }
        }
    }
    parts.join("\n\n")
}

/// URL normalization for dedup: strip the fragment, drop trailing
/// slashes, lowercase the host (path/query stay case-sensitive).
pub(crate) fn normalize_url(u: &str) -> String {
    let mut s = u.split('#').next().unwrap_or(u).trim().to_string();
    while s.ends_with('/') {
        s.pop();
    }
    if let Some(pos) = s.find("://") {
        let host_start = pos + 3;
        let host_end = s[host_start..]
            .find(['/', '?'])
            .map(|i| host_start + i)
            .unwrap_or(s.len());
        let host_lower = s[host_start..host_end].to_ascii_lowercase();
        s.replace_range(host_start..host_end, &host_lower);
    }
    s
}

/// Trim a link title to [`WEBSEARCH_TITLE_MAX_CHARS`] (char-boundary
/// safe), appending an ellipsis when cut.
pub(crate) fn trim_title(t: &str) -> String {
    if t.chars().count() <= WEBSEARCH_TITLE_MAX_CHARS {
        return t.to_string();
    }
    let cut: String = t.chars().take(WEBSEARCH_TITLE_MAX_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Whitespace collapse for prose items, CODE-SAFE: trailing whitespace is
/// stripped per line and runs of blank lines collapse to one, but leading
/// indentation and interior spacing are preserved — prose passages carry
/// fenced code blocks (the fixture has a `Cargo.toml` snippet) where
/// interior whitespace is meaningful.
pub(crate) fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0usize;
    for line in s.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(t);
        out.push('\n');
    }
    out.trim_end_matches('\n')
        .trim_start_matches('\n')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real captured PostToolUse stdin for a WebSearch call (verbatim).
    const WS_DUMP: &str = include_str!("../../tests/fixtures/ws_dump.json");

    /// Every test that CALLS websearch_post_decision (or mutates
    /// TOKENADE_NO_WEBSEARCH) holds this lock — the kill-switch test sets
    /// that env var, and parallel test threads share process env.
    /// ALIAS for [`tokenade_core::platform::ENV_LOCK`], not a second lock. Process
    /// env has exactly one owner: two locks over it do not exclude each other.
    /// Measured as a real flake on 2026-08-27 between the capability lock and
    /// `ScopedEnv` — a test that passed alone failed in the full suite. An earlier
    /// fix named `ENV_LOCK` the single home; these were the copies it missed.
    static ENV_LOCK: &std::sync::Mutex<()> = &tokenade_core::platform::ENV_LOCK;

    /// Hermeticity guard: `stash_for_reference` resolves the stash from
    /// process env, which sibling tests rewrite. Hold the env lock for the
    /// duration; poison-tolerant so one failing test can't cascade
    /// PoisonErrors through the module.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Std-only tempdir (tokenade-cli has no tempfile dev-dep): unique
    /// per test invocation, left for the OS tmp cleaner.
    fn tmpdir(tag: &str) -> std::path::PathBuf {
        // The clock alone does not make this unique: `SystemTime` granularity
        // is coarse on some platforms (~15 ms on Windows), so parallel tests
        // can read the same nanos, and `create_dir_all` adopts an existing
        // directory silently. A process-wide counter makes the name unique by
        // construction; `create_dir` fails loudly on any residual collision.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "tokenade-test-{tag}-{}-{}-{seq}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir(&p).expect("test tempdir name must be unique");
        p
    }

    fn fixture() -> serde_json::Value {
        serde_json::from_str(WS_DUMP).expect("fixture parses")
    }

    /// Doctored fixture: adds the metadata ballast real WebSearch
    /// responses carry (page_age, encrypted content) so the slimming has
    /// something to save and the net-savings gate passes.
    fn fixture_with_ballast() -> serde_json::Value {
        let mut v = fixture();
        let results = v
            .pointer_mut("/tool_response/results")
            .and_then(|r| r.as_array_mut())
            .unwrap();
        for item in results.iter_mut() {
            if let Some(content) = item.get_mut("content").and_then(|c| c.as_array_mut()) {
                for e in content.iter_mut() {
                    let o = e.as_object_mut().unwrap();
                    o.insert("page_age".into(), serde_json::json!("January 2, 2026"));
                    o.insert(
                        "encrypted_content".into(),
                        serde_json::json!("EqgCCioIABgBIiQ3YmY0MjdlZS1jY2ExLTQ0NzEtOTUwYi1i\
                         ZWJlNGYyNzQyOTAaDDVhYmEyYzNlZDIxNCIkN2JmNDI3ZWUtY2NhMS00NDcxLTk1MGItYmViZTRmMjc0MjkwKgw1"),
                    );
                }
            }
        }
        v
    }

    #[test]
    fn compaction_keeps_only_title_url_and_preserves_item_shape() {
        let v = fixture_with_ballast();
        let results = v
            .pointer("/tool_response/results")
            .and_then(|r| r.as_array())
            .unwrap();
        let mut seen = WebsearchSeen::default();
        let c = compact_websearch_results(results, &mut seen);
        assert_eq!(c.dropped, 0, "fresh session: nothing dropped");
        // Object item survives with tool_use_id + slimmed content.
        let obj = c.results[0]
            .as_object()
            .expect("first item is the links object");
        assert!(
            obj.contains_key("tool_use_id"),
            "extra item fields preserved"
        );
        let content = obj["content"].as_array().unwrap();
        assert_eq!(content.len(), 10);
        for e in content {
            let keys: Vec<&String> = e.as_object().unwrap().keys().collect();
            assert_eq!(keys.len(), 2, "only title+url survive, got {keys:?}");
            assert!(e.get("title").is_some() && e.get("url").is_some());
            assert!(e.get("page_age").is_none() && e.get("encrypted_content").is_none());
        }
        // Long title (>120 chars in the fixture) is trimmed with ellipsis.
        let long = content
            .iter()
            .find(|e| e["url"] == "https://github.com/tokio-rs/tokio")
            .unwrap();
        let t = long["title"].as_str().unwrap();
        assert!(t.chars().count() <= WEBSEARCH_TITLE_MAX_CHARS);
        assert!(t.ends_with('…'));
        // Prose string item kept (collapsed, not dropped).
        assert!(c.results[1].is_string());
        // Everything served was marked seen.
        assert_eq!(seen.urls.len(), 10);
        assert_eq!(seen.prose.len(), 1);
    }

    #[test]
    fn full_decision_emits_shape_preserving_updated_tool_output() {
        let _g = env_guard();
        let dir = tmpdir("ws-full");
        let v = fixture_with_ballast();
        let out = websearch_post_decision(&v, &dir).expect("gate passes with ballast");
        assert!(out.before_chars > out.after_chars);
        assert_eq!(out.dropped, 0);
        let emitted: serde_json::Value = serde_json::from_str(&out.json_line).unwrap();
        assert_eq!(
            emitted.pointer("/hookSpecificOutput/hookEventName"),
            Some(&serde_json::json!("PostToolUse"))
        );
        let updated = emitted
            .pointer("/hookSpecificOutput/updatedToolOutput")
            .expect("updatedToolOutput present");
        // Top-level tool_response keys preserved verbatim.
        assert_eq!(
            updated["query"],
            v.pointer("/tool_response/query").unwrap().clone()
        );
        assert_eq!(
            updated["durationSeconds"],
            v.pointer("/tool_response/durationSeconds").unwrap().clone()
        );
        assert_eq!(
            updated["searchCount"],
            v.pointer("/tool_response/searchCount").unwrap().clone()
        );
        // Banner appended as the final string item, pointing at expand-ref.
        let results = updated["results"].as_array().unwrap();
        let banner = results.last().unwrap().as_str().unwrap();
        assert!(
            banner.starts_with("[tokenade:websearch-compact]"),
            "{banner}"
        );
        assert!(banner.contains("tokenade expand-ref "), "{banner}");
        assert!(!banner.contains('\n'), "banner is one line");
    }

    #[test]
    fn cross_call_dedup_drops_previously_served_urls() {
        let _g = env_guard();
        let dir = tmpdir("ws-dedup");
        let v = fixture_with_ballast();
        websearch_post_decision(&v, &dir).expect("first call fires");
        // Second call, SAME session_id, same URLs + same prose.
        let out2 = websearch_post_decision(&v, &dir)
            .expect("second call fires (dedup makes it a huge save)");
        assert_eq!(out2.dropped, 11, "10 urls + 1 prose item dropped");
        let emitted: serde_json::Value = serde_json::from_str(&out2.json_line).unwrap();
        let results = emitted
            .pointer("/hookSpecificOutput/updatedToolOutput/results")
            .and_then(|r| r.as_array())
            .unwrap();
        // Whole links item dropped (all dups) — what remains is the
        // omitted note + the banner.
        assert!(results.iter().all(|r| r.is_string()));
        let note = results
            .iter()
            .filter_map(|r| r.as_str())
            .find(|s| s.contains("omitted"))
            .expect("omitted note present");
        assert!(note.contains("11 result(s) omitted"), "{note}");
        assert!(note.contains("already listed in earlier searches this session"));
        // State is per-session: a DIFFERENT session dedups nothing.
        let mut other = fixture_with_ballast();
        other["session_id"] = serde_json::json!("00000000-0000-0000-0000-00000000beef");
        let out3 = websearch_post_decision(&other, &dir).unwrap();
        assert_eq!(out3.dropped, 0);
    }

    #[test]
    fn within_call_duplicate_url_kept_once() {
        let mut seen = WebsearchSeen::default();
        let results = vec![serde_json::json!({
            "tool_use_id": "x",
            "content": [
                {"title": "A", "url": "https://Example.com/page/"},
                {"title": "A again", "url": "https://example.com/page#frag"},
                {"title": "B", "url": "https://example.com/other"},
            ]
        })];
        let c = compact_websearch_results(&results, &mut seen);
        assert_eq!(c.dropped, 1);
        let content = c.results[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["title"], "A");
    }

    #[test]
    fn net_savings_gate_skips_tiny_responses() {
        let _g = env_guard();
        let dir = tmpdir("ws-gate");
        // The RAW fixture is already lean ({title,url} only): slimming
        // saves ~nothing and the banner adds cost → gate must refuse.
        let v = fixture();
        assert!(
            websearch_post_decision(&v, &dir).is_none(),
            "lean first call must NOT be replaced for crumbs"
        );
        // …but it still recorded the served URLs for later dedup.
        let sid = v["session_id"].as_str().unwrap();
        let seen = load_websearch_seen(&websearch_seen_path(&dir, sid));
        assert_eq!(seen.urls.len(), 10);
    }

    #[test]
    fn malformed_or_foreign_payloads_produce_no_output() {
        let _g = env_guard();
        let dir = tmpdir("ws-malformed");
        // Wrong tool.
        let mut v = fixture_with_ballast();
        v["tool_name"] = serde_json::json!("WebFetch");
        assert!(websearch_post_decision(&v, &dir).is_none());
        // Wrong event.
        let mut v = fixture_with_ballast();
        v["hook_event_name"] = serde_json::json!("PreToolUse");
        assert!(websearch_post_decision(&v, &dir).is_none());
        // tool_response missing / not an object / results missing.
        let mut v = fixture_with_ballast();
        v.as_object_mut().unwrap().remove("tool_response");
        assert!(websearch_post_decision(&v, &dir).is_none());
        let mut v = fixture_with_ballast();
        v["tool_response"] = serde_json::json!("a plain string");
        assert!(websearch_post_decision(&v, &dir).is_none());
        let mut v = fixture_with_ballast();
        v["tool_response"] = serde_json::json!({"query": "q"});
        assert!(websearch_post_decision(&v, &dir).is_none());
    }

    #[test]
    fn kill_switch_env_produces_no_output() {
        let _g = env_guard();
        let dir = tmpdir("ws-kill");
        std::env::set_var("TOKENADE_NO_WEBSEARCH", "1");
        let out = websearch_post_decision(&fixture_with_ballast(), &dir);
        std::env::remove_var("TOKENADE_NO_WEBSEARCH");
        assert!(out.is_none());
        // And with it removed the same payload fires.
        assert!(websearch_post_decision(&fixture_with_ballast(), &dir).is_some());
    }

    #[test]
    fn url_normalization_rules() {
        assert_eq!(
            normalize_url("https://EXAMPLE.com/Path/?q=A#frag"),
            "https://example.com/Path/?q=A"
        );
        assert_eq!(normalize_url("https://a.com/x/"), "https://a.com/x");
        assert_eq!(normalize_url("https://a.com///"), "https://a.com");
        // Query/path case preserved, host lowered even with no path.
        assert_eq!(normalize_url("HTTPS://A.COM"), "HTTPS://a.com");
    }

    #[test]
    fn collapse_whitespace_is_code_safe() {
        let s = "line one   \n\n\n\n    indented code\nend  ";
        assert_eq!(collapse_whitespace(s), "line one\n\n    indented code\nend");
    }

    #[test]
    fn seen_state_roundtrips() {
        let dir = tmpdir("ws-state");
        let p = websearch_seen_path(&dir, "ab-12/..\\evil");
        assert!(p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("ab-12"));
        let mut seen = WebsearchSeen::default();
        seen.urls.insert("https://a.com/x".into());
        seen.prose.insert("hash1".into());
        save_websearch_seen(&p, &seen);
        let loaded = load_websearch_seen(&p);
        assert!(loaded.urls.contains("https://a.com/x"));
        assert!(loaded.prose.contains("hash1"));
        // Corrupt file → empty set, never a panic.
        std::fs::write(&p, "{not json").unwrap();
        let loaded = load_websearch_seen(&p);
        assert!(loaded.urls.is_empty() && loaded.prose.is_empty());
    }
}
