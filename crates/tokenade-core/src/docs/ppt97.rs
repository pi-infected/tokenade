//! **PowerPoint 97–2003 (`.ppt`) → text, slide by slide.**
//!
//! The `PowerPoint Document` stream is a sequence of RECORDS (8-byte
//! header: version/instance, type, length; version `0xF` = container). A
//! record saved « quickly » is APPENDED at the end of the stream without removing
//! the old one: reading the slides in stream order would serve stale
//! versions. So we follow the path [MS-PPT] §2.1.2 prescribes:
//! `Current User` → last `UserEditAtom` → persistence directories (the most
//! recent wins) → `DocumentContainer` → `SlideListWithText`.
//!
//! A slide's text lives in two places: that of the placeholders
//! (title, body) in the `SlideListWithText`, that of the other text boxes
//! in the slide's drawing (`OfficeArtClientTextbox`). The notes live
//! in the instance-2 list. Rendered like the `.pptx`: `## Slide N`, the body
//! as bullets, `### Notes`, and hidden slides silenced but counted.

mod bullets;
mod links;
mod tables;

use bullets::Para;
use std::collections::HashMap;
use std::io::{Cursor, Read};

const DOCUMENT: u16 = 0x03E8;
const SLIDE: u16 = 0x03EE;
const NOTES: u16 = 0x03F0;
const NOTES_ATOM: u16 = 0x03F1;
const SLIDE_PERSIST: u16 = 0x03F3;
const SLIDE_SHOW_INFO: u16 = 0x03F9;
const OUTLINE_TEXT_REF: u16 = 0x0F9E;
const TEXT_HEADER: u16 = 0x0F9F;
const TEXT_CHARS: u16 = 0x0FA0;
const TEXT_BYTES: u16 = 0x0FA8;
const SLIDE_LIST: u16 = 0x0FF0;
const USER_EDIT: u16 = 0x0FF5;
const PERSIST_DIRECTORY: u16 = 0x1772;
const STYLE_TEXT_PROP: u16 = 0x0FA1;

#[derive(Clone, Copy)]
struct Rec {
    instance: u16,
    kind: u16,
    /// Start of the BODY (after the header), and its end.
    start: usize,
    end: usize,
    container: bool,
}

fn rec_at(s: &[u8], at: usize) -> Option<Rec> {
    let h = s.get(at..at + 8)?;
    let ver_inst = u16::from_le_bytes([h[0], h[1]]);
    let len = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
    let end = (at + 8).checked_add(len)?;
    (end <= s.len()).then_some(Rec {
        instance: ver_inst >> 4,
        kind: u16::from_le_bytes([h[2], h[3]]),
        start: at + 8,
        end,
        container: ver_inst & 0xF == 0xF,
    })
}

/// The direct children of a container.
fn children(s: &[u8], r: Rec) -> Vec<Rec> {
    let mut out = Vec::new();
    let mut at = r.start;
    while let Some(c) = rec_at(s, at) {
        if c.end > r.end {
            break;
        }
        out.push(c);
        at = c.end;
    }
    out
}

/// All descendants, in document order.
fn descendants(s: &[u8], r: Rec, out: &mut Vec<Rec>, depth: usize) {
    if depth > 32 {
        return;
    }
    for c in children(s, r) {
        out.push(c);
        if c.container {
            descendants(s, c, out, depth + 1);
        }
    }
}

fn u32_at(s: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(s.get(at..at + 4)?.try_into().ok()?))
}

/// The text of a `TextCharsAtom` (UTF-16) or of a `TextBytesAtom` (one byte per
/// character, the low half of UTF-16 — i.e. Latin-1).
fn atom_text(s: &[u8], r: Rec) -> String {
    let body = &s[r.start..r.end];
    let raw: String = if r.kind == TEXT_CHARS {
        let units = body
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]));
        char::decode_utf16(units)
            .map(|c| c.unwrap_or('\u{FFFD}'))
            .collect()
    } else {
        body.iter().map(|&b| b as char).collect()
    };
    // `\r` separates paragraphs, `\x0B` forces a line break INSIDE the
    // paragraph (U+2028, rendered by `placeholder_text`).
    raw.chars()
        .map(|c| match c {
            '\r' => '\n',
            '\u{B}' => '\u{2028}',
            c => c,
        })
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect()
}

