//! MCP stdio proxy: wraps a child MCP server and compacts its
//! tool-call results on the way back (generic shape-aware fold + per-tool
//! registry + per-tool savings stats). Extracted from main.rs (refactor).

mod redact;
mod response;
mod subcommands;
pub(crate) use redact::*;
pub(crate) use response::*;
pub(crate) use subcommands::*;

/// `read_line`, under the ceiling every other input path in this product uses.
///
/// A JSON-RPC line has no declared length, so `read_line` grows until the peer
/// sends a newline — and the peer here is a THIRD-PARTY MCP server the user
/// wired in, on the transport this product's own guidance tells them to wrap.
/// Measured 2026-09-06 against a server that answers one line and never ends
/// it: `tokenade mcp-proxy` reached **50 972 MB** on the published binary and
/// was still climbing at 30 s.
///
/// The ceiling is `TOKENADE_MAX_INPUT_BYTES` (1 GiB by default), the same one
/// `read_capped` applies to stdin and to a path — generous enough that a real
/// tool result, which is exactly one long line, is untouched.
///
/// Returns `Ok(None)` when the line ran past the ceiling: the stream cannot be
/// resynchronised (a truncated line is corrupt JSON and the next bytes are the
/// middle of a message), so the caller stops the pump and says why.
fn read_line_capped<R: std::io::BufRead>(
    r: &mut R,
    buf: &mut String,
    cap: u64,
) -> std::io::Result<Option<usize>> {
    // Byte at a time under the ceiling: `Take` is not `BufRead`, and buffering
    // the peer's bytes to find a newline is precisely what has no end here.
    let mut raw = Vec::new();
    let mut n: u64 = 0;
    loop {
        let avail = match r.fill_buf() {
            Ok([]) => break,
            Ok(b) => b,
            Err(e) => return Err(e),
        };
        let room = (cap - n).min(avail.len() as u64) as usize;
        let taken = match avail[..room].iter().position(|&b| b == b'\n') {
            Some(i) => {
                raw.extend_from_slice(&avail[..=i]);
                i + 1
            }
            None => {
                raw.extend_from_slice(&avail[..room]);
                room
            }
        };
        r.consume(taken);
        n += taken as u64;
        if raw.ends_with(b"\n") {
            break;
        }
        if n >= cap {
            return Ok(None);
        }
    }
    // ONE place decides that a line ran past the ceiling — the check inside the
    // loop. A second, equivalent test lived here and made the first
    // untestable: a mutation removing the in-loop `return` still refused,
    // because this one caught it, so the guard could not tell whether the rule
    // was enforced or merely duplicated.
    let n = n as usize;
    buf.push_str(&String::from_utf8_lossy(&raw));
    Ok(Some(n))
}

