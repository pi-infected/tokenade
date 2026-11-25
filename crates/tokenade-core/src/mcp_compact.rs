//! Tool-SPECIFIC MCP result compactors.
//!
//! The mcp-proxy tries these FIRST, then falls back to the generic
//! shape-aware [`crate::auto::compact`]. A tool-specific compactor earns its
//! place only when it can beat the generic path for a KNOWN tool's output
//! shape (a proprietary envelope, a level-prioritised log, an image-as-text
//! data URI, …). Each returns `Some(folded)` ONLY when it confidently applies,
//! else `None` so the proxy falls through to the generic compactor — never a
//! degraded result.
//!
//! Keyed on a substring of the MCP tool name. Claude Code names MCP tools
//! `mcp__<server>__<tool>`, so matching on the trailing `<tool>` part (e.g.
//! `console_logs`) is server-agnostic.
//!
//! The server-side `mcp-uncompacted` stats (rows the proxy logs for results it
//! could NOT fold) are the backlog for THIS registry: the tools with the most
//! uncompacted volume are the ones worth a dedicated compactor here.

/// Canonical MCP server families we have tool-specific compactors for. Derived
/// from the server's authoritative identity (`initialize` → `serverInfo.name`,
/// captured by the proxy), NOT from the response content. This is what makes
/// detection false-positive-free: a `query`-named tool only routes to the DB
/// compactor when the SERVER is a database server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Filesystem,
    Database,
    Git,
    Github,
    /// Browser-automation MCPs: Playwright, Puppeteer, Claude-in-Chrome,
    /// Claude Preview, generic `browser`/`chrome` servers. Console-message and
    /// page-text tools fold; screenshots are images (left alone).
    Browser,
    Memory,
    Unknown,
}

/// Map a server identity (serverInfo.name, e.g. `secure-filesystem-server`,
/// `sqlite`, `mcp-git`, `github-mcp-server`) to a [`Family`]. Order matters:
/// `github`/`gitlab` are checked before the bare `git` substring.
pub fn server_family(server: &str) -> Family {
    let s = server.to_ascii_lowercase();
    if s.contains("github") || s.contains("gitlab") {
        Family::Github
    } else if ["playwright", "puppeteer", "chrome", "browser", "preview"]
        .iter()
        .any(|k| s.contains(k))
    {
        Family::Browser
    } else if s.contains("filesystem") {
        Family::Filesystem
    } else if [
        "sqlite",
        "postgres",
        "postgresql",
        "mysql",
        "mariadb",
        "supabase",
        "duckdb",
    ]
    .iter()
    .any(|k| s.contains(k))
    {
        Family::Database
    } else if s.contains("git") {
        Family::Git
    } else if s.contains("memory") {
        Family::Memory
    } else {
        Family::Unknown
    }
}

/// Skip the generic `auto::compact` fallback: this text *is* a screenshot
/// capture. False positives here strand a foldable MCP result as raw, so
/// the gate is closed unless a witness matches.
///
/// Two arms, both tight:
/// - **Name:** last `mcp__…__<tool>` segment is `screenshot` or ends in
///   `_screenshot` (`browser_take_screenshot`). Accessibility `snapshot`
///   trees are text and must still fold — they are not in this list.
/// - **Body:** Claude-in-Chrome capture notices only (`computer` /
///   `browser_batch` never put `screenshot` in the tool name). First 4 KiB.
///   The word `screenshot` in docs, a `data:image` in markdown, or a
///   `1920x1080` size in CSS is not enough.
pub fn leave_uncompacted(tool: &str, text: &str) -> bool {
    screenshot_tool_name(tool) || text_looks_like_screenshot_capture(text)
}

fn screenshot_tool_name(tool: &str) -> bool {
    let t = tool
        .rsplit("__")
        .next()
        .unwrap_or(tool)
        .to_ascii_lowercase();
    t == "screenshot" || t.ends_with("_screenshot")
}

/// Body-only capture fingerprint. Do not widen without a captured
/// transcript witness — every extra substring is a false-positive risk.
pub fn text_looks_like_screenshot_capture(text: &str) -> bool {
    let head = crate::caps::truncate_at_char_boundary(text, 4096);
    let s = head.to_ascii_lowercase();
    s.contains("successfully captured screenshot")
        || s.contains("successfully captured zoomed screenshot")
        || s.contains("[computer:screenshot]")
        || s.contains("captured zoomed screenshot of region")
}

