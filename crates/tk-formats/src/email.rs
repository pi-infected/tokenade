//! **An e-mail (RFC 5322 / MIME), as the person it was sent to reads it.**
//!
//! Served raw, a `.eml` is mostly transport: `Received:`/`DKIM-Signature:`
//! chains, a quoted-printable body (`valid=C3=A9 =C3=A0 125 000 =E2=82=AC`),
//! the same message again as base64 HTML, and every attachment in base64 — the
//! raw file was LONGER than the message, and the one header that names it, an
//! RFC 2047 `Subject:`, even tripped the secret redactor (`=?utf-8?q?R=…`).
//!
//! Kept: the addressing headers, decoded; the text body (the HTML part only
//! when there is no text one), decoded; each attachment by name, type and
//! size. Left out, and counted: the transport headers.

use std::fmt::Write;

const KEPT_HEADERS: [&str; 7] = [
    "from",
    "to",
    "cc",
    "reply-to",
    "subject",
    "date",
    "message-id",
];

/// A message: starts with header lines, has an addressing header and a
/// header that only mail carries.
pub fn detect(text: &str) -> bool {
    if let Some(rest) = mbox_body(text) {
        return detect_message(rest);
    }
    detect_message(text)
}

/// An mbox (Thunderbird, a Google Takeout export, `/var/mail/…`): messages
/// one after another, each opened by a `From <sender> <date>` line. A
/// `git format-patch` file starts the same way, with a fixed fake date —
/// that one is a patch, and stays one.
fn mbox_body(text: &str) -> Option<&str> {
    let (first, rest) = text.split_once('\n')?;
    // `From ` with a space: the `From:` HEADER has a colon there.
    (first.starts_with("From ") && !first.contains("Mon Sep 17 00:00:00 2001")).then_some(rest)
}

