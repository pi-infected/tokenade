//! Shared outbound-HTTP helpers: proxy resolution + the ONE in-process HTTP
//! client the whole tree uses (reqwest + rustls).
//!
//! The web bricks and the infrastructure paths (updater,
//! model fetch, the LLM judge and the light agents) all make their requests
//! through the reqwest helpers below, so there is exactly one HTTP client in the
//! tree. [`resolve_proxy`] mirrors the semantics of `npm/install.js`'s
//! `proxyFor` EXACTLY so the CLI and the npm installer make the same routing
//! decision; every client built here reads it, so a corporate-proxy user keeps
//! working on all paths.
//!
//! Control surface is the standard env only — `HTTPS_PROXY`/`HTTP_PROXY`/
//! `ALL_PROXY` (+ lowercase) select, `NO_PROXY` bypasses. There is no
//! tokenade-specific variable. Fail-open + never panic: an unparseable target
//! or proxy yields a direct connection, and credentials are never logged.

/// First present, non-empty value among `keys`. Mirrors JS `A || B` falsiness,
/// where an empty string is falsy and falls through to the next key.
fn env_any(keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Ok(v) = std::env::var(k) {
            if !v.trim().is_empty() {
                return Some(v);
            }
        }
    }
    None
}

// The five ADDRESS primitives moved to `tk_base::url`: they made no request.
// Reading a `scheme://host`, recognising an internal address, climbing up to
// the registrable domain — that is string parsing, and it is what forced the
// rendering of an RSS feed to name the web layer. `resolve_proxy` below calls
// them going downwards, like everyone else.

/// Resolve the proxy URL that applies to `target_url`, honouring the standard
/// env vars (both cases). Returns `None` when no proxy applies — including when
/// `NO_PROXY` matches. Mirrors `npm/install.js` `proxyFor`.
///
/// Exposed for the `curl`-subprocess call sites (e.g. the WebSearch gate):
/// curl honours most of these env vars itself but reads `HTTP_PROXY` in
/// lowercase only, so resolving here and passing `--proxy` explicitly keeps
/// every outbound path on the SAME routing decision as the rest of the CLI.
pub fn resolve_proxy(target_url: &str) -> Option<String> {
    let (scheme, host) = crate::url::scheme_host(target_url)?;

    // NO_PROXY: comma list; `*` bypasses all; leading-dot / exact host suffix.
    if let Some(no_proxy) = env_any(&["NO_PROXY", "no_proxy"]) {
        for raw in no_proxy.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if raw == "*" {
                return None;
            }
            let entry = raw.trim_start_matches('.').to_ascii_lowercase();
            if host == entry || host.ends_with(&format!(".{entry}")) {
                return None;
            }
        }
    }

    // Scheme-specific proxy, then ALL_PROXY as the catch-all.
    let scheme_choice = if scheme == "https" {
        env_any(&["HTTPS_PROXY", "https_proxy"])
    } else {
        env_any(&["HTTP_PROXY", "http_proxy"])
    };
    scheme_choice.or_else(|| env_any(&["ALL_PROXY", "all_proxy"]))
}

