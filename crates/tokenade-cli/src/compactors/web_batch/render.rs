//! **Render one page of the batch, within its share.**
//!
//! The budget is SHARED among the targets, and a page that overflows must not
//! eat the one next to it. What was left out is NAMED: two of four requested
//! pages had vanished from a batch without even their name surviving, and a
//! boundary written as a HEADING survives the fold.

use super::*;

/// One rendered target of a [`cmd_web_batch`]: what the agent is shown, the op
/// name it books under, and the credit assiette for it.
///
/// The savings travel WITH the render because only this function knows which
/// branch produced it — a document extraction, a stashed binary, or an ordinary
/// page — and those are three different assiettes. Returning the markdown alone
/// is what forced the caller to guess, and it guessed the document one for
/// every branch.
pub struct Rendered {
    pub(super) md: String,
    pub(super) op: &'static str,
    pub(super) booked: Savings,
    /// Whether THIS render already carries a recovery pointer we put there.
    ///
    /// A fact reported by the stash, never a search of the markdown: a fetched
    /// page can contain `[stashed:` or `[tokenade:` in its own body, and the
    /// caller's "is anything already pointed at?" test then answers on the
    /// page's say-so instead of ours.
    pub(super) pointed: bool,
    /// What `booked` was computed from, for a web page: a replay pointer that
    /// replaces it is credited from the same basis.
    pub(super) basis: Option<tokenade_core::web_credit::WebBasis>,
}

impl Rendered {
    fn with_pointer(mut self, pointed: bool) -> Self {
        self.pointed = pointed;
        self
    }
}

/// One target of a [`cmd_web_batch`], rendered to markdown (or `None` on a
/// fetch/read miss). Same transport + content bricks as the single path.
/// `Err(reason)` carries what to PUT IN FRONT OF THE CALLER, not a code.
///
/// The single-target path has named its cause for a while
/// ("answer with a readable refusal naming the reason and the way around it"):
/// it calls `fetch_detailed`, which keeps the transport's own verdict, and
/// prints the io error verbatim for a local path. This one returned `Option`
/// and its caller printed `fetch/read failed` — four words for a closed port, a
/// nonexistent host, a timeout, a typo in a path and a directory alike. Same
/// command, same failure, two answers, and the mute one is the BATCH form this
/// product's own guidance tells agents to prefer ("several URLs in ONE call").
/// Build a rendered target, provenance first.
///
/// The argument order is the point: what the fetch learned about the response
/// is the first thing this takes, so a fourth return path cannot be written
/// that quietly drops it. Three of them existed and all three did.
pub fn rendered(provenance: &str, md: String, op: &'static str, booked: Savings) -> Rendered {
    Rendered {
        md: format!("{provenance}{md}"),
        op,
        booked,
        pointed: false,
        basis: None,
    }
}

/// A web delivery, credited on what it finally carries.
pub fn rendered_web(
    provenance: &str,
    md: String,
    op: &'static str,
    basis: tokenade_core::web_credit::WebBasis,
) -> Rendered {
    let mut r = rendered(provenance, md, op, Savings::new(0, 0));
    r.booked = tokenade_core::web_credit::credit_on_delivery(
        basis,
        tokenade_core::tokenizer::estimate(&r.md),
    );
    r.basis = Some(basis);
    r
}

/// Ask the seventeen dedicated readers whether they claim `url`, and read it
/// through the one that does.
///
/// Every path that fetches a page FOR A MODEL owes this question: a reader
/// turns a URL into its structured source — the GitHub API for an issue,
/// old.reddit for a thread — where generic scraping gets the JavaScript shell.
/// One function because the question kept being asked on one path and not its
/// sibling: the batch was given it a round after the single target, and
/// `answer_from_results` — which reads the top THREE results of a web search,
/// the very URLs the readers exist for — was still scraping. Measured
/// 2026-09-03 on the same GitHub issue, first hit of a live search:
/// 18,965 bytes of thread through `tokenade web`, 832 bytes of page chrome
/// (`Description`, then a `References` list of login and signup links) through
/// `search --answer`.
///
/// Fail-open: no plan, or a reader that misses, returns `None` and the caller
/// falls through to its generic pipeline rather than failing the target.
///
/// `channel` names the caller in the debug log. The question is logged whether
/// or not a reader claims the URL, because a source sweep can see the call and
/// cannot see whether it RUNS — measured 2026-09-03, wrapping the block in
/// `if false` left every textual guard green.
pub fn site_reader_read(
    channel: &'static str,
    url: &str,
) -> Option<crate::hooks::webfetch_pretool::readers::Rendered> {
    let plan = tokenade_core::site_readers::plan(url);
    tokenade_core::debug_log::log(
        channel,
        "site_reader_considered",
        serde_json::json!({
            "url": url,
            "via": plan.as_ref().map(|p| p.via).unwrap_or("none"),
        }),
    );
    let plan = plan?;
    let started = std::time::Instant::now();
    crate::hooks::webfetch_pretool::readers::render_site_reader(&plan, url, started)
}

