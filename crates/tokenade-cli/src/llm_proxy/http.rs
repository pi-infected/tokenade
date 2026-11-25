//! The HTTP/1.1 this proxy actually needs, hand-rolled, and nothing more.
//!
//! Why not a framework: the tree already carries exactly one HTTP client
//! (`reqwest`, workspace comment in `Cargo.toml`) and no other server. Adding an inbound framework would add an async
//! runtime to a synchronous binary for a listener that serves one agent on
//! loopback. The parsing below is the part that has to be right, so it is a
//! pure transformation over a `BufRead` and is tested against byte strings
//! rather than against a socket.
//!
//! The one rule that outranks every other in this file: **whatever we could not
//! understand, we must still be able to relay**. Every failure path here either
//! yields the original bytes or refuses the connection outright — none of them
//! yields a half-parsed request, because a half-parsed request is how a proxy
//! silently truncates somebody's conversation.

use std::io::{BufRead, Write};

/// Header bytes we refuse to accumulate before calling the sender hostile.
/// A real request's headers are a few kilobytes; this is three orders of
/// magnitude of headroom and still bounded.
const MAX_HEADER_BYTES: usize = 256 * 1024;

/// How much of a body we are willing to hand to a JSON parser.
///
/// Not a limit on what we RELAY — see [`Request::body`]. A body above this is
/// still forwarded byte for byte, it simply never becomes a `serde_json::Value`.
/// Parsing is the optional half of this proxy; relaying is not.
pub const PARSE_CAP: usize = 32 * 1024 * 1024;

/// Why a request could not be read.
#[derive(Debug, PartialEq, Eq)]
pub enum ReadError {
    /// The peer closed cleanly before sending anything. On a keep-alive
    /// connection this is the normal end, not a fault.
    Eof,
    /// Something we cannot serve. Carries a reason for the log line ONLY — it
    /// must never be echoed to the client, since a malformed request can embed
    /// a key in its request line.
    Malformed(&'static str),
    /// The socket died mid-request.
    Io,
}

/// One inbound request, fully in hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// The request target as sent: `/v1/messages?beta=true`.
    pub target: String,
    /// Headers in arrival order, names as sent. Order is kept because a
    /// provider may be sensitive to duplicate headers, and re-ordering a
    /// request is a change we have no reason to make.
    pub headers: Vec<(String, String)>,
    /// The body, verbatim. This is the buffer we relay; nothing downstream may
    /// replace it except a rewrite that was positively decided.
    pub body: Vec<u8>,
}

impl Request {
    /// First value for `name`, case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// True when the client asked for a streaming response.
    ///
    /// Read off the BODY, not off `Accept`. Both wires carry `"stream": true`
    /// in the JSON, and an agent that forgets the `Accept: text/event-stream`
    /// header still gets SSE from the provider — so trusting the header would
    /// make us buffer a stream the client is reading incrementally, which is
    /// the exact "agent looks frozen" failure.
    pub fn wants_stream(&self) -> bool {
        // Gemini says it in the URL, not the body:
        // `…:streamGenerateContent?alt=sse`. Read as JSON, its SSE answer
        // failed to parse and its usage was never seen.
        if self.target.contains(":streamGenerateContent") || self.target.contains("alt=sse") {
            return true;
        }
        if self.body.len() > PARSE_CAP {
            return false;
        }
        serde_json::from_slice::<serde_json::Value>(&self.body)
            .ok()
            .and_then(|v| v.get("stream").and_then(serde_json::Value::as_bool))
            .unwrap_or(false)
    }

    /// Whether the connection may be reused after this request.
    pub fn keep_alive(&self) -> bool {
        !self
            .header("connection")
            .is_some_and(|v| v.eq_ignore_ascii_case("close"))
    }
}

