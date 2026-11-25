//! **An Outlook message (`.msg`) → the same rendering as an `.eml`.**
//!
//! A `.msg` is an OLE2 container ([MS-OXMSG]): each MAPI property is a
//! stream `__substg1.0_PPPPTTTT` (identifier, then type: `001F` UTF-16, `001E`
//! bytes, `0102` binary), the fixed-size properties — including the send
//! date — are stored in `__properties_version1.0`, and each
//! attachment is a sub-storage `__attach_version1.0_#NNNNNNNN`.
//!
//! Rendered like the e-mail reader (`tk_formats::email`): addressing
//! headers, text body (HTML only as a fallback), attachments
//! named with their size.

use std::fmt::Write;
use std::io::{Cursor, Read};

type Msg<'a> = cfb::CompoundFile<Cursor<&'a [u8]>>;

fn stream(cf: &mut Msg, path: &str) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    cf.open_stream(path).ok()?.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// A text property, in UTF-16 (`001F`) or in bytes (`001E`).
fn text_prop(cf: &mut Msg, dir: &str, id: u16) -> Option<String> {
    if let Some(b) = stream(cf, &format!("{dir}/__substg1.0_{id:04X}001F")) {
        let units = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]));
        let s: String = char::decode_utf16(units)
            .map(|c| c.unwrap_or('\u{FFFD}'))
            .collect();
        return Some(s.trim_end_matches('\0').to_string());
    }
    let b = stream(cf, &format!("{dir}/__substg1.0_{id:04X}001E"))?;
    let s = encoding_rs::WINDOWS_1252.decode_without_bom_handling(&b).0;
    Some(s.trim_end_matches('\0').to_string())
}

/// The send date (`PidTagClientSubmitTime`, 0x0039, a FILETIME) in the
/// fixed-properties stream: 32 header bytes for the message, then
/// 16-byte entries (tag, flags, value).
fn submit_time(cf: &mut Msg) -> Option<String> {
    let props = stream(cf, "/__properties_version1.0")?;
    props.get(32..)?.chunks_exact(16).find_map(|e| {
        let tag = u32::from_le_bytes(e[0..4].try_into().ok()?);
        if tag != 0x0039_0040 {
            return None;
        }
        let ft = u64::from_le_bytes(e[8..16].try_into().ok()?);
        let secs = (ft / 10_000_000).checked_sub(11_644_473_600)?;
        Some(tk_base::platform::epoch_to_iso8601_z(secs))
    })
}