pub(crate) fn cmd_mcp_proxy(rest: &[String]) -> i32 {
    // The ceiling both pumps read under — the same one stdin and paths use.
    let cap = crate::bounded_input::max_unsized_input_bytes();
    use std::io::{BufReader, Write};
    // Management subcommands sit in front of the passthrough proxy. A bare
    // leading `install`/`uninstall`/`report` token (never after a `--`, which
    // always introduces the child binary) routes to config rewriting / the
    // per-tool report instead of spawning a child named "install".
    match rest.first().map(String::as_str) {
        Some("install") => return cmd_mcp_proxy_install(&rest[1..]),
        Some("uninstall") => return cmd_mcp_proxy_uninstall(&rest[1..]),
        Some("report") => return cmd_mcp_proxy_report(&rest[1..]),
        _ => {}
    }
    // Allow either `tokenade mcp-wrap -- <bin> <args>...` or
    // `tokenade mcp-wrap <bin> <args>...` — the leading `--` is the
    // standard delimiter but we don't require it.
    // The app whose config `mcp-wrap install` wrapped, written into the
    // entry's `env` so the rows name it: a desktop app's environment does not.
    if let Ok(app) = std::env::var(tokenade_core::mcp_wrap::AGENT_ENV) {
        if !app.trim().is_empty() {
            tokenade_core::gain::set_process_agent(app.trim());
        }
    }
    let target: Vec<String> = if rest.first().map(String::as_str) == Some("--") {
        rest[1..].to_vec()
    } else {
        rest.to_vec()
    };
    if target.is_empty() {
        eprintln!("usage: tokenade mcp-wrap -- <bin> [<args>…]");
        return 2;
    }
    // #982 — Without this guard, `tokenade mcp-wrap --bogus`
    // happily passes `--bogus` to `Command::new` as the binary
    // name. The OS returns "No such file or directory", which we
    // wrap in a JSON-RPC error envelope and ship to the agent —
    // the user sees a confusing mix of stderr + JSON-RPC error
    // for what was just a flag typo. `--help` itself is caught
    // earlier by the central --help interceptor (#788), so by the
    // time we get here a leading `--…` token is a typo by
    // construction. Reject loud + suggest the `--` delimiter
    // syntax so users separating their MCP binary from tokenade
    // flags know the shape.
    if target[0].starts_with("--") {
        eprintln!(
            "tokenade mcp-wrap: unknown flag '{}'.\n  \
             Pass the MCP server binary as the first positional, optionally \
             after a `--` delimiter:\n    \
             tokenade mcp-wrap -- /path/to/mcp-server arg1 arg2\n  \
             Run `tokenade mcp-wrap --help` for details.",
            target[0]
        );
        return 2;
    }
    // Spawn the child MCP. Inherit env (the child needs API keys
    // etc. from the calling agent's environment). Forward stderr so
    // log lines surface to the user.
    let mut mcp_cmd = std::process::Command::new(&target[0]);
    mcp_cmd
        .args(&target[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());
    // stdin/stdout are the JSON-RPC pipes; the child never needs a console.
    // Suppress the window flash when tokenade itself has none.
    tokenade_core::process_util::configure_no_window(&mut mcp_cmd);
    // Rank 3 of the ladder had the same hole as ranks 1 and 2: killing the
    // proxy left the wrapped MCP SERVER running, holding whatever it holds,
    // with its JSON-RPC pipes dangling. Normal shutdown here is EOF-driven,
    // which is why it never showed — a signal is the path nobody exercised.
    tokenade_core::process_util::configure_own_group(&mut mcp_cmd);
    tokenade_core::process_util::configure_die_with_parent(&mut mcp_cmd);
    let mut child = match mcp_cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tokenade mcp-wrap: failed to spawn '{}': {e}", target[0]);
            // Emit a JSON-RPC-shaped error so the agent sees a
            // structured failure rather than a silent close.
            let err = serde_json::json!({
                "jsonrpc": "2.0",
                "error": {
                    "code": -32099,
                    "message": format!("tokenade mcp-wrap: spawn failed: {e}"),
                }
            });
            println!("{err}");
            return 127;
        }
    };
    let mut child_stdin = match child.stdin.take() {
        Some(s) => s,
        None => return 1,
    };
    let child_stdout = match child.stdout.take() {
        Some(s) => s,
        None => return 1,
    };
    if !tokenade_core::process_util::terminal_owns_the_group() {
        // `_and_die`, unlike every other caller: the loop below joins the STDIN
        // pump before waiting on the child, and no child death closes stdin.
        // Relaying without dying made the proxy UNKILLABLE by SIGTERM —
        // measured at 504 s still alive, with a dead child, blocking a gate.
        tokenade_core::process_util::forward_termination_and_die(child.id());
    }
    // The MCP server is a long-lived process, so an orphaned one is worse here
    // than anywhere else: it keeps its port, socket or lock for the rest of the
    // session with no client attached. Same tty precondition as the relay above.
    let reaper = if !tokenade_core::process_util::terminal_owns_the_group() {
        tokenade_core::process_util::guard_group_against_our_death(child.id())
    } else {
        None
    };
    // Shared request-id → tool-name map: a `tools/call` request carries the
    // tool name in `params.name`, but the matching RESULT only carries the
    // JSON-RPC `id`. We record id→name on the way IN so the response
    // transformer can attribute compaction to the specific MCP tool (per-tool
    // stats + the "which tools were/weren't compacted" report).
    let id_tool: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, (String, serde_json::Value)>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let id_tool_in = id_tool.clone();
    // Thread A: stdin → child stdin (forward client requests verbatim, but
    // sniff `tools/call` to learn each request's tool name + arguments).
    let in_thread = std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut handle = stdin.lock();
        let mut buf = String::new();
        loop {
            buf.clear();
            match read_line_capped(&mut handle, &mut buf, cap) {
                Ok(None) => {
                    eprintln!(
                        "tokenade mcp-proxy: a request line ran past {cap} bytes with no end; \
                         the stream cannot be resynchronised, so the proxy is stopping."
                    );
                    break;
                }
                Ok(Some(0)) => break,
                Ok(Some(_)) => {
                    if let Some((id, name, args)) = parse_tools_call_id_name(&buf) {
                        if let Ok(mut m) = id_tool_in.lock() {
                            // Cap the map so a long-lived proxy can't grow it
                            // unbounded (responses delete their entry below).
                            if m.len() > 4096 {
                                m.clear();
                            }
                            m.insert(id, (name, args));
                        }
                    }
                    if child_stdin.write_all(buf.as_bytes()).is_err() {
                        break;
                    }
                    let _ = child_stdin.flush();
                }
                Err(_) => break,
            }
        }
        // Closing child_stdin signals EOF to the child.
        drop(child_stdin);
    });
    // #493 — derive a stable per-target label so each wrapped MCP
    // gets its own dashboard row. The proxy launcher only sees the
    // child argv, so we pick the LAST positional arg (after stripping
    // its dir prefix and a trailing exe extension) — for `npx
    // mcp-server-github` that yields `mcp-server-github`; for
    // `/opt/foo/bar` it yields `bar`; for `tokenade` it yields
    // `tokenade`. Stable across restarts, distinct per MCP.
    let argv_label = derive_mcp_target_label(&target);
    let id_tool_out = id_tool.clone();
    // Thread B: child stdout → stdout (transform each response).
    let out_thread = std::thread::spawn(move || {
        let mut reader = BufReader::new(child_stdout);
        let stdout = std::io::stdout();
        let mut buf = String::new();
        // The AUTHORITATIVE server identity, learned from the `initialize`
        // response's `serverInfo.name` (the argv label is unreliable). Set once,
        // before any `tools/call` response arrives (initialize is always first).
        let mut server_name: Option<String> = None;
        loop {
            buf.clear();
            match read_line_capped(&mut reader, &mut buf, cap) {
                Ok(None) => {
                    eprintln!(
                        "tokenade mcp-proxy: the server sent a response line past {cap} bytes \
                         with no end; the stream cannot be resynchronised, so the proxy is \
                         stopping."
                    );
                    break;
                }
                Ok(Some(0)) => break,
                Ok(Some(_)) => {
                    if server_name.is_none() {
                        if let Some(n) = parse_server_info(&buf) {
                            server_name = Some(n);
                        }
                    }
                    // Correlate this response to its call via the JSON-RPC id,
                    // then consume the map entry (responses are 1:1 with calls).
                    let rid = response_id(&buf);
                    let call = rid
                        .clone()
                        .and_then(|id| id_tool_out.lock().ok().and_then(|mut m| m.remove(&id)));
                    let (tool, args) = match call {
                        Some((t, a)) => (Some(t), Some(a)),
                        None => (None, None),
                    };
                    // serverInfo if known, else the (best-effort) argv label.
                    let server = server_name.as_deref().unwrap_or(&argv_label);
                    // A fault while compacting this response would kill the
                    // proxy between the server's answer and ours: the client
                    // never sees a reply to a request it already sent, so it
                    // waits out its timeout instead of failing. Arm a minimal,
                    // protocol-valid error carrying THIS request's id, so the
                    // fatal-signal handler still closes the exchange.
                    //
                    // Deliberately NOT the server's own bytes: this choke point
                    // is where redaction runs, and every branch that returned
                    // the server's line verbatim was once a way out for a
                    // secret. An id and a fixed sentence carry none, and cost
                    // nothing to build.
                    let fallback = rid
                        .as_deref()
                        .map(fault_fallback_response)
                        .unwrap_or_default();
                    let transformed = {
                        let _salvage = tokenade_core::crash_guard::arm_salvage(&fallback, 0);
                        let t = transform_mcp_response_line(
                            &buf,
                            server,
                            tool.as_deref(),
                            args.as_ref(),
                        );
                        // The `initialize` response carries
                        // `result.instructions`, which the client adds to the
                        // model's context. It is the ONLY place, on an agent
                        // whose sole channel to us is MCP, where the guidance
                        // reaches a session — and a sub-agent session
                        // initialises like its parent's, so it receives it too.
                        // That was the missing half of the "Sub-agent parity"
                        // box on those agents: folding was shared, the brief
                        // was not.
                        brief_into_initialize(&t)
                    };
                    let mut h = stdout.lock();
                    let _ = h.write_all(transformed.as_bytes());
                    if !transformed.ends_with('\n') {
                        let _ = writeln!(h);
                    }
                    let _ = h.flush();
                }
                Err(_) => break,
            }
        }
    });
    let _ = in_thread.join();
    let _ = out_thread.join();
    let status = child.wait().ok();
    tokenade_core::process_util::stop_forwarding_termination();
    if let Some(r) = reaper {
        r.stand_down();
    }
    status.and_then(|s| s.code()).unwrap_or(0)
}

