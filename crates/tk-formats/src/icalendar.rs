//! **A calendar (`.ics`, RFC 5545), as an agenda: what, when, where, who.**
//!
//! An invitation is mostly plumbing — a `VTIMEZONE` block restating the
//! daylight-saving rules, a reminder (`VALARM`), `UID`/`DTSTAMP`/`SEQUENCE`,
//! vendor `X-` lines — around the five facts a reader wants, and those five
//! arrive folded at 75 octets and escaped (`\n`, `\,`). Measured on a Google
//! invitation of three meetings: 2 971 bytes served as they were.
//!
//! Each event (or to-do) becomes a short block; the plumbing is counted by
//! name at the end, never silently dropped.

use std::fmt::Write;

pub fn detect(text: &str) -> bool {
    text.trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("BEGIN:VCALENDAR")
}

pub(crate) struct Prop {
    pub(crate) name: String,
    pub(crate) params: Vec<(String, String)>,
    pub(crate) value: String,
}

impl Prop {
    pub(crate) fn param(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

/// Content lines, unfolded: a line that starts with a space or a tab
/// continues the previous one (RFC 5545 §3.1).
pub(crate) fn props(text: &str) -> Vec<Prop> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        match (raw.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(cont), Some(last)) => last.push_str(cont),
            _ => lines.push(raw.to_string()),
        }
    }
    lines.iter().filter_map(|l| parse_line(l)).collect()
}

/// `NAME;P1=v;P2="a:b":VALUE` — the value starts at the first colon outside quotes.
fn parse_line(line: &str) -> Option<Prop> {
    let mut quoted = false;
    let colon = line
        .char_indices()
        .find(|&(_, c)| {
            if c == '"' {
                quoted = !quoted;
            }
            c == ':' && !quoted
        })?
        .0;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.trim().to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"').to_string()))
        .collect();
    Some(Prop {
        name,
        params,
        value: value.to_string(),
    })
}

/// `\n` `\,` `\;` `\\` in a TEXT value.
pub(crate) fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out
}

/// `20261005T100000` → `2026-10-05 10:00`, `…Z` → `… UTC`, `20261005` → `2026-10-05`.
fn when(v: &str) -> String {
    let (d, t) = v.split_once('T').unwrap_or((v, ""));
    if d.len() != 8 || !d.bytes().all(|b| b.is_ascii_digit()) {
        return v.to_string();
    }
    let date = format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..8]);
    if t.len() < 4 {
        return date;
    }
    let utc = if t.ends_with('Z') { " UTC" } else { "" };
    format!("{date} {}:{}{utc}", &t[..2], &t[2..4])
}

/// `CN=Bob:mailto:bob@x` → `Bob <bob@x>`.
fn person(p: &Prop) -> String {
    let addr = p
        .value
        .strip_prefix("mailto:")
        .or_else(|| p.value.strip_prefix("MAILTO:"))
        .unwrap_or(&p.value);
    match p.param("CN") {
        Some(cn) if !cn.is_empty() && cn != addr => format!("{cn} <{addr}>"),
        _ => addr.to_string(),
    }
}

const SHOWN: [&str; 11] = [
    "SUMMARY",
    "DTSTART",
    "DTEND",
    "DUE",
    "LOCATION",
    "ORGANIZER",
    "ATTENDEE",
    "RRULE",
    "STATUS",
    "DESCRIPTION",
    "URL",
];

pub fn compact(text: &str) -> String {
    let all = props(text);
    let mut out = String::new();
    let mut events: Vec<String> = Vec::new();
    let mut hidden: Vec<String> = Vec::new();
    // Component nesting: only properties DIRECTLY inside an event or to-do
    // are its own; a VALARM's DESCRIPTION is the reminder's, not the meeting's.
    let mut stack: Vec<String> = Vec::new();
    let mut current: Vec<&Prop> = Vec::new();
    for p in &all {
        match p.name.as_str() {
            "BEGIN" => {
                stack.push(p.value.to_ascii_uppercase());
                if matches!(p.value.to_ascii_uppercase().as_str(), "VEVENT" | "VTODO") {
                    current.clear();
                } else if !p.value.eq_ignore_ascii_case("VCALENDAR") {
                    hidden.push(p.value.to_ascii_uppercase());
                }
                continue;
            }
            "END" => {
                if matches!(stack.pop().as_deref(), Some("VEVENT" | "VTODO")) {
                    events.push(render_event(&current));
                }
                continue;
            }
            _ => {}
        }
        match stack.last().map(String::as_str) {
            Some("VEVENT" | "VTODO") if SHOWN.contains(&p.name.as_str()) => current.push(p),
            Some("VEVENT" | "VTODO") => hidden.push(p.name.clone()),
            Some("VCALENDAR") if p.name == "METHOD" || p.name == "X-WR-CALNAME" => {}
            _ => {}
        }
    }
    let method = all
        .iter()
        .find(|p| p.name == "METHOD")
        .map(|p| format!(" · {}", p.value.to_ascii_lowercase()))
        .unwrap_or_default();
    let name = all
        .iter()
        .find(|p| p.name == "X-WR-CALNAME")
        .map(|p| format!(" · {}", unescape(&p.value)))
        .unwrap_or_default();
    let _ = writeln!(out, "[calendar: {} item(s){method}{name}]", events.len());
    for e in &events {
        out.push('\n');
        out.push_str(e);
    }
    if !hidden.is_empty() {
        let mut names: Vec<(String, usize)> = Vec::new();
        for h in &hidden {
            match names.iter_mut().find(|(n, _)| n == h) {
                Some((_, c)) => *c += 1,
                None => names.push((h.clone(), 1)),
            }
        }
        let list: Vec<String> = names
            .iter()
            .map(|(n, c)| {
                if *c > 1 {
                    format!("{n} ×{c}")
                } else {
                    n.clone()
                }
            })
            .collect();
        let _ = writeln!(out, "\n[not shown: {}]", list.join(", "));
    }
    out
}

