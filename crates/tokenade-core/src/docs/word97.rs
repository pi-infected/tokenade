//! **Word 97–2003 (`.doc`) → text.**
//!
//! A `.doc` is an OLE2 (CFB) container, not a zip: the text lives in the
//! `WordDocument` stream, split into PIECES whose table (the `Clx`) is stored
//! in the `0Table` or `1Table` stream depending on a header bit. Each piece
//! says where its characters are and whether they take one byte (cp1252) or
//! two (UTF-16LE). [MS-DOC] §2.4.1 "Retrieving Text".
//!
//! Before this module, a `.doc` was taken for an Excel workbook (same
//! container), `calamine` returned nothing, and the message claimed "tokenade
//! reads doc files; … most likely truncated, empty, or encrypted" about a
//! healthy file.
//!
//! Output: the body (paragraphs, tables as `|` rows, links), then footnotes
//! and endnotes, called `[^N]` / `[^eN]` like those of a `.docx`. Review
//! comments are NOT served (they carry their author's name), nor hidden text
//! or text deleted under change tracking ([`hidden`]). Headings come from each
//! paragraph's style ([`paragraphs`]), automatic numbering from its lists
//! ([`lists`]).

mod hidden;
mod lists;
mod paragraphs;
mod textboxes;

use paragraphs::Piece;
use std::io::{Cursor, Read};

/// The text of a `.doc`, or `None` if it is not a readable one (no
/// `WordDocument` stream, encrypted document, unreadable piece table).
pub(crate) fn extract_doc(bytes: &[u8]) -> Option<String> {
    let mut cf = cfb::CompoundFile::open(Cursor::new(bytes)).ok()?;
    let word = read_stream(&mut cf, "/WordDocument")?;
    let u16_at = |b: &[u8], o: usize| Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?));
    let u32_at = |b: &[u8], o: usize| Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?));
    if u16_at(&word, 0)? != 0xA5EC {
        return None;
    }
    let flags = u16_at(&word, 0x0A)?;
    if flags & 0x0100 != 0 {
        return None; // fEncrypted
    }
    let table = read_stream(
        &mut cf,
        if flags & 0x0200 != 0 {
            "/1Table"
        } else {
            "/0Table"
        },
    )?;
    // FibRgW97 (csw words), FibRgLw97 (cslw integers), then the fc/lcb pairs.
    let csw = u16_at(&word, 32)? as usize;
    let lw = 34 + csw * 2 + 2;
    let cslw = u16_at(&word, lw - 2)? as usize;
    let ccp = |i: usize| u32_at(&word, lw + i * 4).map(|v| v as usize);
    let (text, ftn, hdd, mcr, atn, edn) = (ccp(3)?, ccp(4)?, ccp(5)?, ccp(6)?, ccp(7)?, ccp(8)?);
    let fc_lcb = lw + cslw * 4 + 2;
    let fc_clx = u32_at(&word, fc_lcb + 33 * 8)? as usize;
    let lcb_clx = u32_at(&word, fc_lcb + 33 * 8 + 4)? as usize;
    let (all, pieces) = piece_text(&word, table.get(fc_clx..fc_clx.checked_add(lcb_clx)?)?)?;

    let span = |from: usize, len: usize| all.get(from..from + len).unwrap_or(&[]);
    // FibRgFcLcb97: `fcStshf` is pair 1, `fcPlcfBtePapx` pair 13.
    let pair = |i: usize| -> Option<&[u8]> {
        let (fc, lcb) = (
            u32_at(&word, fc_lcb + i * 8)? as usize,
            u32_at(&word, fc_lcb + i * 8 + 4)? as usize,
        );
        table.get(fc..fc.checked_add(lcb)?)
    };
    let levels = pair(1).map(paragraphs::heading_styles).unwrap_or_default();
    // FibRgFcLcb97: `fcPlfLst` pair 73, `fcPlfLfo` pair 74; the `LVL`s follow
    // the `PlfLst`, hence reading up to the end of the stream.
    let lst_at = u32_at(&word, fc_lcb + 73 * 8).filter(|_| pair(73).is_some_and(|b| !b.is_empty()));
    let lists = lists::Lists::read(
        lst_at
            .and_then(|at| table.get(at as usize..))
            .unwrap_or(&[]),
        pair(74).unwrap_or(&[]),
    );
    // `fcPlcfBteChpx`, pair 12: hidden or deleted text does not come out.
    let mut body = pair(12).map_or_else(
        || span(0, text).to_vec(),
        |plc| hidden::mask_hidden(&word, plc, &pieces, span(0, text)),
    );
    // `fcPlcfendRef`, pair 46: the CPs of ENDNOTE calls. The body writes them
    // `\x02` like footnote calls; a single counter called the 1st endnote
    // `[^2]`, defined as `[^e1]` further down.
    if let Some(plc) = pair(46) {
        let calls = plc.len().saturating_sub(4) / 6;
        for cp in (0..calls).filter_map(|i| u32_at(plc, i * 4)) {
            if let Some(c) = body.get_mut(cp as usize).filter(|c| **c == '\u{2}') {
                *c = ENDNOTE_CALL;
            }
        }
    }
    let mut marks = pair(13)
        .map(|plc| paragraphs::paragraph_marks(&word, plc, &levels, &lists, &pieces, &body))
        .unwrap_or_default();
    // `ccpTxbx` (FibRgLw97, word 9): the text-box story follows the endnotes;
    // `fcPlcSpaMom` pair 40, `fcPlcftxbxTxt` pair 56.
    let txbx_at = text + ftn + hdd + mcr + atn + edn;
    let story = span(txbx_at, ccp(9).unwrap_or(0));
    if let (Some(spa), Some(txbx)) = (pair(40), pair(56)) {
        textboxes::insert(&mut body, &mut marks, textboxes::read(spa, txbx, story));
    }
    let body = &body;
    let mut out = render(&with_paragraph_prefixes(body, &marks), Notes::Calls);
    let ftn_at = text;
    let edn_at = text + ftn + hdd + mcr + atn;
    for (label, at, len, e) in [
        ("Footnotes", ftn_at, ftn, ""),
        ("Endnotes", edn_at, edn, "e"),
    ] {
        let notes: Vec<String> = render(span(at, len), Notes::Bodies)
            .split('\u{2}')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .enumerate()
            .map(|(i, n)| format!("[^{e}{}]: {n}", i + 1))
            .collect();
        if !notes.is_empty() {
            out.push_str(&format!("\n## {label}\n{}\n", notes.join("\n")));
        }
    }
    Some(out)
}

