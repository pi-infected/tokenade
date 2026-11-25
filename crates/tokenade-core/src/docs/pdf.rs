//! PDF text extraction, and the repairs a real PDF needs.
//!
//! A PDF carries glyphs at coordinates, not paragraphs: columns arrive
//! interleaved, hyphenated words arrive split, running heads repeat on every
//! page, and a scan carries no text at all. Each repair here is one named
//! pass over what the extractor gave back.

mod extract;
mod invisible;
mod layout;
mod links;
mod repairs;
pub use extract::*;
pub(crate) use invisible::*;
pub use layout::*;
pub(crate) use repairs::*;

use super::*;

/// `%PDF-` may sit after a few junk bytes; the spec tolerates it and readers do
/// too, so accept it anywhere in the header rather than at offset 0 only.
pub(super) fn pdf_magic(head: &[u8]) -> bool {
    head.len() >= 5
        && head[..head.len().min(1024)]
            .windows(5)
            .any(|w| w == b"%PDF-")
}

/// Pure-Rust PDF text extraction via the `pdf-extract` crate. Returns the
/// extracted text (page-ordered) or an empty string if the document is not a
/// readable PDF (encrypted, image-only, malformed). Never panics — wraps the
/// extractor in a `catch_unwind` because real-world PDFs trigger edge cases
/// in the encoding tables that the underlying `lopdf` parser may panic on.
///
/// The `pdf-extract` floor is 0.12 and that is LOAD-BEARING, not cosmetic:
/// 0.7 had 15 hard-coded `println!`s and dumped a font dictionary to STDOUT for
/// every glyph missing from a unicode map ("missing char … /BaseFont
/// /CAAAAA+NotoSansArabic-Bold/Widths […]"). Measured on an Arabic PDF: 84 such
/// dumps, 10031 tokens of noise for a 47-token document. Any subsetted or CID
/// font triggers it — i.e. most LaTeX papers and every non-Latin script — and
/// stdout is the HOOK PROTOCOL, so those lines also corrupt the JSON a hook
/// emits. Never downgrade below a version that routes diagnostics through `log`.
pub(super) fn extract_pdf(bytes: &[u8]) -> String {
    let text = extract_pdf_text(bytes);
    let text = unreverse_hebrew_words(&text).unwrap_or(text);
    // A contents page is dot leaders to the margin: 7 276 of the 83 399 bytes
    // of a 40-page specification (2026-10-02). Same rule as `artisan`.
    crate::leaders::collapse_dot_leaders_in(&text).unwrap_or(text)
}