/// Where each paragraph of a text atom starts, in units of the text
/// (UTF-16 or bytes): what the runs of the `StyleTextPropAtom` count.
fn paragraph_starts(s: &[u8], r: Rec) -> Vec<usize> {
    let units: Vec<u16> = match s.get(r.start..r.end) {
        Some(b) if r.kind == TEXT_CHARS => b
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect(),
        Some(b) => b.iter().map(|&b| b as u16).collect(),
        None => Vec::new(),
    };
    std::iter::once(0)
        .chain(
            units
                .iter()
                .enumerate()
                .filter(|(_, u)| **u == 13)
                .map(|(i, _)| i + 1),
        )
        .collect()
}

/// The paragraph style of a text atom, read in the
/// `StyleTextPropAtom` that follows it among its siblings (none: all inherited).
fn text_paras(s: &[u8], siblings: &[Rec], text: usize) -> Vec<Para> {
    let Some(r) = siblings.get(text) else {
        return Vec::new();
    };
    match siblings.get(text + 1).filter(|n| n.kind == STYLE_TEXT_PROP) {
        Some(style) => bullets::paragraph_styles(
            s.get(style.start..style.end).unwrap_or(&[]),
            &paragraph_starts(s, *r),
        ),
        None => Vec::new(),
    }
}

/// The text of a placeholder, formatted paragraph by paragraph: a
/// bullet and an indent when its style carries one, otherwise according to the
/// placeholder type (`TextHeaderAtom`) — a body reads as bullets, the rest as is.
fn placeholder_text(text: &str, text_type: u32, paras: &[Para]) -> String {
    // 1 = Body, 5 = CenterBody, 7 = HalfBody, 8 = QuarterBody.
    let body = matches!(text_type, 1 | 7 | 8);
    let mut out: Vec<String> = Vec::new();
    for (i, para) in text.split('\n').enumerate() {
        if para.trim().is_empty() {
            continue;
        }
        let style = paras.get(i).copied().unwrap_or_default();
        let lines = para
            .split('\u{2028}')
            .map(str::trim)
            .filter(|l| !l.is_empty());
        if style.bullet.unwrap_or(body) {
            let hang = "  ".repeat(style.level);
            let joined = lines.collect::<Vec<_>>().join(&format!("\n{hang}  "));
            out.push(format!("{hang}- {joined}"));
        } else {
            out.extend(lines.map(str::to_string));
        }
    }
    out.join("\n")
}