/// Try a tool-specific compactor. `server` is the authoritative server identity
/// (serverInfo.name / proxied label), `tool` the exact `tools/call` name, and
/// `args` the exact call arguments (used to disambiguate variants — e.g. a
/// SELECT vs a write `query`). Returns `Some(folded)` only when a compactor
/// written for THAT (server-family, tool) confidently applies; `None` defers to
/// the generic `auto::compact` **unless** [`leave_uncompacted`] is true (the
/// proxy skips the generic fallback in that case). Detection never guesses
/// from response content for *which* compactOR to fire; the screenshot-body
/// leave is a keep-raw gate, not a fold.
pub fn compact_for_tool(
    server: &str,
    tool: &str,
    args: &serde_json::Value,
    text: &str,
) -> Option<String> {
    let t = tool.to_ascii_lowercase();
    match server_family(server) {
        // Filesystem: only `directory_tree` is the bespoke shape; `read_*`
        // returns file content (generic sniff handles it, and head/tail args
        // mean the server already truncated), `list_directory` is already lean.
        Family::Filesystem => {
            if t == "directory_tree" {
                return fs_tree::compact(text);
            }
            None
        }
        // Database: fold row-returning reads to CSV. The exact read tool names
        // AND (when visible) the SQL in `args` confirm it returns rows — a
        // write/DDL `query` does not, so we leave those for the generic path.
        Family::Database => {
            let read_tool = matches!(
                t.as_str(),
                "read_query"
                    | "query"
                    | "read-query"
                    | "execute_query"
                    | "run_query"
                    | "select"
                    | "sql"
                    | "execute_sql"
            );
            let sql_is_read = args
                .get("query")
                .or_else(|| args.get("sql"))
                .and_then(|v| v.as_str())
                .map(|q| {
                    let q = q.trim_start().to_ascii_lowercase();
                    ["select", "with", "pragma", "show", "explain"]
                        .iter()
                        .any(|k| q.starts_with(k))
                })
                .unwrap_or(true); // no SQL arg visible → trust the tool name
            if read_tool && sql_is_read {
                return db_rows::compact(text);
            }
            None
        }
        // GitHub / GitLab: the result is the provider's raw REST JSON. Now that
        // we KNOW the server, strip the chrome (the `*_url` zoo, `node_id`,
        // `_links`) directly — no content signature needed.
        Family::Github => rest_api::compact(text),
        // Browser MCPs (Playwright / Puppeteer / Claude-in-Chrome / Preview):
        //  - console message streams fold losslessly;
        //  - page-text tools (get_page_text/read_page/get_content/…) that
        //    return raw HTML fold to markdown (same lever as WebFetch);
        //  - screenshots/snapshots are images or already a11y-routed → leave.
        Family::Browser => {
            if t.contains("console") {
                return console_logs::compact(text);
            }
            if t.contains("page")
                || t.contains("content")
                || t.contains("html")
                || t.contains("dom")
            {
                return browser_page::compact(text);
            }
            None
        }
        // Git / Memory families: recognised (so they never misroute into the DB
        // or REST arms) but no bespoke compactor yet — defer to generic.
        Family::Git | Family::Memory => None,
        // No server identity (the non-proxy inline path — e.g. an MCP result
        // the Read hook sees). Only fire on a tool name that self-identifies
        // unambiguously; never guess from content here (that is the false
        // positive we are eliminating).
        Family::Unknown => {
            if t.contains("console_log") || t.contains("console_message") {
                return console_logs::compact(text);
            }
            None
        }
    }
}

/// Filesystem MCP `directory_tree` — the reference server returns a
/// pretty-printed JSON array of `{name, type: "file"|"directory", children?}`
/// nodes (every node is a 3-line object). Render it as an indented text tree
/// (`dir/` then nested entries) — LOSSLESS on names + structure, a fraction of
/// the tokens. `None` if the payload isn't that shape.
pub mod fs_tree {
    use serde_json::Value;

