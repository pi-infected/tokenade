//! **The SUMMARY that replaces what was stashed.**
//!
//! What the agent reads in place of the body. It must be faithful enough that it
//! does not need to come back, and short enough that the fold serves some
//! purpose — both at once. The bounds here are measured: a cut that
//! falls in the middle of a construct leaves an `[image:` never closed, and a
//! preview truncated at the byte cuts a word in two.

use super::*;

/// **Both ends of a preview, cut without breaking what they cut.**
///
/// This block was written TWICE, fifteen lines apart, and both
/// copies carried the same defect: the cut fell in the middle of a construct.
/// The original comment promised « can never cut a UTF-8 sequence »,
/// which is true at the BYTE level and false at the STRUCTURE level — an
/// `[image: …]` sliced in two returns a bracket that never closes, and the
/// reader sees an image whose description swallows the rest of the document.
/// MEASURED 2026-09-09 on the corpus's 39 pages: two of the ten orphan
/// brackets came from here, the other eight from `markdown::short_preview`.
///
/// The rule shared with `markdown` backs up to before the construct left
/// open. Backing up to a LINE boundary was tried and REMOVED: on
/// `investopedia`, the last line of the head is a section heading followed by
/// its preview, and throwing it away lost the heading — a falling total hid a
/// real loss, a price already paid once before.
///
/// The announced number is what was REALLY removed, recomputed after the cut:
/// the old `total - head_budget - tail_budget` announced the budgets and not the
/// sizes obtained, so it would have lied as soon as the cut backs up.
/// **A preview's tail starts at a LINE, never in the middle of a word.**
///
/// The tail cut falls at a character count, so at random: MEASURED
/// 2026-09-10 on the corpus's 39 pages, **9 of the 10 clamped pages** resumed
/// mid-word — `ch service to …")`, `s the expected type for…`,
/// `w/index.php?title=…&action=edit>)`. The fragment no longer has its section
/// heading, which stayed on the other side of the cut, and so it reads as
/// the continuation of what precedes the banner.
///
/// The symmetry with the HEAD is false, and that is what matters: backing the head up
/// to a line boundary throws away a WHOLE line — on investopedia,
/// a section heading and its preview — and that was removed for this reason.
/// Moving the tail forward only throws away a fragment whose start is ALREADY lost.
///
/// Nothing is moved forward when the cut already fell on a boundary, nor when the
/// tail carries no line break: better a fragment than nothing.
pub(super) fn start_the_tail_on_a_line<'a>(body: &str, start: usize, tail: &'a str) -> &'a str {
    if start == 0 || body.chars().nth(start.saturating_sub(1)) == Some('\n') {
        return tail;
    }
    match tail.find('\n') {
        Some(i) if i + 1 < tail.len() => &tail[i + 1..],
        _ => tail,
    }
}

pub(super) fn head_tail_preview(body: &str, head_budget: usize, tail_budget: usize) -> String {
    let total = body.chars().count();
    let head: String = body.chars().take(head_budget).collect();
    let head = crate::textshape::cut_before_an_unclosed_bracket(&head).to_string();
    let tail: String = body.chars().skip(total - tail_budget).collect();
    let tail = start_the_tail_on_a_line(body, total.saturating_sub(tail_budget), &tail);
    let dropped = total.saturating_sub(head.chars().count() + tail.chars().count());
    format!("{head}\n… (preview clamped, {dropped} chars elided — the full text is in the stash) …\n{tail}")
}

/// **The recovery menu, spelled in ONE place.**
///
/// Short by construction: this menu goes out on EVERY fold, and a line that
/// doubles in length is paid 265 times over the corpus. Measured: the first
/// draft cost +1 417 bytes, this one returns ~0.
///
/// MOVED DOWN into the core on 2026-09-14. It lived in `tokenade-cli`, so the
/// STASH footer written here could not call it and spelled its own by
/// hand: **483 characters versus 145**, the hash named THREE times, and without
/// `--budget 0` — the option that had just been added to the other one.
/// Two spellings of the same promise drift, and it is the longer one that
/// kept an option quiet.
pub const RAW_ADVICE: &str = "`--raw` bytes, bounded (`--budget 0` for all)";