/// Headers that describe THIS hop and must not be copied to the next one.
///
/// RFC 9110 §7.6.1. Copying `Transfer-Encoding` is the classic way to hand a
/// peer a body it then decodes twice; copying `Connection` leaks our keep-alive
/// decision into somebody else's.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Should this header be dropped when relaying a REQUEST upstream?
///
/// Beyond hop-by-hop: `host` and `content-length` are re-derived by the client
/// from the URL and the body we hand it, and `accept-encoding` is the client's
/// to negotiate — we would otherwise promise the agent's encoding preference
/// while reqwest transparently decodes to something else.
pub fn drop_from_request(name: &str) -> bool {
    HOP_BY_HOP.iter().any(|h| name.eq_ignore_ascii_case(h))
        || name.eq_ignore_ascii_case("host")
        || name.eq_ignore_ascii_case("content-length")
        || name.eq_ignore_ascii_case("accept-encoding")
}

/// Should this header be dropped when relaying a RESPONSE back to the agent?
///
/// `content-length` and `content-encoding` both describe bytes we no longer
/// have: the client decompressed the body, and we re-frame it as chunked. A
/// relayed `content-length` from a decompressed stream is a promise we break
/// mid-body, which reads to the agent as a truncated answer.
pub fn drop_from_response(name: &str) -> bool {
    HOP_BY_HOP.iter().any(|h| name.eq_ignore_ascii_case(h))
        || name.eq_ignore_ascii_case("content-length")
        || name.eq_ignore_ascii_case("content-encoding")
}

/// Read one request off `r`.
pub fn read_request<R: BufRead>(r: &mut R) -> Result<Request, ReadError> {
    let mut line = String::new();
    match r.read_line(&mut line) {
        Ok(0) => return Err(ReadError::Eof),
        Ok(_) => {}
        Err(_) => return Err(ReadError::Io),
    }
    // A stray CRLF before the request line is explicitly tolerated by RFC 9112
    // §2.2, and some clients emit one after a keep-alive body.
    if line == "\r\n" || line == "\n" {
        line.clear();
        match r.read_line(&mut line) {
            Ok(0) => return Err(ReadError::Eof),
            Ok(_) => {}
            Err(_) => return Err(ReadError::Io),
        }
    }
    let mut parts = line.trim_end().splitn(3, ' ');
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let version = parts.next().unwrap_or_default();
    if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/") {
        return Err(ReadError::Malformed("request line"));
    }
    // ORIGIN-FORM ONLY: the target must begin with `/`.
    //
    // `server.rs` builds the upstream URL as
    // `format!("{}{}", cfg.upstream.trim_end_matches('/'), req.target)`, so a
    // target that does not start with `/` is pasted directly onto the END OF
    // THE HOSTNAME. Measured 2026-08-27 against upstream
    // `https://api.anthropic.com`:
    //
    //   target `@evil.example/v1/messages`  →  host `evil.example`
    //       (`api.anthropic.com` becomes userinfo)
    //   target `.evil.example/x`            →  host `api.anthropic.com.evil.example`
    //       (a suffix the attacker owns in DNS)
    //
    // and this proxy attaches the user's API key to whatever it dials. The
    // listener is loopback-only, so the sender is another process on this
    // machine — a dependency, a script, an MCP server — which is a smaller
    // blast radius than the open internet and not a reason to leave it.
    //
    // Found by reading headroom's 2026-08-27
    // `fix(vertex): validate location region to close a path-parameter SSRF`,
    // where a request-controlled path segment was interpolated into an
    // upstream hostname. Same class, different interpolation.
    //
    // A proxy for this traffic only ever sees origin-form; absolute-form
    // (`POST http://host/x`) and asterisk-form (`OPTIONS *`) are not requests
    // any of the agent clients make, so refusing them costs nothing real.
    if !target.starts_with('/') {
        return Err(ReadError::Malformed("request target must be origin-form"));
    }

    let mut headers: Vec<(String, String)> = Vec::new();
    let mut header_bytes = 0usize;
    loop {
        let mut h = String::new();
        match r.read_line(&mut h) {
            Ok(0) => return Err(ReadError::Malformed("headers ended without a blank line")),
            Ok(n) => header_bytes += n,
            Err(_) => return Err(ReadError::Io),
        }
        if header_bytes > MAX_HEADER_BYTES {
            return Err(ReadError::Malformed("header block too large"));
        }
        if h == "\r\n" || h == "\n" {
            break;
        }
        let Some((k, v)) = h.split_once(':') else {
            return Err(ReadError::Malformed("header without a colon"));
        };
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }

    let find = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    let chunked =
        find("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    let body = if chunked {
        read_chunked(r)?
    } else {
        let len: usize = find("content-length")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        let mut b = vec![0u8; len];
        if len > 0 {
            r.read_exact(&mut b).map_err(|_| ReadError::Io)?;
        }
        b
    };

    Ok(Request {
        method,
        target,
        headers,
        body,
    })
}