/// The text of a `.msg`, or `None` if it is not one.
pub(crate) fn extract_msg(bytes: &[u8]) -> Option<String> {
    let mut cf = cfb::CompoundFile::open(Cursor::new(bytes)).ok()?;
    if !cf.is_stream("/__properties_version1.0") {
        return None;
    }
    let mut out = String::new();
    let sender = text_prop(&mut cf, "", 0x0C1A);
    let address = text_prop(&mut cf, "", 0x0C1F).or_else(|| text_prop(&mut cf, "", 0x5D01));
    match (sender, address) {
        (Some(n), Some(a)) if !a.is_empty() && n != a => {
            let _ = writeln!(out, "From: {n} <{a}>");
        }
        (Some(n), _) | (None, Some(n)) if !n.is_empty() => {
            let _ = writeln!(out, "From: {n}");
        }
        _ => {}
    }
    for (label, id) in [("To", 0x0E04), ("Cc", 0x0E03), ("Subject", 0x0037)] {
        if let Some(v) = text_prop(&mut cf, "", id).filter(|v| !v.is_empty()) {
            let _ = writeln!(out, "{label}: {v}");
        }
    }
    if let Some(d) = submit_time(&mut cf) {
        let _ = writeln!(out, "Date: {d}");
    }
    out.push('\n');
    let body = text_prop(&mut cf, "", 0x1000).filter(|b| !b.trim().is_empty());
    match body {
        Some(b) => out.push_str(b.replace("\r\n", "\n").trim_end()),
        None => {
            if let Some(html) = stream(&mut cf, "/__substg1.0_10130102") {
                let html = String::from_utf8_lossy(&html);
                out.push_str(tk_formats::email::html_to_text(&html).trim_end());
            }
        }
    }
    out.push('\n');

    let attach_dirs: Vec<String> = cf
        .read_root_storage()
        .filter(|e| e.is_storage() && e.name().starts_with("__attach_version1.0_#"))
        .map(|e| format!("/{}", e.name()))
        .collect();
    if !attach_dirs.is_empty() {
        out.push('\n');
    }
    for dir in attach_dirs {
        let name = text_prop(&mut cf, &dir, 0x3707)
            .or_else(|| text_prop(&mut cf, &dir, 0x3704))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "(unnamed)".into());
        let mime = text_prop(&mut cf, &dir, 0x370E)
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "application/octet-stream".into());
        let size = stream(&mut cf, &format!("{dir}/__substg1.0_37010102")).map_or(0, |d| d.len());
        let _ = writeln!(out, "[attachment: {name} ({mime}, {size} bytes)]");
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    fn make_msg(body: Option<&str>, html: Option<&str>) -> Vec<u8> {
        let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        let put = |cf: &mut cfb::CompoundFile<Cursor<Vec<u8>>>, path: &str, data: &[u8]| {
            cf.create_stream(path).unwrap().write_all(data).unwrap();
        };
        let mut props = vec![0u8; 32];
        props.extend(0x0039_0040u32.to_le_bytes());
        props.extend(0u32.to_le_bytes());
        props.extend(134_354_880_000_000_000u64.to_le_bytes()); // 2026-10-03T08:00:00Z
        put(&mut cf, "/__properties_version1.0", &props);
        put(&mut cf, "/__substg1.0_0C1A001F", &utf16("Alice Martin"));
        put(
            &mut cf,
            "/__substg1.0_0C1F001F",
            &utf16("alice@example.com"),
        );
        put(&mut cf, "/__substg1.0_0E04001F", &utf16("Bob"));
        put(
            &mut cf,
            "/__substg1.0_0037001F",
            &utf16("R\u{e9}union \u{65e5}\u{672c}"),
        );
        if let Some(b) = body {
            put(&mut cf, "/__substg1.0_1000001F", &utf16(b));
        }
        if let Some(h) = html {
            put(&mut cf, "/__substg1.0_10130102", h.as_bytes());
        }
        cf.create_storage("/__attach_version1.0_#00000000").unwrap();
        put(
            &mut cf,
            "/__attach_version1.0_#00000000/__substg1.0_3707001F",
            &utf16("budget.pdf"),
        );
        put(
            &mut cf,
            "/__attach_version1.0_#00000000/__substg1.0_370E001F",
            &utf16("application/pdf"),
        );
        put(
            &mut cf,
            "/__attach_version1.0_#00000000/__substg1.0_37010102",
            b"%PDF-1.4 fake",
        );
        cf.flush().unwrap();
        cf.into_inner().into_inner()
    }

    #[test]
    fn an_outlook_message_reads_like_an_eml() {
        let msg = make_msg(Some("Bonjour Bob,\r\nLe budget est valid\u{e9}.\r\n"), None);
        assert_eq!(
            crate::docs::detect_format(None, &msg),
            Some(crate::docs::Format::Msg)
        );
        assert_eq!(
            extract_msg(&msg).unwrap(),
            "From: Alice Martin <alice@example.com>\nTo: Bob\nSubject: Réunion 日本\nDate: 2026-10-03T08:00:00Z\n\n\
             Bonjour Bob,\nLe budget est validé.\n\n[attachment: budget.pdf (application/pdf, 13 bytes)]\n"
        );
    }

    #[test]
    fn a_message_with_only_an_html_body_is_read_from_it() {
        let msg = make_msg(
            None,
            Some("<html><body><p>Caf\u{e9} <b>ouvert</b></p></body></html>"),
        );
        let out = extract_msg(&msg).unwrap();
        assert!(out.contains("Café ouvert"), "{out}");
    }
}
