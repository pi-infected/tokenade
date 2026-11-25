#![allow(unused_imports)]
//! AfterTool compactor for Gemini CLI's web tools
//! (`tokenade gemini-afterhook`).
//!
//! Gemini CLI's `google_web_search` and `web_fetch` results reach the
//! model uncompacted — the BeforeTool wrap/redirect can't touch them
//! (there is nothing to rewrite pre-execution). Gemini's AfterTool hook
//! offers a documented replace channel (docs/hooks/reference):
//! `{"decision":"deny","reason":"<text>"}` where `reason` REPLACES the
//! tool result sent back to the model — AfterTool deny is a clean
//! replace, not an error (BeforeTool deny is the error-ish one).
//! `hookSpecificOutput.additionalContext` is append-only, so it is NOT
//! used here.
//!
//! Routing: with a non-empty user intent (`tool_input.query` /
//! `tool_input.prompt`) the replacement is a prompt-scoped BM25
//! extract; otherwise `auto::compact` (SERP/HTML/markdown/prose
//! auto-detect, anti-inflation guard built in). The ORIGINAL llmContent
//! is stashed for `tokenade expand-ref` recovery and a one-line banner
//! is appended. Replacement only happens past the net-savings gate
//! (≥600 chars AND ≥15%, same thresholds as the WebSearch compactor);
//! below it — and on ANY gate/parse problem, error result, or
//! multimodal (non-text-part) llmContent — the hook prints NOTHING and
//! exits 0, leaving the native result untouched. Kill-switch:
//! `TOKENADE_NO_GEMINI_WEB=1` (or the broader `TOKENADE_HOOK_DISABLED=1`).
//!
//! Per-agent replace matrix (surveyed 2026-07, RE-CHECKED against the
//! 2026-09 documentation): Copilot, opencode, Cline and Hermes already
//! replace tool outputs via their shipped bridges — nothing to add there.
//! Codex's PostToolUse block semantics are lossy (the folded text becomes
//! feedback and the result's shape is lost) and its `updatedMCPToolOutput`
//! is "parsed but not supported yet", so it is still NOT wired.
//! Antigravity: its `PostToolUse` "Returns an empty JSON object `{}`" and its
//! input does not even carry the tool output — there is nothing to wire.
//! Factory Droid: same verdict, `"block"` + `reason` and nothing else; its
//! compaction already goes through `PreToolUse.updatedInput`. Qwen and
//! Reasonix expose no replacement path to date.
use super::*;
use crate::*;

#[cfg(test)]
mod two_aftertools_never_fight_over_one_result {
    use super::*;

    fn event(tool: &str, text: &str) -> serde_json::Value {
        serde_json::json!({
            "hook_event_name": "AfterTool",
            "tool_name": tool,
            "tool_input": {"path": "/var/log/app.log"},
            "tool_response": {"llmContent": [{"text": text}], "error": null},
        })
    }

