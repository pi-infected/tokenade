//! JSON Lines (NDJSON) log-stream compaction.
//!
//! Modern cloud platforms (CloudWatch, GCP Cloud Logging, Datadog, Vercel,
//! Fly.io, Loki, Sentry, …) emit logs as one JSON object per line:
//!
//! ```text
//! {"timestamp":"2024-01-15T10:00:00Z","level":"info","message":"Server started","service":"api"}
//! {"timestamp":"2024-01-15T10:00:01Z","level":"info","message":"Handling /v1/users","duration_ms":12}
//! {"timestamp":"2024-01-15T10:00:02Z","level":"error","message":"DB connection failed","err":"timeout"}
//! …
//! ```
//!
//! A real production day's logs run 10⁵ – 10⁷ lines. The agent reading them
//! wants:
//!
//!   - **count** by severity (errors, warnings, info, debug)
//!   - **schema**: union of keys across the stream (what fields exist)
//!   - **first/last few lines** for context boundaries
//!   - **every error and warning** verbatim — diagnostic gold
//!
//! Detection: ≥3 line-starts with `{` whose `{`/`}` braces balance, AND
//! those make up ≥70 % of non-blank head lines. This rejects:
//!   * single-document JSON (only one `{…}` line in head)
//!   * JSON arrays `[{…},{…}]` (no per-line objects)
//!   * pretty-printed JSON (only one `{` at the start, rest is indented)

use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Write;

pub const SAMPLE_FIRST: usize = 3;
pub const SAMPLE_LAST: usize = 2;
pub const ERRORS_CAP: usize = 100;
pub const WARNINGS_CAP: usize = 50;
pub const KEYS_CAP: usize = 60;

pub fn detect(text: &str) -> bool {
    let head: &str = text.get(..text.len().min(16_384)).unwrap_or(text);
    let mut json_object_lines = 0usize;
    let mut total_nonblank = 0usize;
    for line in head.lines().take(80) {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        total_nonblank += 1;
        if t.starts_with('{') && t.ends_with('}') && braces_balance(t) {
            json_object_lines += 1;
        }
    }
    json_object_lines >= 3 && total_nonblank > 0 && (json_object_lines * 100) / total_nonblank >= 70
}

