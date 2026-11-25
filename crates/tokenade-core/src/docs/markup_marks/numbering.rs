//! **A list item's number is the one the document DISPLAYS.**
//!
//! The marker of a numbered item was always `N.`, counted from 1: a
//! list `a) b)` came out `1. 2.`, a roman list `1. 2. 3.`, a list that
//! starts at 5 came out `1.` — a number the document's reader does not
//! see (renumbering is fabrication). The `.rtf` of the same document
//! got it right, because it carries the label itself (2026-10-04). Here, the
//! format of each level is read where the document declares it: ODF
//! `style:num-format` / `num-prefix` / `num-suffix` / `text:start-value`,
//! Word `w:numFmt` / `w:lvlText` / `w:start`.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(in crate::docs) enum NumKind {
    #[default]
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::docs) struct NumberStyle {
    pub kind: NumKind,
    pub prefix: String,
    pub suffix: String,
    /// The level's first number.
    pub start: usize,
    /// Word: the abstract list whose counter is shared across `numId`s.
    pub list: Option<String>,
    /// Word: this start comes from a `startOverride` and resets the counter.
    pub restarts: bool,
}

impl Default for NumberStyle {
    fn default() -> Self {
        NumberStyle {
            kind: NumKind::Decimal,
            prefix: String::new(),
            suffix: ".".into(),
            start: 1,
            list: None,
            restarts: false,
        }
    }
}

/// `list_key` → level style.
pub(in crate::docs) type NumberStyles = std::collections::HashMap<String, NumberStyle>;

/// The marker of item `n` (already counted from `start`), space included.
pub(in crate::docs) fn label(style: Option<&NumberStyle>, n: usize) -> String {
    let default = NumberStyle::default();
    let s = style.unwrap_or(&default);
    let body = match s.kind {
        NumKind::Decimal => n.to_string(),
        NumKind::LowerAlpha => alpha(n),
        NumKind::UpperAlpha => alpha(n).to_uppercase(),
        NumKind::LowerRoman => roman(n),
        NumKind::UpperRoman => roman(n).to_uppercase(),
    };
    // Without suffix or prefix, a bare number would not make a markdown list.
    let suffix = if s.prefix.is_empty() && s.suffix.is_empty() {
        "."
    } else {
        s.suffix.as_str()
    };
    format!("{}{body}{suffix} ", s.prefix)
}

/// a … z, aa … zz (Word and LibreOffice double the letter).
fn alpha(n: usize) -> String {
    let n = n.max(1);
    let letter = (b'a' + ((n - 1) % 26) as u8) as char;
    std::iter::repeat_n(letter, (n - 1) / 26 + 1).collect()
}