/// **A requested page does not vanish without a word.**
///
/// The batch is assembled then summarised as a single document, and the
/// outline cuts its middle. MEASURED 2026-09-09 on
/// `web bbc stripe aljazeera guardian`: 18 069 characters elided, and **two of
/// the four requested pages** had not a single line in the reply — not even
/// their name. The agent asked for four targets, got two, and nothing said so.
///
/// The outline keeps the headings, and every page carries one since it is
/// separated by `# {target}`: the question is therefore exactly decidable,
/// not guessed. Says nothing when nothing was summarised, nor when
/// everything is there.
/// The batch outline, one share per page, each held to its share of the
/// budget.
///
/// `SUMMARY_MAX_BODY_CHARS` is the budget of the delivered body; it is divided
/// by the number of pages rather than spent on the head and tail of the
/// whole. An unused share is not redistributed: that would hand the next page
/// the space of one that did not need it, and the result would then depend on
/// the order of the targets.
pub fn fair_share_body(emitted: &[(String, String)]) -> String {
    let n = emitted.len().max(1);
    let share = tokenade_core::disclose::SUMMARY_MAX_BODY_CHARS / n;
    emitted
        .iter()
        .map(|(t, md)| {
            let plan = tokenade_core::markdown::outline(&format!("# {t}\n{md}"));
            tokenade_core::disclose::clamp_body_to(plan, share)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn name_the_pages_the_summary_left_out(summary: String, emitted: &[String]) -> String {
    if emitted.len() < 2 || !summary.starts_with("[disclose:summary") {
        return summary;
    }
    let absent: Vec<&str> = emitted
        .iter()
        .filter(|t| {
            let head = format!("# {t}");
            !summary
                .lines()
                .any(|l| l == head || l.starts_with(&format!("{head} (")))
        })
        .map(String::as_str)
        .collect();
    if absent.is_empty() {
        return summary;
    }
    format!(
        "[tokenade:web batch] {} of the {} pages fetched are NOT in the outline below — the \
         batch was summarised as one document and they were clamped out. They ARE in the stash: \
         ask for one by name with `tokenade expand-ref <hash from the footer> --prompt \"what \
         does <page> say about …\"`.\n  absent: {}\n{summary}",
        absent.len(),
        emitted.len(),
        absent.join(", ")
    )
}

pub fn web_batch_render_one(
    target: &str,
    max_input_tokens: usize,
    prompt: Option<&str>,
) -> Result<Rendered, String> {
    const CAP: usize = 16 * 1024 * 1024;
    let is_url = target.starts_with("http://") || target.starts_with("https://");
    // A domain with a dedicated reader has a known-better path, and the SINGLE
    // target path takes it. This one did not, so batching — the call this
    // product's own guidance tells agents to prefer, "several URLs in ONE call"
    // — silently downgraded every target to generic HTML scraping. Measured
    // 2026-09-03 on a GitHub issue: alone, `> source: api.github.com` and the
    // whole thread in 5 247 bytes; in a pair, 1 987 bytes of the page's own
    // JavaScript shell, opening `### Uh oh!`. The single path's note counts
    // seventeen readers and the cost of missing them (`reddit.com` served "37
    // characters of SPA shell"); this is that finding one channel over.
    //
    // Fail-open exactly like the single path: a reader that misses falls
    // through to the generic pipeline below rather than failing the target.
    if is_url {
        if let Some(r) = site_reader_read("web-batch", target) {
            let basis = crate::hooks::webfetch_pretool::readers::reader_basis(&r);
            // The reader's markdown already carries its own `> source:` and
            // provenance, so it goes through the constructor with none to add
            // rather than acquiring a second one.
            let (md, pointed) = tokenade_core::disclose::maybe_stash_reporting(
                r.md,
                tokenade_core::disclose::SourceHint::web_with_content_type("text/markdown"),
            );
            return Ok(rendered_web("", md, "site-reader", basis).with_pointer(pointed));
        }
    }
    let mut landing: Option<String> = None;
    let (bytes, content_type, provenance) = if is_url {
        let f = tokenade_core::web_fetch::fetch_detailed(
            target,
            &tokenade_core::web_fetch::FetchOpts::page("25", CAP),
        )
        .map_err(|why| {
            // The same sentence the single-target path prints, pointer
            // included: the cause names the LAST attempt, the log every one.
            format!(
                "{why}. Set TOKENADE_DEBUG_LOG=<path> and re-run to see what \
                 tokenade tried."
            )
        })?;
        // The status and the landing URL the fetch knows, which THIS path
        // dropped. `cmd_web` was given them earlier, with the measurement
        // that motivated it — "23 % of real fetches are non-2xx and 11.6 % end
        // on a different host, all of it invisible here" — and the batch, the
        // call this product's own guidance tells agents to prefer, kept going
        // without. Measured 2026-09-03: `tokenade web <good> <404>` rendered
        // the failing target's section as the two words `Not Found`, which is
        // indistinguishable from a page that says so, while the same URL fetched
        // alone is prefixed `[tokenade: HTTP 404 — the page does not exist. What
        // follows is the ERROR page, not the requested content.]`.
        let provenance = crate::hooks::webfetch_pretool::provenance_notes(
            f.status,
            target,
            f.effective_url.as_deref(),
            f.truncated,
        );
        // The address that ANSWERED. See the note on the single-target path:
        // a relative URL resolves against it, not against the requested one.
        landing = f.effective_url.clone();
        (f.bytes, f.content_type, provenance)
    } else {
        let local = std::fs::read(target).map_err(|e| e.to_string())?;
        let ct =
            tokenade_core::web::content_type_for_local_file(target, &local).map(str::to_string);
        (
            local,
            // The label a local file has instead of a server's header. Without
            // it the pipeline defaults to HTML — correct for an unlabelled
            // FETCH, and it collapsed every newline of a `.md`/`.txt` file into
            // one line here. The single-target path was given this and this one
            // was not, which is the third time a `web` behaviour has lived on
            // one of the two routes only (site readers, provenance notes).
            ct,
            String::new(),
        )
    };
    let kind = tokenade_core::web_fetch::classify_body(&bytes, content_type.as_deref());
    if let Some(fmt) = kind
        .detected
        .as_deref()
        .and_then(|ext| tokenade_core::docs::detect_format(Some(&format!("x.{ext}")), &bytes))
        .or_else(|| tokenade_core::docs::detect_format(Some(target), &bytes))
    {
        let (text, _sav) = tokenade_core::media::extract_document_with_savings(&bytes, fmt);
        let text = tokenade_core::redact::redact_for_model(&text);
        if !text.trim().is_empty() {
            let (md, pointed) = tokenade_core::disclose::maybe_stash_reporting(
                text,
                tokenade_core::disclose::SourceHint::web(),
            );
            // A DOCUMENT really is the binary assiette — the turn tax avoided,
            // and the FORMAT decides whether native rendering applies (a PDF
            // does, a spreadsheet does not).
            let booked = credit_binary_for(fmt, tokenade_core::tokenizer::estimate(&md));
            return Ok(
                rendered(&provenance, md, "webfetch_binary_extracted", booked)
                    .with_pointer(pointed),
            );
        }
    }
    if kind.binary && kind.detected.as_deref().is_some_and(|k| k != "bin") {
        let (md, pointed) = tokenade_core::disclose::maybe_stash_bytes_reporting(
            bytes,
            tokenade_core::disclose::SourceHint::web(),
        );
        // A binary we RECOGNISE but cannot extract — an image, an archive.
        // `docs::detect_format` has already refused it, so it is not a PDF and
        // native rendering has nothing to say about it.
        let booked = credit_binary_raw(false, tokenade_core::tokenizer::estimate(&md));
        return Ok(
            rendered(&provenance, md, "webfetch_binary_extracted", booked).with_pointer(pointed),
        );
    }
    let (html, _enc) = tokenade_core::text_encoding::decode(&bytes)
        .ok_or_else(|| "the body decoded to no readable text".to_string())?;
    let opts = tokenade_core::web_pipeline::PageOpts { max_input_tokens };
    let d = tokenade_core::web_pipeline::distill_page_with(
        &html,
        landing.as_deref().unwrap_or(target),
        content_type.as_deref(),
        opts,
    );
    // The two branches above stash and this one did not — the HTML branch,
    // which is every ordinary web page. So a batch skipped the summary the
    // single-target path always applies, and did it on the call this product's
    // own guidance tells agents to prefer ("several URLs in ONE call").
    // Measured 2026-08-28 on Wikipedia: ONE url delivered 3 976 bytes behind a
    // `[disclose:summary kind=md …]`; the same page in a pair delivered whole,
    // 122 050 for two and 586 101 for six — ~146 k tokens in one answer.
    //
    // Per page rather than on the concatenation: each page keeps its own
    // recovery pointer, and a batch of one big page and five small ones does
    // not lose the five.
    // An ordinary page: the per-agent WEB model on the RAW page the agent never
    // had to carry — the same book, through the same declarer, that the
    // single-target path keeps.
    let (op, basis) = book_distilled_page(&d);
    // Stash FIRST, then answer the question against the full page and carry the
    // pointer over — the order the single-target path uses. Extracting before
    // the stash would hand back a small answer with no way to the page bytes.
    // **The question is asked of the PAGE, not of the cut — and by REFERENCE.**
    //
    // `d.md` is the view after the hard ceiling: on a large page, it is a
    // sample of lines that keeps the headings and drops the body. Asking the
    // question of it means asking what is left AFTER the answer was thrown
    // away. Measured on the Claude Code hooks page (2.8 MB of HTML, 233 795
    // characters of markdown) with `--prompt updatedToolOutput`: the term
    // appears SEVEN times in the page's markdown and ZERO times in the clipped
    // view, so the extractor found no anchor and delivered « nothing in this
    // source answers that question » followed by the whole outline. `full_md`
    // carries precisely the pre-cut markdown, for the stash; it also carries
    // the answer.
    //
    // The extraction is done HERE, before the stash consumes the markdown: it
    // is a pure computation, and the ordering avoids one page copy per page
    // read.
    let asked = prompt.is_some_and(|p| !p.trim().is_empty());
    let scoped = prompt
        .filter(|p| !p.trim().is_empty())
        .and_then(|q| super::scope_extract(d.full_md.as_deref().unwrap_or(&d.md), q));
    // The stash holds the PAGE, not the cut: when the hard ceiling bit,
    // `d.md` is a sampled view and an `expand-ref` to it brings back the
    // sample. Same rule as the single-target route.
    // Stash = the page, delivered outline = the VIEW's. See the single-target
    // route: the whole page's outline is both longer and poorer in headings,
    // and delivering the view itself costs six times the outline.
    // …and only if that outline MAPS the page: without headings, it fits on
    // one line and the clipped sample says more. Same rule as the
    // single-target route.
    let (delivered, pointed) = match d.full_md {
        Some(full) => {
            let plan = tokenade_core::markdown::outline(&d.md);
            let body = if tokenade_core::markdown::outline_maps_the_page(&plan) {
                plan
            } else {
                d.md
            };
            tokenade_core::disclose::maybe_stash_reporting_with_body(
                full,
                tokenade_core::disclose::SourceHint::web(),
                body,
            )
        }
        None => tokenade_core::disclose::maybe_stash_reporting(
            d.md,
            tokenade_core::disclose::SourceHint::web(),
        ),
    };
    // **Only a question that was asked can go unanswered.**
    //
    // `attach_scoped(delivered, None)` does not tell « no question » from
    // « question asked, nothing anchored »: the latter deserves the note, the
    // former receives it wrongly. Measured on the 27 corpus pages when the
    // extraction was split in two: **27 pages out of 27** received « nothing
    // in this source answers that question » without any question being
    // asked, 108 characters each. So the question is asked again here.
    let delivered = if asked {
        super::attach_scoped(delivered, scoped)
    } else {
        delivered
    };
    Ok(rendered_web(&provenance, delivered, op, basis).with_pointer(pointed))
}
