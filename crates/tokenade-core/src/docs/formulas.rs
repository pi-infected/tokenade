//! **An OpenDocument formula lives in a SUB-document.**
//!
//! The text keeps only a reference to it, `<draw:object xlink:href="./Object 1"/>`,
//! followed by a replacement image without text; the formula is in
//! `Object 1/content.xml`, as MathML. Read without it, `Energy E = mc² holds.`
//! came out `Energy  holds.` when the same document as `.docx` stated the
//! formula (2026-10-04). The `.fodt` writes it INLINE, in the `draw:object`.
//!
//! Rendering: the StarMath annotation if the document carries one — it is the form
//! the author typed, and the only linear one that keeps an exponent (`c^2`) —
//! otherwise the text of the MathML tokens. Between backticks: it is a notation,
//! not prose, and copying it as is invents nothing.

use super::markup_marks::re;

/// The text of a MathML formula, or `None` if this document carries neither
/// annotation nor token (a chart, an embedded spreadsheet).
pub(super) fn formula_text(math_xml: &str) -> Option<String> {
    let starmath = re(r#"(?s)<(?:[\w-]+:)?annotation\b[^>]*encoding="StarMath[^"]*"[^>]*>(.*?)</"#)
        .captures(math_xml)
        .map(|c| c[1].to_string());
    let text = starmath.unwrap_or_else(|| {
        re(r"(?s)<(?:[\w-]+:)?m[ino]\b[^>]*>(.*?)</")
            .captures_iter(math_xml)
            .map(|c| c[1].trim().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    });
    let text = super::markup_marks::decode_entities(text.trim());
    (!text.is_empty()).then_some(text)
}

/// The ODF body, each formula object replaced by its text. `object` gives
/// the `content.xml` of a sub-document by its path (`Object 1`).
pub(super) fn with_odf_formulas(
    content: &str,
    mut object: impl FnMut(&str) -> Option<String>,
) -> String {
    let span = |f: String| {
        format!(
            "<text:span>`{}`</text:span>",
            f.replace('&', "&amp;").replace('<', "&lt;")
        )
    };
    // Both forms: self-closing (reference only), or paired — the
    // `.fodt` writes the formula in it, LibreOffice's `.odp` a reference AND an
    // empty content.
    re(r#"(?s)<draw:object\b([^>]*?)(?:/>|>(.*?)</draw:object>)"#)
        .replace_all(content, |c: &regex::Captures| {
            let inline = c.get(2).and_then(|m| formula_text(m.as_str()));
            let linked = || {
                let name = re(r#"\bxlink:href="(?:\./)?([^"]+)""#).captures(&c[1])?[1]
                    .trim_end_matches('/')
                    .to_string();
                formula_text(&object(&name)?)
            };
            inline
                .or_else(linked)
                .map_or_else(|| c[0].to_string(), span)
        })
        .into_owned()
}

/// Word: an OMML formula (`<m:oMath>`) inline, as Word types it in linear
/// mode — `E=mc^2`, `(a+b)/2`, `√(x)`. Read as text, it lost its
/// structure: `E=mc2`, the exponent turned into one more digit (2026-10-04).
/// `ns` is the prefix of the host's text: `w` (Word), `a` (PowerPoint, where the
/// formula lives in `a14:m`).
pub(super) fn with_ooxml_formulas(doc_xml: &str, ns: &str) -> String {
    if !doc_xml.contains("<m:oMath") {
        return doc_xml.to_string();
    }
    re(r"(?s)<m:oMath\b[^>]*>(.*?)</m:oMath>")
        .replace_all(doc_xml, |c: &regex::Captures| {
            let text = omml(&parse(&c[1]));
            let text = text.trim();
            if text.is_empty() {
                return String::new();
            }
            format!(
                "<{ns}:r><{ns}:t>`{}`</{ns}:t></{ns}:r>",
                text.replace('&', "&amp;").replace('<', "&lt;")
            )
        })
        .into_owned()
}

struct El {
    name: String,
    attrs: String,
    kids: Vec<El>,
    text: String,
}

/// An element tree over an OMML fragment (no comment or CDATA). The
/// root is `stack[0]`, never popped: a closed element joins its parent.
fn parse(xml: &str) -> El {
    let new = |name: &str, attrs: &str| El {
        name: name.to_string(),
        attrs: attrs.to_string(),
        kids: vec![],
        text: String::new(),
    };
    let mut stack = vec![new("", "")];
    let close = |stack: &mut Vec<El>| {
        if stack.len() > 1 {
            if let Some(done) = stack.pop() {
                if let Some(parent) = stack.last_mut() {
                    parent.kids.push(done);
                }
            }
        }
    };
    for c in re(r#"(?s)<(/?)([\w:]+)([^>]*?)(/?)>|([^<]+)"#).captures_iter(xml) {
        match (
            c.get(5),
            c.get(1).map_or("", |m| m.as_str()),
            c.get(4).is_some_and(|m| !m.as_str().is_empty()),
        ) {
            (Some(t), _, _) => stack
                .last_mut()
                .into_iter()
                .for_each(|e| e.text.push_str(t.as_str())),
            (None, "/", _) => close(&mut stack),
            (None, _, empty) => {
                stack.push(new(&c[2], &c[3]));
                if empty {
                    close(&mut stack);
                }
            }
        }
    }
    while stack.len() > 1 {
        close(&mut stack);
    }
    stack.pop().unwrap_or_else(|| new("", ""))
}

fn omml(el: &El) -> String {
    let kid = |name: &str| el.kids.iter().find(|k| k.name == name);
    let part = |name: &str| kid(name).map(omml).unwrap_or_default();
    // The character a property declares (`<m:begChr m:val="["/>`).
    let chr = |pr: &str, name: &str, default: &str| {
        kid(pr)
            .and_then(|p| p.kids.iter().find(|k| k.name == name))
            .map_or(default.to_string(), |k| {
                re(r#"m:val="([^"]*)""#)
                    .captures(&k.attrs)
                    .map_or(String::new(), |m| m[1].to_string())
            })
    };
    let scripts = |out: &mut String| {
        for (mark, name) in [("_", "m:sub"), ("^", "m:sup")] {
            let s = part(name);
            if !s.is_empty() {
                out.push_str(mark);
                out.push_str(&group(&s));
            }
        }
    };
    match el.name.as_str() {
        "m:t" => super::markup_marks::decode_entities(&el.text),
        "m:sSup" | "m:sSub" | "m:sSubSup" => {
            let mut out = group(&part("m:e"));
            scripts(&mut out);
            out
        }
        "m:sPre" => {
            let mut out = String::new();
            scripts(&mut out);
            out + &group(&part("m:e"))
        }
        "m:f" => format!("{}/{}", group(&part("m:num")), group(&part("m:den"))),
        "m:rad" => format!("{}√{}", part("m:deg"), group(&part("m:e"))),
        "m:d" => {
            let items: Vec<String> = el
                .kids
                .iter()
                .filter(|k| k.name == "m:e")
                .map(omml)
                .collect();
            let sep = chr("m:dPr", "m:sepChr", "|");
            format!(
                "{}{}{}",
                chr("m:dPr", "m:begChr", "("),
                items.join(&sep),
                chr("m:dPr", "m:endChr", ")")
            )
        }
        "m:nary" => {
            let mut out = chr("m:naryPr", "m:chr", "∫");
            scripts(&mut out);
            format!("{out} {}", part("m:e"))
        }
        "m:func" => {
            let (name, arg) = (part("m:fName"), part("m:e"));
            if arg.starts_with('(') {
                name + &arg
            } else {
                format!("{name} {arg}")
            }
        }
        n if n.ends_with("Pr") => String::new(),
        _ => el.kids.iter().map(omml).collect(),
    }
}

/// An exponent, a numerator of more than one character goes in parentheses
/// — `c^(n+1)`, not `c^n+1`.
fn group(s: &str) -> String {
    let s = s.trim();
    let wrapped = s.starts_with('(') && s.ends_with(')');
    if s.chars().count() > 1 && !wrapped {
        format!("({s})")
    } else {
        s.to_string()
    }
}

/// RTF: Word writes the SAME OMML formula as control words —
/// `{\mmath {\*\moMath {\mr E}…{\msSup {\me {\mr c}}{\msup {\mr 2}}}}{\mmathPict …}}`.
/// The starred destination was skipped and only the replacement image
/// remained: `Energy [image] holds.` (2026-10-04). Each `\mmath`
/// group becomes the text of its formula, rendered like the `.docx` one.
pub(super) fn with_rtf_formulas(src: &str) -> String {
    let opener = super::markup_marks::re(r"\{\\mmath[^A-Za-z]");
    if !opener.is_match(src) {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(m) = opener.find(rest) {
        out.push_str(&rest[..m.start()]);
        let tail = &rest[m.start()..];
        let Some(end) = super::rtf::rtf_group_end(tail) else {
            out.push_str(tail);
            return out;
        };
        let group = &tail[..end];
        let math = group.find("{\\*\\moMath").map(|at| &group[at..]);
        let math = math.and_then(|m| Some(&m[..super::rtf::rtf_group_end(m)?]));
        let text = math
            .map(|m| omml(&parse(&rtf_math_as_xml(m))))
            .unwrap_or_default();
        let text = text.trim();
        if !text.is_empty() {
            out.push_str(&rtf_literal(&format!("`{text}`")));
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// The `{\mName …}` groups as `<m:Name>` elements, the text as `<m:t>`. A
/// property character (`{\mbegChr [}`) becomes the `m:val` attribute that the
/// OMML rendering reads. The other control words (font, bold) say nothing
/// about the formula.
fn rtf_math_as_xml(group: &str) -> String {
    let mut out = String::new();
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut chr_text: Option<String> = None;
    let mut text = String::new();
    let flush = |text: &mut String, out: &mut String, chr: &mut Option<String>| {
        if let Some(c) = chr.as_mut() {
            c.push_str(text);
        } else if !text.is_empty() {
            out.push_str(&format!(
                "<m:t>{}</m:t>",
                text.replace('&', "&amp;").replace('<', "&lt;")
            ));
        }
        text.clear();
    };
    let mut it = group.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '{' => {
                flush(&mut text, &mut out, &mut chr_text);
                if it.peek() == Some(&'\\') {
                    let mut word = String::new();
                    let mut look = it.clone();
                    look.next();
                    if look.peek() == Some(&'*') {
                        look.next();
                        if look.peek() == Some(&'\\') {
                            look.next();
                        }
                    }
                    while let Some(&l) = look.peek().filter(|l| l.is_ascii_alphabetic()) {
                        word.push(l);
                        look.next();
                    }
                    if let Some(name) = word.strip_prefix('m').filter(|n| !n.is_empty()) {
                        if look.peek() == Some(&' ') {
                            look.next();
                        }
                        it = look;
                        if name.ends_with("Chr") {
                            chr_text = Some(String::new());
                        } else {
                            out.push_str(&format!("<m:{name}>"));
                        }
                        stack.push(Some(name.to_string()));
                        continue;
                    }
                }
                stack.push(None);
            }
            '}' => {
                flush(&mut text, &mut out, &mut chr_text);
                if let Some(Some(name)) = stack.pop() {
                    match chr_text.take().filter(|_| name.ends_with("Chr")) {
                        Some(v) => out.push_str(&format!(
                            r#"<m:{name} m:val="{}"/>"#,
                            v.trim().replace('&', "&amp;").replace('"', "&quot;")
                        )),
                        None => out.push_str(&format!("</m:{name}>")),
                    }
                }
            }
            '\\' => match it.peek().copied() {
                Some(l @ ('\\' | '{' | '}')) => {
                    it.next();
                    text.push(l);
                }
                Some('\'') => {
                    it.next();
                    let hex: String = it.by_ref().take(2).collect();
                    if let Ok(b) = u8::from_str_radix(&hex, 16) {
                        text.push(char::from(b));
                    }
                }
                _ => {
                    let mut word = String::new();
                    while let Some(&l) = it.peek().filter(|l| l.is_ascii_alphabetic()) {
                        word.push(l);
                        it.next();
                    }
                    let mut num = String::new();
                    while let Some(&d) = it
                        .peek()
                        .filter(|d| d.is_ascii_digit() || (**d == '-' && num.is_empty()))
                    {
                        num.push(d);
                        it.next();
                    }
                    if it.peek() == Some(&' ') {
                        it.next();
                    }
                    if word == "u" {
                        if let Some(ch) = num
                            .parse::<i32>()
                            .ok()
                            .and_then(|n| char::from_u32(n.rem_euclid(65536) as u32))
                        {
                            text.push(ch);
                        }
                        if it.peek() == Some(&'?') {
                            it.next();
                        }
                    }
                }
            },
            '\r' | '\n' => {}
            c => text.push(c),
        }
    }
    flush(&mut text, &mut out, &mut chr_text);
    out
}

/// Text in the RTF stream: braces and backslashes escaped, and everything that
/// leaves ASCII as `\uN?` — the reader assumes no code page.
fn rtf_literal(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '\\' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_ascii() => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{}?", *u as i16));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_formula_object_reads_as_its_typed_form_and_a_chart_stays_put() {
        let math = r#"<math xmlns="http://www.w3.org/1998/Math/MathML"><semantics><mrow><mi>E</mi><mo>=</mo><mi>m</mi><msup><mi>c</mi><mn>2</mn></msup></mrow><annotation encoding="StarMath 5.0">E = m c^2</annotation></semantics></math>"#;
        let body = r#"<text:p>Energy <draw:frame><draw:object xlink:href="./Object 1" xlink:type="simple"/><draw:image xlink:href="./ObjectReplacements/Object 1"/></draw:frame> holds; <draw:frame><draw:object xlink:href="./Object 2"/></draw:frame>.</text:p>"#;
        let out = with_odf_formulas(body, |name| match name {
            "Object 1" => Some(math.to_string()),
            "Object 2" => Some("<office:document><chart:chart/></office:document>".to_string()),
            _ => None,
        });
        assert!(
            out.contains("Energy <draw:frame><text:span>`E = m c^2`</text:span><draw:image"),
            "{out}"
        );
        assert!(
            out.contains(r#"<draw:object xlink:href="./Object 2"/>"#),
            "{out}"
        );
    }

    #[test]
    fn without_an_annotation_the_tokens_are_read_and_an_inline_formula_too() {
        let body = r#"<text:p>Sum <draw:object><math:math><math:mi>a</math:mi><math:mo>&lt;</math:mo><math:mn>2</math:mn></math:math></draw:object>.</text:p>"#;
        assert_eq!(
            with_odf_formulas(body, |_| None),
            "<text:p>Sum <text:span>`a &lt; 2`</text:span>.</text:p>"
        );
    }

    #[test]
    fn an_omml_formula_keeps_its_structure() {
        let r = |t: &str| format!("<m:r><m:t>{t}</m:t></m:r>");
        let sup = format!(
            "<m:sSup><m:sSupPr><m:ctrlPr/></m:sSupPr><m:e>{}</m:e><m:sup>{}{}{}</m:sup></m:sSup>",
            r("c"),
            r("n"),
            r("+"),
            r("1")
        );
        let frac = format!(
            "<m:f><m:num>{}{}{}</m:num><m:den>{}</m:den></m:f>",
            r("a"),
            r("+"),
            r("b"),
            r("2")
        );
        let sum = format!("<m:nary><m:naryPr><m:chr m:val=\"∑\"/></m:naryPr><m:sub>{}</m:sub><m:sup>{}</m:sup><m:e>{}</m:e></m:nary>", r("i=1"), r("n"), r("x"));
        let set = format!("<m:d><m:dPr><m:begChr m:val=\"[\"/><m:endChr m:val=\"]\"/><m:sepChr m:val=\",\"/></m:dPr><m:e>{}</m:e><m:e>{}</m:e></m:d>", r("0"), r("1"));
        let xml = format!("<w:p><w:r><w:t>See </w:t></w:r><m:oMath>{}{}{sup}</m:oMath> and <m:oMath>{frac}</m:oMath> <m:oMath>{sum}</m:oMath> <m:oMath>{set}<m:rad><m:radPr><m:degHide/></m:radPr><m:deg/><m:e>{}</m:e></m:rad></m:oMath></w:p>", r("E"), r("=m"), r("x"));
        let out = with_ooxml_formulas(&xml, "w");
        assert!(out.contains("<w:t>`E=mc^(n+1)`</w:t>"), "{out}");
        assert!(out.contains("<w:t>`(a+b)/2`</w:t>"), "{out}");
        assert!(out.contains("<w:t>`∑_(i=1)^n x`</w:t>"), "{out}");
        assert!(out.contains("<w:t>`[0,1]√x`</w:t>"), "{out}");
    }

    #[test]
    fn an_rtf_formula_reads_as_its_docx_twin_and_its_picture_goes() {
        let rtf = r"{\rtf1 Energy {\mmath {\*\moMath {\mr E}{\mr =m}{\msSup {\me {\mr c}}{\msup {\mr 2}}}}{\mmathPict {\pict\pngblip 89504e47}}} holds; {\mmath {\*\moMath {\md {\mdPr {\mbegChr [}{\mendChr ]}}{\me {\mr a}}}{\mr \u8804?b}}} end.}";
        let out = with_rtf_formulas(rtf);
        assert_eq!(out, r"{\rtf1 Energy `E=mc^2` holds; `[a]\u8804?b` end.}");
        assert!(crate::docs::extract_rtf(out.as_bytes())
            .contains("Energy `E=mc^2` holds; `[a]≤b` end."));
    }
}