/// **A Hebrew word written backwards says so itself.** The five final forms
/// (ך ם ן ף ץ) only ever END a word. A PDF that stores Hebrew in visual order
/// came back with every word's letters reversed — `םולש םלוע` for `שלום עולם`,
/// measured 2026-10-03 on a LibreOffice export, on BOTH engines. When words
/// that OPEN on a final form clearly outnumber those that close on one, every
/// Hebrew word is put back, letter by letter, each letter keeping its vowel
/// marks. A document read correctly has the opposite count and is untouched.
fn unreverse_hebrew_words(text: &str) -> Option<String> {
    const FINALS: [char; 5] = ['ך', 'ם', 'ן', 'ף', 'ץ'];
    let is_letter = |c: char| ('\u{05D0}'..='\u{05EA}').contains(&c);
    let is_mark = |c: char| ('\u{0591}'..='\u{05C7}').contains(&c);
    let words = || {
        text.split(|c: char| !is_letter(c) && !is_mark(c))
            .filter(|w| w.chars().filter(|&c| is_letter(c)).count() >= 2)
    };
    let opens = words()
        .filter(|w| w.chars().next().is_some_and(|c| FINALS.contains(&c)))
        .count();
    let closes = words()
        .filter(|w| {
            w.chars()
                .rfind(|&c| is_letter(c))
                .is_some_and(|c| FINALS.contains(&c))
        })
        .count();
    if opens < 3 || opens <= closes * 3 {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut run: Vec<Vec<char>> = Vec::new();
    let flush = |run: &mut Vec<Vec<char>>, out: &mut String| {
        for g in run.drain(..).rev() {
            out.extend(g);
        }
    };
    for c in text.chars() {
        if is_letter(c) {
            run.push(vec![c]);
        } else if is_mark(c) {
            // Visual order puts a letter's marks BEFORE it, so a mark opens the
            // cluster of the letter that follows; reversed, it ends up after.
            match run.last_mut() {
                Some(g) if g.iter().all(|&x| is_mark(x)) => g.push(c),
                _ => run.push(vec![c]),
            }
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    Some(out)
}

fn extract_pdf_text(bytes: &[u8]) -> String {
    // Drop text the page never shows BEFORE extraction. `pdf-extract` has no
    // notion of the `Tr` operator (there is no `render_mode` anywhere in the
    // crate), so invisible text reaches the agent as ordinary prose. See
    // `strip_invisible_text`.
    let Some(cleaned) = strip_invisible_text(bytes) else {
        return remap_ligatures(&raw_pdf_text(bytes));
    };
    let stripped = raw_pdf_text(&cleaned);
    // Last-resort guard: if the rewrite emptied a document that had text, the
    // rewrite is what is broken. Serve the original — hidden text is a possible
    // harm, an empty document is a certain one.
    if stripped.trim().is_empty() {
        let original = raw_pdf_text(bytes);
        if !original.trim().is_empty() {
            return remap_ligatures(&original);
        }
    }
    remap_ligatures(&stripped)
}

/// **Why a right-to-left document cannot be sent to the other
/// engine.**
///
/// `docs/pdf.rs` carried 944 lines and zero tests. This is the first, and it
/// closes one of the three open issues: the corpus's `ar.pdf` loses its seven
/// Arabic words under `pdf_oxide`, and the obvious idea is to route it to
/// `pdf_extract` — already a dependency, already wired as a fallback in
/// [`raw_pdf_text`] when the primary returns almost nothing.
///
/// MEASURED on 2026-09-11: `pdf_extract` does not decode this document's fonts.
/// Its first line comes back riddled with U+0000 — one glyph per unresolved code —
/// and in VISUAL order, so the last word first. `pdf_oxide` returns the
/// same text in logical order, up to two neighbour swaps. The issue is
/// therefore closed by measurement, not by preference.
///
/// This test ASSERTS that measurement instead of narrating it: it will go red the day
/// `pdf_extract` can decode these fonts, and that day the issue will be worth
/// reopening.
#[cfg(test)]
mod the_other_engine_does_not_decode_this_page {
    const AR_PDF: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/docs-eval/corpus/ar.pdf"
    );

    #[test]
    fn pdf_extract_returns_unresolved_glyphs_where_pdf_oxide_returns_text() {
        let bytes = std::fs::read(AR_PDF).expect("the corpus must be there");
        let theirs = super::pdf_extract_pages(&bytes).join("\n");
        let ours = super::pdf_oxide_pages(&bytes).join("\n");
        assert!(
            theirs.contains('\u{0}'),
            "pdf_extract no longer returns unresolved glyphs on this page: \
             the refused routing is worth re-measuring\n{theirs:?}"
        );
        assert!(
            !ours.contains('\u{0}'),
            "pdf_oxide started returning unresolved glyphs: it is no longer \
             the engine that decodes this document best"
        );
        // And the primary engine does return Arabic, otherwise the comparison
        // above would pit two failures against each other.
        assert!(
            ours.chars()
                .filter(|c| ('\u{600}'..='\u{6FF}').contains(c))
                .count()
                > 20,
            "pdf_oxide no longer returns any Arabic: {ours:?}"
        );
    }
}

#[cfg(test)]
mod a_right_to_left_page {
    use super::rtl_dominant;

    /// The corpus carries the same sentence in six formats; this one comes from
    /// `ar.odt`, that is, Arabic text IN LOGICAL ORDER, as a
    /// reader reads it.
    const AR: &str = "تقرير الاستيعاب الفصلي — الإنتاجية";

    #[test]
    fn an_arabic_sentence_is_dominant() {
        assert!(rtl_dominant(AR));
    }

    #[test]
    fn a_latin_sentence_is_not() {
        assert!(!rtl_dominant(
            "Attention is all you need. Region Ingest Errors Retries"
        ));
    }

    /// **The case that forbids gating on « contains RTL ».** An article in
    /// English quoting an Arabic word would otherwise keep its layout for nothing,
    /// and lose its tables.
    #[test]
    fn an_arabic_word_in_an_english_page_does_not_tip_the_page() {
        let page = format!(
            "The Arabic word for productivity is {} and it appears once in this \
             otherwise English paragraph about quarterly ingest reports.",
            "الإنتاجية"
        );
        assert!(!rtl_dominant(&page));
    }

    /// Counted on LETTERS: a table of digits and vertical bars
    /// belongs to no script and must not tip the page.
    #[test]
    fn digits_and_markup_do_not_vote() {
        assert!(rtl_dominant("| 128 | 5 | 64 | 2 | --- | مصر تدفق |"));
        assert!(!rtl_dominant("| 128 | 5 | 64 | 2 |"));
    }

    /// The refusal NAMES its reason, and the three forms are distinct: that is what
    /// keeps the fourth from being one more `&&` in a guard that
    /// nobody rereads.
    #[test]
    fn the_refusal_says_why() {
        use super::layout_pass_refused;
        assert_eq!(layout_pass_refused("   \n  "), Some("empty"));
        assert_eq!(layout_pass_refused("| مصر | تدفق |"), Some("rtl"));
        assert_eq!(
            layout_pass_refused("A perfectly ordinary English paragraph of prose."),
            None
        );
    }

    /// **The defect itself, on the document that showed it.**
    ///
    /// The corpus's `ar.pdf`: the layout pass split `دفعة` into the
    /// single letter `ة` spread in a table cell. The simple path
    /// returns it whole. This test reads the real file — a synthetic fixture would not
    /// reproduce an Arabic PDF produced by LibreOffice.
    #[test]
    fn an_arabic_page_keeps_its_words_whole() {
        let p = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/docs-eval/corpus/ar.pdf"
        );
        let Ok(bytes) = std::fs::read(p) else {
            panic!("missing corpus: {p}");
        };
        let pages = super::pdf_oxide_pages(&bytes);
        let text: String = pages.join("\n");
        assert!(
            rtl_dominant(&text),
            "this document must be recognised as RTL, otherwise the test measures nothing:\n{text}"
        );
        for whole in ["دفعة", "تدفق"] {
            assert!(
                text.contains(whole),
                "« {whole} » arrived split — the layout pass took the page back:\n{text}"
            );
        }
        // And the shape the layout pass produced: a markdown table
        // row. Its absence is what proves it did not run.
        assert!(
            !text.contains("|---|"),
            "the page was laid out anyway:\n{text}"
        );
    }
}

/// A table of contents: dots up to the margin (7 276 of the 83 399 bytes
/// of a 40-page specification, 2026-10-02).
#[cfg(test)]
mod a_table_of_contents_loses_its_dots {
    /// A one-page PDF whose content stream writes `lines`, built by hand so the
    /// test needs no fixture file: the xref offsets are computed, not guessed.
    fn tiny_pdf(lines: &[&str]) -> Vec<u8> {
        let mut content = String::from("BT /F1 12 Tf 72 720 Td 14 TL\n");
        for l in lines {
            content.push_str(&format!("({l}) Tj T*\n"));
        }
        content.push_str("ET");
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 1));
        }
        let xref = pdf.len();
        pdf.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objs.len() + 1
        ));
        for off in offsets {
            pdf.push_str(&format!("{off:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        ));
        pdf.into_bytes()
    }

    #[test]
    fn leader_dots_fall_titles_and_pages_stay() {
        let pdf = tiny_pdf(&[
            "Preface ..................................... 7",
            "Contexte .................................... 12",
        ]);
        let text = super::extract_pdf(&pdf);
        assert!(
            text.contains("Preface") && text.contains("12"),
            "extraction failed: {text:?}"
        );
        assert!(!text.contains("........"), "leaders delivered: {text:?}");
        assert!(text.contains("Preface … 7"), "{text:?}");
    }
}

#[cfg(test)]
mod hebrew_in_visual_order {
    use super::unreverse_hebrew_words;

    /// Final forms only END a Hebrew word: a run where they OPEN words is a
    /// visual-order extraction, put back letter by letter.
    #[test]
    fn reversed_words_are_put_back_and_correct_ones_are_left_alone() {
        let reversed = "םולש םלוע, והז ךמסמ הקידב םע רפסמה 2026.";
        assert_eq!(
            unreverse_hebrew_words(reversed).as_deref(),
            Some("שלום עולם, זהו מסמך בדיקה עם המספר 2026.")
        );
        assert_eq!(
            unreverse_hebrew_words("שלום עולם, זהו מסמך בדיקה עם המספר 2026."),
            None
        );
        // Too little evidence: one reversed-looking word is not a document.
        assert_eq!(unreverse_hebrew_words("the word םולש means peace"), None);
        // Mostly CORRECT words with a few reversed-looking ones: ambiguous,
        // left alone rather than reversing the correct majority.
        let mixed = "םולש םלוע ךמסמ שלום עולם מסמך החשבון במלואו ביום שני ספרים ילדים חלון";
        assert_eq!(unreverse_hebrew_words(mixed), None);
    }
}