/// #493 — derive a stable label for a wrapped MCP from the proxied
/// argv. Picks the LAST positional arg's basename (sans extension),
/// which corresponds to the canonical "server name" for the most
/// common wrap shapes (`npx mcp-server-X` → `mcp-server-X`;
/// `/opt/x/bin/foo --flag` → `foo`).
pub(crate) fn derive_mcp_target_label(argv: &[String]) -> String {
    let last = argv
        .iter()
        .rev()
        .find(|s| !s.starts_with('-'))
        .map(String::as_str)
        .unwrap_or("unknown");
    let basename = std::path::Path::new(last)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(last);
    if basename.is_empty() {
        "unknown".to_string()
    } else {
        basename.to_string()
    }
}

/// Transform one JSON-RPC line emitted by a child MCP server.
///
/// Rules (each preserves protocol semantics):
///   * Empty / non-JSON line → forwarded verbatim (some servers
///     emit blank separator lines; transformer must be a no-op).
///   * JSON-RPC error response → forwarded verbatim (the agent
///     needs the exact error shape to diagnose).
///   * `result.content` array of MCP content items: walk each
///     `text` entry, run it through `disclose::maybe_stash`. Image
///     / audio / resource items pass through (already encoded).
///   * Anything else (notifications, list responses, etc.) →
///     forwarded verbatim.
///
/// #493 — logs savings to gain.jsonl under `mcp-proxy:<target_label>`
/// so the dashboard's "by op" section surfaces wrapped-MCP compaction
/// alongside the built-in compactors. `target_label` comes from the
/// caller (`derive_mcp_target_label`) — one row per wrapped server.
/// Parse a JSON-RPC REQUEST line: if it's a `tools/call`, return
/// `(id, tool_name)` (id stringified so number/string ids unify). Used to
/// learn which tool each request targets so the matching response can be
/// attributed. Returns None for any other line (cheap — full parse only on
/// lines that mention the method).
pub(crate) fn parse_tools_call_id_name(line: &str) -> Option<(String, String, serde_json::Value)> {
    if !line.contains("tools/call") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("method").and_then(|m| m.as_str()) != Some("tools/call") {
        return None;
    }
    let id = jsonrpc_id_string(v.get("id")?)?;
    let name = v.pointer("/params/name").and_then(|n| n.as_str())?;
    // The exact call arguments — used to disambiguate compactor variants (e.g.
    // a SELECT vs a write `query`). Absent/odd args → Null (the compactor then
    // falls back to the tool name alone).
    let args = v
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Some((id, name.to_string(), args))
}