/// The text of a `SlideListWithText`, grouped by `SlidePersistAtom`:
/// (persistIdRef, slideId) → its texts, in order.
fn slide_list(
    s: &[u8],
    list: Rec,
    links: &HashMap<u32, String>,
) -> Vec<(u32, u32, Vec<(u32, String)>)> {
    let mut out: Vec<(u32, u32, Vec<(u32, String)>)> = Vec::new();
    let mut text_type = 4u32; // Other
    let kids = children(s, list);
    for (i, c) in kids.iter().copied().enumerate() {
        match c.kind {
            SLIDE_PERSIST => {
                let persist = u32_at(s, c.start).unwrap_or(0);
                let slide_id = u32_at(s, c.start + 12).unwrap_or(0);
                out.push((persist, slide_id, Vec::new()));
            }
            TEXT_HEADER => text_type = u32_at(s, c.start).unwrap_or(4),
            TEXT_CHARS | TEXT_BYTES => {
                if let Some(last) = out.last_mut() {
                    let text = links::apply(&atom_text(s, c), links::ranges(s, &kids, i, links));
                    let t = placeholder_text(&text, text_type, &text_paras(s, &kids, i));
                    if !t.is_empty() {
                        last.2.push((text_type, t));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// persistId → position in the stream, following the chain of `UserEditAtom`s
/// from most recent to oldest; the first write of an identifier wins.
fn persist_map(s: &[u8], mut edit: usize) -> Option<(HashMap<u32, usize>, u32)> {
    let mut map = HashMap::new();
    let mut doc_ref = None;
    for _ in 0..1024 {
        let ue = rec_at(s, edit).filter(|r| r.kind == USER_EDIT)?;
        doc_ref.get_or_insert(u32_at(s, ue.start + 16)?);
        let last = u32_at(s, ue.start + 8)? as usize;
        let dir_at = u32_at(s, ue.start + 12)? as usize;
        if let Some(dir) = rec_at(s, dir_at).filter(|r| r.kind == PERSIST_DIRECTORY) {
            let mut at = dir.start;
            while at + 4 <= dir.end {
                let entry = u32_at(s, at)?;
                let (first, count) = (entry & 0xF_FFFF, (entry >> 20) as usize);
                at += 4;
                for k in 0..count {
                    let off = u32_at(s, at + k * 4)? as usize;
                    map.entry(first + k as u32).or_insert(off);
                }
                at += count * 4;
            }
        }
        if last == 0 {
            break;
        }
        edit = last;
    }
    Some((map, doc_ref?))
}

/// A hidden shape: `fHidden` AND its usage bit `fUsefHidden` in its
/// group-shape boolean properties (0x03BF) — [MS-ODRAW] §2.3.4.44.
fn shape_is_hidden(s: &[u8], shape: Rec) -> bool {
    children(s, shape)
        .iter()
        .filter(|c| matches!(c.kind, 0xF00B | 0xF122))
        .any(|opt| {
            (0..opt.instance as usize).any(|i| {
                let at = opt.start + i * 6;
                let id = s
                    .get(at..at + 2)
                    .and_then(|b| <[u8; 2]>::try_from(b).ok())
                    .map(u16::from_le_bytes);
                id.is_some_and(|id| id & 0x3FFF == 0x03BF)
                    && u32_at(s, at + 2).is_some_and(|v| v & 0x0002_0002 == 0x0002_0002)
            })
        })
}

/// The text boxes of a slide's drawing that do NOT refer to a
/// placeholder (those are already served by the `SlideListWithText`).
fn drawing_texts(s: &[u8], slide: Rec, links: &HashMap<u32, String>) -> Vec<(u32, String)> {
    let mut all = Vec::new();
    descendants(s, slide, &mut all, 0);
    let mut out = Vec::new();
    // Hidden shapes (« Selection » pane): their range, or that of the whole
    // group when it is the GROUP shape (first child of a `0xF003`)
    // that is hidden. Their text is never shown.
    let hidden: Vec<(usize, usize)> = all
        .iter()
        .filter(|r| r.kind == 0xF004 && r.container && shape_is_hidden(s, **r))
        .map(|sp| {
            all.iter()
                .find(|g| {
                    g.kind == 0xF003
                        && children(s, **g)
                            .first()
                            .is_some_and(|c| c.start == sp.start)
                })
                .map_or((sp.start, sp.end), |g| (g.start, g.end))
        })
        .collect();
    let all: Vec<Rec> = all
        .into_iter()
        .filter(|r| !hidden.iter().any(|&(a, b)| a <= r.start && r.end <= b))
        .collect();
    // Tables (`0xF003` groups), served whole in place of their
    // first cell; their cells are no longer read one by one.
    let tables: Vec<(Rec, String)> = all
        .iter()
        .filter(|r| r.kind == 0xF003 && r.container)
        .filter_map(|g| tables::table_text(s, *g).map(|t| (*g, t)))
        .collect();
    let mut served = vec![false; tables.len()];
    for tb in all.iter().filter(|r| r.kind == 0xF00D && r.container) {
        if let Some(i) = tables
            .iter()
            .position(|(g, _)| g.start <= tb.start && tb.end <= g.end)
        {
            if let (Some(done), Some((_, table))) = (served.get_mut(i), tables.get(i)) {
                if !*done {
                    *done = true;
                    // The empty line closes the table: glued on, the next line
                    // would become a row of it.
                    out.push((4, format!("{table}\n")));
                }
            }
            continue;
        }
        let kids = children(s, *tb);
        if kids.iter().any(|k| k.kind == OUTLINE_TEXT_REF) {
            continue;
        }
        // A placeholder can also carry its text HERE (LibreOffice writes
        // it there), with its `TextHeaderAtom`: a body stays a body.
        let text_type = kids
            .iter()
            .find(|k| k.kind == TEXT_HEADER)
            .and_then(|k| u32_at(s, k.start))
            .unwrap_or(4);
        // A field (slide number, date, header, footer) is written `*` in
        // the text, at the position its « meta-character » atom gives: it
        // is not a character of the document.
        let fields: Vec<usize> = kids
            .iter()
            .filter(|k| matches!(k.kind, 0x0FD8 | 0x0FF7..=0x0FFB))
            .filter_map(|k| u32_at(s, k.start).map(|p| p as usize))
            .collect();
        for (i, k) in kids
            .iter()
            .enumerate()
            .filter(|(_, k)| matches!(k.kind, TEXT_CHARS | TEXT_BYTES))
        {
            let text: String = atom_text(s, *k)
                .chars()
                .enumerate()
                .filter(|(i, c)| !(*c == '*' && fields.contains(i)))
                .map(|(_, c)| c)
                .collect();
            let text = links::apply(&text, links::ranges(s, &kids, i, links));
            let t = placeholder_text(&text, text_type, &text_paras(s, &kids, i));
            if !t.is_empty() {
                out.push((text_type, t));
            }
        }
    }
    out
}

fn is_hidden(s: &[u8], slide: Rec) -> bool {
    children(s, slide)
        .iter()
        .find(|c| c.kind == SLIDE_SHOW_INFO)
        .and_then(|c| s.get(c.start + 10..c.start + 12))
        .is_some_and(|f| u16::from_le_bytes([f[0], f[1]]) & 0x4 != 0)
}

/// The text of a `.ppt`, or `None` if it is not a readable one (no
/// `PowerPoint Document` stream, encrypted document, broken edit chain).
pub(crate) fn extract_ppt(bytes: &[u8]) -> Option<String> {
    let mut cf = cfb::CompoundFile::open(Cursor::new(bytes)).ok()?;
    let mut read = |p: &str| -> Option<Vec<u8>> {
        let mut buf = Vec::new();
        cf.open_stream(p).ok()?.read_to_end(&mut buf).ok()?;
        Some(buf)
    };
    let user = read("/Current User")?;
    let s = read("/PowerPoint Document")?;
    // headerToken: 0xE391C05F in clear, 0xF3D1C4DF encrypted.
    if u32_at(&user, 12)? != 0xE391_C05F {
        return None;
    }
    let (persist, doc_ref) = persist_map(&s, u32_at(&user, 16)? as usize)?;
    let doc = rec_at(&s, *persist.get(&doc_ref)?).filter(|r| r.kind == DOCUMENT)?;
    let links = links::targets(&s, doc);
    let lists = children(&s, doc);
    let list = |inst: u16| {
        lists
            .iter()
            .find(|r| r.kind == SLIDE_LIST && r.instance == inst)
            .map(|r| slide_list(&s, *r, &links))
    };
    let slides = list(0).unwrap_or_default();
    // Notes: the notes page cites its slide (`NotesAtom.slideIdRef`);
    // its text is in the instance-2 list OR in its own drawing.
    let mut notes_lists: HashMap<u32, Vec<String>> = HashMap::new();
    for (persist_ref, _, texts) in list(2).unwrap_or_default() {
        notes_lists.insert(persist_ref, texts.into_iter().map(|(_, t)| t).collect());
    }
    let mut notes_by_id: HashMap<u32, Vec<String>> = HashMap::new();
    for (id, &at) in &persist {
        let Some(page) = rec_at(&s, at).filter(|r| r.kind == NOTES && r.container) else {
            continue;
        };
        let Some(slide_id) = children(&s, page)
            .into_iter()
            .find(|c| c.kind == NOTES_ATOM)
            .and_then(|a| u32_at(&s, a.start))
        else {
            continue;
        };
        let mut texts = notes_lists.remove(id).unwrap_or_default();
        texts.extend(drawing_texts(&s, page, &links).into_iter().map(|(_, t)| t));
        let texts: Vec<String> = texts
            .iter()
            .flat_map(|t| {
                t.lines()
                    .map(|l| l.trim_start_matches("- ").to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        if !texts.is_empty() {
            notes_by_id.insert(slide_id, texts);
        }
    }

    let mut out = String::new();
    let mut hidden = Vec::new();
    for (n, (persist_ref, slide_id, texts)) in slides.iter().enumerate() {
        let container = persist
            .get(persist_ref)
            .and_then(|&at| rec_at(&s, at))
            .filter(|r| r.kind == SLIDE);
        if container.is_some_and(|c| is_hidden(&s, c)) {
            hidden.push((n + 1).to_string());
            continue;
        }
        let mut typed: Vec<(u32, String)> = texts.clone();
        if let Some(c) = container {
            typed.extend(drawing_texts(&s, c, &links));
        }
        // 0 = Title, 6 = CenterTitle: the title leads the heading, as in the
        // pptx and odp renderings (`slide_section`).
        let title = typed
            .iter()
            .find(|(t, _)| matches!(t, 0 | 6))
            .map(|(_, x)| x.split_whitespace().collect::<Vec<_>>().join(" "));
        let body: Vec<String> = typed.into_iter().map(|(_, t)| t).collect();
        out.push_str(
            super::notes_pages::slide_section("Slide", n + 1, None, title, &body.join("\n"))
                .trim_start(),
        );
        if let Some(notes) = notes_by_id.get(slide_id) {
            out.push_str(&format!("\n### Notes\n{}\n", notes.join("\n")));
        }
        out.push('\n');
    }
    let mut out = out.trim_end().to_string() + "\n";
    out.push_str(&super::spreadsheet::hidden_note(
        "slide",
        hidden.len(),
        &hidden,
    ));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn rec(ver_inst: u16, kind: u16, body: &[u8]) -> Vec<u8> {
        let mut v = ver_inst.to_le_bytes().to_vec();
        v.extend(kind.to_le_bytes());
        v.extend((body.len() as u32).to_le_bytes());
        v.extend(body);
        v
    }
    fn container(kind: u16, inst: u16, kids: &[Vec<u8>]) -> Vec<u8> {
        rec(0xF | inst << 4, kind, &kids.concat())
    }
    fn text_type(t: u32) -> Vec<u8> {
        rec(0, TEXT_HEADER, &t.to_le_bytes())
    }
    fn chars(t: &str) -> Vec<u8> {
        rec(
            0,
            TEXT_CHARS,
            &t.encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        )
    }
    fn bytes(t: &str) -> Vec<u8> {
        rec(0, TEXT_BYTES, t.as_bytes())
    }
    fn textbox(kids: &[Vec<u8>]) -> Vec<u8> {
        container(0x040C, 0, &[container(0xF00D, 0, kids)])
    }
    fn persist_atom(persist: u32, slide_id: u32) -> Vec<u8> {
        let body: Vec<u8> = [persist, 0, 0, slide_id, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        rec(0, SLIDE_PERSIST, &body)
    }

    /// A deck as PowerPoint lays it out: placeholders in the slide list, a free
    /// text box in the slide's drawing, notes in the notes page's drawing with
    /// a slide-number field (`*`), and a hidden second slide.
    fn make_ppt() -> Vec<u8> {
        let mut s = Vec::new();
        let at = |s: &mut Vec<u8>, r: Vec<u8>| {
            let o = s.len() as u32;
            s.extend(r);
            o
        };
        // LibreOffice: a content placeholder exported as a plain text box,
        // typed « Other »; its paragraphs' style says bullet and level.
        let pf = |count: u32, level: u16| {
            [
                count.to_le_bytes().to_vec(),
                level.to_le_bytes().to_vec(),
                1u32.to_le_bytes().to_vec(),
                vec![1, 0],
            ]
            .concat()
        };
        let style = rec(0, STYLE_TEXT_PROP, &[pf(9, 0), pf(7, 1)].concat());
        let outline = textbox(&[text_type(4), chars("Item one\rDeeper"), style]);
        let slide1 = at(
            &mut s,
            container(
                SLIDE,
                0,
                &[
                    textbox(&[text_type(4), chars("Box \u{65e5}\u{672c}")]),
                    outline,
                ],
            ),
        );
        let mut info = vec![0u8; 16];
        info[10] = 0x4;
        let slide2 = at(
            &mut s,
            container(SLIDE, 0, &[rec(0, SLIDE_SHOW_INFO, &info)]),
        );
        let field = rec(0, 0x0FD8, &5u32.to_le_bytes());
        let notes = at(
            &mut s,
            container(
                NOTES,
                0,
                &[
                    rec(0, NOTES_ATOM, &[256u32.to_le_bytes(), [0; 4]].concat()),
                    textbox(&[text_type(2), bytes("Note *one"), field]),
                ],
            ),
        );
        let list = container(
            SLIDE_LIST,
            0,
            &[
                persist_atom(2, 256),
                text_type(0),
                bytes("Title one"),
                text_type(1),
                chars("Point A\rPoint B"),
                persist_atom(3, 257),
                text_type(0),
                bytes("Secret"),
            ],
        );
        let doc = at(&mut s, container(DOCUMENT, 0, &[list]));
        let dir_body: Vec<u8> = [1u32 | 4 << 20, doc, slide1, slide2, notes]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let dir = at(&mut s, rec(0, PERSIST_DIRECTORY, &dir_body));
        let edit_body: Vec<u8> = [0u32, 0, 0, dir, 1, 4]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let edit = at(&mut s, rec(0, USER_EDIT, &edit_body));
        let user_body: Vec<u8> = [0x14u32, 0xE391_C05F, edit]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let user = rec(0, 0x0FF6, &user_body);
        let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        cf.create_stream("/Current User")
            .unwrap()
            .write_all(&user)
            .unwrap();
        cf.create_stream("/PowerPoint Document")
            .unwrap()
            .write_all(&s)
            .unwrap();
        cf.flush().unwrap();
        cf.into_inner().into_inner()
    }

    /// A table is a group: a frame shape carrying `tableProperties`, one
    /// rectangle per cell placed by its child anchor, border segments. It
    /// reads as rows, the merged cell's neighbour left empty.
    #[test]
    fn a_table_group_reads_as_rows() {
        let opt = rec(0x3 | 1 << 4, 0xF122, &[0xA0, 0x03, 0, 0, 0, 0]);
        let frame = container(0xF004, 0, &[opt]);
        let cell = |left: i32, top: i32, text: &str| {
            let anchor: Vec<u8> = [left, top, left + 10, top + 10]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect();
            container(
                0xF004,
                0,
                &[
                    rec(0x2 | 1 << 4, 0xF00A, &[0; 8]),
                    rec(0, 0xF00F, &anchor),
                    container(0xF00D, 0, &[text_type(4), chars(text)]),
                ],
            )
        };
        let line = container(
            0xF004,
            0,
            &[
                rec(0x2 | 20 << 4, 0xF00A, &[0; 8]),
                rec(0, 0xF00F, &[0; 16]),
            ],
        );
        let group = container(
            0xF003,
            0,
            &[
                frame,
                cell(0, 0, "Plan"),
                cell(10, 0, "Cost"),
                cell(0, 10, "Pro"),
                cell(10, 10, "9 | 12"),
                cell(0, 20, "Merged"),
                line,
            ],
        );
        let after = textbox(&[text_type(4), chars("After")]);
        let slide = container(
            SLIDE,
            0,
            &[
                container(0x040C, 0, &[container(0xF002, 0, &[group])]),
                after,
            ],
        );
        let texts: Vec<String> = drawing_texts(&slide, rec_at(&slide, 0).unwrap(), &HashMap::new())
            .into_iter()
            .map(|(_, t)| t)
            .collect();
        assert_eq!(
            texts,
            [
                "| Plan | Cost |\n| --- | --- |\n| Pro | 9 \\| 12 |\n| Merged |  |\n",
                "After"
            ]
        );
    }

    /// A link: its address in the document's external-object list, its range
    /// beside the text atom (`InteractiveInfo` then `TxInteractiveInfoAtom`).
    #[test]
    fn a_linked_range_reads_as_a_markdown_link() {
        let cstring = |inst: u16, t: &str| {
            rec(
                inst << 4,
                0x0FBA,
                &t.encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
        };
        let link = container(
            0x0FD7,
            0,
            &[
                rec(0, 0x0FD3, &[7, 0, 0, 0]),
                cstring(0, "friendly"),
                cstring(1, "https://example.com/deck"),
            ],
        );
        let local = container(
            0x0FD7,
            0,
            &[rec(0, 0x0FD3, &[8, 0, 0, 0]), cstring(1, "slide2")],
        );
        let doc = container(DOCUMENT, 0, &[container(0x0409, 0, &[link, local])]);
        let links = links::targets(&doc, rec_at(&doc, 0).unwrap());
        assert_eq!(links.len(), 1);
        let info = |id: u32| {
            container(
                0x0FF2,
                0,
                &[rec(
                    0,
                    0x0FF3,
                    &[[0u8; 4].to_vec(), id.to_le_bytes().to_vec(), vec![0; 8]].concat(),
                )],
            )
        };
        let range = |a: u32, b: u32| rec(0, 0x0FDF, &[a.to_le_bytes(), b.to_le_bytes()].concat());
        let tb = textbox(&[
            text_type(4),
            chars("Visit our site today."),
            rec(0, STYLE_TEXT_PROP, &[]),
            info(7),
            range(6, 14),
            info(8),
            range(15, 20),
        ]);
        let slide = container(SLIDE, 0, &[tb]);
        let texts: Vec<String> = drawing_texts(&slide, rec_at(&slide, 0).unwrap(), &links)
            .into_iter()
            .map(|(_, t)| t)
            .collect();
        assert_eq!(texts, ["Visit [our site](https://example.com/deck) today."]);
        // The same pairs follow a placeholder's text in the slide list.
        let list = container(
            SLIDE_LIST,
            0,
            &[
                persist_atom(2, 256),
                text_type(0),
                chars("Visit our site today."),
                info(7),
                range(6, 14),
            ],
        );
        let placed = slide_list(&list, rec_at(&list, 0).unwrap(), &links);
        assert_eq!(
            placed[0].2,
            [(
                0,
                "Visit [our site](https://example.com/deck) today.".to_string()
            )]
        );
    }

    /// A shape hidden from the slide show (`fHidden` with its use bit, 0x03BF)
    /// is not served; a hidden GROUP shape takes its whole group with it.
    #[test]
    fn a_hidden_shape_is_not_served() {
        let hidden_opt = rec(0x3 | 1 << 4, 0xF00B, &[0xBF, 0x03, 0x02, 0x00, 0x02, 0x00]);
        let shape = |hide: bool, t: &str| {
            let mut kids = vec![rec(0x2, 0xF00A, &[0; 8])];
            if hide {
                kids.push(hidden_opt.clone());
            }
            kids.push(container(0xF00D, 0, &[text_type(4), chars(t)]));
            container(0xF004, 0, &kids)
        };
        let group_frame = container(0xF004, 0, &[rec(0x2, 0xF00A, &[0; 8]), hidden_opt.clone()]);
        let group = container(0xF003, 0, &[group_frame, shape(false, "IN-HIDDEN-GROUP")]);
        let drawing = container(
            0xF002,
            0,
            &[shape(false, "Shown"), shape(true, "HIDDEN-BOX"), group],
        );
        let slide = container(SLIDE, 0, &[container(0x040C, 0, &[drawing])]);
        let texts: Vec<String> = drawing_texts(&slide, rec_at(&slide, 0).unwrap(), &HashMap::new())
            .into_iter()
            .map(|(_, t)| t)
            .collect();
        assert_eq!(texts, ["Shown"]);
    }

    #[test]
    fn a_ppt_deck_reads_slide_by_slide_with_notes_and_hidden_slides_counted() {
        let out = extract_ppt(&make_ppt()).expect("readable");
        assert_eq!(
            out,
            "## Slide 1 — Title one\n- Point A\n- Point B\nBox 日本\n- Item one\n  - Deeper\n\n### Notes\nNote one\n\n[1 hidden slide not shown, as the document hides it: 2]\n"
        );
        assert!(!out.contains("Secret"));
        assert_eq!(
            crate::docs::detect_format(None, &make_ppt()),
            Some(crate::docs::Format::Ppt)
        );
    }
}