/// Mask any `user:pass@` credentials in a proxy URL so it's safe to log.
pub(crate) fn redact_proxy(proxy_url: &str) -> String {
    let Some((scheme, rest)) = proxy_url.split_once("://") else {
        return proxy_url.to_string();
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    match authority.rsplit_once('@') {
        Some((_creds, hostport)) => format!("{scheme}://***@{hostport}"),
        None => proxy_url.to_string(),
    }
}

// ── The shared reqwest infra client (Stage 5) ────────────────────────────────
//
// The web bricks migrated to reqwest first (the risky, high-traffic part); the
// infrastructure paths — updater, model download, the LLM judge and the
// light agents — reroute onto the SAME in-process client here, so the tree ends
// with exactly ONE HTTP client and both curl and ureq can be removed. Every path
// reads the SAME `resolve_proxy` above, so a corporate-proxy user keeps working
// on all of them.

use std::time::Duration;

/// A buffered infra HTTP response: final status + the whole body. Infra
/// responses are small (JSON) or written straight to disk (downloads use
/// [`http_get_stream`] instead), so buffering here is fine.
/// Ceiling on a body read whole into memory by [`http_get`].
///
/// Its callers fetch WEB PAGES — `scout`'s page reader, the agent's fetch
/// tool, a health probe — and the page path next door already drains under
/// 5–10 MB. Ten leaves every real page untouched (`js_render` refuses to run
/// past 3.5 MB of HTML) while putting a bound between a hostile or merely
/// broken origin and the process. A caller that genuinely needs more asks for
/// it with [`http_get_capped`], or streams with [`http_get_stream`].
pub const BUFFERED_BODY_CAP: usize = 10 * 1024 * 1024;

pub struct Resp {
    pub status: u16,
    pub body: Vec<u8>,
    /// The drain stopped at [`BUFFERED_BODY_CAP`] (or the caller's own cap)
    /// before the body ended, so `body` is a prefix and not the whole answer.
    pub truncated: bool,
}

impl Resp {
    /// A 2xx.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        serde_json::from_slice(&self.body).ok()
    }
}

/// Build an infra reqwest client for `url`: the shared proxy, a 10 s connect
/// deadline and a per-request read `timeout`.
/// A client for a LARGE streaming download, where the deadline that matters is
/// per-read and not per-request.
///
/// `ClientBuilder::timeout` is a TOTAL deadline — connect through last body
/// byte — and `updater.rs` believed it was per-read, in a comment that said so
/// ("a 120-second per-read deadline … while still allowing large binary
/// downloads (~130 MB) on slow or distant links"). It never was. 130 MB inside
/// 120 s demands 1.08 MB/s sustained, so anyone slower than that could not
/// upgrade at all. Reported from the field 2026-06-10 on macOS:
/// `✗ Upgrade failed: http body: timed out reading response`.
///
/// The history is in that same comment and it is the same mistake twice: the
/// total deadline had already been raised from 10 s after it truncated
/// downloads into a "size mismatch". Raising the number made the wall further
/// away; it did not stop it being a wall.
///
/// `read_timeout` is the real per-read knob: it fires when no byte arrives for
/// that long, so a stalled connection still dies while a slow-but-moving one
/// finishes. `hard_cap` stays as an absolute backstop — without one, a server
/// dripping a byte every `per_read - 1` seconds holds the process forever.
fn streaming_client(
    url: &str,
    per_read: Duration,
    hard_cap: Duration,
    headers: &[(&str, &str)],
) -> reqwest::Client {
    let mut b = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(per_read)
        .timeout(hard_cap)
        .redirect(redirect_policy(headers))
        .gzip(true);
    if let Some(proxy_url) = resolve_proxy(url) {
        if let Ok(proxy) = reqwest::Proxy::all(&proxy_url) {
            b = b.proxy(proxy);
        }
    }
    b.build().unwrap_or_else(|_| reqwest::Client::new())
}

/// Absolute ceiling on a streaming download, whatever the per-read deadline.
/// Generous on purpose — it exists so nothing hangs forever, not to bound a
/// slow link: 30 minutes is ~72 kB/s for a 130 MB binary, well under any
/// connection that could plausibly finish at all.
const STREAM_HARD_CAP: Duration = Duration::from_secs(30 * 60);