/// The order of the fallbacks comes from the blob's SHAPE: on a stream of records
/// the exact line comes first, on prose it is the question.
pub fn recovery_menu(hash: &str, blob: &str) -> String {
    let (first, rest) = if crate::disclose::is_record_stream(blob) {
        (
            "`--grep <pat>` exact lines",
            "`--prompt \"q1, q2\"` to ask (RAG)",
        )
    } else {
        (
            "`--prompt \"q1, q2\"` to ask (RAG, several at once)",
            "`--grep <pat>` exact lines",
        )
    };
    // No punctuation glued after the hash: a consumer that splits
    // on `expand-ref ` brought back two characters too many in the command it
    // was going to rerun.
    // `--raw` is NOT here. The File footer already stopped promoting it
    // (107/111 bare dumps measured); gluing `{RAW_ADVICE}` onto the Shell arm
    // (and so onto wrap / too_large) sent the model back to the dump. It
    // stays in `--help` and in the refusal of a `--prompt` with no match.
    format!("recover with `tokenade expand-ref {hash}` · {first} · {rest}.")
}

// [`clamp_summary_body`] with an explicit budget, for one side of the preview.
pub fn clamp_body_to(body: String, budget: usize) -> String {
    if body.chars().count() <= budget {
        return body;
    }
    let head_budget = budget * 2 / 3;
    let tail_budget = budget - head_budget;
    head_tail_preview(&body, head_budget, tail_budget)
}

