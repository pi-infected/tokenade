//! **The fediverse and X**: a status, a thread, a post — through an open API.
//!
//! Mastodon, Bluesky, Lemmy and X each publish an anonymous API that returns
//! the text of a message and its replies, where the page delivers a
//! JavaScript shell.
//!
//! They live together because they answer the SAME question — "what did this
//! person say, and what was said back?" — and because the shape of the answer
//! (a status, then its thread) is the same everywhere. What differs is the
//! field names, not what is done with them.

use super::*;

/// Render a Mastodon status + its reply context. `body` = status JSON,
/// then [`AUX_SEP`], then the `/context` JSON (descendants = replies).
pub(crate) fn mastodon_status(body: &str) -> Option<String> {
    let (status_raw, ctx_raw) = body.split_once(AUX_SEP).unwrap_or((body, ""));
    let s: serde_json::Value = serde_json::from_str(status_raw).ok()?;
    let acct = s.pointer("/account/acct").and_then(|a| a.as_str())?;
    let name = s
        .pointer("/account/display_name")
        .and_then(|n| n.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(acct);
    let content = crate::web::fragment_to_markdown(
        s.get("content")
            .and_then(|c| c.as_str())
            .unwrap_or_default(),
    );
    let mut out = format!("# Post by {name} (@{acct})\n");
    let mut meta: Vec<String> = Vec::new();
    if let Some(n) = s.get("favourites_count").and_then(|n| n.as_u64()) {
        meta.push(format!("{n} favs"));
    }
    if let Some(n) = s.get("reblogs_count").and_then(|n| n.as_u64()) {
        meta.push(format!("{n} boosts"));
    }
    if let Some(d) = s.get("created_at").and_then(|d| d.as_str()) {
        meta.push(crate::caps::truncate_at_char_boundary(d, 10).to_string());
    }
    if !meta.is_empty() {
        out.push_str(&format!("> {} — mastodon\n", meta.join(" • ")));
    }
    out.push_str(&format!("\n{content}\n"));
    // Replies (context.descendants), size-capped.
    if let Ok(ctx) = serde_json::from_str::<serde_json::Value>(ctx_raw) {
        if let Some(desc) = ctx.get("descendants").and_then(|d| d.as_array()) {
            let mut len = 0usize;
            let mut wrote = false;
            for r in desc {
                let a = r
                    .pointer("/account/acct")
                    .and_then(|a| a.as_str())
                    .unwrap_or("?");
                let c = crate::web::fragment_to_markdown(
                    r.get("content")
                        .and_then(|c| c.as_str())
                        .unwrap_or_default(),
                );
                if c.is_empty() || len > 12_000 {
                    continue;
                }
                if !wrote {
                    out.push_str("\n## Replies\n\n");
                    wrote = true;
                }
                let e = format!("**@{a}**: {}\n\n", c.replace('\n', " "));
                len += e.chars().count();
                out.push_str(&e);
            }
        }
    }
    Some(out.trim_end().to_string())
}

/// Render a Bluesky thread from `getPostThread` JSON: post + first-level
/// replies (already in the one response).
pub(crate) fn bluesky_thread(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let t = v.get("thread")?;
    let p = t.get("post")?;
    let handle = p.pointer("/author/handle").and_then(|h| h.as_str())?;
    let name = p
        .pointer("/author/displayName")
        .and_then(|n| n.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(handle);
    let text = p
        .pointer("/record/text")
        .and_then(|t| t.as_str())
        .unwrap_or_default();
    let mut out = format!("# Post by {name} (@{handle})\n");
    let mut meta: Vec<String> = Vec::new();
    if let Some(n) = p.get("likeCount").and_then(|n| n.as_u64()) {
        meta.push(format!("{n} likes"));
    }
    if let Some(n) = p.get("replyCount").and_then(|n| n.as_u64()) {
        meta.push(format!("{n} replies"));
    }
    if let Some(d) = p.pointer("/record/createdAt").and_then(|d| d.as_str()) {
        meta.push(crate::caps::truncate_at_char_boundary(d, 10).to_string());
    }
    if !meta.is_empty() {
        out.push_str(&format!("> {} — bsky.app\n", meta.join(" • ")));
    }
    out.push_str(&format!("\n{}\n", text.trim()));
    if let Some(replies) = t.get("replies").and_then(|r| r.as_array()) {
        let mut len = 0usize;
        let mut wrote = false;
        for r in replies {
            let a = r
                .pointer("/post/author/handle")
                .and_then(|h| h.as_str())
                .unwrap_or("?");
            let c = r
                .pointer("/post/record/text")
                .and_then(|t| t.as_str())
                .unwrap_or_default();
            if c.trim().is_empty() || len > 12_000 {
                continue;
            }
            if !wrote {
                out.push_str("\n## Replies\n\n");
                wrote = true;
            }
            let e = format!("**@{a}**: {}\n\n", c.replace('\n', " "));
            len += e.chars().count();
            out.push_str(&e);
        }
    }
    Some(out.trim_end().to_string())
}

/// Render a Lemmy post + top comments. `body` = post JSON, [`AUX_SEP`],
/// then comment-list JSON.
pub(crate) fn lemmy_post(body: &str) -> Option<String> {
    let (post_raw, comments_raw) = body.split_once(AUX_SEP).unwrap_or((body, ""));
    let v: serde_json::Value = serde_json::from_str(post_raw).ok()?;
    let pv = v.get("post_view")?;
    let title = pv.pointer("/post/name").and_then(|t| t.as_str())?;
    let mut out = format!("# {title}\n");
    let mut meta: Vec<String> = Vec::new();
    if let Some(c) = pv.pointer("/creator/name").and_then(|n| n.as_str()) {
        meta.push(format!("by {c}"));
    }
    if let Some(comm) = pv.pointer("/community/name").and_then(|n| n.as_str()) {
        meta.push(format!("c/{comm}"));
    }
    if let Some(n) = pv.pointer("/counts/score").and_then(|n| n.as_i64()) {
        meta.push(format!("{n} pts"));
    }
    if let Some(n) = pv.pointer("/counts/comments").and_then(|n| n.as_u64()) {
        meta.push(format!("{n} comments"));
    }
    if !meta.is_empty() {
        out.push_str(&format!("> {} — lemmy\n", meta.join(" • ")));
    }
    // Post body (markdown already).
    if let Some(b) = pv.pointer("/post/body").and_then(|b| b.as_str()) {
        if !b.trim().is_empty() {
            out.push_str(&format!("\n{}\n", b.trim()));
        }
    }
    if let Ok(cl) = serde_json::from_str::<serde_json::Value>(comments_raw) {
        if let Some(comments) = cl.get("comments").and_then(|c| c.as_array()) {
            let mut len = 0usize;
            let mut wrote = false;
            for cv in comments {
                let a = cv
                    .pointer("/creator/name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("?");
                let txt = cv
                    .pointer("/comment/content")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default();
                let score = cv
                    .pointer("/counts/score")
                    .and_then(|s| s.as_i64())
                    .unwrap_or(0);
                if txt.trim().is_empty() || len > 16_000 {
                    continue;
                }
                if !wrote {
                    out.push_str("\n## Comments\n\n");
                    wrote = true;
                }
                let e = format!("**{a}** ({score} pts): {}\n\n", txt.replace('\n', " "));
                len += e.chars().count();
                out.push_str(&e);
            }
        }
    }
    Some(out.trim_end().to_string())
}

/// Render an X/Twitter post from the syndication `tweet-result` JSON.
pub(crate) fn x_post(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let text = v.get("text").and_then(|t| t.as_str())?;
    let user = v.get("user");
    let name = user
        .and_then(|u| u.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("");
    let handle = user
        .and_then(|u| u.get("screen_name"))
        .and_then(|n| n.as_str())
        .unwrap_or("");
    let mut out = format!("# Post by {name} (@{handle})\n");
    let mut meta: Vec<String> = Vec::new();
    if let Some(n) = v.get("favorite_count").and_then(|n| n.as_u64()) {
        meta.push(format!("{n} likes"));
    }
    if let Some(n) = v.get("conversation_count").and_then(|n| n.as_u64()) {
        meta.push(format!("{n} replies"));
    }
    if let Some(d) = v.get("created_at").and_then(|d| d.as_str()) {
        meta.push(crate::caps::truncate_at_char_boundary(d, 10).to_string());
    }
    if !meta.is_empty() {
        out.push_str(&format!("> {} — x.com\n", meta.join(" • ")));
    }
    out.push_str(&format!("\n{}\n", text.trim()));
    // A quoted post, when present, carries context the reply refers to.
    if let Some(q) = v.get("quoted_tweet") {
        let qt = q.get("text").and_then(|t| t.as_str()).unwrap_or("");
        let qn = q
            .pointer("/user/screen_name")
            .and_then(|n| n.as_str())
            .unwrap_or("");
        if !qt.is_empty() {
            out.push_str(&format!("\n> quoting @{qn}: {}\n", qt.trim()));
        }
    }
    out.push_str("\n(Post shown; replies are not available without authentication.)\n");
    Some(out.trim_end().to_string())
}