/// What an OLE2 container PROVES through its streams: `WordDocument` ⇒ Word,
/// `Workbook`/`Book` ⇒ BIFF workbook, `EncryptedPackage` ⇒ an encrypted OOXML
/// (kept on `Xls` so the notice names the password), `PowerPoint Document` ⇒
/// slide deck, `__properties_version1.0` ⇒ Outlook message. The rest is not a
/// format we read.
pub(crate) fn cfb_format(bytes: &[u8]) -> Option<super::Format> {
    let cf = cfb::CompoundFile::open(Cursor::new(bytes)).ok()?;
    if cf.is_stream("/WordDocument") {
        Some(super::Format::Doc)
    } else if cf.is_stream("/PowerPoint Document") {
        Some(super::Format::Ppt)
    } else if cf.is_stream("/__properties_version1.0") {
        Some(super::Format::Msg)
    } else if ["/Workbook", "/Book", "/EncryptedPackage"]
        .iter()
        .any(|s| cf.is_stream(s))
    {
        Some(super::Format::Xls)
    } else {
        None
    }
}

fn read_stream(cf: &mut cfb::CompoundFile<Cursor<&[u8]>>, path: &str) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    cf.open_stream(path).ok()?.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// All the document's characters, in position (CP) order, following the
/// `Clx` piece table.
fn piece_text(word: &[u8], clx: &[u8]) -> Option<(Vec<char>, Vec<Piece>)> {
    let mut i = 0;
    // The `Prc`s (property modifications) precede the table: skip them.
    while clx.get(i) == Some(&0x01) {
        let cb = u16::from_le_bytes(clx.get(i + 1..i + 3)?.try_into().ok()?) as usize;
        i += 3 + cb;
    }
    if clx.get(i) != Some(&0x02) {
        return None;
    }
    let lcb = u32::from_le_bytes(clx.get(i + 1..i + 5)?.try_into().ok()?) as usize;
    let plc = clx.get(i + 5..i + 5 + lcb)?;
    // n+1 positions of 4 bytes, then n descriptors of 8 bytes.
    let n = plc.len().checked_sub(4)? / 12;
    let cp = |k: usize| {
        u32::from_le_bytes(plc[k * 4..k * 4 + 4].try_into().unwrap_or_default()) as usize
    };
    let mut chars = Vec::new();
    let mut pieces = Vec::new();
    for k in 0..n {
        let len = cp(k + 1).checked_sub(cp(k))?;
        let pcd = &plc[(n + 1) * 4 + k * 8..(n + 1) * 4 + k * 8 + 8];
        let fc = u32::from_le_bytes(pcd[2..6].try_into().ok()?);
        let narrow = fc & 0x4000_0000 != 0;
        let at = if narrow {
            ((fc & !0x4000_0000) / 2) as usize
        } else {
            fc as usize
        };
        pieces.push(Piece {
            cp: cp(k),
            cp_end: cp(k + 1),
            fc: at,
            narrow,
        });
        if narrow {
            let raw = word.get(at..at + len)?;
            chars.extend(
                encoding_rs::WINDOWS_1252
                    .decode_without_bom_handling(raw)
                    .0
                    .chars(),
            );
        } else {
            let raw = word.get(at..at + len * 2)?;
            let units = raw
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]));
            chars.extend(char::decode_utf16(units).map(|c| c.unwrap_or('\u{FFFD}')));
        }
    }
    Some((chars, pieces))
}