fn roman(mut n: usize) -> String {
    const UNITS: [(usize, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (v, s) in UNITS {
        while n >= v {
            out.push_str(s);
            n -= v;
        }
    }
    out
}

/// OpenDocument's numbered styles, by `list_key(style, level - 1)`.
pub(in crate::docs) fn odf_number_styles(xml: &str) -> NumberStyles {
    let mut out = NumberStyles::new();
    for c in re(r"(?s)<text:list-style\b[^>]*\bstyle:name=\x22([^\x22]*)\x22.*?</text:list-style>")
        .captures_iter(xml)
    {
        for l in re(r"<text:list-level-style-number\b[^>]*>").find_iter(&c[0]) {
            let attr = |name: &str| {
                re(&format!(r#"\b{name}="([^"]*)""#))
                    .captures(l.as_str())
                    .map(|m| m[1].to_string())
            };
            let Some(level) = attr("text:level").and_then(|v| v.parse::<usize>().ok()) else {
                continue;
            };
            let kind = match attr("style:num-format").as_deref() {
                Some("a") => NumKind::LowerAlpha,
                Some("A") => NumKind::UpperAlpha,
                Some("i") => NumKind::LowerRoman,
                Some("I") => NumKind::UpperRoman,
                _ => NumKind::Decimal,
            };
            let start = attr("text:start-value")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            let style = NumberStyle {
                kind,
                prefix: attr("style:num-prefix").unwrap_or_default(),
                suffix: attr("style:num-suffix").unwrap_or_default(),
                start,
                ..Default::default()
            };
            out.insert(list_key(&c[1], level.saturating_sub(1)), style);
        }
    }
    out
}

/// Word's numbered levels, by `list_key(numId, ilvl)`.
pub(in crate::docs) fn ooxml_number_styles(numbering_xml: &str) -> NumberStyles {
    ooxml_level_details(numbering_xml)
        .into_iter()
        .map(|(key, (fmt, text, start, list, restarts))| {
            let kind = match fmt.as_str() {
                "lowerLetter" => NumKind::LowerAlpha,
                "upperLetter" => NumKind::UpperAlpha,
                "lowerRoman" => NumKind::LowerRoman,
                "upperRoman" => NumKind::UpperRoman,
                _ => NumKind::Decimal,
            };
            // `w:lvlText="%1)"`: what surrounds the number of THIS level. A
            // multi-level text (`%1.%2.`) keeps the default format.
            let text = text.unwrap_or_default();
            let (prefix, suffix) = match text.matches('%').count() {
                1 => text
                    .split_once('%')
                    .map_or((String::new(), ".".into()), |(p, rest)| {
                        (p.to_string(), rest.get(1..).unwrap_or("").to_string())
                    }),
                _ => (String::new(), ".".into()),
            };
            (
                key,
                NumberStyle {
                    kind,
                    prefix,
                    suffix,
                    start: start.unwrap_or(1),
                    list: Some(list),
                    restarts,
                },
            )
        })
        .collect()
}

/// HTML (an EPUB chapter): `<ol type="a" start="5">`, or the
/// `list-style-type` of its inline style. The suffix is the one a
/// browser draws: a period.
pub(in crate::docs) fn html_list_style(tag: &str) -> NumberStyle {
    let css = re(r#"(?i)list-style-type\s*:\s*([a-z-]+)"#)
        .captures(tag)
        .map(|m| m[1].to_ascii_lowercase());
    let kind = match (
        re(r#"\btype\s*=\s*["']?([aAiI1])"#)
            .captures(tag)
            .map(|m| m[1].to_string())
            .as_deref(),
        css.as_deref(),
    ) {
        (Some("a"), _) | (_, Some("lower-alpha" | "lower-latin")) => NumKind::LowerAlpha,
        (Some("A"), _) | (_, Some("upper-alpha" | "upper-latin")) => NumKind::UpperAlpha,
        (Some("i"), _) | (_, Some("lower-roman")) => NumKind::LowerRoman,
        (Some("I"), _) | (_, Some("upper-roman")) => NumKind::UpperRoman,
        _ => NumKind::Decimal,
    };
    let start = re(r#"(?i)\bstart\s*=\s*["']?(\d+)"#)
        .captures(tag)
        .and_then(|m| m[1].parse().ok())
        .unwrap_or(1);
    NumberStyle {
        kind,
        start,
        ..Default::default()
    }
}

/// PowerPoint (DrawingML) : `<a:buAutoNum type="alphaLcParenR" startAt="3"/>`.
pub(in crate::docs) fn drawingml_auto_num(tag: &str) -> NumberStyle {
    let ty = re(r#"\btype="([A-Za-z]+)""#)
        .captures(tag)
        .map(|m| m[1].to_string())
        .unwrap_or_default();
    let kind = if ty.starts_with("alphaLc") {
        NumKind::LowerAlpha
    } else if ty.starts_with("alphaUc") {
        NumKind::UpperAlpha
    } else if ty.starts_with("romanLc") {
        NumKind::LowerRoman
    } else if ty.starts_with("romanUc") {
        NumKind::UpperRoman
    } else {
        NumKind::Decimal
    };
    let (prefix, suffix) = if ty.ends_with("ParenBoth") {
        ("(", ")")
    } else if ty.ends_with("ParenR") {
        ("", ")")
    } else {
        ("", ".")
    };
    let start = re(r#"\bstartAt="(\d+)""#)
        .captures(tag)
        .and_then(|m| m[1].parse().ok())
        .unwrap_or(1);
    NumberStyle {
        kind,
        prefix: prefix.into(),
        suffix: suffix.into(),
        start,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_is_what_the_document_shows() {
        let st = |kind, prefix: &str, suffix: &str| NumberStyle {
            kind,
            prefix: prefix.into(),
            suffix: suffix.into(),
            ..Default::default()
        };
        assert_eq!(label(None, 3), "3. ");
        assert_eq!(label(Some(&st(NumKind::LowerAlpha, "", ")")), 2), "b) ");
        assert_eq!(label(Some(&st(NumKind::UpperAlpha, "(", ")")), 27), "(AA) ");
        assert_eq!(label(Some(&st(NumKind::LowerRoman, "", ".")), 14), "xiv. ");
        assert_eq!(label(Some(&st(NumKind::UpperRoman, "", "")), 4), "IV. ");
    }

    #[test]
    fn styles_are_read_from_both_schemas() {
        let odf = r#"<text:list-style style:name="L1"><text:list-level-style-number text:level="1" style:num-format="a" style:num-suffix=")"/><text:list-level-style-number text:level="2" style:num-format="I" style:num-suffix="." text:start-value="3"/></text:list-style>"#;
        let o = odf_number_styles(odf);
        assert_eq!(o[&list_key("L1", 0)].kind, NumKind::LowerAlpha);
        assert_eq!(
            (o[&list_key("L1", 1)].kind, o[&list_key("L1", 1)].start),
            (NumKind::UpperRoman, 3)
        );
        let w = r#"<w:numbering><w:abstractNum w:abstractNumId="4"><w:lvl w:ilvl="0"><w:start w:val="5"/><w:numFmt w:val="lowerRoman"/><w:lvlText w:val="(%1)"/></w:lvl></w:abstractNum><w:num w:numId="2"><w:abstractNumId w:val="4"/></w:num></w:numbering>"#;
        let s = &ooxml_number_styles(w)[&list_key("2", 0)];
        assert_eq!(
            (s.kind, s.prefix.as_str(), s.suffix.as_str(), s.start),
            (NumKind::LowerRoman, "(", ")", 5)
        );
    }

    #[test]
    fn html_and_drawingml_declare_their_own_formats() {
        let h = html_list_style(r#"<ol type="I" start="4">"#);
        assert_eq!((h.kind, h.start), (NumKind::UpperRoman, 4));
        assert_eq!(
            html_list_style(r#"<ol style="list-style-type: lower-latin">"#).kind,
            NumKind::LowerAlpha
        );
        let d = drawingml_auto_num(r#"<a:buAutoNum type="alphaLcParenR" startAt="3"/>"#);
        assert_eq!(label(Some(&d), 3), "c) ");
        assert_eq!(
            label(
                Some(&drawingml_auto_num(
                    r#"<a:buAutoNum type="romanUcParenBoth"/>"#
                )),
                2
            ),
            "(II) "
        );
    }
}
