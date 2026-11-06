//! **Rendering results, and saying when none were recognised.**
//!
//! A degraded walk that serves numbered anchors as if they were results, with
//! exit code 0, reads like a successful search. What lives here must therefore
//! SAY it in the body: an absence asserts nothing, and only the delivered text
//! can assert it.

use super::*;

/// Percent-encode a query for a URL query-string value (space → `+`, RFC 3986
/// unreserved kept verbatim). std-only, mirrors the crate's no-new-deps posture.
pub fn urlencode_query(q: &str) -> String {
    let mut out = String::with_capacity(q.len() * 3);
    for b in q.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// **A list of anchors is not a list of results, and nothing said so.**
///
/// `tokenade search <file.html>` serves the saved page. When no engine is
/// recognised, [`extract_saved_page`] falls back to the generic anchor walk —
/// and its output is numbered exactly like an engine's. MEASURED 2026-09-09 on
/// an arbitrary documentation page: `tokenade search docsrs.html` returned
/// "1. Apache-2.0 …  2. Privacy policy …", with exit code 0 and not a word to
/// say these are the page's links.
///
/// This is the silent fallback this module already documents for engines
/// ("yahoo and mojeek were returning their own navigation bar, and every test
/// was green"): the only signal was the ABSENCE of the line "parsed as a X
/// results page", and an absence asserts nothing.
///
/// Nothing to say when an engine was recognised, nor when the list is empty:
/// the message of [`render`] already names that wall.
pub fn unrecognised_saved_page_note(
    source: &str,
    engine: Option<Engine>,
    hits: usize,
) -> Option<String> {
    if engine.is_some() || hits == 0 {
        return None;
    }
    Some(format!(
        "[tokenade] {source}: no engine we know produced this page, so the {} below are the \
         page's OWN links, not results a search engine returned.",
        crate::markdown::counted(hits, "link")
    ))
}

/// Render an outcome as the markdown both channels serve.
pub fn render(query: &str, o: &Outcome) -> String {
    let mut s = String::new();
    for (i, h) in o.results.iter().enumerate() {
        s.push_str(&format!("{}. {}\n   {}\n", i + 1, h.title, h.url));
        if !h.snippet.is_empty() {
            s.push_str(&format!("   {}\n", h.snippet));
        }
    }
    if s.is_empty() {
        // Say WHICH wall, not "no results". An agent that is told the engines
        // refused us will search natively; one told "nothing found" concludes
        // the web is empty on the subject and moves on.
        //
        // And name the wall that was actually hit. "no engine was reachable"
        // was the answer to every empty outcome, including two where it is
        // false. `tokenade search <file|->` compacts a SAVED results page and
        // queries nobody: a page this build could not parse came back
        // `no usable results for "saved page" — no engine was reachable`,
        // which sends the agent to look at its network for a page it is
        // holding in its hand. And an engine that answers with zero hits
        // leaves `used` full and `refused` empty, so a reachable engine was
        // reported as unreachable.
        let why = if !o.refused.is_empty() {
            o.refused
                .iter()
                .map(|(e, r)| format!("{e}: {}", r.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        } else if !o.used.is_empty() {
            format!("nothing came back from {}", o.used.join(", "))
        } else {
            "no engine was queried".to_string()
        };
        s = format!("no usable results for {query:?} — {why}\n");
    }
    // A refusal is a wall, not an answer: the agent must go to its own search
    // rather than stall or conclude the web is empty. This used to be said
    // only when every breaker was ALREADY open (`RateLimited`); the search
    // that first hits the wall — every engine answering with a challenge, a
    // 403 or a timeout — said "no usable results" and nothing else, which is
    // exactly the moment an agent that has been searching hard needs the
    // way out. Also when a few hits came back, under the usable bar.
    if !o.is_usable() && !o.refused.is_empty() {
        s.push_str(
            if o.refused.iter().any(|(_, r)| *r == Refusal::RateLimited) {
                "all engines are rate-limited or busy right now — use your native web search \
             for this query; `tokenade search` recovers on its own in a few minutes.\n"
            } else {
                "the engines refused or failed this search — use your native web search for this \
             query; `tokenade search` recovers on its own in a few minutes.\n"
            },
        );
    }
    s
}