fn detect_message(text: &str) -> bool {
    let head = &text[..floor_boundary(text, 8192)];
    let Some(end) = head.find("\n\n").or_else(|| head.find("\r\n\r\n")) else {
        return false;
    };
    let headers = &head[..end];
    let first = headers.lines().next().unwrap_or("");
    let name_ok = |l: &str| {
        l.split_once(':').is_some_and(|(n, _)| {
            !n.is_empty() && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    };
    let lower = headers.to_ascii_lowercase();
    name_ok(first)
        && (lower.contains("\nfrom:") || lower.starts_with("from:"))
        && (lower.contains("mime-version:")
            || lower.contains("\nreceived:")
            || lower.starts_with("received:")
            || lower.contains("\nmessage-id:"))
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

pub fn compact(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    if mbox_body(&text).is_some() {
        return compact_mbox(&text);
    }
    compact_message(&text)
}

/// Every message of an mbox, in order, each read as one message. A body line
/// that began with `From ` was written `>From ` so it would not split the
/// file; one `>` comes off again (mboxrd).
fn compact_mbox(text: &str) -> String {
    let mut messages: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    let mut prev_blank = true;
    for line in text.split_inclusive('\n') {
        if prev_blank && line.starts_with("From ") {
            messages.extend(current.take());
            current = Some(String::new());
        } else if let Some(m) = current.as_mut() {
            let quoted = line.trim_start_matches('>');
            if quoted.starts_with("From ") && quoted.len() < line.len() {
                m.push_str(&line[1..]);
            } else {
                m.push_str(line);
            }
        }
        prev_blank = line.trim().is_empty();
    }
    messages.extend(current);
    let n = messages.len();
    messages
        .iter()
        .enumerate()
        .map(|(i, m)| format!("## Message {} of {n}\n{}", i + 1, compact_message(m)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn compact_message(text: &str) -> String {
    let (headers, body) = split_headers(text);
    let mut out = String::new();
    for want in KEPT_HEADERS {
        for (name, value) in headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case(want)) {
            let _ = writeln!(out, "{name}: {}", decode_words(value));
        }
    }
    // The MIME structure headers are CONSUMED below, not dropped.
    let dropped: Vec<&str> = headers
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| {
            let l = n.to_ascii_lowercase();
            !KEPT_HEADERS.contains(&l.as_str()) && !l.starts_with("content-") && l != "mime-version"
        })
        .collect();
    let mut part = Part {
        text: None,
        html: None,
        attachments: Vec::new(),
    };
    walk(&headers, body, &mut part, 0);
    out.push('\n');
    match (part.text, part.html) {
        (Some(t), _) => out.push_str(t.trim_end()),
        (None, Some(h)) => out.push_str(html_to_text(&h).trim_end()),
        (None, None) => {}
    }
    out.push('\n');
    if !part.attachments.is_empty() {
        out.push('\n');
    }
    for (name, mime, size) in &part.attachments {
        let _ = writeln!(out, "[attachment: {name} ({mime}, {size} bytes)]");
    }
    if !dropped.is_empty() {
        // Named by what they ARE, most frequent first: a message relayed five
        // times says `Received ×5`, not a family it may not carry.
        let mut names: Vec<(String, usize)> = Vec::new();
        for n in &dropped {
            match names.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(n)) {
                Some((_, c)) => *c += 1,
                None => names.push((n.to_string(), 1)),
            }
        }
        names.sort_by_key(|b| std::cmp::Reverse(b.1));
        let shown: Vec<String> = names
            .iter()
            .take(4)
            .map(|(n, c)| {
                if *c > 1 {
                    format!("{n} ×{c}")
                } else {
                    n.clone()
                }
            })
            .collect();
        let more = if names.len() > 4 {
            format!(", +{} more", names.len() - 4)
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "[{} other header(s) not shown: {}{more}]",
            dropped.len(),
            shown.join(", ")
        );
    }
    out
}

/// An HTML body as text — what a mail reader shows of a message that has no
/// text part. Shared with the Outlook `.msg` reader.
pub fn html_to_text(html: &str) -> String {
    crate::notebook::html_as_text(html)
}

struct Part {
    text: Option<String>,
    html: Option<String>,
    attachments: Vec<(String, String, usize)>,
}

/// Header lines (unfolded) and the body after the first blank line.
fn split_headers(text: &str) -> (Vec<(String, String)>, &str) {
    let (head, body) = match text.find("\n\n") {
        Some(i) => (&text[..i], &text[i + 2..]),
        None => (text, ""),
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in head.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some((_, v)) = headers.last_mut() {
                v.push(' ');
                v.push_str(line.trim());
            }
        } else if let Some((n, v)) = line.split_once(':') {
            headers.push((n.trim().to_string(), v.trim().to_string()));
        }
    }
    (headers, body)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> &'a str {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

/// A parameter of a structured header: `boundary="…"`, `charset=utf-8`.
fn param(value: &str, key: &str) -> Option<String> {
    value.split(';').skip(1).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim()
            .eq_ignore_ascii_case(key)
            .then(|| v.trim().trim_matches('"').to_string())
    })
}

fn walk(headers: &[(String, String)], body: &str, part: &mut Part, depth: usize) {
    let ctype = header(headers, "content-type");
    let mime = ctype
        .split(';')
        .next()
        .unwrap_or("text/plain")
        .trim()
        .to_ascii_lowercase();
    let mime = if mime.is_empty() {
        "text/plain".to_string()
    } else {
        mime
    };
    if mime.starts_with("multipart/") && depth < 16 {
        if let Some(b) = param(ctype, "boundary") {
            let delim = format!("--{b}");
            for chunk in body.split(&delim).skip(1) {
                if chunk.starts_with("--") {
                    break;
                }
                let chunk = chunk.strip_prefix('\n').unwrap_or(chunk);
                let (h, b) = split_headers(chunk);
                walk(&h, b, part, depth + 1);
            }
        }
        return;
    }
    let disposition = header(headers, "content-disposition");
    let name = param(disposition, "filename")
        .or_else(|| param(ctype, "name"))
        .map(|n| decode_words(&n));
    let raw = decode_transfer(header(headers, "content-transfer-encoding"), body);
    // An invitation's MEETING is its `text/calendar` part, and a shared
    // contact is a vCard: listed as `[attachment: invite.ics (…)]`, the date,
    // the time and the room never reached the reader. Read them, like the
    // files they are.
    let structured: Option<fn(&str) -> String> = match mime.as_str() {
        "text/calendar" | "application/ics" => Some(crate::icalendar::compact),
        "text/vcard" | "text/x-vcard" | "text/directory" => Some(crate::vcard::compact),
        _ => None,
    };
    if let Some(render) = structured {
        let text = decode_charset(&raw, param(ctype, "charset").as_deref());
        let title = name.unwrap_or_else(|| mime.clone());
        append_section(part, &title, render(&text).trim_end());
        return;
    }
    let is_attachment = disposition.to_ascii_lowercase().starts_with("attachment")
        || (name.is_some() && !mime.starts_with("text/"));
    if is_attachment || !(mime == "text/plain" || mime == "text/html" || mime == "message/rfc822") {
        part.attachments
            .push((name.unwrap_or_else(|| "(unnamed)".into()), mime, raw.len()));
        return;
    }
    if mime == "message/rfc822" {
        let inner = String::from_utf8_lossy(&raw).into_owned();
        append_section(part, "forwarded message", compact(&inner).trim_end());
        return;
    }
    let text = decode_charset(&raw, param(ctype, "charset").as_deref());
    let slot = if mime == "text/html" {
        &mut part.html
    } else {
        &mut part.text
    };
    match slot {
        Some(existing) => {
            existing.push_str("\n\n");
            existing.push_str(&text);
        }
        None => *slot = Some(text),
    }
}

/// A titled section after the body, one blank line apart however the body ended.
fn append_section(part: &mut Part, title: &str, body: &str) {
    let text = part.text.get_or_insert_with(String::new);
    text.truncate(text.trim_end().len());
    if !text.is_empty() {
        text.push_str("\n\n");
    }
    let _ = write!(text, "--- {title} ---\n{body}");
}

fn decode_transfer(encoding: &str, body: &str) -> Vec<u8> {
    use base64::Engine;
    match encoding.trim().to_ascii_lowercase().as_str() {
        "base64" => {
            let clean: String = body.chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD
                .decode(clean.as_bytes())
                .unwrap_or_else(|_| body.as_bytes().to_vec())
        }
        "quoted-printable" => quoted_printable(body, false),
        _ => body.as_bytes().to_vec(),
    }
}

/// `=XX` is a byte, `=` at a line end is a soft break; in an encoded word
/// (`q` = true) `_` is a space.
fn quoted_printable(s: &str, q: bool) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'=' if b.get(i + 1) == Some(&b'\n') => i += 2,
            b'=' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
                {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'=');
                        i += 1;
                    }
                }
            }
            b'_' if q => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn decode_charset(raw: &[u8], charset: Option<&str>) -> String {
    let enc = charset
        .and_then(|c| encoding_rs::Encoding::for_label(c.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    enc.decode_without_bom_handling(raw).0.into_owned()
}

/// RFC 2047 encoded words: `=?utf-8?q?R=C3=A9union?=`, `=?UTF-8?B?…?=`.
/// Whitespace BETWEEN two encoded words is not text.
fn decode_words(value: &str) -> String {
    use base64::Engine;
    let re = tk_base::regexes::re(r"=\?([^?]+)\?([bBqQ])\?([^?]*)\?=");
    let mut out = String::new();
    let mut last = 0;
    let mut prev_was_word = false;
    for c in re.captures_iter(value) {
        let m = c.get(0).map_or(0..0, |m| m.range());
        let gap = &value[last..m.start];
        if !(prev_was_word && gap.trim().is_empty()) {
            out.push_str(gap);
        }
        let raw = if c[2].eq_ignore_ascii_case("b") {
            base64::engine::general_purpose::STANDARD
                .decode(c[3].as_bytes())
                .unwrap_or_default()
        } else {
            quoted_printable(&c[3], true)
        };
        out.push_str(&decode_charset(&raw, Some(&c[1])));
        last = m.end;
        prev_was_word = true;
    }
    out.push_str(&value[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELAYED: &str = "Received: from mx1 by mx2\r\nReceived: from a by mx1\r\nReceived: from b by a\r\nDKIM-Signature: v=1; a=rsa-sha256; b=AAAA\r\nFrom: Alice <alice@example.com>\r\nTo: bob@example.com\r\nSubject: =?utf-8?q?R=C3=A9union?= =?UTF-8?B?IGJ1ZGdldA==?= Q3\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"OUT\"\r\n\r\n--OUT\r\nContent-Type: multipart/alternative; boundary=\"ALT\"\r\n\r\n--ALT\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nLe budget est valid=C3=A9 =C3=A0 125 000 =E2=82=AC, merci de confir=\r\nmer.\r\n--ALT\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\nPGI+SFRNTCBDT1BZPC9iPg==\r\n--ALT--\r\n--OUT\r\nContent-Type: application/pdf; name=\"budget.pdf\"\r\nContent-Disposition: attachment; filename=\"budget.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQgZmFrZQ==\r\n--OUT--\r\n";

    #[test]
    fn a_relayed_message_reads_as_its_recipient_reads_it() {
        assert!(detect(RELAYED));
        let out = compact(RELAYED);
        assert_eq!(
            out,
            "From: Alice <alice@example.com>\nTo: bob@example.com\nSubject: Réunion budget Q3\n\n\
             Le budget est validé à 125 000 €, merci de confirmer.\n\n\
             [attachment: budget.pdf (application/pdf, 13 bytes)]\n\
             [4 other header(s) not shown: Received ×3, DKIM-Signature]\n"
        );
    }

    #[test]
    fn an_html_only_message_is_read_from_its_html() {
        let m = "From: a@b.c\nMessage-ID: <1@b.c>\nContent-Type: text/html; charset=iso-8859-1\nContent-Transfer-Encoding: quoted-printable\n\n<p>Caf=E9 ouvert</p>\n";
        assert!(detect(m));
        assert!(compact(m).contains("Café ouvert"), "{}", compact(m));
    }

    #[test]
    fn a_patch_or_an_http_response_is_not_mail() {
        // `git format-patch` adds MIME-Version as soon as a name is not ASCII.
        assert!(!detect("From 3f2a Mon Sep 17 00:00:00 2001\nFrom: Andr\u{e9} <a@b.c>\nDate: x\nSubject: [PATCH] fix\nMIME-Version: 1.0\n\n---\n"));
        assert!(!detect(
            "HTTP/1.1 200 OK\nContent-Type: text/html\nFrom: x\n\nbody"
        ));
    }

    #[test]
    fn an_mbox_reads_message_by_message() {
        let mbox = "From alice@example.com Fri Oct  3 10:00:00 2026\nFrom: Alice <alice@example.com>\nMessage-ID: <1@x>\nReceived: from a by b\nSubject: Un\n\nPremier.\n>From the start, it was clear.\n\nFrom bob@example.com Fri Oct  3 11:00:00 2026\nFrom: Bob <bob@example.com>\nMessage-ID: <2@x>\nSubject: =?utf-8?q?D=C3=A9ux?=\nContent-Transfer-Encoding: quoted-printable\n\nSecond =C3=A9t=C3=A9.\n";
        assert!(detect(mbox));
        assert_eq!(
            compact(mbox),
            "## Message 1 of 2\nFrom: Alice <alice@example.com>\nSubject: Un\nMessage-ID: <1@x>\n\nPremier.\nFrom the start, it was clear.\n[1 other header(s) not shown: Received]\n\n\
             ## Message 2 of 2\nFrom: Bob <bob@example.com>\nSubject: Déux\nMessage-ID: <2@x>\n\nSecond été.\n"
        );
    }

    /// The meeting of an invitation is its calendar part: read, not listed.
    #[test]
    fn an_invitation_shows_its_meeting() {
        let m = "From: a@b.c\nMessage-ID: <1@b.c>\nContent-Type: multipart/mixed; boundary=B\n\n--B\nContent-Type: text/plain\n\nVous etes invite.\n--B\nContent-Type: text/calendar; method=REQUEST\nContent-Disposition: attachment; filename=invite.ics\nContent-Transfer-Encoding: base64\n\nQkVHSU46VkNBTEVOREFSDQpCRUdJTjpWRVZFTlQNClNVTU1BUlk6UmV2dWUNCkRUU1RBUlQ6MjAyNjEwMDVUMTAwMDAwWg0KRU5EOlZFVkVOVA0KRU5EOlZDQUxFTkRBUg0K\n--B--\n";
        let out = compact(m);
        assert!(
            out.contains(
                "--- invite.ics ---\n[calendar: 1 item(s)]\n\n## Revue\nWhen: 2026-10-05 10:00 UTC"
            ),
            "{out}"
        );
        assert!(!out.contains("[attachment: invite.ics"), "{out}");
    }
}
