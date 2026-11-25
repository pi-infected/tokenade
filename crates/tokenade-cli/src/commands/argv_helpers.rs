//! **Reading an argv: bounds, values, and refusals.**
//!
//! The primitives several commands share — a flag and its value, an input
//! that must be stdin OR a single file, and the question "does this
//! subcommand touch content?" on which compaction depends.

/// `validate_simple_bool_flags` but for compactor commands that take
/// `[<file> | -]` — i.e. 0 or 1 positional + stdin fallback. Rejects
/// unknown flags and extra positionals up front so typos (`tokenade
/// dns --json file`, `tokenade systemd file extra`) no longer get
/// silently dropped. Returns `Ok(Some(path))` for an explicit file,
/// `Ok(None)` for stdin mode (no positional OR positional == "-").
///
/// Why: the cmd_dns / cmd_systemd / cmd_netstat / cmd_filelist / …
/// family all called `rest.first()` and ignored everything past
/// index 0 — same class as #655-#657 but spread across ~30 handlers.
pub(crate) fn validate_stdin_or_single_file(
    rest: &[String],
    cmd: &str,
    allowed_flags: &[&str],
) -> Result<Option<String>, i32> {
    validate_stdin_or_single_file_with_values(rest, cmd, allowed_flags, &[])
}

/// Like `validate_stdin_or_single_file` but also recognises value-flags
/// (e.g. `--limit N`). Each name in `value_flags` is allowed AND its
/// next arg is consumed as the flag's value instead of being treated
/// as a positional. Needed by cmd_serp / cmd_execute-style handlers
/// that mix bool-flags with `--key VALUE` syntax.
pub(crate) fn validate_stdin_or_single_file_with_values(
    rest: &[String],
    cmd: &str,
    allowed_bool_flags: &[&str],
    allowed_value_flags: &[&str],
) -> Result<Option<String>, i32> {
    let mut found: Option<&String> = None;
    let mut i = 0;
    while i < rest.len() {
        let a = &rest[i];
        if a.starts_with("--") || (a.starts_with('-') && a.len() > 1 && a != "-") {
            if allowed_value_flags.contains(&a.as_str()) {
                // Consume the value argument too.
                i += 2;
                continue;
            }
            if !allowed_bool_flags.contains(&a.as_str()) {
                eprintln!("tokenade {cmd}: unknown flag '{a}'. Try `tokenade {cmd} --help`.");
                return Err(2);
            }
        } else if found.is_some() {
            eprintln!(
                "tokenade {cmd}: unexpected positional '{a}' (only one <file> accepted; \
                 use '-' for stdin)."
            );
            return Err(2);
        } else {
            found = Some(a);
        }
        i += 1;
    }
    Ok(match found {
        Some(p) if p == "-" => None,
        Some(p) => Some(p.clone()),
        None => None,
    })
}

/// Strip a leading `key=` and any surrounding quotes/backticks from a
/// recovery-banner argument. Agents copy the banner form verbatim — both
/// `expand-ref hash="abc"` and bare `expand-ref abc` must work. Hashes and
/// template_ids never contain `=`, so splitting on the first `=` is safe.
pub(crate) fn unkey(s: &str) -> &str {
    let s = s.trim();
    let s = s.split_once('=').map(|(_, v)| v).unwrap_or(s);
    s.trim_matches(|c| c == '"' || c == '\'' || c == '`')
}

/// Parse `--key N` or `--key=N` out of `rest`. Returns the first match.
pub(crate) fn flag_usize(rest: &[String], key: &str) -> Option<usize> {
    let eq = format!("{key}=");
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == key {
            return rest.get(i + 1).and_then(|v| v.parse().ok());
        }
        if let Some(v) = rest[i].strip_prefix(&eq) {
            return v.parse().ok();
        }
        i += 1;
    }
    None
}

/// Parse a `--flag <value>` pair out of an argv slice, returning the value
/// if present. Tiny and dep-free — we avoid clap to keep the binary small
/// and the help text readable.
///
/// #649 — a value that starts with `--` is treated as "no value"
/// because it's almost certainly another flag the user typed after
/// forgetting the actual argument. Single `-` is preserved (it's a
/// valid stdin marker for `--script -`). Without this filter,
/// `tokenade execute --lang --script foo` consumed `--script` as the
/// language name and the script flag's intent was lost.
pub(crate) fn get_flag<'a>(rest: &'a [String], flag: &str) -> Option<&'a str> {
    let i = rest.iter().position(|a| a == flag)?;
    let v = rest.get(i + 1)?.as_str();
    if v.starts_with("--") {
        return None;
    }
    Some(v)
}

/// Does this subcommand handle model-bound CONTENT, and therefore pay for the
/// redaction rules?
///
/// An allowlist, not a denylist: a new subcommand that forgets to appear here
/// is merely as slow as every subcommand was before 2026-08-08, whereas a new
/// bookkeeping command wrongly included would spawn a thread compiling 102
/// regexes it never consults. The wrong answer should cost latency, not work.
pub(crate) fn subcommand_touches_content(sub: Option<&str>) -> bool {
    matches!(
        sub,
        Some(
            "read"
                | "auto"
                | "filter"
                | "wrap"
                | "exec-compact"
                | "proxy"
                | "shellwrap"
                | "shim-exec"
                | "exec"
                | "web"
                | "search"
                | "mcp-wrap"
                | "mcp_wrap"
                | "mcp-proxy"
                | "mcp_proxy"
                | "mcp"
                | "subagent-brief"
                | "subagent_brief"
                | "session-tick"
                | "fold-payload"
                | "fold_payload"
        ) | Some("hookwrap")
    ) || sub.is_some_and(|s| s.starts_with("hookwrap-"))
}
