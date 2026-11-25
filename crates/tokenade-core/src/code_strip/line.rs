//! **One line at a time** — what the OTHER modules call.
//!
//! Twelve format compactors need the same question: « does this line
//! carry a comment, and where does it start? ». They want neither the
//! whole file nor the strip mode, just the line.
//!
//! This is the primitive, and it is here because it used to be elsewhere: placed
//! in a product layer, it pulled nine format modules up the
//! graph and on its own held part of the cycle.

use super::*;

/// Strip `/* … */` block comments from ONE line, carrying the "still inside a
/// block" state across lines, and never mistaking a string literal for a
/// comment. `quotes` is the set of characters that OPEN a string in the target
/// language; a backslash escapes the next character inside one.
///
/// This existed three times — `proto`, `sql_source`, `hcl` — two of them
/// byte-identical apart from the quote set, and the third with no string
/// awareness at all, which meant a `/*` inside an HCL string value was eaten as
/// a comment. Three copies also meant the `bytes[i] as char` corruption had to
/// be found and fixed three times.
pub(crate) fn strip_block_comment_line(
    line: &str,
    mut in_block: bool,
    quotes: &[u8],
) -> (String, bool) {
    let mut out: Vec<u8> = Vec::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut in_string: Option<u8> = None;
    while i < bytes.len() {
        if in_block {
            if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                in_block = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        let c = bytes[i];
        if c == b'\\' && i + 1 < bytes.len() {
            out.push(bytes[i]);
            out.push(bytes[i + 1]);
            i += 2;
            continue;
        }
        match in_string {
            Some(q) => {
                if c == q {
                    in_string = None;
                }
                out.push(c);
                i += 1;
            }
            None => {
                if quotes.contains(&c) {
                    in_string = Some(c);
                    out.push(c);
                    i += 1;
                    continue;
                }
                if i + 1 < bytes.len() && c == b'/' && bytes[i + 1] == b'*' {
                    in_block = true;
                    i += 2;
                    continue;
                }
                out.push(c);
                i += 1;
            }
        }
    }
    (crate::text_encoding::utf8_owned(out), in_block)
}

/// Find where an inline comment starts on ONE line, or `None` when the line has
/// no comment. String literals are tracked so a marker inside a value is not
/// mistaken for one.
///
/// * `markers` — characters that begin a comment outside a string (`#`, `;`, …).
/// * `quotes` — characters that open and close a string.
/// * `escaping` — the subset of `quotes` inside which a backslash escapes the
///   next byte. TOML literal strings and INI single quotes do NOT escape, so
///   `'C:\path'` must not swallow the closing quote.
/// * `escape_outside_strings` — whether a backslash escapes the next byte even
///   when no string is open. CMake does escape in UNQUOTED arguments
///   (`set(A a\#b)`), so a `#` behind a backslash is not a comment there.
/// * `confirm` — final say on a candidate: INI treats `;` as a comment only
///   when whitespace and text follow it, because `value;` at end of line is an
///   nginx statement terminator.
///
/// Four modules had a private copy of this scan — `bazel`, `cmake`, `ini`,
/// `toml_compact` — each subtly different, and the differences were invisible
/// until they were put side by side. (`latex` keeps its own: `%` is escaped by
/// a backslash COUNT rather than by string context, which is a different
/// algorithm, not a different configuration.)
pub(crate) fn find_inline_comment(
    line: &str,
    markers: &[u8],
    quotes: &[u8],
    escaping: &[u8],
    escape_outside_strings: bool,
    confirm: impl Fn(u8, usize) -> bool,
) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut in_string: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match in_string {
            Some(q) => {
                if escaping.contains(&q) && c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if c == q {
                    in_string = None;
                }
            }
            None => {
                if escape_outside_strings && c == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if quotes.contains(&c) {
                    in_string = Some(c);
                } else if markers.contains(&c) && confirm(c, i) {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

/// [`find_inline_comment`], applied: everything before the marker, trailing
/// whitespace removed.
pub(crate) fn strip_inline_comment_line(
    line: &str,
    markers: &[u8],
    quotes: &[u8],
    escaping: &[u8],
    escape_outside_strings: bool,
    confirm: impl Fn(u8, usize) -> bool,
) -> String {
    match find_inline_comment(
        line,
        markers,
        quotes,
        escaping,
        escape_outside_strings,
        confirm,
    ) {
        Some(i) => line[..i].trim_end().to_string(),
        None => line.to_string(),
    }
}

/// Comment/blank-run stripper for a language with `/* … */` blocks: drop block
/// spans, drop whole-line comments, collapse blank runs to one, trim trailing
/// whitespace. Indentation is preserved BYTE-FOR-BYTE.
///
/// `proto.rs` and `sql_source.rs` carried this thirty-line loop twice. The
/// bodies differed by exactly one token — `"//"` against `"--"` — which is why
/// the marker is the parameter and nothing else is.
pub fn strip_block_language(
    text: &str,
    line_marker: &str,
    strip_block: fn(&str, bool) -> (String, bool),
    strip_inline: fn(&str) -> String,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;
    let mut in_block_comment = false;
    for line in text.lines() {
        let (cleaned, still_in) = strip_block(line, in_block_comment);
        in_block_comment = still_in;
        let trimmed = cleaned.trim_end();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
            continue;
        }
        let t = trimmed.trim_start();
        if t.starts_with(line_marker) {
            // Same question as the line-marker pass above, asked here so sql,
            // hcl and proto get it from one edit rather than three.
            if t.strip_prefix(line_marker)
                .is_some_and(comment_is_a_directive)
            {
                blank_run = 0;
                out.push_str(trimmed);
                out.push('\n');
            }
            continue;
        }
        let stripped = strip_inline(trimmed);
        let stripped_t = stripped.trim_end();
        if stripped_t.is_empty() {
            continue;
        }
        blank_run = 0;
        out.push_str(stripped_t);
        out.push('\n');
    }
    out
}

/// Comment/blank-run stripper for a line-comment language without block
/// spans: drop whole-line comments, collapse blank runs to one, trim trailing
/// whitespace. Indentation is preserved BYTE-FOR-BYTE.
///
/// `is_comment_line` decides what opens a comment (`#` for Bazel and CMake,
/// `#`/`;`/`//` for INI); `strip_inline` cuts a trailing comment off a code
/// line, and is the identity for languages that have none (Dockerfile).
///
/// Four modules carried this loop. Re-diffing the FULL bodies — not a sliding
/// window — is what separated them from the four that stay their own
/// (`bibtex` clamps indentation and comments with `%`, `makefile` has
/// tab-significant recipe lines, `latex` has verbatim environments,
/// `toml_compact` is a different shape entirely).
pub fn strip_line_language(
    text: &str,
    is_comment_line: fn(&str) -> bool,
    strip_inline: fn(&str) -> String,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
            continue;
        }
        let t = trimmed.trim_start();
        if is_comment_line(t) {
            // A DIRECTIVE is kept, whatever marker this language uses for its
            // comments. `# syntax=docker/dockerfile:1.7` selects the BuildKit
            // frontend, so a Dockerfile served without it is built by a
            // different parser than the one on disk. Asked HERE, in the shared
            // line pass, so bazel, cmake, dockerfile and ini get it from one
            // edit instead of four.
            if comment_is_a_directive(t.trim_start_matches(['#', ';', '/', '-'])) {
                blank_run = 0;
                out.push_str(trimmed);
                out.push('\n');
            }
            continue;
        }
        let stripped = strip_inline(trimmed);
        if stripped.is_empty() {
            continue;
        }
        blank_run = 0;
        out.push_str(&stripped);
        out.push('\n');
    }
    out
}

/// Declare that a comment stripper removed something.
///
/// A comment is where the *why* lives — an SPDX licence, `# pinned by security
/// policy`, `-- rollback: DROP INDEX …`, `// wire-compat: do NOT renumber` —
/// and an agent reading a silently stripped file believes it has the whole
/// file. §"HCL/Terraform" fixed this on ONE compactor in 2026-08-04 and the
/// note stayed there. Measured 2026-08-26, every language below stripped every
/// comment with no marker, no count and no pointer:
/// `.sql` `.proto` `.bzl` `.ini` `.cmake` `Dockerfile` `.toml`. A 449-byte
/// migration lost its licence, its identifier, a warning about an ACCESS
/// EXCLUSIVE lock, and its rollback statement, and came back looking whole.
///
/// One declarer rather than eight, so the next language added cannot quietly
/// skip it.
///
/// `exact_comment_lines` is for the compactor that counts them itself; the
/// others report the line delta, which is honest about its own imprecision
/// (blank-run collapsing is in there too) and still gives the magnitude.
///
/// The note is not free: on a short file the comments it discloses are worth
/// less than the sentence disclosing them, and the agent would pay to be told
/// about a saving it never got. There the honest AND cheaper answer is the file
/// itself.
pub fn declare_strip(
    text: &str,
    out: String,
    tag: &str,
    exact_comment_lines: Option<usize>,
) -> String {
    // Bytes, not lines, decide whether anything went: an inline comment
    // trimmed off the end of a line removes text without removing a line.
    if out.len() >= text.len() {
        return out;
    }
    let noted = match exact_comment_lines {
        Some(0) => return out,
        Some(n) => format!(
            "[tokenade:{tag}] {n} comment line(s) stripped; the file on disk is unchanged\n{out}"
        ),
        None => {
            let delta = text.lines().count().saturating_sub(out.lines().count());
            let what = if delta > 0 {
                format!("{delta} line(s) fewer")
            } else {
                "inline comments trimmed".to_string()
            };
            format!(
                "[tokenade:{tag}] comments stripped, {what}; the file on disk is unchanged\n{out}"
            )
        }
    };
    if noted.len() >= text.len() {
        return text.to_string();
    }
    noted
}