/// Does this request carry a credential the redirect target must never see?
///
/// reqwest strips `Authorization`, `Cookie` and `Proxy-Authorization` on a
/// cross-host redirect, and NOTHING else — so `x-api-key`, the header
/// Anthropic, Azure OpenAI and Google actually use, rides the 3xx to wherever
/// it points. This product sends exactly that header: `judge.rs` posts the
/// user's `ANTHROPIC_API_KEY`, and the LLM proxy forwards the agent's own
/// provider headers upstream.
///
/// Matched on MARKERS inside the name rather than an exact list, because the
/// spelling varies per vendor (`x-api-key`, `api-key`, `x-goog-api-key`) and a
/// list of exact names is the shape that drifts. A false positive costs one
/// thing only: that request stops following redirects, which for an API call
/// is the behaviour we want anyway.
fn carries_credential(headers: &[(&str, &str)]) -> bool {
    const MARKERS: [&str; 6] = [
        "authorization",
        "api-key",
        "api_key",
        "auth-token",
        "access-token",
        "secret",
    ];
    headers.iter().any(|(name, _)| {
        let n = name.to_ascii_lowercase();
        n == "cookie" || MARKERS.iter().any(|m| n.contains(m))
    })
}

/// Refuse to follow a redirect when the request carries a credential.
///
/// A 3xx from a JSON API is not a legitimate answer, and every caller here
/// already gates on the status — so returning the 3xx fails cleanly, while
/// following it would deliver the key to the new host. Credential-free
/// requests keep the default policy: the download paths legitimately redirect
/// to a CDN, and turning that off would break them.
fn redirect_policy(headers: &[(&str, &str)]) -> reqwest::redirect::Policy {
    if carries_credential(headers) {
        reqwest::redirect::Policy::none()
    } else {
        reqwest::redirect::Policy::default()
    }
}

fn infra_client(url: &str, timeout: Duration, headers: &[(&str, &str)]) -> reqwest::Client {
    let mut b = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .redirect(redirect_policy(headers))
        .gzip(true);
    if let Some(proxy_url) = resolve_proxy(url) {
        if let Ok(proxy) = reqwest::Proxy::all(&proxy_url) {
            crate::debug_log::log(
                "net",
                "proxy",
                serde_json::json!({ "target": url, "proxy": redact_proxy(&proxy_url) }),
            );
            b = b.proxy(proxy);
        }
    }
    b.build().unwrap_or_else(|_| reqwest::Client::new())
}

/// Read a response body into memory, stopping at `cap`.
///
/// Returns the bytes and whether the drain cut the body short. reqwest
/// decompresses before `chunk()` yields, so the cap bounds what is
/// DECOMPRESSED — the only figure that bounds the allocation. Breaking out
/// also stops the decompression, rather than paying for it and discarding the
/// tail.
///
/// One drain for both buffered readers: they had the same loop twice, and the
/// half that reported a cut inside the final chunk was present in one copy and
/// missing from the other.
async fn drain_capped(resp: &mut reqwest::Response, cap: usize) -> (Vec<u8>, bool) {
    let mut body = Vec::new();
    let mut truncated = false;
    while let Ok(Some(chunk)) = resp.chunk().await {
        let room = cap.saturating_sub(body.len());
        if room == 0 {
            truncated = true;
            break;
        }
        let take = room.min(chunk.len());
        body.extend_from_slice(&chunk[..take]);
        // The cut can land inside the LAST chunk, and then no further read
        // happens to notice it: without this the body came back short and
        // called itself whole.
        if take < chunk.len() {
            truncated = true;
            break;
        }
    }
    (body, truncated)
}

async fn send(rb: reqwest::RequestBuilder, headers: &[(&str, &str)]) -> Option<Resp> {
    let mut rb = rb;
    for (k, v) in headers {
        rb = rb.header(*k, *v);
    }
    let mut resp = rb.send().await.ok()?;
    let status = resp.status().as_u16();
    // `resp.bytes()` materialises the WHOLE body, and reqwest gunzips before
    // handing it over, so a small download could become an arbitrarily large
    // allocation. Measured against a local server: 65 251 bytes on the wire
    // decompressed to 67 108 864 in memory, an amplification of 1 028x, with
    // nothing between it and the process. `web_fetch` has drained its page
    // bodies under a cap all along; this helper — which `scout` and the
    // agent's fetch tool point at arbitrary URLs — did not.
    let (body, truncated) = drain_capped(&mut resp, BUFFERED_BODY_CAP).await;
    if truncated {
        // A cap that trims in silence reads as a complete answer. Say so where
        // the fetch happened, so a short page is never mistaken for the page.
        crate::debug_log::log(
            "net",
            "body_capped",
            serde_json::json!({ "cap": BUFFERED_BODY_CAP, "kept": body.len() }),
        );
    }
    Some(Resp {
        status,
        body,
        truncated,
    })
}