/// The end of a paragraph INSIDE a cell: `layout` keeps it in the cell
/// instead of closing the table. Outside a document's character set (private
/// use area), it does not survive `layout`.
const CELL_PARAGRAPH: char = '\u{E000}';

/// An ENDNOTE call in the body (the file writes it `\x02`, like a footnote
/// call): rendered `[^eN]`, like its definitions.
const ENDNOTE_CALL: char = '\u{E003}';

/// Wrap, after a row's end mark, the right edges of its cells
/// (`1440,4320,5760`): the grid that places a merged cell. Private use area
/// like [`CELL_PARAGRAPH`] — `\x1E` would be a hyphen.
const ROW_EDGES_OPEN: char = '\u{E001}';
const ROW_EDGES_CLOSE: char = '\u{E002}';

/// The body, each paragraph preceded by its prefix (`#` of a heading, marker
/// of a list item) and, inside a cell, closed by [`CELL_PARAGRAPH`].
fn with_paragraph_prefixes(body: &[char], marks: &[paragraphs::Mark]) -> Vec<char> {
    if marks.is_empty() {
        return body.to_vec();
    }
    let mut out = Vec::with_capacity(body.len() + marks.len() * 3);
    let mut start = 0;
    for mark in marks {
        let end = mark.cp;
        let Some(para) = body.get(start..=end) else {
            continue;
        };
        if !mark.row_edges.is_empty() {
            out.extend_from_slice(para);
            out.push(ROW_EDGES_OPEN);
            out.extend(
                mark.row_edges
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
                    .chars(),
            );
            out.push(ROW_EDGES_CLOSE);
            start = end + 1;
            continue;
        }
        // The paragraph starts after the end mark (`\r`) or cell mark (`\x07`)
        // preceding it.
        let last = para.len() - 1;
        let from = para[..last]
            .iter()
            .rposition(|c| matches!(c, '\r' | '\u{7}'))
            .map_or(0, |i| i + 1);
        out.extend_from_slice(&para[..from]);
        out.extend(mark.prefix.chars());
        out.extend_from_slice(&para[from..last]);
        out.push(if mark.in_table { CELL_PARAGRAPH } else { '\r' });
        start = end + 1;
    }
    out.extend_from_slice(body.get(start..).unwrap_or(&[]));
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Notes {
    /// Document body: every automatic call becomes `[^N]`.
    Calls,
    /// Note text: the `\x02` mark opening each note is kept as a separator, so
    /// the caller can split them.
    Bodies,
}

/// [MS-DOC] special characters → text: fields, note calls, then paragraphs
/// (`\r`) and cells (`\x07`).
fn render(chars: &[char], notes: Notes) -> String {
    layout(&resolve_fields(chars, notes))
}

/// A field is written `\x13` instruction `\x14` result `\x15`, and nests: the
/// result is kept (what Word displays), a link becomes `[text](url)`, a table
/// of contents disappears (a copy of the headings).
fn resolve_fields(chars: &[char], notes: Notes) -> String {
    struct Field {
        instr: String,
        result: String,
        in_result: bool,
    }
    let mut out = String::new();
    let mut fields: Vec<Field> = Vec::new();
    let (mut calls, mut end_calls) = (0usize, 0usize);
    for &c in chars {
        let piece: String = match c {
            '\u{13}' => {
                fields.push(Field {
                    instr: String::new(),
                    result: String::new(),
                    in_result: false,
                });
                continue;
            }
            '\u{14}' => {
                if let Some(f) = fields.last_mut() {
                    f.in_result = true;
                }
                continue;
            }
            '\u{15}' => {
                let Some(f) = fields.pop() else { continue };
                let instr = f.instr.trim();
                if super::fields::is_a_copy_of_the_text(instr) {
                    continue;
                }
                match super::fields::hyperlink_url(instr) {
                    Some(url) if !f.result.trim().is_empty() => {
                        format!("[{}]({url})", f.result.trim())
                    }
                    _ => f.result,
                }
            }
            '\u{2}' if notes == Notes::Calls => {
                calls += 1;
                format!("[^{calls}]")
            }
            ENDNOTE_CALL => {
                end_calls += 1;
                format!("[^e{end_calls}]")
            }
            '\u{b}' | '\u{c}' => "\n".into(),
            '\u{1e}' => "-".into(),
            '\r' | '\u{7}' | '\t' | '\u{2}' => c.to_string(),
            c if (c as u32) < 0x20 => continue,
            c => c.to_string(),
        };
        match fields.last_mut() {
            Some(f) if f.in_result => f.result.push_str(&piece),
            Some(f) => f.instr.push_str(&piece),
            None => out.push_str(&piece),
        }
    }
    out
}

/// A paragraph ends with `\r`, a cell with `\x07`, and a table row with one
/// more `\x07`, alone. The paragraphs preceding a cell in the SAME row are its
/// own (a multi-paragraph cell); those of the FIRST cell arrive closed by
/// [`CELL_PARAGRAPH`] (property `sprmPFInTable`), without which nothing set
/// them apart from an ordinary paragraph and they came out before the table.
fn layout(flat: &str) -> String {
    let mut out = String::new();
    let mut unit = String::new();
    let mut row: Vec<String> = Vec::new();
    let mut held: Vec<String> = Vec::new();
    let mut table: Vec<Vec<String>> = Vec::new();
    let mut edges: Vec<Vec<i64>> = Vec::new();
    let mut chars = flat.chars();
    while let Some(c) = chars.next() {
        match c {
            // The edges of the row that the previous `\x07` just closed.
            ROW_EDGES_OPEN => {
                let list: String = chars
                    .by_ref()
                    .take_while(|&c| c != ROW_EDGES_CLOSE)
                    .collect();
                edges.resize(table.len().saturating_sub(1), Vec::new());
                edges.push(list.split(',').filter_map(|e| e.parse().ok()).collect());
            }
            '\r' if row.is_empty() => {
                if !table.is_empty() {
                    push_table(&mut out, &mut table, &mut edges);
                    out.push('\n');
                }
                out.push_str(unit.trim_end());
                out.push('\n');
                unit.clear();
            }
            '\r' | CELL_PARAGRAPH => held.push(std::mem::take(&mut unit)),
            '\u{7}' if unit.is_empty() && held.is_empty() && !row.is_empty() => {
                table.push(std::mem::take(&mut row));
            }
            '\u{7}' => {
                held.push(std::mem::take(&mut unit));
                let cell = held
                    .drain(..)
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty());
                row.push(cell.collect::<Vec<_>>().join(" ").replace('|', "\\|"));
            }
            c => unit.push(c),
        }
    }
    push_table(&mut out, &mut table, &mut edges);
    out.push_str(unit.trim_end());
    out.trim_end().to_string() + "\n"
}