/// Cheap brace-balance check that respects string quoting + escape sequences.
/// Used at detect time to avoid false-classifying a truncated `{…},` line
/// (e.g. the first line of a pretty-printed JSON array) as a complete object.
fn braces_balance(s: &str) -> bool {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;
    for c in s.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn classify_level(s: &str) -> Level {
    match s.to_ascii_lowercase().as_str() {
        "error" | "err" | "fatal" | "critical" | "crit" | "alert" | "emerg" | "emergency"
        | "panic" => Level::Error,
        "warn" | "warning" => Level::Warning,
        "info" | "notice" | "information" => Level::Info,
        "debug" | "trace" => Level::Debug,
        _ => Level::Other,
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Level {
    Error,
    Warning,
    Info,
    Debug,
    Other,
}

/// Extract the severity from a parsed JSON object, checking the usual
/// suspects in priority order. Returns `Level::Other` when no severity-like
/// field is present.
fn extract_level(obj: &serde_json::Map<String, Value>) -> Level {
    // Most common conventions, in the order log libraries adopt them.
    const KEYS: &[&str] = &[
        "level",
        "severity",
        "lvl",
        "Level",
        "Severity",
        "log.level",
        "@l",
        "loglevel",
    ];
    for k in KEYS {
        if let Some(v) = obj.get(*k).and_then(|x| x.as_str()) {
            return classify_level(v);
        }
        // Numeric severity (syslog convention: 0-7, lower = more severe).
        if let Some(n) = obj.get(*k).and_then(|x| x.as_u64()) {
            if n <= 3 {
                return Level::Error;
            }
            if n == 4 {
                return Level::Warning;
            }
            if n == 5 || n == 6 {
                return Level::Info;
            }
            return Level::Debug;
        }
    }
    Level::Other
}

pub fn compact(text: &str) -> String {
    if !detect(text) {
        return text.to_string();
    }

    // Collect non-blank lines once so we can also do a last-N pass cheaply.
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect();

    let mut total_json = 0usize;
    // Three `continue`s below drop a line without a word: a line that
    // does not open on `{`, one that opens on `{` and does not parse, and one
    // that parses to something other than an object. The header then says
    // "N JSON lines" about a document with more lines than that, and `other`
    // is a LOG LEVEL, not a home for these. Measured on a 13-line document
    // carrying one plain-text line: "12 JSON lines", and nothing said the
    // thirteenth had existed.
    let mut unparsed = 0usize;
    let mut total_errors = 0usize;
    let mut total_warnings = 0usize;
    let mut total_info = 0usize;
    let mut total_debug = 0usize;
    let mut total_other = 0usize;
    let mut all_keys: BTreeSet<String> = BTreeSet::new();
    let mut samples_first: Vec<String> = Vec::new();
    let mut errors_kept: Vec<String> = Vec::new();
    let mut warnings_kept: Vec<String> = Vec::new();
    // One representative line per distinct categorical value — keeps event-type
    // diversity visible even when the stream is dominated by one type.
    const EVENT_KEYS: &[&str] = &["event", "event_type", "type", "action", "kind", "@type"];
    let mut event_type_order: Vec<String> = Vec::new();
    let mut event_type_samples: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for raw in &lines {
        let line = raw.trim();
        if !line.starts_with('{') {
            unparsed += 1;
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                unparsed += 1;
                continue;
            }
        };
        let obj = match v.as_object() {
            Some(o) => o,
            None => {
                unparsed += 1;
                continue;
            }
        };
        total_json += 1;

        if all_keys.len() < KEYS_CAP {
            for k in obj.keys() {
                if all_keys.len() >= KEYS_CAP {
                    break;
                }
                all_keys.insert(k.clone());
            }
        }

        let level = extract_level(obj);
        match level {
            Level::Error => {
                total_errors += 1;
                if errors_kept.len() < ERRORS_CAP {
                    errors_kept.push(line.to_string());
                }
            }
            Level::Warning => {
                total_warnings += 1;
                if warnings_kept.len() < WARNINGS_CAP {
                    warnings_kept.push(line.to_string());
                }
            }
            Level::Info => total_info += 1,
            Level::Debug => total_debug += 1,
            Level::Other => total_other += 1,
        }

        if samples_first.len() < SAMPLE_FIRST {
            samples_first.push(line.to_string());
        }

        // Diversity sampling: keep one line per distinct categorical value.
        for k in EVENT_KEYS {
            if let Some(v) = obj.get(*k).and_then(|x| x.as_str()) {
                if !event_type_samples.contains_key(v) {
                    event_type_order.push(v.to_string());
                    event_type_samples.insert(v.to_string(), line.to_string());
                }
                break;
            }
        }
    }

    // Walk from the end to grab the trailing SAMPLE_LAST JSON object lines.
    let mut samples_last: Vec<String> = Vec::new();
    for raw in lines.iter().rev() {
        if samples_last.len() >= SAMPLE_LAST {
            break;
        }
        let line = raw.trim();
        if !line.starts_with('{') {
            continue;
        }
        if serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|v| v.as_object().map(|_| ()))
            .is_some()
        {
            samples_last.push(line.to_string());
        }
    }
    samples_last.reverse();

    // ---- Render ----
    // Records, not a log: no levels, no event kinds, every line JSON.
    if total_errors + total_warnings + total_info + total_debug == 0
        && event_type_order.len() <= 1
        && unparsed == 0
        && lines.len() >= 10
    {
        let objs: Option<Vec<serde_json::Map<String, Value>>> = lines
            .iter()
            .map(|l| {
                serde_json::from_str::<Value>(l.trim())
                    .ok()
                    .and_then(|v| v.as_object().cloned())
            })
            .collect();
        if let Some(table) = objs.and_then(|o| crate::json::records_table(&o, text, "JSON records"))
        {
            return table;
        }
    }

    let mut out = String::with_capacity(4096);
    let _ = writeln!(
        out,
        "{total_json} JSON lines · {total_errors} error · {total_warnings} warning · {total_info} info · {total_debug} debug · {total_other} other"
    );
    // The empty-render rule, and here it has a second edge. The header is written
    // unconditionally, so this render is never empty and the empty floor
    // upstream cannot help; what protects a document of pure prose claimed as
    // NDJSON is the all-zero-counts floor. A disclosure carrying a NON-ZERO
    // count would defeat exactly that floor and hand back one line of elision
    // in place of the document. So it is posted only once something did parse.
    if unparsed > 0 && total_json > 0 {
        let _ = writeln!(out, "… {unparsed} line(s) not JSON, elided …");
    }

    if !all_keys.is_empty() {
        let listed: Vec<&str> = all_keys.iter().map(String::as_str).collect();
        let _ = write!(
            out,
            "\nkeys ({}{}): {}\n",
            all_keys.len(),
            if all_keys.len() >= KEYS_CAP { "+" } else { "" },
            listed.join(", ")
        );
    }

    if event_type_order.len() > 1 {
        let _ = write!(out, "\nevent types ({}):\n", event_type_order.len());
        for key in &event_type_order {
            if let Some(sample) = event_type_samples.get(key) {
                let _ = writeln!(out, "  [{key}] {sample}");
            }
        }
    }

    if !samples_first.is_empty() {
        let _ = write!(out, "\nfirst {} line(s):\n", samples_first.len());
        for s in &samples_first {
            out.push_str("  ");
            out.push_str(s);
            out.push('\n');
        }
    }

    crate::archetypes::capped_section_with_header(
        &mut out,
        &format!("errors ({total_errors} total, {} shown)", errors_kept.len()),
        &errors_kept,
        total_errors,
        "error(s)",
    );

    crate::archetypes::capped_section_with_header(
        &mut out,
        &format!(
            "warnings ({total_warnings} total, {} shown)",
            warnings_kept.len()
        ),
        &warnings_kept,
        total_warnings,
        "warning(s)",
    );

    // The MIDDLE, which is where every line that is neither sampled nor an
    // error nor a warning went.
    //
    // An earlier fix gave this compactor a counter for lines that are not JSON at all.
    // A perfectly valid line whose value happens to be the one an agent is
    // looking for has no counter: it falls between `first 3 line(s)` and
    // `last 2 line(s)`, and those two headings imply a middle without ever
    // naming one. Measured 2026-08-27 by substituting one field's value
    // on a middle line: gone on both doors, and nothing in 230 tokens of
    // output says a line went away.
    //
    // The header already carries the TOTAL, so this is the second half of a
    // subtraction the reader should not have to do.
    let sampled = samples_first.len()
        + if total_json > SAMPLE_FIRST + SAMPLE_LAST {
            samples_last.len()
        } else {
            0
        };
    let middle = total_json
        .saturating_sub(sampled)
        .saturating_sub(errors_kept.len())
        .saturating_sub(warnings_kept.len());
    if middle > 0 {
        let _ = writeln!(out, "\n… {middle} line(s) elided between the samples above");
    }

    if !samples_last.is_empty() && total_json > SAMPLE_FIRST + SAMPLE_LAST {
        let _ = write!(out, "\nlast {} line(s):\n", samples_last.len());
        for s in &samples_last {
            out.push_str("  ");
            out.push_str(s);
            out.push('\n');
        }
    }

    out.trim_end().to_string()
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NDJSON_SAMPLE: &str = r#"{"timestamp":"2024-01-15T10:00:00Z","level":"info","message":"Server started","service":"api"}
{"timestamp":"2024-01-15T10:00:01Z","level":"info","message":"Handling /v1/users","duration_ms":12}
{"timestamp":"2024-01-15T10:00:02Z","level":"warn","message":"Cache miss","key":"user-1234"}
{"timestamp":"2024-01-15T10:00:03Z","level":"error","message":"DB connection failed","err":"timeout","retries":3}
{"timestamp":"2024-01-15T10:00:04Z","level":"info","message":"Reconnected","duration_ms":150}
{"timestamp":"2024-01-15T10:00:05Z","level":"info","message":"Handling /v1/posts","duration_ms":8}
{"timestamp":"2024-01-15T10:00:06Z","level":"debug","message":"Cache key lookup","key":"posts-recent"}
{"timestamp":"2024-01-15T10:00:07Z","level":"warning","message":"Rate limit approaching","client":"1.2.3.4"}
{"timestamp":"2024-01-15T10:00:08Z","level":"info","message":"Shutting down"}
"#;

    #[test]
    fn detects_ndjson() {
        assert!(detect(NDJSON_SAMPLE));
    }

    #[test]
    fn rejects_single_document_json() {
        // Pretty-printed single JSON document — only one `{` line.
        let s = "{\n  \"foo\": 1,\n  \"bar\": [2, 3]\n}\n";
        assert!(!detect(s));
    }

    #[test]
    fn rejects_json_array() {
        // Multi-line JSON array — first line is `[`, not `{`.
        let s = "[\n  {\"a\":1},\n  {\"b\":2}\n]\n";
        assert!(!detect(s));
    }

    #[test]
    fn rejects_too_few_json_lines() {
        // 2 JSON lines + other prose — below the 3-line threshold.
        let s = "Some explanation\n{\"a\":1}\n{\"b\":2}\nMore text\n";
        assert!(!detect(s));
    }

    #[test]
    fn rejects_mostly_prose() {
        // 1 JSON line buried in prose.
        let s = "line 1\nline 2\nline 3\n{\"a\":1}\nline 5\n";
        assert!(!detect(s));
    }

    #[test]
    fn rejects_random_text() {
        assert!(!detect("Random text without any JSON\n"));
    }

    #[test]
    fn extracts_counts_and_samples() {
        let out = compact(NDJSON_SAMPLE);
        /* L122: banner removed; assertion deleted */
        // Keys union: timestamp, level, message, service, duration_ms, key,
        // err, retries, client = 9 unique keys.
        assert!(out.contains("keys (9): "));
        assert!(out.contains("timestamp"));
        assert!(out.contains("level"));
        assert!(out.contains("duration_ms"));
        assert!(out.contains("err"));
        // Error line preserved verbatim.
        assert!(out.contains("DB connection failed"));
        assert!(out.contains("\"retries\":3"));
        // Both warnings preserved.
        assert!(out.contains("Cache miss"));
        assert!(out.contains("Rate limit approaching"));
        // First samples present.
        assert!(out.contains("Server started"));
        // Last samples present.
        assert!(out.contains("Shutting down"));
    }

    #[test]
    fn handles_severity_aliases() {
        let s = r#"{"sev":"err","msg":"oops"}
{"severity":"WARN","msg":"careful"}
{"@l":"Information","msg":"normal"}
{"loglevel":"DEBUG","msg":"trace"}
{"level":"emerg","msg":"system down"}
"#;
        // sev (not in our list) → other. WARN → warning. @l Information → info.
        // loglevel DEBUG → debug. emerg → error. That's 1 error, 1 warn, 1 info,
        // 1 debug, 1 other.
        let out = compact(s);
        assert!(out.contains("5 JSON lines · 1 error · 1 warning · 1 info · 1 debug · 1 other"));
    }

    #[test]
    fn handles_numeric_severity() {
        let s = r#"{"level":3,"msg":"critical"}
{"level":4,"msg":"warning"}
{"level":6,"msg":"info"}
{"level":7,"msg":"debug"}
{"a":1,"b":2}
"#;
        // syslog: 0-3 error, 4 warn, 5-6 info, 7+ debug.
        let out = compact(s);
        assert!(out.contains("5 JSON lines · 1 error · 1 warning · 1 info · 1 debug · 1 other"));
    }

    #[test]
    fn unknown_passes_through() {
        let t = "not ndjson";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn handles_unicode_in_messages() {
        let s = r#"{"level":"error","message":"Failed: 中文 éàü München"}
{"level":"info","message":"OK 🚀"}
{"level":"warn","message":"⚠️ slow"}
{"level":"info","message":"done"}
"#;
        let out = compact(s);
        assert!(out.contains("中文 éàü München"));
        assert!(out.contains('🚀'));
        assert!(out.contains("⚠️ slow"));
    }

    #[test]
    fn handles_braces_in_quoted_strings() {
        // Quoted strings can contain `{` and `}` — the detector's
        // brace-balance must respect quoting.
        let s = r#"{"msg":"object literal: {a: 1}","level":"info"}
{"msg":"another {nested} {braces}","level":"info"}
{"msg":"third","level":"info"}
"#;
        assert!(detect(s));
        let out = compact(s);
        assert!(out.contains("3 JSON lines"));
    }

    #[test]
    fn savings_huge_stream() {
        // 5 000-line synthetic CloudWatch-style log: ~95 % INFO with a few
        // errors interspersed.
        let mut s = String::with_capacity(800_000);
        for i in 0..5_000 {
            let level = match i % 100 {
                0 => "error",
                17 => "warn",
                _ => "info",
            };
            let _ = writeln!(
                s,
                "{{\"timestamp\":\"2024-01-15T10:{:02}:{:02}Z\",\"level\":\"{level}\",\"message\":\"event #{i}\",\"request_id\":\"req-{i}\",\"duration_ms\":{}}}",
                (i / 60) % 60,
                i % 60,
                i % 200
            );
        }
        let (out, sav) = compact_with_savings(&s);
        assert!(out.contains("5000 JSON lines"));
        assert!(out.contains("50 error"));
        assert!(out.contains("50 warning"));
        assert!(sav.percent() > 90.0, "got {sav:?}");
    }

    /// Three `continue`s drop a line without a word. A 13-line document
    /// with one plain-text line came back saying "12 JSON lines", and `other`
    /// is a log LEVEL, not a home for unparsed lines — the thirteenth simply
    /// never existed as far as the reader could tell. Found by the router's
    /// fuzzer the moment NDJSON got a generator (three seeds in four hundred,
    /// plus two where the NDJSON section sat inside a mixed document).
    #[test]
    fn a_line_that_is_not_json_is_disclosed() {
        let mut lines: Vec<String> = (0..12)
            .map(|i| format!("{{\"level\":\"info\",\"svc\":\"alpha\",\"n\":{i}}}"))
            .collect();
        lines.insert(
            6,
            "NEEDLE-0123456789abcdef unique diagnostic marker".to_string(),
        );
        // The second shape the counter must catch: a line that DOES open on
        // `{` and does not parse. A different `continue`, and truncation makes
        // it the common one in a real stream.
        lines.insert(9, "{\"level\":\"info\",\"svc\":\"alp".to_string());
        let out = super::compact(&lines.join("\n"));
        assert!(out.contains("12 JSON lines"), "still a real fold: {out}");
        assert!(
            out.contains("… 2 line(s) not JSON, elided …"),
            "a dropped line is not disclosed: {out}"
        );
    }

    /// A measured NEGATIVE, so nobody writes a fixture for it and gives up.
    /// The third `continue` — a line that parses to something that is not an
    /// object — is UNREACHABLE from where it sits: the branch above it already
    /// required the line to start with `{`, and a JSON document that starts
    /// with `{` and parses at all IS an object. The counter stays there for
    /// symmetry, and this test states why it can never be exercised.
    #[test]
    fn a_line_opening_on_a_brace_never_parses_to_a_non_object() {
        for line in ["{}", "{\"a\":1}", "{ }", "{\"a\":[1,2]}"] {
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(line);
            assert!(
                parsed.map(|v| v.is_object()).unwrap_or(true),
                "{line:?} would make the third continue reachable"
            );
        }
    }

    /// The empty-render rule, with the edge this compactor adds. Its header is written
    /// unconditionally, so the render is NEVER empty and the empty floor
    /// upstream cannot protect a document of prose claimed as NDJSON. What
    /// protects it is the all-zero-counts floor — and a disclosure carrying a
    /// non-zero count would defeat exactly that, handing back one line of
    /// elision in place of the document.
    #[test]
    fn the_disclosure_never_defeats_the_all_zero_floor() {
        // Detected: every line opens on `{`, closes on `}` and balances.
        // Parsed: none of them. Long enough to clear the floor's own minimum
        // input size, or the fixture would prove nothing about the floor.
        let doc: String = (0..40)
            .map(|i| format!("{{ligne de prose ordinaire numero {i} sans guillemets}}\n"))
            .collect();
        let doc = doc.as_str();
        assert!(
            super::detect(doc),
            "fixture no longer reaches the NDJSON rebuild"
        );
        assert!(
            crate::tokenizer::estimate(doc) >= 100,
            "fixture too small to reach the all-zero floor, which has a minimum input size"
        );
        let out = super::compact(doc);
        assert!(
            !out.contains("elided"),
            "nothing parsed, so nothing may be claimed as folded: {out}"
        );
        for run in out.split(|c: char| !c.is_ascii_digit()) {
            assert!(
                run.is_empty() || run.bytes().all(|b| b == b'0'),
                "a non-zero count here defeats the floor that returns the input: {out}"
            );
        }
        // The end-to-end consequence — the document comes back WHOLE through
        // the router — is in
        // `tests/misc/a_format_is_reached_by_its_router.rs`, with this
        // fixture.
    }

    /// A playlist dump: titles and ids in the table, the constant channel once,
    /// the URL that only wraps the id and the duration's second spelling out.
    #[test]
    fn records_read_as_a_table_of_what_varies() {
        let mut text = String::new();
        for i in 1..=12 {
            let d = [725, 834, 725][i % 3];
            let ds = if d == 725 { "12:05" } else { "13:54" };
            text.push_str(&format!(
                "{{\"title\": \"Challenge {i}\", \"thumbnails\": [{{\"url\": \"t\"}}], \"id\": \"vid{i:03}\", \"url\": \"https://y/watch?v=vid{i:03}\", \"duration\": {d}, \"duration_string\": \"{ds}\", \"index\": {i}, \"autonumber\": {i}, \"playlist\": \"Coding\"}}\n"
            ));
        }
        let out = compact(&text);
        assert!(
            out.starts_with("12 JSON records · 9 keys\nsame on every record: playlist=Coding\n"),
            "{out}"
        );
        assert!(out.contains("restating a column (not repeated): "), "{out}");
        // One of each pair that restate each other goes; which one is a
        // matter of rank (`13:54` or `834` say the same thing).
        let restating = out.lines().nth(2).unwrap_or("");
        assert!(restating.contains("url"), "{out}");
        assert!(restating.contains("duration"), "{out}");
        assert!(
            restating.contains("autonumber") || restating.contains("index"),
            "{out}"
        );
        assert!(out.contains("nested, not shown: thumbnails"), "{out}");
        let row = out
            .lines()
            .find(|l| l.contains("Challenge 7 "))
            .unwrap_or("");
        assert!(
            row.contains("| vid007 |") && (row.contains("| 834 |") || row.contains("| 13:54 |")),
            "{out}"
        );
    }
}