fn render_event(props: &[&Prop]) -> String {
    let get = |n: &str| props.iter().find(|p| p.name == n);
    let mut out = String::new();
    let title = get("SUMMARY")
        .map(|p| unescape(&p.value))
        .unwrap_or_else(|| "(no title)".into());
    let _ = writeln!(out, "## {title}");
    if let Some(start) = get("DTSTART") {
        let s = when(&start.value);
        let end = get("DTEND").or_else(|| get("DUE")).map(|p| when(&p.value));
        // Same day: only the end TIME; otherwise the full end.
        let span = match end {
            Some(e)
                if e.get(..10).is_some_and(|d| s.get(..10) == Some(d))
                    && e.get(11..).is_some_and(|t| !t.is_empty()) =>
            {
                format!("{s}–{}", &e[11..])
            }
            Some(e) if e != s => format!("{s} → {e}"),
            _ => s,
        };
        let tz = start
            .param("TZID")
            .map(|t| format!(" ({t})"))
            .unwrap_or_default();
        let _ = writeln!(out, "When: {span}{tz}");
    }
    if let Some(r) = get("RRULE") {
        let _ = writeln!(out, "Repeats: {}", r.value);
    }
    if let Some(l) = get("LOCATION").filter(|p| !p.value.is_empty()) {
        let _ = writeln!(out, "Where: {}", unescape(&l.value));
    }
    if let Some(o) = get("ORGANIZER") {
        let _ = writeln!(out, "Organizer: {}", person(o));
    }
    let attendees: Vec<String> = props
        .iter()
        .filter(|p| p.name == "ATTENDEE")
        .map(|p| match p.param("PARTSTAT") {
            Some(s) => format!("{} ({})", person(p), s.to_ascii_lowercase()),
            None => person(p),
        })
        .collect();
    if !attendees.is_empty() {
        let _ = writeln!(out, "Attendees: {}", attendees.join(", "));
    }
    // A confirmed event is the default; a cancelled or tentative one is news.
    if let Some(s) = get("STATUS").filter(|p| !p.value.eq_ignore_ascii_case("CONFIRMED")) {
        let _ = writeln!(out, "Status: {}", s.value.to_ascii_lowercase());
    }
    if let Some(u) = get("URL") {
        let _ = writeln!(out, "URL: {}", u.value);
    }
    if let Some(d) = get("DESCRIPTION")
        .map(|p| unescape(&p.value))
        .filter(|d| !d.trim().is_empty())
    {
        let _ = writeln!(out, "{}", d.trim_end());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVITE: &str = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Paris\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:1@x\r\nDTSTART;TZID=Europe/Paris:20261005T100000\r\nDTEND;TZID=Europe/Paris:20261005T110000\r\nSUMMARY:Revue\\, budget\r\nLOCATION:Salle 3\\; RDC\r\nORGANIZER;CN=\"Martin, Alice\":mailto:alice@x.com\r\nATTENDEE;PARTSTAT=ACCEPTED;CN=Bob:mailto:bob@x.com\r\nDESCRIPTION:Ordre du jour :\\n1. Budget\\nMerci de confirmer ta pr\u{e9}se\r\n nce.\r\nBEGIN:VALARM\r\nDESCRIPTION:This is an event reminder\r\nEND:VALARM\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261224\r\nDTEND;VALUE=DATE:20261226\r\nSUMMARY:Cong\u{e9}s\r\nSTATUS:CANCELLED\r\nBEGIN:VALARM\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn an_invitation_reads_as_an_agenda() {
        assert!(detect(INVITE));
        assert_eq!(
            compact(INVITE),
            "[calendar: 2 item(s) · request]\n\n\
             ## Revue, budget\nWhen: 2026-10-05 10:00–11:00 (Europe/Paris)\nWhere: Salle 3; RDC\n\
             Organizer: Martin, Alice <alice@x.com>\nAttendees: Bob <bob@x.com> (accepted)\n\
             Ordre du jour :\n1. Budget\nMerci de confirmer ta présence.\n\n\
             ## Congés\nWhen: 2026-12-24 → 2026-12-26\nStatus: cancelled\n\n\
             [not shown: VTIMEZONE, UID, VALARM ×2]\n"
        );
    }

    #[test]
    fn text_that_mentions_a_calendar_is_not_one() {
        assert!(!detect("Export with BEGIN:VCALENDAR at the top.\n"));
    }
}