/// Decode a `Transfer-Encoding: chunked` body.
///
/// Present because a proxy does not get to choose what its client sends. An
/// agent behind its own HTTP layer may chunk a large request, and a proxy that
/// only understood `Content-Length` would read a zero-length body and relay an
/// EMPTY conversation to the provider — a silent, total data loss that looks
/// like the model went stupid rather than like a bug in tokenade.
fn read_chunked<R: BufRead>(r: &mut R) -> Result<Vec<u8>, ReadError> {
    let mut out = Vec::new();
    loop {
        let mut size_line = String::new();
        match r.read_line(&mut size_line) {
            Ok(0) => return Err(ReadError::Malformed("chunked body ended early")),
            Ok(_) => {}
            Err(_) => return Err(ReadError::Io),
        }
        // A chunk-size line may carry `;ext=value` extensions we ignore.
        let hex = size_line
            .trim_end()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        let Ok(size) = usize::from_str_radix(hex, 16) else {
            return Err(ReadError::Malformed("chunk size is not hex"));
        };
        if size == 0 {
            // Trailers, then the final blank line.
            loop {
                let mut t = String::new();
                match r.read_line(&mut t) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => return Err(ReadError::Io),
                }
                if t == "\r\n" || t == "\n" {
                    break;
                }
            }
            return Ok(out);
        }
        let start = out.len();
        out.resize(start + size, 0);
        r.read_exact(&mut out[start..]).map_err(|_| ReadError::Io)?;
        let mut crlf = [0u8; 2];
        r.read_exact(&mut crlf).map_err(|_| ReadError::Io)?;
    }
}

/// Write a response head, framing the body as chunked.
///
/// Chunked rather than `Connection: close` on purpose: an agent makes many
/// calls per session and closing every one costs it a fresh TCP+TLS handshake
/// to the local proxy on each turn.
pub fn write_head<W: Write>(
    w: &mut W,
    status: u16,
    headers: &[(String, String)],
) -> std::io::Result<()> {
    write!(w, "HTTP/1.1 {status} {}\r\n", reason(status))?;
    for (k, v) in headers {
        write!(w, "{k}: {v}\r\n")?;
    }
    write!(w, "Transfer-Encoding: chunked\r\n\r\n")
}

/// Write one chunk. An empty slice is skipped rather than written, because a
/// zero-length chunk terminates the body.
pub fn write_chunk<W: Write>(w: &mut W, data: &[u8]) -> std::io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    write!(w, "{:X}\r\n", data.len())?;
    w.write_all(data)?;
    write!(w, "\r\n")?;
    // Flushing per chunk is the entire point on an SSE stream: a buffered
    // writer that holds three events back is indistinguishable, from the
    // agent's side, from a model that has not started answering.
    w.flush()
}

/// Terminate a chunked body.
pub fn end_chunks<W: Write>(w: &mut W) -> std::io::Result<()> {
    write!(w, "0\r\n\r\n")?;
    w.flush()
}