pub(super) fn clamp_summary_body(body: String) -> String {
    // A short answer is not improved by cutting its lines: if
    // the WHOLE body already fits the budget, a long line inside it costs
    // nothing and truncating it only destroys content. The per-line bound
    // exists to stop one pathological line from eating a budget that is
    // actually contested — so it applies only once the total is over.
    if body.chars().count() <= SUMMARY_MAX_BODY_CHARS {
        return body;
    }
    // Per-line bound first: it is what keeps the preview READABLE, and it
    // often brings the whole body under the total bound on its own.
    let body = if body
        .lines()
        .any(|l| l.chars().count() > SUMMARY_MAX_LINE_CHARS)
    {
        body.lines()
            .map(|l| {
                let n = l.chars().count();
                if n <= SUMMARY_MAX_LINE_CHARS {
                    return l.to_string();
                }
                let keep: String = l.chars().take(SUMMARY_MAX_LINE_CHARS).collect();
                format!("{keep}… (+{} chars)", n - SUMMARY_MAX_LINE_CHARS)
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        body
    };
    if body.chars().count() <= SUMMARY_MAX_BODY_CHARS {
        return body;
    }
    let head_budget = SUMMARY_MAX_BODY_CHARS * 2 / 3;
    let tail_budget = SUMMARY_MAX_BODY_CHARS - head_budget;
    head_tail_preview(&body, head_budget, tail_budget)
}

/// How many error lines the generic head+tail summary rescues from the middle
/// it drops. Bounded so an all-error log cannot turn the summary back into the
/// log; past the cap the stash is still the complete copy.
pub(super) const SUMMARY_RESCUED_ERROR_LINES: usize = 20;

/// What the delivered body IS, said in the stash footer — the agent decides
/// from it whether grepping the body can answer an exact question.
///
/// One phrase used to serve every mode: "folded summary (omits middle
/// lines)". True of the head-and-tail preview; false of an outline, a schema
/// fold or a diff plan, which hold no line of the body at all — an agent
/// reading "the middle is missing" expects the first and last lines to be
/// there, and greps them.
pub(super) const BODY_IS_HEAD_TAIL: &str = "folded summary (omits middle lines)";
pub(super) const BODY_IS_OUTLINE: &str = "outline only (no line of the body)";
pub(super) const BODY_IS_SCHEMA: &str = "schema summary (sample items only)";
pub(super) const BODY_IS_INDEX: &str = "help index (every option/command name, descriptions cut)";

pub(super) fn build_summary(
    text: &str,
    tokens: usize,
    stash_path: &Path,
    kind_ext: &str,
    hash: &str,
    hint: &SourceHint,
) -> String {
    build_summary_with_body(text, tokens, stash_path, kind_ext, hash, hint, None)
}

/// [`build_summary`], with the BODY supplied by the caller.
///
/// A batch of web pages is assembled then summarised as a single document, and the
/// outline of the whole is cut at head and tail: a page in the MIDDLE disappears
/// entirely. Measured on four pages, two were missing. The caller
/// is the only one who knows its document is made of N parts and what each
/// deserves; the stash remains that of the COMPLETE text, so
/// retrieval does not change.
pub(super) fn build_summary_with_body(
    text: &str,
    tokens: usize,
    stash_path: &Path,
    kind_ext: &str,
    hash: &str,
    hint: &SourceHint,
    override_body: Option<String>,
) -> String {
    // Delegation affordance on VERY large stashes (opencode-validated
    // pattern): expanding 20k+ tokens inline re-bills them every remaining
    // turn — a sub-agent reads the stash in ITS own context instead.
    let delegate_hint = if tokens >= 20_000 {
        " · LARGE: with a Task/subagent tool, delegate the analysis (sub-agent runs `tokenade expand-ref` in its own context) instead of expanding here"
    } else {
        ""
    };

    let how_to_query = parsing_hint_for(kind_ext);
    // #705 — companion to #704: now that `expand_ref` falls back to
    // the stash dir, advertise it explicitly in the footer alongside
    // the format-specific query hint. The agent can pick:
    //   - `how_to_query` (jq / xpath / grep): efficient structured
    //     access without re-shipping all the bytes.
    //   - `expand_ref hash=…`: full bytes through MCP, redacted and
    //     stash-skip-flagged so the response isn't re-folded.
    // Without this line agents either re-Read the stash file (works
    // but loses the redact pass and clutters the Read history) or
    // tried `expand_ref` and got "hash not found" pre-#704.
    let total_lines = text.lines().count();
    // #1051 — footer WORDING drives which recovery path the agent
    // picks, and the old wording mis-steered exact-match lookups. The
    // THOL `log-needle` benchmark caught the failure mode: a folded
    // `cat <log>` led the agent through 8× `search_stash` (BM25 — which
    // misses exact tokens like status codes / digests / package
    // versions) then grep on the STASH file (a head+tail preview, so it
    // misses every middle line) — ~14 wasted turns before it finally
    // grepped the ORIGINAL file directly. Two fixes here:
    //   1. When the source path is known (`File`), redirect exact
    //      lookups to grep/Read the ORIGINAL file — complete & verbatim,
    //      one turn, exactly what the control run did.
    //   2. Demote `search_stash` to "fuzzy / natural-language recall"
    //      and label it as NOT for exact values, so it stops being the
    //      reflex first choice on a needle-in-log search.
    // The summary itself is a head+tail (or schema/outline) fold, so we
    // also say plainly that it omits middle lines — grepping the stash
    // preview is a trap.
    // Question-FIRST: a ref is a document you ASK, not a dump handle. Lead with
    // the RAG path (targeted passages, several questions at once), keep grep of
    // the ORIGINAL for exact values, and demote raw bytes to a rarely-needed
    // escape — field data showed agents bare-dumping refs and re-inflating
    // context (107/111 recoveries, up to 30 KB) when a question was what they
    // needed.
    // The COMMAND variant used to send exact lookups back through
    // `tokenade raw <cmd>`, and that command REFUSES the programs whose output
    // is most often stashed: `grep`, `cat`, `sed`, `wc`, `ls`, `find`, `head`,
    // `tail`, `awk` all answer "refusing bypass for `X`" and point back here.
    // So the advice cost a turn to be told to come back. `--grep` is the verb
    // that answers an exact-value question without re-executing anything, and
    // a miss on it NAMES what it searched instead of reading as
    // "never printed" — which is what made grepping a preview a trap before.
    // The FILE variant keeps pointing at the original path: when the source is
    // known, reading it is complete, verbatim and cheaper still.
    // Lead with the SIZE IN TOKENS. The agent is deciding how to spend its
    // context; given the number it picks the right verb by itself (~800 tokens
    // → just read it; ~40k → obviously ask). That does the steering a nudge
    // alone could not, and it is why `--raw` is NOT advertised here: promoting
    // it invites the bare dump we measured 107/111 times. It stays discoverable
    // in `--help` and in the refusal message, where the size is already known.
    let footer_for = |what: &str| {
        match hint {
        // **The FILE arm keeps its own menu, and that is not an oversight.**
        //
        // The common menu offers `--grep` ON THE STASH, which is a head+tail
        // summary: a line dropped from the middle answers « 0 matches » there
        // while it exists. A false negative on the retrieval path
        // is worse than no pointer at all — the agent concludes the
        // string was never written. When the source is ON DISK, the
        // right answer to an exact search is the file: complete,
        // verbatim, and cheaper than a fallback through the hash.
        //
        // Unifying that arm with the other was tried on 2026-09-14 and reverted:
        // it is `footer_redirects_exact_lookups_to_source_for_file_hint` that
        // said so, on the macOS build machine.
        SourceHint::File(src) => format!(
            "[stashed: {} · ~{tokens} tokens · {what} · ASK it what you need → `tokenade expand-ref {hash} --prompt \"q1, q2\"` returns just the relevant passages (RAG, several questions at once) · EXACT value/line → grep or Read the original `{}` directly (complete & verbatim) · {how_to_query}{delegate_hint}]",
            stash_path.display(),
            src.display(),
        ),
        _ => format!(
            "[stashed: {} · ~{tokens} tokens · {what} · {menu} {how_to_query}{delegate_hint}]",
            stash_path.display(),
            menu = crate::disclose::recovery_menu(hash, text),
        ),
    }
    };
    // The caller's body comes before any mode choice: it KNOWS more
    // than we do about what this text is.
    if let Some(body) = override_body {
        return format!(
            "[disclose:summary kind={kind_ext} tokens={tokens} mode=md-outline-per-part lines={total_lines}]\n\
             {body}\n\
             {footer}",
            footer = footer_for(BODY_IS_OUTLINE),
        );
    }
    // JSON / GraphQL / API REST envelopes → try schema-fold first.
    if kind_ext.ends_with("json") || kind_ext == "ndjson" {
        if let Some(folded) = crate::json::schema_fold(text) {
            let folded = clamp_summary_body(folded);
            return format!(
                "[disclose:summary kind={kind_ext} tokens={tokens} mode=json-schema-fold]\n\
                 {folded}\n\
                 {footer}",
                footer = footer_for(BODY_IS_SCHEMA),
            );
        }
    }
    // A diff → its list of files. Asked before the markdown outline: a normal
    // diff is labelled `md` by the kind detector (its `---` lines), and the
    // head-and-tail fallback named one file of 36 on a real `diff -r`.
    if let Some(plan) = crate::diff::plan(text) {
        let plan = clamp_summary_body(plan);
        return format!(
            "[disclose:summary kind={kind_ext} tokens={tokens} mode=diff-plan lines={total_lines}]\n\
             {plan}\n\
             {footer}",
            footer = footer_for(BODY_IS_OUTLINE),
        );
    }
    // Markdown / HTML → outline (post html_to_markdown the content
    // is already markdown, but we accept both).
    if let Some(index) = super::help_index::help_index(text, SUMMARY_MAX_BODY_CHARS) {
        return format!(
            "[disclose:summary kind={kind_ext} tokens={tokens} mode=help-index lines={total_lines}]\n\
             {index}\n\
             {footer}",
            footer = footer_for(BODY_IS_INDEX),
        );
    }

    if kind_ext == "md" || kind_ext == "html" || kind_ext == "api.rest.html" {
        let outline = clamp_summary_body(crate::markdown::outline(text));
        // **An outline of ONE entry is not an outline.**
        //
        // `!outline.is_empty()` let the outline of a page without headings win,
        // from which `outline` draws only one line — `# (before the first heading)
        // (33 lines · "…")`. MEASURED on 2026-09-14 on real pages from
        // transcripts: a 177 175-byte page delivered in 610 characters
        // of which **95 % was our own furniture** — the banner, this line, and a
        // 484-character recovery footer. The only content was the
        // 80-character preview.
        //
        // The rule already existed and is called `outline_maps_the_page`; the
        // two web routes have applied it for a while. This layer, which is
        // the ONLY one `read`, `expand-ref` and the commands go through, did not
        // know it. Without a usable outline, the head and the tail say
        // more — that is the fallback just below.
        if crate::markdown::outline_maps_the_page(&outline) {
            return format!(
                "[disclose:summary kind={kind_ext} tokens={tokens} mode=md-outline lines={total_lines}]\n\
                 {outline}\n\
                 {footer}",
                footer = footer_for(BODY_IS_OUTLINE),
            );
        }
    }
    // Generic head + tail fallback.
    if total_lines <= SUMMARY_HEAD_LINES + SUMMARY_TAIL_LINES {
        // Few lines does NOT mean small: this is the branch a minified
        // document lands in, and it used to return `text` whole.
        let body = clamp_summary_body(text.to_string());
        return format!(
            "[disclose:summary kind={kind_ext} tokens={tokens} lines={total_lines}]\n\
             {body}\n\
             {footer}",
            footer = footer_for(BODY_IS_HEAD_TAIL),
        );
    }
    let head: Vec<&str> = text.lines().take(SUMMARY_HEAD_LINES).collect();
    let tail: Vec<&str> = text
        .lines()
        .skip(total_lines.saturating_sub(SUMMARY_TAIL_LINES))
        .collect();
    let elided = total_lines - SUMMARY_HEAD_LINES - SUMMARY_TAIL_LINES;
    // Whatever else the middle held, it must not hold the failure.
    //
    // This branch used to drop the middle blind. On a large log whose lines are
    // all distinct — a build, a test run, a deploy — the one line that mattered
    // sat in that middle, and the agent was handed a clean tail and read the run
    // as having passed. MEASURED on 2 500-line logs: a Python traceback frame, a
    // JUnit `[ERROR]`, a Docker build failure, a Go `--- FAIL` and an
    // `npm ERR!` were lost, five times out of five. The bytes stayed
    // recoverable through the stash, which is no help at all: nothing in the
    // delivered text gave the agent a reason to go looking.
    //
    // `compress::is_error_line` is the same predicate `smart_fold_indices`
    // already uses to protect the middle everywhere else — this branch simply
    // never asked it.
    // How many there ARE, not just how many fit. The sentence below said "the
    // error/failure lines among them are kept below" — definite article, no
    // number — while the rescue takes at most SUMMARY_RESCUED_ERROR_LINES.
    // Measured 2026-09-06 on 30 DISTINCT failures buried in a heterogeneous
    // 810-line log: 20 were taken, 7 survived the preview clamp, and the
    // sentence claimed all of them. A reader counts what it sees.
    let found_errors = text
        .lines()
        .skip(SUMMARY_HEAD_LINES)
        .take(elided)
        .filter(|l| crate::compress::is_error_line(l))
        .count();
    let rescued: Vec<String> = text
        .lines()
        .skip(SUMMARY_HEAD_LINES)
        .take(elided)
        .filter(|l| crate::compress::is_error_line(l))
        .take(SUMMARY_RESCUED_ERROR_LINES)
        // A failure line stays legible under the per-line bound, and a 2 000
        // char one must not eat the whole rescue budget on its own.
        .map(|l| {
            let n = l.chars().count();
            if n <= SUMMARY_MAX_LINE_CHARS {
                l.to_string()
            } else {
                let keep: String = l.chars().take(SUMMARY_MAX_LINE_CHARS).collect();
                format!("{keep}… (+{} chars)", n - SUMMARY_MAX_LINE_CHARS)
            }
        })
        .scan(0usize, |used, l| {
            let n = l.chars().count() + 1;
            if *used + n > RESCUE_BUDGET_CHARS && *used > 0 {
                return None;
            }
            *used += n;
            Some(l)
        })
        .collect();
    // 30 lines is not 30 short lines — one long line in the head is enough
    // to blow the budget, so the byte bound applies here too.
    // Count what the elided VIEW lines stand for in the ORIGINAL, not just how
    // many of them there are.
    //
    // This summary usually runs SECOND, over a view a compactor already
    // collapsed, and it counted lines of that view. Measured 2026-09-06 on a
    // 400-line log: the compactor folded 222 lines into 111 markers and said
    // so, this stage then elided 260 view lines and said so — every number
    // true of its own stage, and the sum a reader can do, 482, larger than the
    // 400 lines that went in. The two accountings overlap: the markers are
    // INSIDE the 260.
    //
    // `filter::run_weight` states the rule one module over — "say the ORIGINAL
    // magnitude when the elided lines were themselves collapsed runs, because
    // that is the number an agent judges 'how much am I not seeing' by" — and
    // knows `(×N)` runs. A fold MARKER is the same situation and neither knew
    // it.
    // MINUS what the rescue hands back. Those lines are picked OUT of the
    // elided middle and printed under the marker, so counting them as gone is
    // the same double-count one level down: measured 2026-09-06 on a 620-line
    // test log, 571 lines were really missing and the marker claimed 591 —
    // over by exactly the 20 the rescue had just delivered.
    let rescued_weight: usize = rescued
        .iter()
        .map(|l| original_weight(l))
        .fold(0usize, usize::saturating_add);
    // Both sums walk TEXT, and a line that looks like one of our markers may
    // simply be content — a saved transcript, a CI log of a tokenade run, a
    // pasted banner. So the arithmetic saturates rather than aborting the
    // command, and the claim is capped by what this process's own passes
    // actually folded (`fold_count::folded_ceiling`). Measured 2026-09-07: one
    // content line reading `… [900000 lines elided by tokenade] …` in a
    // 9 000-line file made this banner announce 908969, and a `(×N)` with N
    // near `usize::MAX` killed the whole command with an overflow.
    let represented: usize = text
        .lines()
        .skip(SUMMARY_HEAD_LINES)
        .take(elided)
        .map(original_weight)
        .fold(0usize, usize::saturating_add)
        .saturating_sub(rescued_weight)
        .min(elided.saturating_add(crate::fold_count::folded_ceiling()));
    let stands_for = if represented > elided {
        format!(", standing for {represented} line(s) of the original output")
    } else {
        String::new()
    };
    // Name the shortfall when there is one; keep the plain sentence when the
    // rescue really did take them all.
    let rescue_note = if found_errors > rescued.len() {
        format!(
            "{} of the {found_errors} error/failure lines among them are",
            rescued.len()
        )
    } else {
        "the error/failure lines among them are".to_string()
    };
    let middle = if rescued.is_empty() {
        format!("… (elided {elided} lines{stands_for}) …")
    } else {
        format!(
            "… (elided {elided} lines{stands_for} — {rescue_note} kept below) …\n{}",
            rescued.join("\n")
        )
    };
    // Clamp the CONTEXT, never the rescue.
    //
    // `clamp_summary_body` keeps the first two thirds and the last third of the
    // characters, and the rescued failures sit in the middle — exactly the
    // slice it drops. Measured 2026-09-06 on 25 distinct failures: the marker
    // said "20 … are kept below" and the answer held FIVE of them, or NONE at
    // all when the failure lines were long. The head/tail context is what a
    // preview is allowed to lose; the lines saying the run failed are why the
    // rescue exists.
    let middle_chars = middle.chars().count();
    let context_budget = SUMMARY_MAX_BODY_CHARS.saturating_sub(middle_chars).max(600);
    let body = format!(
        "{}\n{middle}\n{}",
        clamp_body_to(head.join("\n"), context_budget * 2 / 3),
        clamp_body_to(tail.join("\n"), context_budget - context_budget * 2 / 3),
    );
    format!(
        "[disclose:summary kind={kind_ext} tokens={tokens} lines={total_lines}]\n\
         {body}\n\
         {footer}",
        footer = footer_for(BODY_IS_HEAD_TAIL),
    )
}