/// A table as `|` rows, each row padded to the widest: a cell merged over two
/// columns (`Quarters` above two quarters) gives a shorter row, and a
/// two-column header over three-column rows made a wrong markdown table.
fn push_table(out: &mut String, table: &mut Vec<Vec<String>>, edges: &mut Vec<Vec<i64>>) {
    edges.resize(table.len(), Vec::new());
    super::tables::spread_merged_cells(table, edges);
    edges.clear();
    let width = table.iter().map(Vec::len).max().unwrap_or(0);
    for (i, mut row) in table.drain(..).enumerate() {
        row.resize(width, String::new());
        out.push_str(&format!("| {} |\n", row.join(" | ")));
        if i == 0 {
            out.push_str(&format!("|{}\n", " --- |".repeat(width)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A minimal `.doc`: FIB, two pieces (cp1252 then UTF-16), a `1Table`.
    fn make_doc(cp1252: &str, utf16: &str, ccp_text: usize, ccp_ftn: usize) -> Vec<u8> {
        let (csw, cslw, cb) = (14usize, 22usize, 93usize);
        let lw = 34 + csw * 2 + 2;
        let fc_lcb = lw + cslw * 4 + 2;
        let text_at = 1024usize;
        let narrow: Vec<u8> = cp1252.chars().map(|c| c as u8).collect();
        let wide_at = text_at + narrow.len() + (narrow.len() % 2);
        let wide: Vec<u8> = utf16.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut word = vec![0u8; wide_at + wide.len()];
        word[0..2].copy_from_slice(&0xA5ECu16.to_le_bytes());
        word[0x0A..0x0C].copy_from_slice(&0x0200u16.to_le_bytes());
        word[32..34].copy_from_slice(&(csw as u16).to_le_bytes());
        word[lw - 2..lw].copy_from_slice(&(cslw as u16).to_le_bytes());
        word[lw + 12..lw + 16].copy_from_slice(&(ccp_text as u32).to_le_bytes());
        word[lw + 16..lw + 20].copy_from_slice(&(ccp_ftn as u32).to_le_bytes());
        word[fc_lcb - 2..fc_lcb].copy_from_slice(&(cb as u16).to_le_bytes());
        word[text_at..text_at + narrow.len()].copy_from_slice(&narrow);
        word[wide_at..].copy_from_slice(&wide);
        let n1 = narrow.len() as u32;
        let n2 = n1 + utf16.encode_utf16().count() as u32;
        let mut plc = Vec::new();
        for cp in [0, n1, n2] {
            plc.extend(cp.to_le_bytes());
        }
        for fc in [(text_at as u32 * 2) | 0x4000_0000, wide_at as u32] {
            plc.extend([0, 0]);
            plc.extend(fc.to_le_bytes());
            plc.extend([0, 0]);
        }
        let mut table = vec![0x02];
        table.extend((plc.len() as u32).to_le_bytes());
        table.extend(plc);
        word[fc_lcb + 33 * 8..fc_lcb + 33 * 8 + 4].copy_from_slice(&0u32.to_le_bytes());
        word[fc_lcb + 33 * 8 + 4..fc_lcb + 33 * 8 + 8]
            .copy_from_slice(&(table.len() as u32).to_le_bytes());
        let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        cf.create_stream("/WordDocument")
            .unwrap()
            .write_all(&word)
            .unwrap();
        cf.create_stream("/1Table")
            .unwrap()
            .write_all(&table)
            .unwrap();
        cf.flush().unwrap();
        cf.into_inner().into_inner()
    }

    #[test]
    fn a_word97_document_reads_as_text_tables_links_and_notes() {
        let body = "Caf\u{e9} intro\rEnv\u{7}Owner\u{7}\u{7}prod\u{7}ops\u{7}\u{7}See \u{13} HYPERLINK \"https://x.y/a\" \u{14}the site\u{15} now\u{2}.\r";
        let wide = "\u{65e5}\u{672c} end.\r";
        let ftn = "\u{2} Note body.\r";
        let ccp_text = body.chars().count() + wide.chars().count();
        let doc = make_doc(body, &format!("{wide}{ftn}"), ccp_text, ftn.chars().count());
        assert_eq!(cfb_format(&doc), Some(crate::docs::Format::Doc));
        let out = extract_doc(&doc).expect("readable");
        let want = "Café intro\n| Env | Owner |\n| --- | --- |\n| prod | ops |\n\nSee [the site](https://x.y/a) now[^1].\n日本 end.\n\n## Footnotes\n[^1]: Note body.\n";
        assert_eq!(out, want);
    }

    /// Headings come from each paragraph's STYLE: LibreOffice's `.doc` of a
    /// Title / Heading 1 / Heading 2 document (the same as `.docx` reads).
    #[test]
    fn a_heading_style_marks_its_paragraph() {
        let doc = include_bytes!("../../tests/fixtures/docs/plan.doc");
        let out = extract_doc(doc).expect("readable");
        for want in ["# Annual Plan\n", "# Goals\n", "## Budget\n"] {
            assert!(out.contains(want), "{want:?} : {out}");
        }
        assert!(out.contains("\nGrow revenue by 20 percent.\n"), "{out}");
        assert!(!out.contains("# Grow") && !out.contains("# Bold"), "{out}");
        // Automatic numbering and bullets, counted per list and level.
        assert!(
            out.contains("\n1. First goal\n2. Second goal\n- A bullet\n  - A nested bullet\n"),
            "{out}"
        );
        // `Quarters` spans two columns: the header row is filled to the width
        // of the rows under it.
        assert!(
            out.contains("| Team | Quarters |  |\n| --- | --- | --- |\n| Core | 10 | 12 |\n"),
            "{out}"
        );
    }

    /// A first cell of two paragraphs stays one cell: its first paragraph
    /// used to come out above the table, the cell left with the second.
    #[test]
    fn a_paragraph_inside_the_first_cell_stays_in_it() {
        let out =
            extract_doc(include_bytes!("../../tests/fixtures/docs/cell.doc")).expect("readable");
        assert_eq!(out, "Intro line.\n| First para Second para | B1 |\n| --- | --- |\n| A2 | B2 |\n\nAfter table.\n");
    }

    /// Hidden text and a tracked deletion are not what Word shows: left out,
    /// as `.docx` leaves them out, and the paragraphs around them intact.
    #[test]
    fn hidden_and_deleted_text_stay_out() {
        let out = extract_doc(include_bytes!(
            "../../tests/fixtures/docs/hidden_and_deleted.doc"
        ))
        .expect("readable");
        assert_eq!(out, "Visible start  visible end.\nSecond paragraph.\n");
    }

    /// Two cells merged in the middle of a row: the cell after them stays in
    /// its own column (`sprmTDefTable` gives the edges), where it used to slide
    /// under the merged pair's second column.
    #[test]
    fn a_cell_merged_mid_row_keeps_the_next_cell_in_its_column() {
        let out =
            extract_doc(include_bytes!("../../tests/fixtures/docs/midspan.doc")).expect("readable");
        assert!(out.contains("| Item | Q1 | Q2 | Total |\n"), "{out}");
        assert!(
            out.contains("| Gadget | pending |  | TOTAL-NEEDLE |\n"),
            "{out}"
        );
    }

    /// An endnote call is numbered with the endnotes, not after the
    /// footnotes: it read `[^2]` while its definition read `[^e1]`.
    #[test]
    fn an_endnote_call_names_the_endnote_it_points_to() {
        let out =
            extract_doc(include_bytes!("../../tests/fixtures/docs/endnote.doc")).expect("readable");
        assert!(
            out.starts_with("Body one[^1] and [^e1] more text.\n"),
            "{out}"
        );
        assert!(
            out.contains("[^1]: FOOT-NEEDLE\n") && out.contains("[^e1]: END-NEEDLE\n"),
            "{out}"
        );
    }

    /// A text box's text lives in its own story: it is served right after
    /// the paragraph that anchors it, as the `.docx` of the same document does.
    #[test]
    fn a_text_box_reads_after_its_anchor_paragraph() {
        let out =
            extract_doc(include_bytes!("../../tests/fixtures/docs/textbox.doc")).expect("readable");
        assert!(
            out.starts_with(
                "First paragraph.\nSIDEBAR-NEEDLE boxed note\nsecond box line\nSecond paragraph.\n"
            ),
            "{out}"
        );
        assert!(out.contains("Last word end.\nINLINE-BOX"), "{out}");
    }

    /// A jump to a bookmark has no address: plain text, as in `.docx` and
    /// `.odt` — where it used to read `[the intro](intro)`.
    #[test]
    fn a_link_to_a_bookmark_is_plain_text_and_a_web_link_keeps_its_address() {
        let out =
            extract_doc(include_bytes!("../../tests/fixtures/docs/links.doc")).expect("readable");
        assert!(out.contains("back to the intro, bare"), "{out}");
        assert!(out.contains("[Ann](mailto:ann@example.com)"), "{out}");
    }

    /// A numbered list shows what its level declares: letters, roman
    /// figures, its start — as LibreOffice shows the same file (`5. Fifth`,
    /// then `1. Sixth`: the file puts them in two unlinked lists).
    #[test]
    fn a_numbered_list_shows_its_own_labels() {
        let out = extract_doc(include_bytes!(
            "../../tests/fixtures/docs/numbered_lists.doc"
        ))
        .expect("readable");
        assert!(out.contains("a) Alpha item\nb) Beta item\ni. Roman one\nii. Roman two\niii. Roman three\n5. Fifth\n1. Sixth"), "{out}");
    }

    fn container_with(stream: &str) -> Vec<u8> {
        let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        cf.create_stream(stream).unwrap().write_all(b"x").unwrap();
        cf.flush().unwrap();
        cf.into_inner().into_inner()
    }

    #[test]
    fn an_ole2_container_is_named_only_by_the_stream_that_proves_it() {
        let xls = container_with("/Workbook");
        assert_eq!(cfb_format(&xls), Some(crate::docs::Format::Xls));
        assert_eq!(extract_doc(&xls), None);
        // A PowerPoint deck and an Outlook message share the container and
        // are read by nothing here: no format, so no « tokenade reads … » claim.
        {
            let unread = "/SummaryInformation";
            let bytes = container_with(unread);
            assert_eq!(
                crate::docs::detect_format(Some("deck.ppt"), &bytes),
                None,
                "{unread}"
            );
            assert_eq!(crate::docs::detect_format(None, &bytes), None, "{unread}");
        }
    }

    /// A content sniffer labels EVERY OLE2 container `doc`; a workbook named
    /// that way must still reach the workbook reader, on both detection paths.
    #[test]
    fn inside_an_ole2_container_the_streams_overrule_the_name() {
        use crate::docs::{detect_format, detect_format_at, Format};
        let xls = container_with("/Workbook");
        assert_eq!(detect_format(Some("x.doc"), &xls), Some(Format::Xls));
        let dir = std::env::temp_dir().join(format!("tk-ole2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes, want) in [
            ("sheet", xls.clone(), Some(Format::Xls)),
            ("report.doc", xls, Some(Format::Xls)),
            (
                "deck",
                container_with("/PowerPoint Document"),
                Some(Format::Ppt),
            ),
            (
                "mail",
                container_with("/__properties_version1.0"),
                Some(Format::Msg),
            ),
            ("other", container_with("/SummaryInformation"), None),
            (
                "letter",
                br"{\rtf1\ansi Hello.\par}".to_vec(),
                Some(Format::Rtf),
            ),
        ] {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            assert_eq!(detect_format_at(&p), want, "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
