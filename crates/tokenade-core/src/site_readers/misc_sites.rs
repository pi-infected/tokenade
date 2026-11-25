//! Single-site readers with nothing else in their family: Hugging Face,
//! TikTok, Hacker News, Discourse.

use super::*;

/// Render a Hugging Face model or dataset: the API metadata, then the model
/// card (README) after [`AUX_SEP`]. The card is the documentation; the API
/// object carries what the card never states — downloads, likes, pipeline.
pub(super) fn huggingface_repo(body: &str) -> Option<String> {
    let (head, card) = match body.split_once(AUX_SEP) {
        Some((h, c)) => (h, c),
        None => (body, ""),
    };
    let v: serde_json::Value = serde_json::from_str(head.trim()).ok()?;
    let id = v.get("id")?.as_str()?;
    let mut out = format!("# {id}");
    let mut facts: Vec<String> = Vec::new();
    if let Some(p) = v.get("pipeline_tag").and_then(|p| p.as_str()) {
        facts.push(p.to_string());
    }
    for (key, label) in [("downloads", "downloads"), ("likes", "likes")] {
        if let Some(n) = v.get(key).and_then(|n| n.as_u64()) {
            facts.push(format!("{n} {label}"));
        }
    }
    if let Some(l) = v
        .get("cardData")
        .and_then(|c| c.get("license"))
        .and_then(|l| l.as_str())
    {
        facts.push(format!("license: {l}"));
    }
    if !facts.is_empty() {
        out.push_str(&format!("\n\n{}", facts.join(" · ")));
    }
    if let Some(tags) = v.get("tags").and_then(|t| t.as_array()) {
        let names: Vec<&str> = tags.iter().filter_map(|t| t.as_str()).take(12).collect();
        if !names.is_empty() {
            out.push_str(&format!("\n\ntags: {}", names.join(", ")));
        }
    }
    let card = card.trim();
    if !card.is_empty() {
        // Strip the YAML front matter: its fields are already in the API
        // object above, in a form we can label.
        let card = match card.strip_prefix("---") {
            Some(rest) => match rest.find("\n---") {
                Some(i) => rest[i + 4..].trim_start(),
                None => card,
            },
            None => card,
        };
        out.push_str(&format!("\n\n{card}"));
    }
    Some(out)
}