/// GET `url`, buffered. `None` on transport failure; a non-2xx still returns a
/// [`Resp`] (the caller inspects `status`, mirroring ureq's `Error::Status`).
pub fn http_get(url: &str, headers: &[(&str, &str)], timeout_secs: u64) -> Option<Resp> {
    let client = infra_client(url, Duration::from_secs(timeout_secs), headers);
    crate::runtime::block_on(send(client.get(url), headers))
}

/// POST a JSON body to `url`, buffered.
pub fn http_post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
    timeout_secs: u64,
) -> Option<Resp> {
    let client = infra_client(url, Duration::from_secs(timeout_secs), headers);
    let rb = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(body).ok()?);
    crate::runtime::block_on(send(rb, headers))
}

/// POST raw `body` with an explicit `content_type`.
pub fn http_post_bytes(
    url: &str,
    headers: &[(&str, &str)],
    content_type: &str,
    body: Vec<u8>,
    timeout_secs: u64,
) -> Option<Resp> {
    let client = infra_client(url, Duration::from_secs(timeout_secs), headers);
    let rb = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .body(body);
    crate::runtime::block_on(send(rb, headers))
}

async fn stream_to(
    rb: reqwest::RequestBuilder,
    headers: &[(&str, &str)],
    sink: &mut dyn std::io::Write,
) -> Option<u16> {
    let mut rb = rb;
    for (k, v) in headers {
        rb = rb.header(*k, *v);
    }
    let mut resp = rb.send().await.ok()?;
    let status = resp.status().as_u16();
    while let Ok(Some(chunk)) = resp.chunk().await {
        if sink.write_all(&chunk).is_err() {
            return None;
        }
    }
    Some(status)
}