/// Add the tokenade brief to the `instructions` field of an `initialize`
/// response.
///
/// Returns the line UNCHANGED for everything else — another method,
/// unreadable JSON, brief already present. The server's instructions are kept
/// and follow ours: they are its own, they describe its tools, and
/// overwriting them would mean taking authority over a text that is not ours.
pub(crate) fn brief_into_initialize(line: &str) -> String {
    if !line.contains("serverInfo") {
        return line.to_string();
    }
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return line.to_string();
    };
    let Some(result) = v.get_mut("result").and_then(|r| r.as_object_mut()) else {
        return line.to_string();
    };
    let brief = crate::hooks::subagent_pretool::subagent_brief("Explore");
    let existing = result
        .get("instructions")
        .and_then(|i| i.as_str())
        .unwrap_or("");
    if existing.contains(crate::hooks::subagent_pretool::PREAMBLE_MARKER) {
        return line.to_string();
    }
    let merged = if existing.is_empty() {
        brief
    } else {
        format!("{brief}\n\n{existing}")
    };
    result.insert(
        "instructions".to_string(),
        serde_json::Value::String(merged),
    );
    match serde_json::to_string(&v) {
        Ok(s) => s,
        Err(_) => line.to_string(),
    }
}

/// Parse a JSON-RPC `initialize` RESPONSE line and return the server's
/// self-reported name (`result.serverInfo.name`). This is the AUTHORITATIVE
/// server identity — the proxied argv is unreliable (it often ends in the
/// server's own positional arg, e.g. a `--db-path` value or an allowed dir).
/// Used to key the false-positive-free per-server compactor dispatch.
pub(crate) fn parse_server_info(line: &str) -> Option<String> {
    if !line.contains("serverInfo") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    v.pointer("/result/serverInfo/name")
        .and_then(|n| n.as_str())
        .map(String::from)
}

/// Stringify a JSON-RPC `id` (spec allows string or number) for map keying.
pub(crate) fn jsonrpc_id_string(id: &serde_json::Value) -> Option<String> {
    match id {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Extract a RESPONSE line's `id` (stringified), to look up its tool.
pub(crate) fn response_id(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    jsonrpc_id_string(v.get("id")?)
}

#[cfg(test)]
include!("mcp_proxy_tests.rs");
