//! Bodies that are already text: served as-is, or distilled when the
//! Content-Type lied and it is really markup.

use super::*;

pub fn distill_text(body: &str, content_type: Option<&str>) -> String {
    // A feed is not a page. `site_readers::render` returns None on anything
    // that is not really a feed, so a mislabelled page continues below.
    if looks_like_feed(body, content_type) {
        if let Some(md) =
            crate::site_readers::render(&crate::site_readers::Kind::FeedDocument, body)
        {
            return format!("> source: feed\n\n{md}");
        }
    }
    let ct_json = content_type
        .map(|c| {
            let mime = c
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            mime == "application/json" || mime.ends_with("+json")
        })
        .unwrap_or(false);
    if ct_json || body.trim_start().starts_with(['{', '[']) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
            return json_to_markdown(&v);
        }
    }
    text_body_to_markdown(body, content_type)
}

/// Text the server did NOT call HTML, and that does not look like HTML either.
///
/// The server's own label decides, because a `.txt` and an `.html` differ by
/// nothing the bytes can settle. Servers mislabel, though, so a body that
/// announces itself as HTML overrides the header — the same precedence
/// `classify_body` uses for binaries.
///
/// An absent content-type stays on the HTML path on purpose: HTML is what an
/// unlabelled web response overwhelmingly is, and guessing "plain" from the
/// bytes would send real pages down a path that strips nothing.
pub(super) fn serves_plain_text(body: &str, content_type: Option<&str>) -> bool {
    let Some(mime) = crate::text_encoding::normalized_content_type(content_type) else {
        return false;
    };
    let Some(sub) = mime.strip_prefix("text/") else {
        return false;
    };
    // `html` belongs to the HTML pipeline; the xml-ish subtypes belong to the
    // feed and sitemap readers, which run before this point and fall through
    // here only when the body turned out not to be one.
    if sub == "html" || sub.ends_with("xml") {
        return false;
    }
    let head = crate::caps::truncate_at_char_boundary(body.trim_start(), 256).to_ascii_lowercase();
    !(head.starts_with("<!doctype html") || head.starts_with("<html"))
}

/// What a LOCAL file's name says its body is, for callers that have a path
/// instead of a server.
///
/// [`serves_plain_text`] defaults an absent content-type to the HTML path, and
/// that is right for a fetch: an unlabelled web response overwhelmingly IS
/// HTML. A local file is not an unlabelled response — nobody omitted a header,
/// there was never a server, and the filename carries the label instead. Left
/// on the HTML default, `tokenade web notes.md` parsed the file as one giant
/// text node and collapsed every newline in it: MEASURED on a three-heading
/// document, 11 lines in and **1 line out**, headings and paragraph breaks
/// fused into a single line, while the same bytes served over HTTP as
/// `text/markdown` came back whole.
///
/// Returns `None` for anything not certainly prose — `.html` included — so an
/// unknown extension keeps exactly the behaviour it has today.
pub fn content_type_for_local_path(path: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        // The prose set `skeleton::structural_truncate` already routes as docs.
        "md" | "markdown" | "mdx" => Some("text/markdown"),
        "txt" | "text" | "rst" | "adoc" | "asciidoc" | "org" => Some("text/plain"),
        _ => None,
    }
}

/// The extensions whose content IS markup: they keep the HTML pipeline
/// whatever the bytes say.
const MARKUP_EXTENSIONS: &[&str] = &["html", "htm", "xhtml", "xht", "shtml"];

/// **What a local file is, when its extension does not say.**
///
/// [`content_type_for_local_path`] lists the "certainly prose" extensions and
/// returns `None` for everything else, which "keeps exactly today's
/// behaviour" — that is, the HTML parser, which reads the file as a single
/// text node and **folds its line breaks**. Yet line structure IS the meaning
/// of a CSV, a log, a `.tsv`, an `.ini`, exactly as it is of a `robots.txt` —
/// the very reason this list exists.
///
/// MEASURED 2026-09-09: `tokenade web donnees.csv` on forty lines returns
/// **a single line**, all rows glued together by spaces
/// (`id,name,qty,price 0,item-0,0,0.0 1,item-1,3,1.5 …`), whereas `tokenade
/// read` on the same file keeps them. Served by a server as `text/csv`, the
/// same content arrives intact: the defect lives only on the local route, the
/// fourth time a `web` behaviour exists on only one of its two routes.
///
/// An unknown extension is no longer GUESSED to be prose: the bytes decide,
/// and they decide in one direction only — anything carrying a closing tag
/// stays on the HTML pipeline.
pub fn content_type_for_local_file(path: &str, body: &[u8]) -> Option<&'static str> {
    if let Some(ct) = content_type_for_local_path(path) {
        return Some(ct);
    }
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if MARKUP_EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    if head_looks_like_markup(body) {
        return None;
    }
    Some("text/plain")
}