    pub fn compact(text: &str) -> Option<String> {
        let v: Value = serde_json::from_str(text.trim()).ok()?;
        let arr = v.as_array()?;
        // Shape check: at least one node, every node an object with name+type.
        if arr.is_empty()
            || !arr.iter().all(|n| {
                n.get("name").map(Value::is_string).unwrap_or(false)
                    && n.get("type").map(Value::is_string).unwrap_or(false)
            })
        {
            return None;
        }
        let mut out = String::with_capacity(text.len() / 3);
        render(arr, 0, &mut out);
        let out = out.trim_end().to_string();
        if out.len() < text.len() {
            Some(out)
        } else {
            None
        }
    }

    fn render(nodes: &[Value], depth: usize, out: &mut String) {
        for n in nodes {
            let name = n.get("name").and_then(Value::as_str).unwrap_or("?");
            let is_dir = n.get("type").and_then(Value::as_str) == Some("directory");
            for _ in 0..depth {
                out.push_str("  ");
            }
            out.push_str(name);
            if is_dir {
                out.push('/');
            }
            out.push('\n');
            if let Some(ch) = n.get("children").and_then(Value::as_array) {
                render(ch, depth + 1, out);
            }
        }
    }
}

/// Database MCP row-sets (sqlite/postgres/mysql/supabase `read_query`/`query`
/// /`execute_sql`). When the result is a JSON array of homogeneous flat
/// objects, the per-row repeated keys dominate the payload. Emit a single CSV
/// header + one RFC-4180-quoted row per record — LOSSLESS, and far smaller for
/// wide or long result sets. `None` unless the shape matches cleanly (defer to
/// the generic JSON compactor otherwise — e.g. the sqlite reference server's
/// Python-`repr` output, which isn't valid JSON).
pub mod db_rows {
    use serde_json::Value;

    const MIN_ROWS: usize = 2;

