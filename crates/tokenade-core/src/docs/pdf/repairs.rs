//! **What must be REPAIRED in what an extractor returns.**
//!
//! A PDF carries glyphs at coordinates, not paragraphs. The
//! columns arrive interleaved, split words stay split, and the running
//! title repeats on every page — delivered as is, it amounts to
//! hundreds of lines that say nothing.
//!
//! Each pass is NAMED and separate because they contradict each other: rejoining
//! two columns assumes knowing that one line continues another, and the
//! running title is precisely the line that continues none.

use super::*;

/// Strip running heads and feet — the title/author/page-number lines a producer
/// stamps on every page.
///
/// A reader skips them; an agent pays for them once per page. Measured on real
/// books: `www.TaleBooks.com` appears on all 245 pages, and a 1245-page
/// upanishads has 13 distinct feet across 1245 pages. No competitor does this —
/// `pdf-mcp` keeps them deliberately.
///
/// Only the first/last [`RUNNING_HEAD_BAND`] lines of each page are candidates,
/// and a line must repeat across [`RUNNING_HEAD_MIN_SHARE`] of pages: a body
/// line that happens to recur is never at the page edge on most pages. Digits
/// are normalised so "Page 3 of 100" and "Page 4 of 100" are recognised as one
/// template, but the comparison is on a NORMALISED key only — what gets dropped
/// is the original line, never a rewritten one.
pub(crate) fn dedup_running_heads(pages: &[String]) -> String {
    if pages.len() < RUNNING_HEAD_MIN_PAGES {
        return pages.join("\n");
    }
    // Two tiers, because digit normalisation is a loaded gun. Blanking digits
    // everywhere makes "corps unique de la page 1".."page 6" collapse to ONE key
    // and the whole body gets classified as boilerplate — my own fixture hit
    // that, and in the wild it would eat "Exercice 1", "Figure 2", any numbered
    // line sitting at a page edge.
    //
    // So: an EXACT repeat is boilerplate ("www.TaleBooks.com", "ACME
    // Confidential"). A digit-varying repeat only counts when the line is
    // essentially a NUMBER — what remains after removing digits is tiny
    // ("Page 3 of 100" -> "Page of", a bare "42" -> ""). Real prose never
    // shrinks to nothing.
    let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let is_numeric_template = |s: &str| -> bool {
        let residue: String = s.chars().filter(|c| !c.is_ascii_digit()).collect();
        s.chars().any(|c| c.is_ascii_digit())
            && residue
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .count()
                <= NUMERIC_TEMPLATE_MAX_RESIDUE
    };
    let norm = move |s: &str| -> String {
        let sq = squash(s);
        if is_numeric_template(&sq) {
            let blanked: String = sq
                .chars()
                .map(|c| if c.is_ascii_digit() { '#' } else { c })
                .collect();
            // ADJACENT numeric groups collapse to one. How MANY numbers a
            // template carries is not identity-bearing — the same running foot
            // appears as "© Ecma International 2024" on one page and
            // "© Ecma International 2024 109" on the next, which blanked to
            // `#### ` vs `#### ###`: two keys, so neither reached the share
            // threshold and 439 copies survived (MEASURED 2026-07-22 on the
            // 816-page ECMA-262 spec). Groups separated by real words still
            // stay distinct — "Table # of #" does not collapse.
            let mut out = String::with_capacity(blanked.len());
            let mut chars = blanked.chars().peekable();
            while let Some(c) = chars.next() {
                if c != '#' {
                    out.push(c);
                    continue;
                }
                out.push('#');
                // Swallow the rest of this run, and any further runs reachable
                // over whitespace alone.
                while let Some(&n) = chars.peek() {
                    if n == '#' || n == ' ' {
                        let save = chars.clone();
                        chars.next();
                        if n == ' ' && chars.peek() != Some(&'#') {
                            chars = save;
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
            out
        } else {
            sq
        }
    };
    let band_keys = |page: &str| -> Vec<String> {
        let lines: Vec<&str> = page.lines().filter(|l| !l.trim().is_empty()).collect();
        let n = lines.len();
        let mut keys = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            if i < RUNNING_HEAD_BAND || i + RUNNING_HEAD_BAND >= n {
                let k = norm(l);
                if !k.is_empty() {
                    keys.push(k);
                }
            }
        }
        keys
    };
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for page in pages {
        // Count each key ONCE per page: a line repeated twice on one page must
        // not look like a line present on two pages.
        for k in band_keys(page)
            .into_iter()
            .collect::<std::collections::HashSet<_>>()
        {
            *counts.entry(k).or_default() += 1;
        }
    }
    // Second tier, position-FREE. The band rule assumes a running foot comes out
    // near the page edge, which held while pages were extracted in
    // content-stream order. Under layout analysis the same foot lands wherever
    // reading order puts it: MEASURED 2026-07-22 on the 816-page ECMA-262 spec,
    // "© Ecma International 2024" sits at line 27 of 33, 17 of 27, 9 of 10,
    // 42 of 54 … — inside the edge band on only 7 of 21 sampled pages against
    // 16 under the old engine, so 439 copies of it survived into the output.
    //
    // Position is no longer the reliable signal; REPETITION still is. The bar is
    // deliberately far above the banded one (80 % of pages, not 60 %) and only
    // short lines qualify, because without the page-edge constraint the share is
    // the only thing standing between this and deleting body prose. On the same
    // document the most repeated real sentence ("This method performs the
    // following steps when called:") sits at 27 % of pages — nowhere near it.
    // Position stays NECESSARY evidence — it is simply no longer required on
    // every page. A line qualifies only if it reaches the page edge on
    // `ANYWHERE_MIN_BAND_SHARE` of pages AND repeats on `ANYWHERE_MIN_SHARE`.
    // That is what separates the two cases: the ECMA foot lands in the band on
    // ~33 % of pages (7 of 21 sampled) while repeating on nearly all of them; a
    // BODY refrain — the case `running_heads_are_dropped_without_eating_body_text`
    // pins, and which this tier ate on its first draft — sits mid-page on every
    // page, so its banded share is 0 and it survives untouched.
    const ANYWHERE_MIN_SHARE: f64 = 0.8;
    const ANYWHERE_MIN_BAND_SHARE: f64 = 0.2;
    const ANYWHERE_MAX_CHARS: usize = 120;
    let mut anywhere: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for page in pages {
        for k in page
            .lines()
            .filter(|l| !l.trim().is_empty() && l.chars().count() <= ANYWHERE_MAX_CHARS)
            .map(&norm)
            .filter(|k| !k.is_empty())
            .collect::<std::collections::HashSet<_>>()
        {
            *anywhere.entry(k).or_default() += 1;
        }
    }
    // Third guard, and the one my own first fixture tripped: digit
    // normalisation is a loaded gun. "corps 1 alpha" … "corps 10 alpha" all
    // collapse to "corps # alpha", so a NUMBERED BODY LINE looks like a template
    // repeated on every page. A real running foot also appears VERBATIM on many
    // pages ("© Ecma International 2024" — 439 of 816), while a numbered body
    // line never repeats verbatim even once. So a digit-normalised key must
    // additionally clear a raw-form share to qualify.
    const ANYWHERE_MIN_RAW_SHARE: f64 = 0.3;
    let mut raw_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for page in pages {
        for l in page
            .lines()
            .map(&squash)
            .filter(|l| !l.is_empty())
            .collect::<std::collections::HashSet<_>>()
        {
            *raw_counts.entry(l).or_default() += 1;
        }
    }
    let raw_floor = (pages.len() as f64 * ANYWHERE_MIN_RAW_SHARE).ceil() as usize;
    let raw_backed = |key: &str| -> bool {
        // Not a digit template → the key IS the raw line, nothing to check.
        !key.contains('#')
            || raw_counts
                .iter()
                .any(|(raw, c)| *c >= raw_floor && norm(raw) == key)
    };

    let anywhere_threshold = (pages.len() as f64 * ANYWHERE_MIN_SHARE).ceil() as usize;
    let band_floor = (pages.len() as f64 * ANYWHERE_MIN_BAND_SHARE).ceil() as usize;
    let repeated_anywhere: std::collections::HashSet<String> = anywhere
        .into_iter()
        .filter(|(k, c)| {
            *c >= anywhere_threshold
                && counts.get(k).copied().unwrap_or(0) >= band_floor
                && raw_backed(k)
        })
        .map(|(k, _)| k)
        .collect();

    let threshold = (pages.len() as f64 * RUNNING_HEAD_MIN_SHARE).ceil() as usize;
    let boilerplate: std::collections::HashSet<&String> = counts
        .iter()
        .filter(|(_, &c)| c >= threshold)
        .map(|(k, _)| k)
        .collect();
    if boilerplate.is_empty() && repeated_anywhere.is_empty() {
        return pages.join("\n");
    }
    let mut out = String::new();
    for page in pages {
        let lines: Vec<&str> = page.lines().collect();
        let non_empty = lines.iter().filter(|l| !l.trim().is_empty()).count();
        let mut seen = 0usize;
        for l in &lines {
            if l.trim().is_empty() {
                continue;
            }
            seen += 1;
            let in_band = seen <= RUNNING_HEAD_BAND || seen + RUNNING_HEAD_BAND > non_empty;
            let key = norm(l);
            if (in_band && boilerplate.contains(&key)) || repeated_anywhere.contains(&key) {
                continue;
            }
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

/// Give back the pages whose columns the primary engine swept in visual ROWS
/// instead of reading down each column in turn.
///
/// MEASURED 2026-07-22 on page 1 of `pdf_mid.pdf`, a two-column paper: layout
/// analysis emitted "a malware family are found, analysts extract the char-" —
/// left column — followed on the SAME line by "preted as a graph of calls of
/// functions and instruc-", which is the right column's line at that height.
/// Every body line of the page came out that way. The damage is not cosmetic:
/// the prose is unreadable in sequence, and the seam manufactures tokens that
/// appear nowhere in the document ("char-preted", "differenti-teristics" from
/// "differenti-" glued to "-teristics"), which then get indexed and embedded as
/// if they were words. `extract_text` fails the same page the same way, worse
/// (it emits "the charpreted", with no seam left to see).
///
/// Repair is per PAGE and takes GEOMETRIC PROOF, never a guess: `pdf_layout`
/// hands back the substrings that exist only in a merged reading — a left line
/// touching the right line that shares its baseline — and a page is rewritten
/// only when the primary engine's own output contains one. A page that reads
/// correctly cannot contain them, so it is returned untouched and every gain
/// the layout path was chosen for (headings, tables, reading order, RAG
/// breadcrumbs) is kept everywhere it actually works.
pub(crate) fn repair_merged_columns(bytes: &[u8], pages: Vec<String>) -> Vec<String> {
    // The geometric pass re-parses the document, so it is worth the cost only
    // once something in the text already looks wrong. A merged row leaves a
    // signature the correct reading cannot: the line ends on a hyphenated word
    // AND carries a second broken word inside it, because the seam falls in the
    // middle of the left column's own hyphenation.
    //
    // The bar is a SHARE of pages, not a single hit, because column merging is
    // a systematic property of a document's layout: an engine that sweeps one
    // two-column page in rows sweeps all of them. One page in hundreds is the
    // other thing that produces this signature — an ordinary hyphenated line
    // that also happens to carry a compound word.
    //
    // MEASURED 2026-07-22, and the two cases are two orders of magnitude apart:
    // `pdf_mid.pdf` (two-column paper, genuinely merged) trips on 4 of 13 pages
    // = 30.8 %, while the 816-page ECMA-262 spec trips on 2 pages = 0.2 %. On
    // that spec the pass costs 1 381 ms against the primary engine's 6 562 ms —
    // a fifth of the extraction — to repair 5 pages out of 816. Declining that
    // trade is the deliberate call here: the repair is given up where merging
    // is incidental, and kept where it is the document's shape.
    let tripped = pages.iter().filter(|p| looks_column_merged(p)).count();
    if (tripped as f64) < pages.len() as f64 * MIN_MERGED_PAGE_SHARE {
        return pages;
    }
    let Some(orders) = crate::pdf_layout::page_orders(bytes) else {
        return pages;
    };
    if orders.len() != pages.len() {
        return pages;
    }
    pages
        .into_iter()
        .zip(orders)
        .map(|(md, order)| {
            if order.columnar && order.merged_in(&md) {
                order.text
            } else {
                md
            }
        })
        .collect()
}

/// The cheap screen for [`repair_merged_columns`]: a line that both ends
/// hyphenated and breaks a word mid-line. In a correct reading a wrapped word's
/// hyphen is the LAST thing on its line; a second one before it means another
/// line was appended after it.
pub(crate) fn looks_column_merged(page: &str) -> bool {
    page.lines().any(|l| {
        let t = l.trim_end();
        t.ends_with('-')
            && t.trim_end_matches('-').split_whitespace().any(|w| {
                let c: Vec<char> = w.chars().collect();
                c.iter().enumerate().any(|(i, ch)| {
                    *ch == '-'
                        && i > 0
                        && i + 1 < c.len()
                        && c[i - 1].is_lowercase()
                        && c[i + 1].is_lowercase()
                })
            })
    })
}

/// Undo the LINE BREAKS a PDF's page layout imposed on prose: rejoin hyphenated
/// words, then rejoin sentences the typesetter wrapped.
///
/// A PDF has no paragraphs, only lines placed on a page. Both artefacts cost us:
///
/// - **Hyphenation is a QUALITY bug, and that is the real reason this exists.**
///   `invraisem-` / `blable` never matches a search for `invraisemblable` —
///   1 307 such words across 20 real PDFs. Exactly the class of the ligature
///   remap above (`eﬃcient` vs `efficient`), and fixed for the same reason. Its
///   token gain is ~0.
/// - **Wrapping costs tokens**, and our own meter cannot see it: BPE charges for
///   a newline mid-sentence, but `tokenizer::estimate` counts CHARACTERS, and a
///   rewrap trades `\n` for a space — 37 chars before, 37 after. Measured with
///   the real cl100k tokenizer over 4.1M tokens of real PDFs: **-2.2%** for the
///   conservative rule below (an unconditional `\n`→space scores -2.6%, and
///   would shred every table, list and code block to buy that 0.4%).
///
/// Only joins what is VISIBLY a wrapped sentence: no terminal punctuation, the
/// next line starts lowercase, and neither line looks structural. Anything else
/// keeps its break — losing a table's shape to save a token is the trade this
/// module spends its life refusing.
pub(crate) fn reflow_lines(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let next = lines.get(i + 1).copied();
        // 1. Hyphenated word split across lines.
        if let Some(next) = next {
            if let Some(stem) = line.strip_suffix('-') {
                if !stem.is_empty()
                    && !is_structural_line(line)
                    && !is_structural_line(next)
                    && stem.chars().last().is_some_and(|c| c.is_alphabetic())
                    && next
                        .trim_start()
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_lowercase())
                {
                    out.push_str(stem);
                    i += 1;
                    continue; // next line continues the word, no separator
                }
            }
        }
        out.push_str(line);
        // 2. Sentence wrapped by the typesetter.
        let joins = next.is_some_and(|n| continues_sentence(line, n));
        out.push(if joins { ' ' } else { '\n' });
        i += 1;
    }
    out
}

/// `**1.1.2) Gimp-Print 4.2** The interim stable release…` → a `###` heading
/// and its paragraph.
///
/// A manual that sets its section titles in BOLD at body size gives the
/// layout analysis nothing to call a heading: measured 2026-10-03 on the
/// Gutenprint 5.2 user's manual (91 KB, 871 lines), the outline named
/// `Robert Krawitz`, `follows:` and `advanced use only.` and none of its
/// numbered chapters, which sat as bold runs glued to their first sentence.
///
/// Only a document that NUMBERS its sections this way is rewritten: three such
/// lines at least, one of them nested (`x.y`). A title starts with a capital
/// and does not end like a sentence — `**3. Remove the epson and canon
/// backends.**` is a list item in bold, not a section.
pub(crate) fn numbered_bold_headings(text: &str) -> String {
    let parse = |line: &str| -> Option<(usize, String, String)> {
        // `***3.3) CUPS***` — bold italic, the next level down in the same
        // manual — before `**`, or `*3.3) CUPS` would read as the label.
        let mark = if line.starts_with("***") { "***" } else { "**" };
        let rest = line.strip_prefix(mark)?;
        let end = rest.find(mark)?;
        let (bold, body) = (&rest[..end], rest[end + mark.len()..].trim());
        let label_len = bold.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ')'))?;
        let (label, title) = (&bold[..label_len], bold[label_len..].trim());
        let depth = label.trim_end_matches(['.', ')']).split('.').count();
        let numbered = label.starts_with(|c: char| c.is_ascii_digit())
            && label
                .trim_end_matches(['.', ')'])
                .split('.')
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
        let titled = title.starts_with(char::is_uppercase)
            && !title.ends_with(['.', ':', ';', ','])
            && title.chars().count() <= 80;
        (numbered && titled).then(|| {
            (
                depth,
                format!("{} {title}", label.trim_end()),
                body.to_string(),
            )
        })
    };
    let found: Vec<usize> = text.lines().filter_map(parse).map(|(d, ..)| d).collect();
    if found.len() < 3 || !found.iter().any(|d| *d > 1) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + found.len() * 8);
    for line in text.lines() {
        match parse(line) {
            Some((depth, heading, body)) => {
                out.push_str(&format!("{} {heading}\n", "#".repeat((depth + 1).min(6))));
                if !body.is_empty() {
                    out.push_str(&body);
                    out.push('\n');
                }
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if !text.ends_with('\n') {
        out.pop();
    }
    out
}

/// Lines that carry SHAPE, not prose: rejoining them destroys the structure the
/// rest of this module works to preserve.
pub(crate) fn is_structural_line(l: &str) -> bool {
    let s = l.trim_start();
    l.contains('|')                       // markdown table
        || l.trim_end().contains("  ")    // column alignment
        || s.starts_with(['-', '*', '•', '#', '>', '_', '='])
        || s.chars().next().is_some_and(|c| c.is_ascii_digit()) // numbered item
}

/// Is `line` a sentence the layout cut, continued by `next`?
pub(crate) fn continues_sentence(line: &str, next: &str) -> bool {
    let lt = line.trim_end();
    if lt.is_empty() || next.trim().is_empty() {
        return false;
    }
    if is_structural_line(line) || is_structural_line(next) {
        return false;
    }
    // Terminal punctuation ends a sentence; a lowercase start continues one.
    if lt.ends_with(['.', '!', '?', ':', ';', '"', '»', '”', ')', ']']) {
        return false;
    }
    next.trim_start()
        .chars()
        .next()
        .is_some_and(|c| c.is_lowercase())
}

#[cfg(test)]
mod numbered_bold_titles {
    use super::numbered_bold_headings;

    #[test]
    fn a_manual_that_numbers_its_sections_in_bold_has_an_outline() {
        let page = "**1) Introduction**\n\
                    **1.1.2) Gimp-Print 4.2** The interim stable release was to become 4.2.\n\
                    **2) General Requirements** Gutenprint will run on any modern computer.\n\
                    ***3.3) CUPS*** Gutenprint may be used as a driver under CUPS.\n\
                    **3. Remove the epson and canon backends.**\n";
        assert_eq!(
            numbered_bold_headings(page),
            "## 1) Introduction\n\
             #### 1.1.2) Gimp-Print 4.2\nThe interim stable release was to become 4.2.\n\
             ## 2) General Requirements\nGutenprint will run on any modern computer.\n\
             ### 3.3) CUPS\nGutenprint may be used as a driver under CUPS.\n\
             **3. Remove the epson and canon backends.**\n"
        );
    }

    /// A numbered list in bold, without nested sections: nothing changes.
    #[test]
    fn a_bold_list_is_not_an_outline() {
        let list = "**1. Install** the package.\n**2. Configure** it.\n**3. Run** it.\n";
        assert_eq!(numbered_bold_headings(list), list);
    }
}