/// Enough to see a doctype, a root, or a fragment's first closing tag.
/// Beyond that, a document that has not yet closed a single tag is not
/// markup.
const MARKUP_SNIFF_BYTES: usize = 4096;

fn head_looks_like_markup(body: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&body[..body.len().min(MARKUP_SNIFF_BYTES)]);
    let trimmed = head.trim_start().to_ascii_lowercase();
    if trimmed.starts_with("<!doctype html") || trimmed.starts_with("<html") {
        return true;
    }
    // A CLOSING tag. A fragment (`<p>bonjour</p>`) announces neither a
    // doctype nor a root, and a CSV never carries one.
    head.match_indices("</").any(|(i, _)| {
        head[i + 2..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
    })
}

/// Convert a fetched TEXT body to markdown, honouring what the server called it.
///
/// `robots.txt`, an `llms.txt` or a raw `.md` used to fall through to
/// [`html_to_markdown`], which parses them as one giant text node and then
/// collapses their newlines: `User-agent:` fuses onto its `Disallow:` line, and
/// the line pairing IS the meaning of the file. Raw markdown fared worse — it
/// was re-parsed as HTML and came back with its setext headings inlined and its
/// link syntax broken. Such a body is already the format we deliver, so the
/// honest conversion is none at all.
pub fn text_body_to_markdown(body: &str, content_type: Option<&str>) -> String {
    if serves_plain_text(body, content_type) {
        return body.to_string();
    }
    html_to_markdown(body)
}

/// [`distill_text_with_savings`] when the caller has ALREADY counted `body`.
/// Tokenising a full page costs ~4 ms; the pipeline had counted the same bytes
/// twice before reaching here.
pub fn distill_text_with_savings_from(
    body: &str,
    content_type: Option<&str>,
    before: usize,
) -> (String, crate::Savings) {
    let md = distill_text(body, content_type);
    let after = tokenizer::estimate(&md);
    (md, crate::Savings::new(before, after))
}

#[cfg(test)]
mod local_file_type_tests {
    use super::{content_type_for_local_file, content_type_for_local_path};

    /// The measured case: forty lines of CSV rendered on ONE, all rows glued
    /// together by spaces.
    #[test]
    fn a_line_structured_file_is_not_html() {
        for (path, body) in [
            (
                "donnees.csv",
                "id,name
0,item-0
1,item-1
",
            ),
            (
                "t.tsv", "a	b
1	2
",
            ),
            (
                "serveur.log",
                "12:00 démarrage
12:01 prêt
",
            ),
            (
                "conf.ini",
                "[section]
cle=valeur
",
            ),
            (
                "sans-extension",
                "une ligne
une autre
",
            ),
        ] {
            assert_eq!(
                content_type_for_local_file(path, body.as_bytes()),
                Some("text/plain"),
                "{path} goes back to the HTML pipeline, which folds its lines"
            );
        }
    }

    /// And the bytes decide in one direction only: anything carrying markup
    /// stays on the HTML pipeline.
    #[test]
    fn anything_carrying_markup_keeps_the_html_pipeline() {
        // The extension says so.
        assert_eq!(
            content_type_for_local_file("page.html", b"pas de balise"),
            None
        );
        assert_eq!(
            content_type_for_local_file("page.HTM", b"pas de balise"),
            None
        );
        // The bytes say so: a complete document…
        assert_eq!(
            content_type_for_local_file("x.dat", b"<!DOCTYPE html><html><body>a</body></html>"),
            None
        );
        // …and a FRAGMENT, which announces neither a doctype nor a root.
        assert_eq!(
            content_type_for_local_file("x.part", b"<p>bonjour</p>"),
            None
        );
        // `</` with no tag behind it is not markup: an integer division in a
        // data file must not flip the routing.
        assert_eq!(
            content_type_for_local_file(
                "calcul.csv",
                b"a,b
10/</2,3
"
            ),
            Some("text/plain")
        );
    }

    /// Known prose always goes by its name, not by its bytes.
    #[test]
    fn a_known_prose_extension_still_wins() {
        assert_eq!(
            content_type_for_local_path("notes.md"),
            Some("text/markdown")
        );
        assert_eq!(
            content_type_for_local_file("notes.md", b"<p>ceci ressemble a du html</p>"),
            Some("text/markdown")
        );
    }
}