/// GET `url`, buffered but bounded at `cap` bytes (the updater's manifest and
/// signed binary — a pre-verification memory bound so a compromised endpoint
/// cannot stream unbounded before the signature/hash is even checked). Stops
/// draining once `cap` is reached, mirroring ureq's `.take(cap)`.
/// A conditional-GET response: the status, the body (empty on 304), and the
/// validator to store for next time.
pub struct CondResp {
    /// The drain stopped at the caller's cap before the body ended.
    pub truncated: bool,
    pub status: u16,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

impl CondResp {
    pub fn not_modified(&self) -> bool {
        self.status == 304
    }
}

/// `GET` with `If-None-Match` / `If-Modified-Since`, returning the response
/// validators so the caller can store them.
///
/// Separate from [`http_get_capped`] rather than widening `Resp`: `Resp` is
/// constructed at several sites and none of them wants header plumbing. A
/// caller polling a large file daily does — a 304 costs a few hundred bytes
/// where an unconditional GET costs the whole file.
pub fn http_get_conditional(
    url: &str,
    timeout_secs: u64,
    cap: usize,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Option<CondResp> {
    // No credential: the only headers this sends are `If-None-Match` and
    // `If-Modified-Since`, so the cost table may live behind a CDN redirect.
    let client = infra_client(url, Duration::from_secs(timeout_secs), &[]);
    let etag = etag.map(str::to_string);
    let last_modified = last_modified.map(str::to_string);
    crate::runtime::block_on(async move {
        let mut rb = client.get(url);
        if let Some(t) = &etag {
            rb = rb.header("If-None-Match", t);
        }
        if let Some(t) = &last_modified {
            rb = rb.header("If-Modified-Since", t);
        }
        let mut resp = rb.send().await.ok()?;
        let status = resp.status().as_u16();
        let hdr = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let new_etag = hdr("etag");
        let new_lm = hdr("last-modified");
        // A 304 carries no body by definition, so nothing was cut either.
        let (body, truncated) = if status == 304 {
            (Vec::new(), false)
        } else {
            drain_capped(&mut resp, cap).await
        };
        Some(CondResp {
            status,
            body,
            truncated,
            etag: new_etag,
            last_modified: new_lm,
        })
    })
}

pub fn http_get_capped(
    url: &str,
    headers: &[(&str, &str)],
    timeout_secs: u64,
    cap: usize,
) -> Option<Resp> {
    // `timeout_secs` is the PER-READ deadline here, not a total one: this is
    // the capped streaming reader, and its only production caller pulls a
    // ~130 MB binary. See `streaming_client`.
    let client = streaming_client(
        url,
        Duration::from_secs(timeout_secs),
        STREAM_HARD_CAP,
        headers,
    );
    let headers: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    crate::runtime::block_on(async move {
        let mut rb = client.get(url);
        for (k, v) in &headers {
            rb = rb.header(k, v);
        }
        let mut resp = rb.send().await.ok()?;
        let status = resp.status().as_u16();
        let (body, truncated) = drain_capped(&mut resp, cap).await;
        Some(Resp {
            status,
            body,
            truncated,
        })
    })
}

/// How long a streaming download may hear NOTHING before it is abandoned.
///
/// `timeout_secs` is the caller's TOTAL budget and has to be generous — the
/// model this pulls is ~700 MB. Without a per-read deadline that generosity
/// also buys a dead connection the whole budget: a server that accepts,
/// answers, and then sends no byte at all held the download for the full hour
/// before the caller's retry loop tried again, four times over.
const STREAM_STALL_DEADLINE: Duration = Duration::from_secs(120);

/// [`STREAM_STALL_DEADLINE`], with `TOKENADE_STREAM_STALL_SECS` as an override
/// for a link whose normal behaviour includes long silences.
fn stream_stall_deadline() -> Duration {
    std::env::var("TOKENADE_STREAM_STALL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .map(Duration::from_secs)
        .unwrap_or(STREAM_STALL_DEADLINE)
}

/// GET `url`, streaming the body into `sink` (a large binary download — the
/// updater's signed binary, a model tarball). Returns the HTTP status. The
/// caller verifies the bytes (Ed25519 / hash) after, so no size cap is imposed.
///
/// `timeout_secs` is the TOTAL budget, connect through last byte. A stall is
/// caught much sooner by [`STREAM_STALL_DEADLINE`]: the reason
/// [`streaming_client`] exists is that a total deadline alone cannot tell a
/// dead link from a slow one, and this function — the one that actually pulls
/// the large binary its doc describes — was still building the plain
/// [`infra_client`], which sets only the total.
pub fn http_get_stream(
    url: &str,
    headers: &[(&str, &str)],
    timeout_secs: u64,
    sink: &mut dyn std::io::Write,
) -> Option<u16> {
    let client = streaming_client(
        url,
        stream_stall_deadline(),
        Duration::from_secs(timeout_secs),
        headers,
    );
    crate::runtime::block_on(stream_to(client.get(url), headers, sink))
}

/// Everything about a streamed response that must be relayed BEFORE its body.
pub struct StreamHead {
    pub status: u16,
    /// Response headers in arrival order, names lowercased by the HTTP stack.
    pub headers: Vec<(String, String)>,
}

/// Where a streamed response goes as it arrives.
///
/// A trait rather than two closures because both halves write to the SAME
/// destination, and two `FnMut`s cannot hold one `&mut` between them.
pub trait StreamSink {
    /// The response head, before any body byte. Return `false` to abandon.
    fn head(&mut self, head: StreamHead) -> bool;
    /// One chunk, as it lands. Return `false` to abandon.
    fn chunk(&mut self, bytes: &[u8]) -> bool;
}

/// Send `body` to `url` with `method` and hand the response back INCREMENTALLY.
///
/// The one primitive [`http_get_stream`] cannot provide and the LLM proxy
/// cannot do without: a request with headers and a body, whose response head
/// arrives before its body, and whose body is delivered chunk by chunk as it
/// lands. Returning `false` from either sink method abandons the transfer — the
/// proxy uses that when its own client hangs up, so a dead agent stops us
/// paying to finish downloading an answer nobody will read.
///
/// The METHOD is the caller's: the proxy once relayed every request as a POST,
/// so Codex's `GET /models` came back 405 and its model list never refreshed.
///
/// **The timeouts differ from every other call in this module on purpose.**
/// [`infra_client`] sets `.timeout()`, a deadline on the WHOLE request, which
/// is right for fetching a manifest and catastrophic here: a model that thinks
/// for two minutes before answering would be cut off mid-sentence. What must be
/// bounded is silence, not duration, so this uses `read_timeout` — the gap
/// between two bytes.
pub fn http_stream(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
    connect_timeout_secs: u64,
    idle_timeout_secs: u64,
    sink: &mut dyn StreamSink,
) -> Result<(), String> {
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
    let mut b = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(connect_timeout_secs))
        .read_timeout(Duration::from_secs(idle_timeout_secs))
        // The LLM proxy relays the AGENT's own provider headers upstream, so
        // this is the request most likely to carry a key.
        .redirect(redirect_policy(headers))
        .gzip(true);
    if let Some(proxy_url) = resolve_proxy(url) {
        if let Ok(proxy) = reqwest::Proxy::all(&proxy_url) {
            b = b.proxy(proxy);
        }
    }
    let client = b.build().map_err(|e| redact_net_error(&e.to_string()))?;

    crate::runtime::block_on(async move {
        let bodiless =
            matches!(method, reqwest::Method::GET | reqwest::Method::HEAD) && body.is_empty();
        let mut rb = client.request(method, url);
        if !bodiless {
            rb = rb.body(body);
        }
        for (k, v) in headers {
            rb = rb.header(*k, *v);
        }
        let resp = rb
            .send()
            .await
            .map_err(|e| redact_net_error(&e.to_string()))?;
        let head = StreamHead {
            status: resp.status().as_u16(),
            headers: resp
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_string(),
                        v.to_str().unwrap_or_default().to_string(),
                    )
                })
                .collect(),
        };
        if !sink.head(head) {
            return Ok(());
        }
        let mut resp = resp;
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    if !sink.chunk(&chunk) {
                        return Ok(());
                    }
                }
                Ok(None) => return Ok(()),
                Err(e) => return Err(redact_net_error(&e.to_string())),
            }
        }
    })
}