    pub fn compact(text: &str) -> Option<String> {
        let v: Value = serde_json::from_str(text.trim()).ok()?;
        let arr = v.as_array()?;
        if arr.len() < MIN_ROWS {
            return None;
        }
        // Column order from the first row; every row must be an object with the
        // SAME key set and only scalar values (objects/arrays → defer).
        let first = arr[0].as_object()?;
        let cols: Vec<&String> = first.keys().collect();
        if cols.is_empty() {
            return None;
        }
        for row in arr {
            let o = row.as_object()?;
            if o.len() != cols.len() || !cols.iter().all(|c| o.contains_key(*c)) {
                return None;
            }
            if o.values().any(|x| x.is_object() || x.is_array()) {
                return None;
            }
        }
        let mut out = String::with_capacity(text.len() / 2);
        out.push_str(
            &cols
                .iter()
                .map(|c| csv_field(c))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
        for row in arr {
            let o = row.as_object().unwrap();
            let line: Vec<String> = cols.iter().map(|c| csv_field(&scalar(&o[*c]))).collect();
            out.push_str(&line.join(","));
            out.push('\n');
        }
        let out = format!("{}({} rows)", out, arr.len());
        if out.len() < text.len() {
            Some(out)
        } else {
            None
        }
    }

    fn scalar(v: &Value) -> String {
        match v {
            Value::Null => "NULL".into(),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    /// RFC-4180 field: quote if it contains a comma, quote, or newline.
    fn csv_field(s: &str) -> String {
        if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    }
}

/// REST-API JSON noise stripper (GitHub, GitLab, and any tool that passes the
/// provider's raw API JSON through). These envelopes are dominated by chrome:
/// a dozen `*_url` variants per object, `node_id`, gravatar ids, `_links`.
/// Recursively drop those keys (keeping `html_url`, the one link an agent
/// actually uses) and re-serialise compactly. Detection is by SHAPE so it is
/// server-agnostic. `None` unless the signature noise is present and removing
/// it actually shrinks the payload.
pub mod rest_api {
    use serde_json::Value;

    fn is_noise_key(k: &str) -> bool {
        if k == "html_url" {
            return false; // the one link worth keeping
        }
        k == "node_id" || k == "gravatar_id" || k == "_links" || k == "url" || k.ends_with("_url")
    }

    fn strip(v: &mut Value) {
        match v {
            Value::Object(map) => {
                map.retain(|k, _| !is_noise_key(k));
                for val in map.values_mut() {
                    strip(val);
                }
            }
            Value::Array(a) => {
                for val in a.iter_mut() {
                    strip(val);
                }
            }
            _ => {}
        }
    }

    fn has_signature(v: &Value) -> bool {
        match v {
            Value::Object(map) => {
                if map.contains_key("node_id")
                    || map.contains_key("_links")
                    || map.keys().filter(|k| k.ends_with("_url")).count() >= 2
                {
                    return true;
                }
                map.values().any(has_signature)
            }
            Value::Array(a) => a.iter().any(has_signature),
            _ => false,
        }
    }

    pub fn compact(text: &str) -> Option<String> {
        let mut v: Value = serde_json::from_str(text.trim()).ok()?;
        if !has_signature(&v) {
            return None;
        }
        strip(&mut v);
        // Compact (no-whitespace) serialisation; then let the generic compactor
        // structurally fold the cleaned JSON further.
        let cleaned = serde_json::to_string(&v).ok()?;
        let folded = crate::auto::compact(&cleaned).1;
        if folded.len() < text.len() {
            Some(folded)
        } else {
            None
        }
    }
}

/// Browser-console output (Claude Preview `preview_console_logs`, Playwright
/// `browser_console_messages`, …). These are dominated by repeated identical
/// lines (a render loop logging the same line hundreds of times, polling
/// fetches, framework debug spam). Collapse runs of identical lines into
/// `line  (×N)` — LOSSLESS, since exact repeats carry no new information — and
/// keep every distinct line in order (temporal order matters for logs, so we
/// do NOT reorder or drop by level). Returns `None` when the input isn't
/// console-like or has nothing to fold, so the generic timestamp-aware log
/// compactor gets a shot instead.
pub mod console_logs {
    /// Minimum lines before we bother (tiny console dumps aren't worth a
    /// marker).
    const MIN_LINES: usize = 4;

    pub fn compact(text: &str) -> Option<String> {
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() < MIN_LINES {
            return None;
        }
        // Heuristic gate: it should LOOK like console output — a majority of
        // lines carrying a level word or a `[..]` / `key:` marker. Otherwise
        // defer to the generic compactor (don't mangle arbitrary text).
        let looks_console = |l: &str| {
            let lo = l.to_ascii_lowercase();
            [
                "error", "warn", "info", "debug", "log", "trace", "[", "console.",
            ]
            .iter()
            .any(|m| lo.contains(m))
        };
        let hits = lines.iter().filter(|l| looks_console(l)).count();
        if hits * 2 < lines.len() {
            return None; // < 50% console-ish → not our shape
        }
        let mut out = String::with_capacity(text.len());
        let mut folded_any = false;
        let mut i = 0;
        while i < lines.len() {
            let mut j = i + 1;
            while j < lines.len() && lines[j] == lines[i] {
                j += 1;
            }
            let n = j - i;
            out.push_str(lines[i]);
            if n > 1 {
                crate::fold_count::note_folded(n);
                out.push_str(&format!("  (×{n})"));
                folded_any = true;
            }
            out.push('\n');
            i = j;
        }
        // Only claim the result if we actually shrank it; else let the generic
        // (timestamp-aware) log compactor try — it folds near-duplicates too.
        if folded_any {
            Some(out.trim_end().to_string())
        } else {
            None
        }
    }

    pub fn compact_with_savings(text: &str) -> Option<(String, crate::Savings)> {
        let folded = compact(text)?;
        let before = crate::tokenizer::estimate(text);
        let after = crate::tokenizer::estimate(&folded);
        Some((folded, crate::Savings::new(before, after)))
    }
}

/// Browser-MCP page-text results. Tools like Claude-in-Chrome `get_page_text`/
/// `read_page` (and Playwright page-content tools) can return raw HTML — fold
/// it to markdown (the same lever WebFetch uses). Gated: only when the body
/// actually LOOKS like HTML and the fold genuinely shrinks it; otherwise the
/// tool already returned plain text/markdown → defer to the generic path.
pub mod browser_page {
    pub fn compact(text: &str) -> Option<String> {
        // Char-safe head sniff (avoid slicing mid-codepoint on UTF-8 pages).
        let head: String = text
            .chars()
            .take(2048)
            .collect::<String>()
            .to_ascii_lowercase();
        let looks_html = [
            "<!doctype",
            "<html",
            "<body",
            "<div",
            "<p>",
            "<span",
            "<table",
        ]
        .iter()
        .any(|m| head.contains(m));
        if !looks_html {
            return None;
        }
        let (md, sav) = crate::web::html_to_markdown_with_savings(text);
        if sav.tokens_after < sav.tokens_before && md.len() < text.len() {
            Some(md)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fingerprint reads a 4096-byte head of text the MCP server chose:
    /// a multi-byte character across that offset used to panic the hook.
    #[test]
    fn a_character_across_the_head_limit_does_not_panic() {
        let text = format!("{}é successfully captured screenshot", "a".repeat(4095));
        assert!(!text_looks_like_screenshot_capture(&text));
    }

    #[test]
    fn fs_directory_tree_renders_indented_tree_lossless() {
        // Real shape captured from @modelcontextprotocol/server-filesystem.
        let raw = r#"[
  {"name":"docs","type":"directory","children":[
    {"name":"guide.md","type":"file"},
    {"name":"readme.md","type":"file"}]},
  {"name":"src","type":"directory","children":[
    {"name":"api","type":"directory","children":[
      {"name":"auth.ts","type":"file"}]}]}
]"#;
        let out = fs_tree::compact(raw).expect("tree-shaped");
        assert!(out.len() < raw.len());
        // Every name survives (lossless) + directories marked + indentation.
        assert!(out.contains("docs/"));
        assert!(out.contains("  guide.md"));
        assert!(out.contains("src/"));
        assert!(out.contains("  api/"));
        assert!(out.contains("    auth.ts"));
        // Routed by (server-family, tool) through the registry.
        let null = serde_json::Value::Null;
        assert!(
            compact_for_tool("secure-filesystem-server", "directory_tree", &null, raw).is_some()
        );
        // `directory_tree` only routes under the Filesystem family — the same
        // tool name under a DB server must NOT fire it (no false positive).
        assert!(compact_for_tool("sqlite", "directory_tree", &null, raw).is_none());
    }

    #[test]
    fn fs_tree_defers_on_non_tree_json() {
        assert!(fs_tree::compact(r#"{"ok":true}"#).is_none());
        assert!(fs_tree::compact("not json").is_none());
    }

    #[test]
    fn db_rows_fold_to_csv_lossless() {
        // JSON array of homogeneous rows (postgres/mysql/supabase shape).
        let raw = r#"[{"id":1,"name":"user1","email":"u1@x.io","active":1},
            {"id":2,"name":"user2","email":"u2@x.io","active":0},
            {"id":3,"name":"a,b","email":null,"active":1}]"#;
        let out = db_rows::compact(raw).expect("rowset-shaped");
        assert!(out.len() < raw.len());
        let lines: Vec<&str> = out.lines().collect();
        // Header carries every column once (order is serde-map order).
        let header: Vec<&str> = lines[0].split(',').collect();
        for c in ["id", "name", "email", "active"] {
            assert!(header.contains(&c), "header missing {c}: {}", lines[0]);
        }
        let col = |n: &str| header.iter().position(|h| *h == n).unwrap();
        // Row 1 values land under the right columns (lossless).
        let r1: Vec<&str> = lines[1].split(',').collect();
        assert_eq!(r1[col("id")], "1");
        assert_eq!(r1[col("name")], "user1");
        assert_eq!(r1[col("active")], "1");
        // null rendered NULL; the comma value is RFC-4180 quoted (so naive
        // split sees the quote) — assert the quoting is present in the line.
        assert!(out.contains("\"a,b\""));
        assert!(out.contains("NULL"));
        assert!(out.contains("(3 rows)"));
        let null = serde_json::Value::Null;
        assert!(compact_for_tool("postgres", "query", &null, raw).is_some());
        // Variant via args: a write `query` returns no rows → defer, even with
        // the same row-shaped text. Detection uses the SQL in args.
        let write_args = serde_json::json!({"query": "INSERT INTO users VALUES (9)"});
        assert!(compact_for_tool("postgres", "query", &write_args, raw).is_none());
        // Same row text under a NON-database server → never folded as a table
        // (a GitHub server routes to the REST stripper, which sees no API
        // signature in plain rows and defers).
        assert!(compact_for_tool("github-mcp-server", "query", &null, raw).is_none());
    }

    #[test]
    fn db_rows_defers_on_nested_or_short() {
        // nested object value → defer (not a flat row-set)
        assert!(db_rows::compact(r#"[{"a":{"x":1}},{"a":{"x":2}}]"#).is_none());
        // single row → not worth it
        assert!(db_rows::compact(r#"[{"a":1}]"#).is_none());
    }

    #[test]
    fn rest_api_strips_url_zoo_keeps_html_url() {
        // Minimal GitHub-issue-shaped envelope (the noisy real shape).
        let raw = concat!(
            r#"[{"number":7,"title":"Bug","state":"open","node_id":"MDU6","#,
            r#""url":"https://api.github.com/repos/o/r/issues/7","#,
            r#""html_url":"https://github.com/o/r/issues/7","#,
            r#""events_url":"https://api.github.com/x","labels_url":"https://api.github.com/y","#,
            r#""user":{"login":"octocat","node_id":"abc","avatar_url":"https://a","gravatar_id":"","#,
            r#""url":"https://api.github.com/users/octocat","followers_url":"https://f"}}]"#
        );
        let out = rest_api::compact(raw).expect("github-shaped");
        assert!(out.len() < raw.len());
        // signal kept
        assert!(out.contains("\"number\":7") || out.contains("number"));
        assert!(out.contains("octocat"));
        assert!(out.contains("html_url")); // the one link we keep
                                           // chrome dropped
        assert!(!out.contains("node_id"));
        assert!(!out.contains("gravatar_id"));
        assert!(!out.contains("events_url"));
        assert!(!out.contains("avatar_url"));
    }

    #[test]
    fn rest_api_defers_on_plain_json() {
        // No url-zoo / node_id → not a REST envelope, leave for generic.
        assert!(rest_api::compact(r#"{"result":42,"ok":true}"#).is_none());
    }

    #[test]
    fn console_folds_repeated_lines_losslessly() {
        let logs = "[log] render tick\n".repeat(50)
            + "[error] TypeError: x is undefined\n"
            + &"[info] fetch /api/status 200\n".repeat(30);
        let out = console_logs::compact(&logs).expect("console-shaped");
        // Distinct lines all survive; runs collapsed to ×N.
        assert!(out.contains("[log] render tick  (×50)"));
        assert!(out.contains("[error] TypeError: x is undefined"));
        assert!(out.contains("[info] fetch /api/status 200  (×30)"));
        // Much smaller than the raw 81 lines.
        assert!(out.lines().count() <= 3);
    }

    #[test]
    fn non_console_text_defers_to_generic() {
        // Prose / non-console → None so auto::compact handles it.
        let prose = "The quick brown fox.\nJumps over the lazy dog.\nA second sentence here.\nAnd a third one too.";
        assert!(console_logs::compact(prose).is_none());
    }

    #[test]
    fn nothing_to_fold_defers() {
        // Console-shaped but all-distinct (nothing to collapse) → None, let the
        // generic timestamp-aware compactor try near-duplicate folding.
        let logs = "[log] a\n[log] b\n[error] c\n[info] d\n[warn] e";
        assert!(console_logs::compact(logs).is_none());
    }

    #[test]
    fn registry_routes_console_tool_names() {
        let logs = "[log] x\n".repeat(10);
        let null = serde_json::Value::Null;
        // Playwright family, console tool → folds.
        assert!(compact_for_tool("playwright", "browser_console_messages", &null, &logs).is_some());
        // Unknown server but a self-identifying console tool name → still folds
        // (the only content-free heuristic we keep on the no-server path).
        assert!(compact_for_tool("claude-preview", "preview_console_logs", &null, &logs).is_some());
        // A GitHub tool whose text is plain logs (no REST signature) → defer.
        assert!(compact_for_tool("github-mcp-server", "create_issue", &null, &logs).is_none());
        // A bog-standard unknown tool → defer to generic.
        assert!(compact_for_tool("acme", "do_thing", &null, &logs).is_none());
    }

    #[test]
    fn server_family_classifies_without_false_positives() {
        assert_eq!(server_family("github-mcp-server"), Family::Github);
        assert_eq!(server_family("gitlab-mcp"), Family::Github);
        // `mcp-git` contains "git" but is NOT github — must be Git, not Github.
        assert_eq!(server_family("mcp-git"), Family::Git);
        assert_eq!(
            server_family("secure-filesystem-server"),
            Family::Filesystem
        );
        assert_eq!(server_family("sqlite"), Family::Database);
        assert_eq!(server_family("postgres-mcp"), Family::Database);
        assert_eq!(server_family("playwright"), Family::Browser);
        assert_eq!(server_family("claude-in-chrome"), Family::Browser);
        assert_eq!(server_family("Claude_Preview"), Family::Browser);
        assert_eq!(server_family("some-random-server"), Family::Unknown);
    }

    #[test]
    fn browser_page_folds_html_defers_plain() {
        let null = serde_json::Value::Null;
        let html = "<!doctype html><html><body><h1>Hi</h1>".to_string()
            + &"<p>paragraph of real content here</p>".repeat(40)
            + "</body></html>";
        // Browser family + a page tool → HTML folded to markdown, smaller.
        let out = compact_for_tool("claude-in-chrome", "get_page_text", &null, &html)
            .expect("html page should fold");
        assert!(out.len() < html.len());
        assert!(
            !out.contains("<p>"),
            "HTML tags should be gone after markdown fold"
        );
        // Plain text from a page tool → defer (not HTML).
        let plain = "Just some already-extracted plain text, no markup.".repeat(20);
        assert!(compact_for_tool("claude-in-chrome", "get_page_text", &null, &plain).is_none());
        // A screenshot tool (base64-ish, not HTML) → never folded as a page.
        assert!(compact_for_tool("claude-in-chrome", "screenshot", &null, &html).is_none());
    }

    #[test]
    fn leave_uncompacted_named_screenshot_tool() {
        assert!(leave_uncompacted("screenshot", "anything"));
        assert!(leave_uncompacted(
            "mcp__playwright__browser_take_screenshot",
            "plain click ack"
        ));
        // Accessibility snapshot trees are text — folding them is the point.
        assert!(!leave_uncompacted(
            "mcp__claude-in-chrome__take_snapshot",
            "plain click ack"
        ));
        assert!(!leave_uncompacted("computer", "Clicked at (882, 693)"));
        assert!(!leave_uncompacted(
            "mcp__tokenade__exec_script",
            "status global: {'ok': 93}"
        ));
    }

    #[test]
    fn leave_uncompacted_screenshot_inside_computer_body() {
        // Captured 2026-09-25 from claude-in-chrome `computer` / `browser_batch`
        // (mcp_quality probe: 13/48 hold-out rows). Tool name is not screenshot.
        let computer = "Successfully captured screenshot (1098x538, jpeg) - ID: ss_8834s8965 — 0.7-scale view; coordinate frame: 1568x769.\n\nTab Context:\n- Executed on tabId: 994507971";
        assert!(leave_uncompacted("computer", computer));
        assert!(leave_uncompacted(
            "mcp__claude-in-chrome__computer",
            computer
        ));
        let batch = "[computer:wait] Waited for 2 seconds\n[computer:screenshot] Successfully captured screenshot (784x384, jpeg) - ID: ss_4750lz56z — 0.5-scale view; coordinate frame: 1568x769.";
        assert!(leave_uncompacted("browser_batch", batch));
        let zoomed = "Successfully captured zoomed screenshot of region (356,40) to (800,260) - 538x267 pixels";
        assert!(leave_uncompacted("computer", zoomed));
    }

    #[test]
    fn leave_uncompacted_false_positives_still_foldable() {
        // Word in docs, data URI in markdown, CSS size, a11y snapshot, click ack.
        let cases = [
            (
                "mcp__tokenade__exec_script",
                "this page documents how to take a screenshot of the dashboard at 1920x1080",
            ),
            (
                "mcp__github__get_file",
                "![hero](data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB)",
            ),
            (
                "computer",
                "Clicked at (882, 693)\n\nTab Context:\n- Executed on tabId: 1",
            ),
            (
                "mcp__claude-in-chrome__take_snapshot",
                "RootWebArea \"Inbox\"\n  button \"Compose\"\n  heading \"Messages\"",
            ),
            (
                "mcp__playwright__browser_snapshot",
                "- banner:\n  - heading \"Docs\" [level=1]\n  - link \"screenshot guide\"",
            ),
            (
                "query",
                "SELECT width, height FROM screenshots WHERE size = '800x600'",
            ),
        ];
        for (tool, body) in cases {
            assert!(
                !leave_uncompacted(tool, body),
                "false positive would skip fold: tool={tool} body={body}"
            );
            assert!(
                !text_looks_like_screenshot_capture(body),
                "body fingerprint false positive: {body}"
            );
        }
    }

    #[test]
    fn compact_for_tool_still_none_on_computer_so_proxy_gate_is_what_keeps_it() {
        let null = serde_json::Value::Null;
        let body = "Successfully captured screenshot (1098x538, jpeg) - ID: ss_8834s8965";
        // Name routing does not fire a fold — the keep-raw is leave_uncompacted.
        assert!(compact_for_tool("claude-in-chrome", "computer", &null, body).is_none());
        assert!(leave_uncompacted("computer", body));
    }

    #[test]
    fn auto_compact_rewrites_a_captured_computer_notice() {
        // Witness: a capture notice sitting in a document auto::compact
        // actually claims (markdown, three heading levels + a repeated list).
        // A short Tab-Context blob is Kind::Unknown and this assertion was
        // vacuous — folded == text — so the gate was untested on a rewrite.
        let notice = "Successfully captured screenshot (1098x538, jpeg) - ID: ss_8834s8965 — 0.7-scale view; coordinate frame: 1568x769.";
        let text = format!(
            "{notice}\n\n# Capture\n\n## Tab Context\n\n### Available tabs\n\n{}",
            "- tabId 1: filler title that makes a long chrome blob\n".repeat(80)
        );
        let (_kind, folded, _) = crate::auto::compact(&text);
        assert!(
            folded != text,
            "this fixture is only useful if auto::compact would rewrite it"
        );
        assert!(
            leave_uncompacted("computer", &text),
            "gate must catch the fixture the fold would rewrite"
        );
    }

    // The corpus lives under `experiments/small-decision-bench/`, gitignored
    // (captured MCP outputs). Run by default, this failed on every clone that
    // lacks it — the release VMs once they were sent the commit instead of
    // the working tree (2026-10-03).
    #[test]
    #[ignore = "needs the gitignored mcp_quality corpus (experiments/small-decision-bench/data); run with --ignored where it exists"]
    fn gate_on_mcp_quality_corpus_false_positives_are_zero() {
        // Body gold = the same four capture phrases the gate uses. Name-only
        // fires are counted separately (screenshot tools, not body FPs).
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = root.join("../../experiments/small-decision-bench/data");
        let mut n = 0u32;
        let mut captures = 0u32;
        let mut gate = 0u32;
        let mut fold_would_change = 0u32;
        let mut capture_would_change = 0u32;
        let mut body_fp = 0u32;
        let mut skipped_useful_fold = 0u32;
        let mut named_only = 0u32;
        for split in ["mcp_quality.train.jsonl", "mcp_quality.test.jsonl"] {
            let path = dir.join(split);
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in raw.lines() {
                if line.is_empty() {
                    continue;
                }
                let v: serde_json::Value = serde_json::from_str(line).expect("jsonl");
                let body = v["state"].as_str().unwrap_or("");
                let tool = v["tool"].as_str().unwrap_or("");
                n += 1;
                let is_capture = text_looks_like_screenshot_capture(body);
                let fires = leave_uncompacted(tool, body);
                if is_capture {
                    captures += 1;
                }
                if fires {
                    gate += 1;
                }
                if fires && !is_capture {
                    if screenshot_tool_name(tool) {
                        named_only += 1;
                    } else {
                        body_fp += 1;
                    }
                }
                let folded = crate::auto::compact(body).1;
                let changed = folded != body;
                if changed {
                    fold_would_change += 1;
                }
                if is_capture && changed {
                    capture_would_change += 1;
                }
                if fires && !is_capture && changed {
                    skipped_useful_fold += 1;
                }
            }
        }
        assert!(
            n >= 48,
            "mcp_quality jsonl missing under experiments/small-decision-bench/data (n={n})"
        );
        eprintln!(
            "mcp_quality n={n} captures={captures} gate={gate} \
             auto_would_change={fold_would_change} capture_would_change={capture_would_change} \
             body_fp={body_fp} named_only={named_only} skipped_useful_fold={skipped_useful_fold}"
        );
        assert_eq!(body_fp, 0, "body fingerprint fired off a capture notice");
        // Every capture the fold would rewrite is caught.
        assert!(
            capture_would_change > 0,
            "corpus has no capture that auto::compact rewrites — gate would be a no-op"
        );
    }
}