    /// **An MCP tool belongs to the CATCH-ALL folder, not to this one.**
    ///
    /// Gemini carries TWO tokenade `AfterTool` entries: this compactor on
    /// `^(google_web_search|web_fetch)$`, and `tokenade-fold` on `.*` which
    /// folds everything else — MCP tools included. Widening this matcher to
    /// `mcp_.*` was tried and reverted: both hooks would have replaced the
    /// same result. This test holds the invariant on BOTH sides, since the
    /// symmetric guard lives in `gemini_after_fold`.
    #[test]
    fn an_mcp_tool_is_left_to_the_catch_all_folder() {
        let noisy = (0..500)
            .map(|i| {
                format!(
                    "2026-09-11T12:00:{i:02}Z INFO cache warm shard={} hit=1",
                    i % 6
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            gemini_aftertool_decision(&event("mcp_logs_tail", &noisy)).is_none(),
            "this compactor took an MCP result that belongs to tokenade-fold"
        );
        // And the catch-all does take it.
        assert!(
            crate::install::gemini_hooks::gemini_after_fold(
                &serde_json::to_string(&event("mcp_logs_tail", &noisy)).unwrap()
            )
            .is_some(),
            "nobody folds this MCP result"
        );
    }

    /// And the catch-all steps aside for the two web tools, which belong to
    /// this compactor: the invariant holds both ways.
    #[test]
    fn the_catch_all_leaves_web_tools_to_the_specialised_compactor() {
        let noisy = "ligne de contenu web. ".repeat(2000);
        for web in ["google_web_search", "web_fetch"] {
            assert!(
                crate::install::gemini_hooks::gemini_after_fold(
                    &serde_json::to_string(&event(web, &noisy)).unwrap()
                )
                .is_none(),
                "{web} folded by both hooks at once"
            );
        }
    }
}

/// Entry point for `tokenade gemini-afterhook`. Reads the AfterTool JSON
/// on stdin; on success prints the `{"decision":"deny","reason":…}`
/// replacement line; on ANY gate/parse failure prints nothing ("Silence
/// is Mandatory": stdout must be pure JSON). Always exits 0 (fail-open —
/// the native result stands).
pub(crate) fn cmd_gemini_afterhook() -> i32 {
    let started = std::time::Instant::now();
    if hook_disabled_by_env() {
        return 0;
    }
    let Some(input) = crate::hooks::stdin::read_event("gemini-afterhook") else {
        return 0;
    };
    let Some(parsed) = crate::hooks::stdin::parse_event(&input) else {
        tokenade_core::debug_log::log(
            "gemini-afterhook",
            "parse_err",
            serde_json::json!({"elapsed_ms": started.elapsed().as_millis() as u64}),
        );
        return 0;
    };
    // This process IS the Gemini hook — attribute the savings we record
    // to gemini-cli regardless of ambient env (same idiom as
    // cmd_gemini_hook).
    std::env::set_var("TOKENADE_AGENT", "gemini-cli");
    // Session-scope the gain rows this hook books — same idiom as
    // cmd_hookwrap_websearch_post.
    if let Some(sid) = crate::hooks::session_id_of(&parsed) {
        let skey: String = sid
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(40)
            .collect();
        tokenade_core::session::set_session_key_authoritative(&skey);
    }
    // #4-style FAIL-OPEN when not activated (silent variant): serving a
    // compacted view + an expand-ref banner the agent cannot expand is
    // strictly worse than the native output.
    let Some(outcome) = gemini_aftertool_decision(&parsed) else {
        return 0;
    };
    println!("{}", outcome.json_line);
    // Gain accounting — bounded by construction, same delivery-based
    // assiette as websearch_compact: `before` is the llmContent text the
    // model would actually pay for, `after` is the replacement we served
    // (banner included). No multiplier, no compounding.
    let sav = tokenade_core::Savings::new(outcome.before_tokens, outcome.after_tokens);
    let label: String = format!("{} {}", outcome.tool_name, outcome.intent)
        .trim()
        .chars()
        .take(120)
        .collect();
    gain_log::record("hook", "gemini_web_compact", &label, sav);
    tokenade_core::debug_log::log(
        "gemini-afterhook",
        "gemini_web_compact_fired",
        serde_json::json!({
            "tool": outcome.tool_name,
            "intent": outcome.intent,
            "route": outcome.route,
            "before_chars": outcome.before_chars,
            "after_chars": outcome.after_chars,
            "before_tokens": outcome.before_tokens,
            "after_tokens": outcome.after_tokens,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
    0
}

/// Everything the caller needs after a successful compaction decision.
pub(crate) struct GeminiWebOutcome {
    /// The exact line to print on stdout: `{"decision":"deny","reason":…}`.
    pub(crate) json_line: String,
    pub(crate) tool_name: String,
    /// The user's intent text (query/prompt), possibly empty.
    pub(crate) intent: String,
    /// Which compaction path served the replacement: "scoped" | "auto".
    pub(crate) route: &'static str,
    pub(crate) before_chars: usize,
    pub(crate) after_chars: usize,
    pub(crate) before_tokens: usize,
    pub(crate) after_tokens: usize,
}

/// The full decision pipeline, minus stdin/gain (kept in
/// [`cmd_gemini_afterhook`] so this stays unit-testable). Returns None on
/// every skip path — the hook then prints nothing and the native result
/// stands.
pub(crate) fn gemini_aftertool_decision(parsed: &serde_json::Value) -> Option<GeminiWebOutcome> {
    if tokenade_core::env_compat::is_truthy_one("TOKENADE_NO_GEMINI_WEB") {
        return None;
    }
    if parsed.get("hook_event_name").and_then(|s| s.as_str()) != Some("AfterTool") {
        return None;
    }
    let tool_name = parsed.get("tool_name").and_then(|s| s.as_str())?;
    // **Two AfterTool hooks NEVER fight over the same result.** This
    // compactor owns the two web tools; its catch-all twin
    // (`tokenade-fold`, matcher `.*`) owns everything else, MCP tools
    // included, and steps aside for the two web tools via the symmetric
    // guard in `gemini_after_fold`. Widening this matcher to `mcp_.*` —
    // tried, reverted — would have two hooks replace the same result at once.
    if !tool_is(tool_name, "google_web_search") && !tool_is(tool_name, "web_fetch") {
        return None;
    }
    let resp = parsed.get("tool_response")?;
    // Never mask an error result — the model must see it verbatim.
    if !resp.get("error").map(|e| e.is_null()).unwrap_or(true) {
        return None;
    }
    let text = llm_content_text(resp.get("llmContent")?)?;
    // The user's intent: the search query / fetch prompt. Empty when the
    // field is absent — the scoped path then never engages.
    let intent_key = if tool_is(tool_name, "google_web_search") {
        "query"
    } else {
        "prompt"
    };
    let intent = parsed
        .pointer(&format!("/tool_input/{intent_key}"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let before_chars = text.len();
    let before_tokens = tokenade_core::tokenizer::estimate(&text);

    // Route: prompt-scoped extract whenever there is a usable intent;
    // auto::compact everywhere else (and as the scoped fallback when the
    // intent has no lexical foothold in the page).
    //
    // NO size gate (PO ruling 2026-08-01). This channel briefly borrowed
    // WebFetch's 12k threshold and was the ONLY per-agent size gate in the tree
    // — every other scoped-route site (`compactors.rs`: read, web, search)
    // already scopes on the presence of a question alone. The ruling is that the
    // WebFetch dollar finding generalises across agents, so the outlier is gone
    // and all channels now agree: a question that anchors gets an extract,
    // whatever the size.
    let (mut replacement, route) = if !intent.is_empty() {
        match tokenade_core::rag::prompt_scoped_extract_dispatch(
            &text,
            &intent,
            tokenade_core::disclose::webfetch_scoped_budget_for(before_tokens),
        ) {
            Some(scoped) => (scoped, "scoped"),
            // Asked, and nothing in the source anchored. Serving the compacted
            // page unannounced makes it read as the answer to a question it
            // never addressed — same silent substitution the file and search
            // routes carried. Name it.
            None => (
                format!(
                    "[tokenade] nothing in this source answers that question — what follows is \
                     the source itself, not an answer.\n{}",
                    tokenade_core::auto::compact(&text).1
                ),
                "auto",
            ),
        }
    } else {
        (tokenade_core::auto::compact(&text).1, "auto")
    };
    if replacement.trim().is_empty() {
        // A deny whose reason is only the banner would destroy the
        // result — fail-open instead.
        return None;
    }

    // Stash the ORIGINAL llmContent byte-exact for `expand-ref`
    // recovery, and reference it in a one-line banner.
    if let Some((hash, _path)) = tokenade_core::disclose::stash_for_reference(
        &text,
        &tokenade_core::disclose::SourceHint::web(),
    ) {
        replacement.push_str(&format!(
            "\n[tokenade:gemini-web] full result folded — recover with \
             `tokenade expand-ref {hash}`"
        ));
    }

    let after_chars = replacement.len();
    // Net-savings gate — identical thresholds to the WebSearch compactor
    // (banner already included in `after`). Both legs must pass.
    let saved = before_chars.saturating_sub(after_chars);
    let pct = (saved * 100).checked_div(before_chars).unwrap_or(0);
    if saved < WEBSEARCH_MIN_SAVED_CHARS || pct < WEBSEARCH_MIN_SAVED_PCT {
        tokenade_core::debug_log::log(
            "gemini-afterhook",
            "gemini_web_skip_low_gain",
            serde_json::json!({
                "tool": tool_name,
                "before_chars": before_chars,
                "after_chars": after_chars,
                "pct": pct,
            }),
        );
        return None;
    }

    let after_tokens = tokenade_core::tokenizer::estimate(&replacement);
    // EXACTLY the documented replace shape — no `continue`, no
    // `stopReason`, no `suppressOutput`, no `hookSpecificOutput`.
    let json_line = serde_json::json!({"decision": "deny", "reason": replacement}).to_string();
    Some(GeminiWebOutcome {
        json_line,
        tool_name: tool_name.to_string(),
        intent,
        route,
        before_chars,
        after_chars,
        before_tokens,
        after_tokens,
    })
}

/// Extract the text of a `tool_response.llmContent` value. Handles both
/// documented shapes: a plain string, or a genai parts array
/// (`[{text}…]`, concatenated). Any non-text part (inlineData, fileData,
/// functionResponse, …) → None — fail-open, never destroy a multimodal
/// result. Conservative: a part qualifies only when `text` is its SOLE
/// key.
pub(crate) fn llm_content_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(parts) => {
            let mut out = String::new();
            for p in parts {
                let obj = p.as_object()?;
                let is_pure_text =
                    obj.len() == 1 && obj.get("text").map(|t| t.is_string()).unwrap_or(false);
                if !is_pure_text {
                    return None;
                }
                out.push_str(obj["text"].as_str().unwrap_or(""));
            }
            if out.trim().is_empty() {
                None
            } else {
                Some(out)
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test that calls gemini_aftertool_decision (or mutates
    /// TOKENADE_NO_GEMINI_WEB) holds this lock — parallel test threads
    /// share process env. Poison-tolerant so one failing test doesn't
    /// cascade PoisonErrors over the real failures.
    /// ALIAS for [`tokenade_core::platform::ENV_LOCK`], not a second lock. Process
    /// env has exactly one owner: two locks over it do not exclude each other.
    /// Measured as a real flake on 2026-08-27 between the capability lock and
    /// `ScopedEnv` — a test that passed alone failed in the full suite.
    /// `ENV_LOCK` was then named the single home; these were the copies missed.
    static ENV_LOCK: &std::sync::Mutex<()> = &tokenade_core::platform::ENV_LOCK;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A SERP-ish llmContent well past the savings gate: search-result
    /// HTML markup (what a web tool relays verbatim) whose tag ballast the
    /// auto compactor folds hard (~40% measured on this fixture). Plain
    /// prose SERps fold little and correctly stay below the gate — that
    /// case is `small_response_below_gate_passes_through`.
    fn big_serp_text() -> String {
        let mut s = String::from(
            "<!DOCTYPE html><html><head><title>tokio runtime tuning - Search</title>\
             <style>.b_algo{margin:0}</style></head><body><div id=\"b_results\">",
        );
        for i in 0..60 {
            s.push_str(&format!(
                "<li class=\"b_algo\"><h2><a href=\"https://example.com/tokio/{i}\
                 ?utm_source=serp&amp;session={i}{i}\">Tokio runtime tuning guide part {i} \
                 — asynchronous Rust performance</a></h2><div class=\"b_caption\"><p>Learn \
                 how to tune the tokio runtime, configure worker threads, adjust the \
                 blocking pool, and profile task latency in production systems, entry {i}.\
                 </p></div><div class=\"b_attribution\"><cite>example.com › tokio</cite>\
                 </div></li>"
            ));
        }
        s.push_str(
            "</div><script>var telemetry={\"a\":1};trackImpressions();</script></body></html>",
        );
        s
    }

    /// A large page (> the 12k-token scoped gate) with ONE anchored
    /// needle line, so the scoped path has a lexical foothold.
    fn big_anchored_page() -> String {
        let mut s = String::new();
        for i in 0..6000 {
            s.push_str(&format!(
                "section {i}: assorted filler prose about deployment pipelines and \
                 container orchestration with no relevance whatsoever, entry {i}.\n"
            ));
        }
        s.push_str("keepalive_timeout default value is 75 seconds in nginx.\n");
        for i in 0..6000 {
            s.push_str(&format!(
                "trailing filler paragraph {i} about unrelated topics.\n"
            ));
        }
        s
    }

    /// Filler prose + one unique needle, sized deliberately BELOW the 12k class
    /// the Gemini route used to gate on — the fixture that shows the gate is gone.
    fn mid_anchored_page() -> String {
        let mut s = String::new();
        for i in 0..300 {
            s.push_str(&format!(
                "section {i}: assorted filler prose about deployment pipelines and \
                 container orchestration with no relevance whatsoever, entry {i}.\n"
            ));
        }
        s.push_str("keepalive_timeout default value is 75 seconds in nginx.\n");
        s
    }

    fn payload(
        tool: &str,
        llm_content: serde_json::Value,
        input: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "session_id": "11111111-2222-3333-4444-555555555555",
            "transcript_path": "/tmp/t.json",
            "cwd": "/tmp",
            "hook_event_name": "AfterTool",
            "timestamp": "2026-07-10T00:00:00Z",
            "tool_name": tool,
            "tool_input": input,
            "tool_response": { "llmContent": llm_content, "returnDisplay": "…" }
        })
    }

    #[test]
    fn serp_string_replaced_with_exact_deny_reason_shape() {
        // The banner asserted below names a stash hash, and stashing reads
        // process env — see `tests::env_guard`.
        let _g = env_lock();
        let _rw = crate::tests::env_guard();
        let v = payload(
            "google_web_search",
            serde_json::json!(big_serp_text()),
            serde_json::json!({"query": "tokio runtime tuning"}),
        );
        let out = gemini_aftertool_decision(&v).expect("big SERP passes the gate");
        assert_eq!(out.tool_name, "google_web_search");
        assert!(out.after_chars < out.before_chars);
        assert!(out.before_tokens > out.after_tokens);
        let emitted: serde_json::Value = serde_json::from_str(&out.json_line).unwrap();
        // EXACT shape: decision + reason, nothing else.
        assert_eq!(emitted["decision"], "deny");
        assert!(emitted["reason"].is_string());
        assert_eq!(
            emitted.as_object().unwrap().len(),
            2,
            "only decision+reason keys, got {emitted}"
        );
        // Banner present, one line, points at expand-ref.
        let reason = emitted["reason"].as_str().unwrap();
        let banner = reason
            .lines()
            .find(|l| l.starts_with("[tokenade:gemini-web]"))
            .expect("banner line present");
        assert!(banner.contains("full result folded"), "{banner}");
        assert!(banner.contains("tokenade expand-ref "), "{banner}");
    }

    #[test]
    fn web_fetch_parts_array_is_concatenated_and_replaced() {
        let _g = env_lock();
        let text = big_serp_text();
        let half = text.len() / 2;
        // Split on a char boundary near the middle.
        let mid = (half..text.len())
            .find(|&i| text.is_char_boundary(i))
            .unwrap();
        let v = payload(
            "web_fetch",
            serde_json::json!([{"text": &text[..mid]}, {"text": &text[mid..]}]),
            serde_json::json!({"prompt": "summarize the page"}),
        );
        let out = gemini_aftertool_decision(&v).expect("parts array folds");
        assert_eq!(out.tool_name, "web_fetch");
        assert_eq!(out.before_chars, text.len(), "parts concatenated");
        let emitted: serde_json::Value = serde_json::from_str(&out.json_line).unwrap();
        assert_eq!(emitted["decision"], "deny");
    }

    #[test]
    fn scoped_path_used_for_large_result_with_intent() {
        let _g = env_lock();
        let page = big_anchored_page();
        let v = payload(
            "web_fetch",
            serde_json::json!(page),
            serde_json::json!({"prompt": "what is the keepalive_timeout default value"}),
        );
        let out = gemini_aftertool_decision(&v).expect("huge page folds");
        assert_eq!(out.route, "scoped", "intent + large ⇒ scoped extract");
        let emitted: serde_json::Value = serde_json::from_str(&out.json_line).unwrap();
        let reason = emitted["reason"].as_str().unwrap();
        assert!(
            reason.contains("keepalive_timeout"),
            "scoped excerpt keeps the needle"
        );
        // Scoped output is budget-bounded — a small fraction of the page.
        assert!(out.after_chars * 4 < out.before_chars);
    }

    #[test]
    fn auto_route_when_no_intent() {
        let _g = env_lock();
        // No intent field at all → nothing to scope by → auto route.
        let v = payload(
            "google_web_search",
            serde_json::json!(big_serp_text()),
            serde_json::json!({}),
        );
        let out = gemini_aftertool_decision(&v).expect("still folds via auto");
        assert_eq!(out.route, "auto");
        assert_eq!(out.intent, "");
    }

    #[test]
    fn scoped_route_under_the_old_size_class_now_that_no_agent_has_a_gate() {
        // This exact case used to take the `auto` route: an anchoring intent on a
        // result BELOW the borrowed 12k class. PO ruling 2026-08-01 — the
        // WebFetch dollar finding generalises to every agent, so no channel
        // carries a size gate and the question decides alone.
        let _g = env_lock();
        let page = mid_anchored_page();
        assert!(
            tokenade_core::tokenizer::estimate(&page) < 12_000,
            "fixture must sit under the OLD class or this test proves nothing"
        );
        let v = payload(
            "web_fetch",
            serde_json::json!(page),
            serde_json::json!({"prompt": "what is the keepalive_timeout default value"}),
        );
        let out = gemini_aftertool_decision(&v).expect("folds");
        assert_eq!(out.route, "scoped", "an anchoring question must scope");
        assert!(
            out.json_line.contains("75 seconds"),
            "the needle must survive: {}",
            out.json_line
        );
    }

    #[test]
    fn a_query_whose_terms_are_in_every_passage_falls_back_to_auto() {
        // Removing the size gate did NOT make every intent scope. The SERP
        // fixture repeats one identical caption across all 60 entries, so the
        // query's terms appear in every passage, BM25 has nothing to
        // discriminate on, `prompt_scoped_extract_dispatch` returns None and the
        // route degrades to `auto::compact`. Found by writing this test against
        // the SERP fixture and watching it come back "auto" — pinned so the
        // fallback stays a deliberate behaviour rather than a surprise.
        let _g = env_lock();
        let v = payload(
            "google_web_search",
            serde_json::json!(big_serp_text()),
            serde_json::json!({"query": "tokio runtime tuning"}),
        );
        let out = gemini_aftertool_decision(&v).expect("folds via the auto fallback");
        assert_eq!(out.route, "auto");
        assert_eq!(out.intent, "tokio runtime tuning", "intent still recorded");
    }

    #[test]
    fn error_response_is_never_masked() {
        let _g = env_lock();
        let mut v = payload(
            "google_web_search",
            serde_json::json!(big_serp_text()),
            serde_json::json!({"query": "q"}),
        );
        v["tool_response"]["error"] = serde_json::json!({"message": "quota exceeded"});
        assert!(gemini_aftertool_decision(&v).is_none());
        // Explicit null error is NOT an error.
        v["tool_response"]["error"] = serde_json::Value::Null;
        assert!(gemini_aftertool_decision(&v).is_some());
    }

    #[test]
    fn small_response_below_gate_passes_through() {
        let _g = env_lock();
        let v = payload(
            "google_web_search",
            serde_json::json!("Three short results. Nothing to fold here."),
            serde_json::json!({"query": "q"}),
        );
        assert!(gemini_aftertool_decision(&v).is_none());
    }

    #[test]
    fn foreign_tool_or_event_passes_through() {
        let _g = env_lock();
        let mut v = payload(
            "run_shell_command",
            serde_json::json!(big_serp_text()),
            serde_json::json!({"command": "ls"}),
        );
        assert!(gemini_aftertool_decision(&v).is_none(), "non-web tool");
        v["tool_name"] = serde_json::json!("google_web_search");
        v["hook_event_name"] = serde_json::json!("BeforeTool");
        assert!(gemini_aftertool_decision(&v).is_none(), "wrong event");
        // Missing tool_response / non-string llmContent.
        let mut v = payload(
            "google_web_search",
            serde_json::json!(42),
            serde_json::json!({}),
        );
        assert!(gemini_aftertool_decision(&v).is_none());
        v.as_object_mut().unwrap().remove("tool_response");
        assert!(gemini_aftertool_decision(&v).is_none());
    }

    #[test]
    fn multimodal_inline_data_part_passes_through() {
        let _g = env_lock();
        let v = payload(
            "web_fetch",
            serde_json::json!([
                {"text": big_serp_text()},
                {"inlineData": {"mimeType": "image/png", "data": "aGVsbG8="}}
            ]),
            serde_json::json!({"prompt": "p"}),
        );
        assert!(
            gemini_aftertool_decision(&v).is_none(),
            "non-text part ⇒ never destroy multimodal"
        );
    }

    #[test]
    fn kill_switch_env_produces_no_output() {
        let _g = env_lock();
        let v = payload(
            "google_web_search",
            serde_json::json!(big_serp_text()),
            serde_json::json!({"query": "q"}),
        );
        std::env::set_var("TOKENADE_NO_GEMINI_WEB", "1");
        let out = gemini_aftertool_decision(&v);
        std::env::remove_var("TOKENADE_NO_GEMINI_WEB");
        assert!(out.is_none());
        assert!(
            gemini_aftertool_decision(&v).is_some(),
            "fires once cleared"
        );
    }

    #[test]
    fn llm_content_text_shapes() {
        assert_eq!(
            llm_content_text(&serde_json::json!("plain")),
            Some("plain".to_string())
        );
        assert_eq!(
            llm_content_text(&serde_json::json!([{"text": "a"}, {"text": "b"}])),
            Some("ab".to_string())
        );
        assert!(llm_content_text(&serde_json::json!([{"text": "a", "extra": 1}])).is_none());
        assert!(llm_content_text(&serde_json::json!([{"inlineData": {}}])).is_none());
        assert!(llm_content_text(&serde_json::json!([])).is_none());
        assert!(llm_content_text(&serde_json::json!(null)).is_none());
        assert!(llm_content_text(&serde_json::json!({"text": "x"})).is_none());
    }
}