/// Make a transport error safe to show and to log.
///
/// `reqwest::Error::to_string()` embeds the URL, and a provider URL is not
/// always secret-free — Gemini takes its key as `?key=…`. This is the boundary
/// where that string first exists, so it is the boundary that scrubs it.
fn redact_net_error(e: &str) -> String {
    crate::redact::redact(e)
}

/// The body of a GET, capped, with the status checked — `Err` instead of
/// `None` so the caller can say WHY.
///
/// It lived in `updater`, the first client to need it, and `model_fetch`
/// therefore climbed up there to download an embeddings model: three sites
/// that made the code layer depend on a product floor. This module says of
/// itself that it is "the ONE in-process HTTP client the whole tree uses" and
/// names `updater` and `model fetch` among its clients; that was true for the
/// transport and false for this wrapper.
pub fn get_body_capped(url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    // The streaming client (10 s connect, 120 s PER READ), proxy-aware via
    // the same `resolve_proxy` every path uses. Bounded at `max_bytes` — a
    // pre-verification memory cap; the caller checks size + sha256 after.
    let resp = http_get_capped(url, &[], 120, max_bytes)
        .ok_or_else(|| format!("http {url}: request failed"))?;
    if !resp.ok() {
        return Err(format!("http {url}: status {}", resp.status));
    }
    Ok(resp.body)
}
#[cfg(test)]
include!("net_tests.rs");