/// A complete, self-framed error response.
///
/// Used only when we have nothing to relay — we could not reach the provider at
/// all. The shape mimics both wires' error envelope so an agent's error
/// handling recognises it instead of reporting "invalid JSON from the API".
pub fn write_error<W: Write>(w: &mut W, status: u16, message: &str) -> std::io::Result<()> {
    let body = serde_json::json!({
        "error": { "type": "tokenade_proxy_error", "message": message }
    })
    .to_string();
    write!(
        w,
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        reason(status),
        body.len(),
        body
    )?;
    w.flush()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        502 => "Bad Gateway",
        504 => "Gateway Timeout",
        // Every other status is relayed from the provider, and the reason
        // phrase carries no meaning to any client written this century.
        _ => "Status",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn req(raw: &str) -> Result<Request, ReadError> {
        read_request(&mut Cursor::new(raw.as_bytes().to_vec()))
    }

    #[test]
    fn reads_a_plain_request_with_content_length() {
        let r =
            req("POST /v1/messages HTTP/1.1\r\nHost: api\r\nContent-Length: 7\r\n\r\n{\"a\":1}")
                .expect("parsed");
        assert_eq!(r.method, "POST");
        assert_eq!(r.target, "/v1/messages");
        assert_eq!(r.body, b"{\"a\":1}");
        assert_eq!(r.header("host"), Some("api"));
        assert_eq!(r.header("HOST"), Some("api"), "lookup is case-insensitive");
    }

    /// A target that does not begin with `/` lands on the END OF THE HOSTNAME
    /// when `server.rs` concatenates it onto the upstream, and this proxy
    /// attaches the user's API key to whatever it dials.
    ///
    /// The two that actually move the host, measured against upstream
    /// `https://api.anthropic.com` before the refusal existed:
    ///   `@evil.example/x` → `evil.example` (the real host becomes userinfo)
    ///   `.evil.example/x` → `api.anthropic.com.evil.example`
    #[test]
    fn a_target_that_is_not_origin_form_is_refused() {
        for target in [
            "@evil.example/v1/messages",
            ".evil.example/v1/messages",
            "http://evil.example/v1/messages",
            "*",
            "evil.example/v1/messages",
        ] {
            let raw = format!("POST {target} HTTP/1.1\r\nHost: api\r\nContent-Length: 0\r\n\r\n");
            assert!(
                req(&raw).is_err(),
                "accepted a non-origin-form target: {target:?}"
            );
        }
        // …and the concatenation this protects is still the one in use, so a
        // rewrite of `server.rs` that stops pasting the target onto the
        // upstream should retire this guard rather than leave it lying.
        // The FAMILY: the cached-prefix boundary moved down into
        // `server/prefix.rs`.
        let root = tokenade_core::source_guard::crates_dir()
            .join("tokenade-cli")
            .join("src")
            .join("llm_proxy");
        let server = tokenade_core::source_guard::module_family(&root, "server");
        let server = server.as_str();
        assert!(
            server.contains(
                r#"format!("{}{}", self.cfg.upstream.trim_end_matches('/'), req.target)"#
            ),
            "server.rs no longer concatenates target onto upstream — re-check this guard"
        );
    }

    /// The forms real clients send must still parse.
    #[test]
    fn origin_form_still_parses() {
        for target in ["/v1/messages", "/v1/messages?beta=true", "/"] {
            let raw = format!("POST {target} HTTP/1.1\r\nHost: api\r\nContent-Length: 0\r\n\r\n");
            let r = req(&raw).expect("origin-form must parse");
            assert_eq!(r.target, target);
        }
    }

    #[test]
    fn a_chunked_request_body_is_reassembled_whole() {
        // The failure this guards is not a crash: a proxy that ignored chunked
        // framing would read a ZERO-length body from a `Content-Length`-free
        // request and cheerfully relay an empty conversation.
        let raw = "POST /v1/chat HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n\
                   5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let r = req(raw).expect("parsed");
        assert_eq!(r.body, b"hello world");
    }

    #[test]
    fn chunk_extensions_and_trailers_do_not_derail_the_body() {
        let raw = "POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n\
                   3;name=value\r\nabc\r\n0\r\nX-Trailer: t\r\n\r\n";
        assert_eq!(req(raw).expect("parsed").body, b"abc");
    }

    #[test]
    fn an_empty_connection_is_eof_and_not_an_error() {
        // Keep-alive: the agent closing an idle connection is the normal end of
        // a conversation, and logging it as a fault would fill the log with
        // one entry per turn.
        assert_eq!(req(""), Err(ReadError::Eof));
    }

    #[test]
    fn a_malformed_request_is_named_but_never_half_parsed() {
        assert_eq!(
            req("GARBAGE\r\n\r\n"),
            Err(ReadError::Malformed("request line"))
        );
        assert_eq!(
            req("POST /x HTTP/1.1\r\nnocolon\r\n\r\n"),
            Err(ReadError::Malformed("header without a colon"))
        );
    }

    #[test]
    fn hop_by_hop_headers_are_dropped_in_both_directions() {
        for h in ["Connection", "Transfer-Encoding", "Upgrade", "TE"] {
            assert!(drop_from_request(h), "{h} must not reach the provider");
            assert!(drop_from_response(h), "{h} must not reach the agent");
        }
        // Direction-specific, and the asymmetry is the point.
        assert!(drop_from_request("Host"));
        assert!(!drop_from_response("Host"));
        assert!(drop_from_response("Content-Encoding"));
        // Authorization is NOT dropped: the proxy relays the agent's own
        // credentials to the provider. It is redacted in logs, never in flight.
        assert!(!drop_from_request("Authorization"));
        assert!(!drop_from_request("x-api-key"));
    }

    #[test]
    fn streaming_is_read_off_the_body_not_off_accept() {
        let with = Request {
            method: "POST".into(),
            target: "/v1/messages".into(),
            headers: vec![],
            body: br#"{"stream":true,"messages":[]}"#.to_vec(),
        };
        assert!(with.wants_stream());
        let without = Request {
            body: br#"{"messages":[]}"#.to_vec(),
            ..with.clone()
        };
        assert!(!without.wants_stream());
        // An `Accept` header alone must not switch us into streaming: we would
        // hold a buffered answer open waiting for events that never come.
        let lying = Request {
            headers: vec![("Accept".into(), "text/event-stream".into())],
            body: br#"{"messages":[]}"#.to_vec(),
            ..with.clone()
        };
        assert!(!lying.wants_stream());
        // Gemini puts it in the URL.
        let gemini = Request {
            target: "/v1beta/models/gemini-3-pro:streamGenerateContent?alt=sse".into(),
            body: br#"{"contents":[]}"#.to_vec(),
            ..with.clone()
        };
        assert!(gemini.wants_stream());
    }

    #[test]
    fn a_chunked_response_is_framed_exactly() {
        let mut out = Vec::new();
        write_head(
            &mut out,
            200,
            &[("Content-Type".into(), "text/event-stream".into())],
        )
        .unwrap();
        write_chunk(&mut out, b"data: hi\n\n").unwrap();
        write_chunk(&mut out, b"").unwrap();
        end_chunks(&mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
             Transfer-Encoding: chunked\r\n\r\nA\r\ndata: hi\n\n\r\n0\r\n\r\n"
        );
    }

    #[test]
    fn an_error_body_never_echoes_what_the_client_sent() {
        // A malformed request line can carry a key. `write_error` takes a
        // message the CALLER wrote, and this asserts the caller's choices are
        // the only thing that can appear.
        let mut out = Vec::new();
        write_error(&mut out, 502, "upstream unreachable").unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("upstream unreachable"));
        assert!(s.contains("Content-Length: "));
        assert!(!s.contains("Transfer-Encoding"));
    }
}
