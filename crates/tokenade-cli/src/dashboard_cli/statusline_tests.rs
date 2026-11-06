// The tests of `statusline`.
//
// Spliced in by `include!` from `statusline.rs`: the same module, hence the same
// private access and the same `super::` paths, without a single visibility line
// changed. This is the form this repository already uses for 33 other modules.
//
// A `//!` is impossible here: a file spliced in by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod host_link_contract {
    /// Every hyperlink this line emits must be terminated by BEL, not ST.
    ///
    /// Claude Code draws the status line through a width-truncation library
    /// whose sanitiser lets exactly two escapes through untouched — SGR colour,
    /// and OSC 8 ENDED BY BEL. Reproduced from the 2.1.263 bundle:
    ///
    /// ```text
    /// var se = /\x1b(?!\[[0-9;]*m|\]8;;[^\x07\x18\x1a\x1b]*\x07)|\x9b/;
    /// function TXn(e){ if(!se.test(e)) return e; /* …rewrites every ESC… */ }
    /// ```
    ///
    /// An ST-terminated link (`ESC \`) fails that lookahead, so the whole line
    /// takes the rewrite path and the link stops being clickable — which is
    /// exactly what "the ✕ does nothing" was. Measured 2026-09-06 by running
    /// that regex over our own output: 4 OSC 8 openers in, 0 out. The same
    /// bundle writes its own links with BEL (`NH="\x07"`).
    ///
    /// The regex is inlined here rather than referenced, because it is the
    /// HOST's contract: if a future Claude Code widens it, this test still
    /// states what we relied on and why.
    fn host_sanitiser_would_rewrite(line: &str) -> bool {
        // `\x1b` NOT followed by an SGR sequence or a BEL-ended OSC 8.
        // Over CHARS, never bytes: the host's regex runs on UTF-16 code units,
        // so `\x9b` means U+009B. Byte 0x9b is also the middle continuation
        // byte of `⛔` (E2 9B 94), which is on this very line — scanning bytes
        // reported the whole gate segment as doomed when nothing was wrong.
        for (i, c) in line.char_indices() {
            if c == '\u{9b}' {
                return true;
            }
            if c != '\u{1b}' {
                continue;
            }
            let rest = &line[i + 1..];
            let sgr = {
                let digits: &str = rest.strip_prefix('[').unwrap_or("");
                let end = digits
                    .find(|c: char| !(c.is_ascii_digit() || c == ';'))
                    .unwrap_or(digits.len());
                rest.starts_with('[') && digits[end..].starts_with('m')
            };
            let osc8_bel = rest.starts_with("]8;;")
                && rest["]8;;".len()..]
                    .find(['\u{7}', '\u{1b}', '\u{18}', '\u{1a}'])
                    .map(|j| rest["]8;;".len() + j..].starts_with('\u{7}'))
                    .unwrap_or(false);
            if !sgr && !osc8_bel {
                return true;
            }
        }
        false
    }

    /// A BEL link followed by an ST one must not have its url run to the later
    /// ST, swallowing the label and the second link whole.
    #[test]
    fn flatten_takes_whichever_terminator_comes_first() {
        let mixed = "\u{1b}]8;;https://a.test\u{7}A\u{1b}]8;;\u{7} \u{1b}]8;;https://b.test\u{1b}\\B\u{1b}]8;;\u{1b}\\";
        let flat = super::flatten_osc8(mixed);
        assert!(flat.contains("A (https://a.test)"), "first link lost: {flat:?}");
        assert!(flat.contains("B (https://b.test)"), "second link lost: {flat:?}");
        assert!(!flat.contains('\u{1b}'), "escapes survived flattening: {flat:?}");
    }

    /// The predicate has to actually catch the old form, or the two tests
    /// above pass for free.
    #[test]
    fn the_predicate_catches_an_st_terminated_link() {
        let st = "x \u{1b}]8;;https://e.test\u{1b}\\label\u{1b}]8;;\u{1b}\\ y";
        assert!(
            host_sanitiser_would_rewrite(st),
            "an ST-terminated link must be recognised as doomed"
        );
        let bel = "x \u{1b}]8;;https://e.test\u{7}label\u{1b}]8;;\u{7} y";
        assert!(!host_sanitiser_would_rewrite(bel), "a BEL one must pass");
        let colour = "\u{1b}[1mbold\u{1b}[22m";
        assert!(!host_sanitiser_would_rewrite(colour), "SGR colour must pass");
        assert!(!host_sanitiser_would_rewrite("plain text"), "plain text passes");
        assert!(
            host_sanitiser_would_rewrite("cursor \u{1b}[2J home"),
            "a non-SGR CSI is rewritten by the host"
        );
    }
}