/// Render a TikTok video or profile page from `__UNIVERSAL_DATA_FOR_REHYDRATION__`.
///
/// The DOM carries nothing — MEASURED, zero extractable characters from 376 KB
/// — because every visible string is written by the client from this payload.
/// Reading it directly is the only HTTP-only way to see the page at all.
pub(super) fn tiktok_page(html: &str) -> Option<String> {
    let raw = re(r#"(?is)<script id="__UNIVERSAL_DATA_FOR_REHYDRATION__"[^>]*>(.*?)</script>"#)
        .captures(html)?
        .get(1)?
        .as_str();
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    let scope = v.get("__DEFAULT_SCOPE__")?;
    let s = |v: &serde_json::Value, p: &str| -> Option<String> {
        let x = v.pointer(p)?.as_str()?.trim().to_string();
        (!x.is_empty()).then_some(x)
    };
    // TikTok answers 200 with a full-size shell for a deleted or private post
    // and marks it only here, so a non-zero status must fail the reader rather
    // than render an empty page as if it were the video.
    let ok = |section: &str| {
        scope
            .pointer(&format!("/{section}/statusCode"))
            .and_then(|c| c.as_i64())
            .unwrap_or(0)
            == 0
    };
    // Counts read as "11600000 plays" otherwise, which is unreadable at a
    // glance and costs more tokens than the rounded form.
    let count = |v: &serde_json::Value, p: &str, label: &str| -> Option<String> {
        let n = v.pointer(p).and_then(|x| {
            x.as_u64()
                .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
        })?;
        Some(match n {
            0 => return None,
            n if n >= 1_000_000 => format!("{:.1}M {label}", n as f64 / 1e6),
            n if n >= 1_000 => format!("{:.1}k {label}", n as f64 / 1e3),
            n => format!("{n} {label}"),
        })
    };
    let date = |v: &serde_json::Value, p: &str| -> Option<String> {
        // Epoch seconds ship as a JSON number on the video payload and as a
        // decimal STRING on some others; reading only one shape silently
        // dropped the date from every render that used the other.
        let t = v.pointer(p).and_then(|x| {
            x.as_u64()
                .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
        })?;
        Some(
            crate::platform::epoch_to_iso8601_z(t)
                .chars()
                .take(10)
                .collect(),
        )
    };

    if let Some(item) = scope
        .pointer("/webapp.video-detail/itemInfo/itemStruct")
        .filter(|_| ok("webapp.video-detail"))
    {
        let handle = s(item, "/author/uniqueId")?;
        let desc = s(item, "/desc").or_else(|| s(scope, "/webapp.video-detail/shareMeta/desc"));
        // The caption IS the page's content, so it heads the render; a
        // caption-less clip falls back to naming what the page is.
        let caption = desc
            .as_deref()
            .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "));
        let caption = caption.filter(|d| !d.is_empty());
        let head = caption
            .as_deref()
            .map(|d| d.chars().take(120).collect::<String>())
            .unwrap_or_else(|| format!("TikTok video by @{handle}"));
        let mut out = format!("# {head}\n");
        let mut meta = vec![format!("@{handle}")];
        if let Some(n) = s(item, "/author/nickname").filter(|n| *n != handle) {
            meta.push(n);
        }
        meta.extend(date(item, "/createTime"));
        for (p, label) in [
            ("/stats/playCount", "plays"),
            ("/stats/diggCount", "likes"),
            ("/stats/commentCount", "comments"),
            ("/stats/shareCount", "shares"),
        ] {
            meta.extend(count(item, p, label));
        }
        out.push_str(&format!("> {} — tiktok.com\n", meta.join(" • ")));
        // Only when the heading had to cut it: repeating a caption that fits
        // in the title is pure duplication.
        if let Some(d) = caption.filter(|d| d.chars().count() > head.chars().count()) {
            out.push_str(&format!("\n{d}\n"));
        }
        if let Some(bio) = s(item, "/author/signature") {
            out.push_str(&format!("\nAuthor bio: {}\n", bio.replace('\n', " ")));
        }
        let tags: Vec<String> = item
            .get("challenges")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
            .filter_map(|c| c.get("title").and_then(|t| t.as_str()))
            .map(|t| format!("#{t}"))
            .collect();
        if !tags.is_empty() {
            out.push_str(&format!("\nHashtags: {}\n", tags.join(" ")));
        }
        if let Some(m) = s(item, "/music/title") {
            let by = s(item, "/music/authorName")
                .map(|a| format!(" — {a}"))
                .unwrap_or_default();
            out.push_str(&format!("\nSound: {m}{by}\n"));
        }
        // Say what an HTTP-only reader cannot reach, so the agent does not read
        // the absence of comments as "this post has none".
        out.push_str(
            "\n[caption and counts only — TikTok loads the comments and the \
             spoken-word transcript over a separate authenticated request that \
             is not part of this page.]\n",
        );
        return Some(out.trim_end().to_string());
    }

    let info = scope
        .pointer("/webapp.user-detail/userInfo")
        .filter(|_| ok("webapp.user-detail"))?;
    let handle = s(info, "/user/uniqueId")?;
    let name = s(info, "/user/nickname").unwrap_or_else(|| handle.clone());
    let mut out = format!("# {name} (@{handle}) on TikTok\n");
    let mut meta: Vec<String> = Vec::new();
    for (p, label) in [
        ("/stats/followerCount", "followers"),
        ("/stats/followingCount", "following"),
        ("/stats/heartCount", "likes"),
        ("/stats/videoCount", "videos"),
    ] {
        meta.extend(count(info, p, label));
    }
    if !meta.is_empty() {
        out.push_str(&format!("> {} — tiktok.com\n", meta.join(" • ")));
    }
    if let Some(bio) = s(info, "/user/signature") {
        out.push_str(&format!("\n{bio}\n"));
    }
    if let Some(link) = s(info, "/user/bioLink/link") {
        out.push_str(&format!("\n{link}\n"));
    }
    out.push_str(
        "\n[profile only — the video list arrives over a separate request \
         that is not part of this page.]\n",
    );
    Some(out.trim_end().to_string())
}

/// Render an HN Algolia item tree: story header + nested comments.
pub(super) fn hn_item(body: &str) -> Option<String> {
    const COMMENTS_CAP_CHARS: usize = 24_000;
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let title = v.get("title").and_then(|t| t.as_str())?;
    let mut out = format!("# {title}\n");
    let mut meta: Vec<String> = Vec::new();
    if let Some(p) = v.get("points").and_then(|p| p.as_u64()) {
        meta.push(format!("{p} pts"));
    }
    if let Some(a) = v.get("author").and_then(|a| a.as_str()) {
        meta.push(format!("by {a}"));
    }
    if let Some(d) = v.get("created_at").and_then(|d| d.as_str()) {
        meta.push(crate::caps::truncate_at_char_boundary(d, 10).to_string());
    }
    if !meta.is_empty() {
        out.push_str(&format!("> {} — news.ycombinator.com\n", meta.join(" • ")));
    }
    if let Some(u) = v.get("url").and_then(|u| u.as_str()) {
        if !u.is_empty() {
            out.push_str(&format!("{u}\n"));
        }
    }
    if let Some(t) = v.get("text").and_then(|t| t.as_str()) {
        let op = crate::web::fragment_to_markdown(t);
        if !op.is_empty() {
            out.push_str(&format!("\n{op}\n"));
        }
    }
    let children = v.get("children").and_then(|c| c.as_array());
    if children.is_some_and(|c| !c.is_empty()) {
        out.push_str("\n## Comments\n\n");
    }
    let mut comments_len = 0usize;
    let mut dropped = 0usize;
    fn walk(
        nodes: &[serde_json::Value],
        depth: usize,
        out: &mut String,
        len: &mut usize,
        dropped: &mut usize,
        cap: usize,
    ) {
        for n in nodes {
            let text = n.get("text").and_then(|t| t.as_str()).unwrap_or_default();
            let body = crate::web::fragment_to_markdown(text);
            let kids = n.get("children").and_then(|c| c.as_array());
            if !body.is_empty() {
                if *len > cap {
                    *dropped += 1;
                } else {
                    let author = n
                        .get("author")
                        .and_then(|a| a.as_str())
                        .unwrap_or("unknown");
                    // One-line indent per depth keeps the reply structure
                    // without wasting a blockquote pyramid.
                    let indent = "  ".repeat(depth);
                    let entry = format!("{indent}- **{author}**: {}\n", body.replace('\n', " "));
                    *len += entry.chars().count();
                    out.push_str(&entry);
                }
            }
            if let Some(kids) = kids {
                walk(kids, depth + 1, out, len, dropped, cap);
            }
        }
    }
    if let Some(kids) = children {
        walk(
            kids,
            0,
            &mut out,
            &mut comments_len,
            &mut dropped,
            COMMENTS_CAP_CHARS,
        );
    }
    if dropped > 0 {
        out.push_str(&format!(
            "({dropped} more comment(s) beyond the size cap not shown)\n"
        ));
    }
    Some(out.trim_end().to_string())
}

/// Render a Discourse `topic.json`: title + every post in the stream.
pub(super) fn discourse_topic(body: &str) -> Option<String> {
    const POSTS_CAP_CHARS: usize = 24_000;
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let title = v.get("title").and_then(|t| t.as_str())?;
    let posts = v
        .get("post_stream")
        .and_then(|s| s.get("posts"))
        .and_then(|p| p.as_array())?;
    let mut out = format!("# {title}\n");
    if let Some(n) = v.get("posts_count").and_then(|n| n.as_u64()) {
        out.push_str(&format!("> {n} post(s) — discourse\n"));
    }
    let mut len = 0usize;
    let mut dropped = 0usize;
    for (i, p) in posts.iter().enumerate() {
        let cooked = p.get("cooked").and_then(|c| c.as_str()).unwrap_or_default();
        let body_md = crate::web::fragment_to_markdown(cooked);
        if body_md.is_empty() {
            continue;
        }
        if len > POSTS_CAP_CHARS {
            dropped += 1;
            continue;
        }
        let user = p
            .get("username")
            .and_then(|u| u.as_str())
            .unwrap_or("unknown");
        let entry = if i == 0 {
            format!("\n{body_md}\n\n## Replies\n\n")
        } else {
            format!("**{user}**: {body_md}\n\n")
        };
        len += entry.chars().count();
        out.push_str(&entry);
    }
    if dropped > 0 {
        out.push_str(&format!(
            "({dropped} more post(s) beyond the size cap not shown)\n"
        ));
    }
    // A topic with only an OP renders no "## Replies" content — trim the
    // dangling header.
    let out = out
        .trim_end()
        .trim_end_matches("## Replies")
        .trim_end()
        .to_string();
    Some(out)
}

/// Extract the balanced `{…}` object assigned to `<marker> =` in a script
/// (e.g. `ytInitialPlayerResponse`). Regex can't match nested braces; this
/// walks from the first `{` counting depth, skipping string literals. `None`
/// if the marker/opening brace is absent or the object never closes.
pub(super) fn extract_js_object<'a>(html: &'a str, marker: &str) -> Option<&'a str> {
    let start = html.find(marker)?;
    let brace = html[start..].find('{')? + start;
    let bytes = html.as_bytes();
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    let mut quote = b'"';
    for i in brace..bytes.len() {
        let c = bytes[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == quote {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' | b'\'' => {
                in_str = true;
                quote = c;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&html[brace..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) fn tiktok_plan(path: &str) -> Option<Plan> {
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    Some(Plan::get(
        format!("https://www.tiktok.com{clean}"),
        Kind::TikTok,
        "tiktok (rehydration data)",
    ))
}

pub(super) fn twitter_plan(path: &str) -> Option<Plan> {
    let id = re(r"/status(?:es)?/(\d+)").captures(path)?[1].to_string();
    Some(Plan::get(
        format!("https://cdn.syndication.twimg.com/tweet-result?id={id}&token=a&lang=en"),
        Kind::XPost,
        "x.com syndication",
    ))
}

pub(super) fn bluesky_plan(path: &str) -> Option<Plan> {
    let c = re(r"^/profile/([^/]+)/post/([A-Za-z0-9]+)").captures(path)?;
    let (actor, rkey) = (c[1].to_string(), c[2].to_string());
    Some(Plan::get(
        format!(
            "https://public.api.bsky.app/xrpc/app.bsky.feed.getPostThread\
                 ?uri=at://{actor}/app.bsky.feed.post/{rkey}&depth=1"
        ),
        Kind::Bluesky,
        "bsky.app API",
    ))
}

pub(super) fn lemmy_plan(host: &str, path: &str) -> Option<Plan> {
    let id = re(r"^/post/(\d+)").captures(path)?[1].to_string();
    Some(
        Plan::get(
            format!("https://{host}/api/v3/post?id={id}"),
            Kind::Lemmy,
            "lemmy API",
        )
        .with_aux(format!(
            "https://{host}/api/v3/comment/list?post_id={id}&max_depth=3&limit=30&sort=Top"
        )),
    )
}

pub(super) fn mastodon_status_plan(host: &str, path: &str) -> Option<Plan> {
    let id = re(r"^/@[^/]+/(\d{6,})$").captures(path)?[1].to_string();
    Some(
        Plan::get(
            format!("https://{host}/api/v1/statuses/{id}"),
            Kind::Mastodon,
            "mastodon API",
        )
        .with_aux(format!("https://{host}/api/v1/statuses/{id}/context")),
    )
}

pub(super) fn hn_item_plan(path: &str) -> Option<Plan> {
    let id = re(r"[?&]id=(\d+)")
        .captures(path)?
        .get(1)?
        .as_str()
        .to_string();
    Some(Plan::get(
        format!("https://hn.algolia.com/api/v1/items/{id}"),
        Kind::HnItem,
        "hn.algolia.com API",
    ))
}

/// The plan for Stack Exchange sites — stackoverflow.com as well as the
/// `*.stackexchange.com` domains.
///
/// It used to exist under TWO names, `stack_overflow_plan` and
/// `stack_exchange_plan`, with byte-for-byte identical bodies: same
/// `Kind::StackExchange`, same reader label, same query cleanup. Two names for
/// one behaviour, so a fix to one silently missed the other.
pub(super) fn stack_exchange_plan(host: &str, path: &str) -> Option<Plan> {
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    Some(Plan::get(
        format!("https://{host}{clean}"),
        Kind::StackExchange,
        "stackexchange reader",
    ))
}

pub(super) fn indeed_plan(host: &str, path: &str) -> Option<Plan> {
    Some(Plan::get(
        format!("https://{host}{path}"),
        Kind::IndeedJobs,
        "indeed (job cards)",
    ))
}

pub(super) fn discourse_plan(host: &str, path: &str) -> Option<Plan> {
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    // Trim a trailing `/<post-number>` deep link so we fetch the topic.
    let base = re(r"^(/t/[^/]+/\d+)").captures(clean)?[1].to_string();
    Some(Plan::get(
        format!("https://{host}{base}.json"),
        Kind::DiscourseTopic,
        "discourse API",
    ))
}