#[cfg(test)]
mod plain_tests {
    use super::flatten_osc8;

    fn link(url: &str, label: &str) -> String {
        format!("\u{1b}]8;;{url}\u{1b}\\{label}\u{1b}]8;;\u{1b}\\")
    }

    #[test]
    fn a_link_becomes_label_then_url() {
        let s = format!(
            "⚡ 1k saved | {}",
            link("https://x.dev/a?b=1", "Refer a friend")
        );
        assert_eq!(
            flatten_osc8(&s),
            "⚡ 1k saved | Refer a friend (https://x.dev/a?b=1)"
        );
        assert!(!flatten_osc8(&s).contains('\u{1b}'));
    }

    #[test]
    fn several_links_and_the_text_between_them_survive() {
        let s = format!("a {} b {} c", link("u1", "L1"), link("u2", "L2"));
        assert_eq!(flatten_osc8(&s), "a L1 (u1) b L2 (u2) c");
    }

    #[test]
    fn a_bel_terminated_link_is_handled_too() {
        // xterm accepts BEL as the string terminator; some emitters use it.
        let s = "go \u{1b}]8;;https://x.dev\u{7}here\u{1b}]8;;\u{7}!";
        assert_eq!(flatten_osc8(s), "go here (https://x.dev)!");
    }

    #[test]
    fn a_label_that_already_shows_the_url_is_not_doubled() {
        let s = link("https://github.com/pi-infected/tokenade", "github.com/pi-infected/tokenade");
        assert_eq!(flatten_osc8(&s), "github.com/pi-infected/tokenade");
    }

    #[test]
    fn plain_text_is_returned_unchanged_and_a_truncated_escape_is_not_eaten() {
        assert_eq!(flatten_osc8("no links here"), "no links here");
        let torn = "before \u{1b}]8;;https://x.dev-and-then-nothing";
        assert_eq!(flatten_osc8(torn), torn);
    }
}

#[test]
fn a_multi_line_user_statusline_keeps_every_line() {
    // Reported: keeping line 1 only put our ticker in the middle of a
    // multi-line custom status line and dropped the lines below it.
    assert_eq!(
        original_statusline_text("model · dir  \nbranch main\ncost $1\n\n"),
        "model · dir\nbranch main\ncost $1"
    );
    assert_eq!(original_statusline_text(""), "");
    assert_eq!(original_statusline_text("\n\n"), "");
    assert_eq!(original_statusline_text("one"), "one");
}

#[cfg(test)]
mod wrapped_command_runs_as_the_host_runs_it {
    use super::*;
    use base64::Engine as _;

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    /// #10: `node "d:\x\statusline.mjs"` — a script path quoted, in the
    /// platform's own spelling (backslashes on Windows), with a space in it.
    #[test]
    fn a_quoted_native_path_runs() {
        let dir = std::env::temp_dir().join(format!("tk-sl spaced-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("status line.sh");
        std::fs::write(&script, "cat >/dev/null; echo \"model · dir\"\n").unwrap();
        let cmd = format!("sh \"{}\"", script.display());
        let out = run_original_statusline(&b64(&cmd), "{\"session_id\":\"s\"}");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(out, "model · dir", "{cmd}");
    }

    /// A failing wrapped command says so instead of vanishing.
    #[test]
    fn a_failing_command_names_itself_and_why() {
        let out = run_original_statusline(&b64("echo boom >&2; exit 3"), "{}");
        assert!(out.starts_with("⚠ status line `echo boom >&2; exit 3` failed: boom"), "{out}");
        // A command that fails but still prints keeps its output.
        let out = run_original_statusline(&b64("echo partial; exit 1"), "{}");
        assert_eq!(out, "partial");
    }
}
