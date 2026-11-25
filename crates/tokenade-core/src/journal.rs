//! systemd-journal / dmesg compaction.
//!
//! Two related-but-different formats agents see in sysadmin debugging:
//!
//! ## `journalctl`
//! ```text
//! Jan 01 12:34:56 hostname systemd[1]: Started network.service.
//! Jan 01 12:34:57 hostname sshd[1234]: Accepted publickey for alice from 10.0.0.1
//! Jan 01 12:34:58 hostname kernel: usb 1-1: new high-speed USB device number 7
//! ```
//!
//! Signal: **service[pid]** + the message. Noise: repeated date/hostname per
//! line, redundant kernel sub-timestamps, and runs of identical (or
//! template-identical) messages.
//!
//! ## `dmesg` / kernel ring buffer
//! ```text
//! [    0.000000] Linux version 6.5.0 …
//! [   12.345678] some_module: status 0
//! [   12.345679] some_module: status 0   ← identical, just newer ts
//! ```
//!
//! Signal: the message. Noise: the bracketed wall-time offset (often
//! microsecond-distinct), repeated/replayed messages.
//!
//! [`compact`] groups by `service[pid]` (or kernel module on the dmesg path),
//! template-dedupes runs of repeated messages, surfaces error/warn lines via
//! a count summary, and drops the per-line date/hostname prefix.

use crate::logs;
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    Journalctl,
    Dmesg,
    Unknown,
}

pub fn detect(text: &str) -> Flavour {
    let head: &str = text.get(..text.len().min(4096)).unwrap_or(text);
    let lines: Vec<&str> = head.lines().take(20).collect();
    let journal_count = lines.iter().filter(|l| journal_line(l).is_some()).count();
    if journal_count >= 3 {
        return Flavour::Journalctl;
    }
    let dmesg_count = lines.iter().filter(|l| dmesg_line(l).is_some()).count();
    if dmesg_count >= 3 {
        return Flavour::Dmesg;
    }
    Flavour::Unknown
}

pub fn compact(text: &str) -> String {
    match detect(text) {
        Flavour::Journalctl => compact_journal(text),
        Flavour::Dmesg => compact_dmesg(text),
        Flavour::Unknown => text.to_string(),
    }
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

/// Try to parse `Mmm DD HH:MM:SS hostname service[pid]: message`. Returns
/// `(service_with_pid, level_marker, message)`. The hostname + date are
/// always dropped on the compacted output.
/// A message that IS an exception: `AttributeError: …`, `java.net.SocketTimeoutException: …`,
/// `TimeoutError` — the closing line of a traceback, counted once per trace.
/// Python and Java write the class name, never a level word: a journal with 67
/// tracebacks read `0 ERR, 0 WARN` and listed none of them (2026-10-03).
fn names_an_exception(msg: &str) -> bool {
    let name = msg.split([':', ' ']).next().unwrap_or("");
    let last = name.rsplit('.').next().unwrap_or("");
    (last.ends_with("Error") || last.ends_with("Exception"))
        && last.len() > 5
        && last.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '_')
        && (msg.len() == name.len() || msg[name.len()..].starts_with(':'))
}

fn journal_line(line: &str) -> Option<(String, &'static str, String)> {
    // Parsed by SHAPE, not by fixed byte offsets.
    //
    // `journalctl`'s short format prints the month name in the machine's
    // LOCALE, and the previous parser could read exactly one of them: it
    // required an ASCII uppercase first character and then indexed bytes 3 and
    // 6 for spaces, assuming a 15-character `Mmm DD HH:MM:SS` prefix. On a
    // French machine the first line reads `août 25 22:28:10 …` — lowercase, and
    // `août` is FIVE bytes for four characters, so both the gate and the
    // offsets were wrong. `detect` then returned `Unknown` and `compact`
    // returns its input unchanged, so journalctl came back whole: measured
    // 2026-08-25 on 56 KB, ratio 1.000.
    //
    // The shape is what is stable across locales: a month WORD, a day number,
    // `HH:MM:SS`, a host, then `service[pid]: message`. It also handles the
    // day-padded English form (`Aug  5 10:00:00`) for free, which the fixed
    // offsets only handled by accident of it being the same width.
    //
    // ONE declarant of that shape, `systemd::is_journal_line`: a second copy
    // lived in `tk-formats` with the English month names, and lost the whole
    // journal of a French `systemctl status`.
    if !crate::systemd::is_journal_line(line) {
        return None;
    }
    let mut it = line.split_whitespace();
    let (_month, _day, clock) = (it.next()?, it.next()?, it.next()?);
    let _host = it.next()?;
    // Everything after the host, located by finding it in the ORIGINAL line so
    // the message keeps its own spacing.
    let clock_at = line.find(clock)? + clock.len();
    let after_date = line.get(clock_at..)?.trim_start();
    let host_end = after_date.find(' ')?;
    let after_host = &after_date[host_end + 1..];
    // The rest is `service[pid]: message` (or `service: message`).
    let colon = after_host.find(": ")?;
    let svc = after_host[..colon].to_string();
    let msg = after_host[colon + 2..].to_string();
    // Heuristic level: ERROR/WARN markers inside the message body.
    let level = if msg.contains("ERROR")
        || msg.contains("error:")
        || msg.contains("CRITICAL")
        || names_an_exception(&msg)
    {
        "ERR"
    } else if msg.contains("WARN") || msg.contains("warning:") {
        "WARN"
    } else {
        ""
    };
    Some((svc, level, msg))
}

fn dmesg_line(line: &str) -> Option<&str> {
    // `[   12.345678] message`
    let t = line.trim_start();
    if !t.starts_with('[') {
        return None;
    }
    let close = t.find("] ")?;
    let inside = &t[1..close].trim();
    // Inside must be a float-shaped timestamp. The FRACTIONAL PART is what makes it
    // one: the kernel prints "%5lu.%06lu", so a real dmesg stamp always carries a
    // dot. A bare integer in brackets is how prose numbers a citation ("[1] Holt,
    // D., Eick, B."), and three of those in a bibliography used to be enough to
    // read a source file as a kernel ring buffer.
    if !inside.contains('.') {
        return None;
    }
    if !inside
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | ' '))
    {
        return None;
    }
    Some(&t[close + 2..])
}

fn compact_journal(text: &str) -> String {
    // Group: service[pid] → Vec<message>. Within each group we template-dedupe.
    let mut by_svc: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut errors: Vec<String> = Vec::new();
    let mut warns: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut unparsed = 0usize;
    let mut unmatched: Vec<String> = Vec::new();
    let mut pids: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
    // Multi-line payloads (a coredump's stack, a traceback): (service, first
    // line, the indented lines that follow it).
    let mut payloads: Vec<(String, String, Vec<String>)> = Vec::new();
    let mut last: Option<(String, String)> = None;
    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        if raw.starts_with([' ', '\t']) {
            if let Some((svc, msg)) = &last {
                match payloads.last_mut() {
                    Some(p) if p.0 == *svc && p.1 == *msg && !p.2.is_empty() => {
                        p.2.push(raw.trim().to_string())
                    }
                    _ => payloads.push((svc.clone(), msg.clone(), vec![raw.trim().to_string()])),
                }
                continue;
            }
        }
        match journal_line(raw) {
            Some((svc, level, msg)) => {
                // One group per SERVICE: keyed with its pid, a boot's 566
                // coredumps were 566 groups of one message each (measured
                // 2026-10-04, `journalctl -b -p err`). The pids are counted.
                let verbatim = svc.clone();
                let (svc, pid) = match svc.strip_suffix(']').and_then(|s| s.rsplit_once('[')) {
                    Some((name, pid)) if pid.bytes().all(|b| b.is_ascii_digit()) => {
                        (name.to_string(), pid.to_string())
                    }
                    _ => (svc, String::new()),
                };
                pids.entry(svc.clone()).or_default().insert(pid);
                last = Some((svc.clone(), msg.clone()));
                total += 1;
                if level == "ERR" {
                    errors.push(format!("{verbatim}: {msg}"));
                } else if level == "WARN" {
                    warns.push(format!("{verbatim}: {msg}"));
                }
                by_svc.entry(svc).or_default().push(msg);
            }
            None => {
                last = None;
                unparsed += 1;
                // Continuation lines (Java/Python stack traces in multi-line
                // journal payloads) — keep, don't reduce to a count.
                unmatched.push(raw.to_string());
            }
        }
    }
    let mut out = String::with_capacity(text.len() / 4);
    let _ = write!(
        out,
        "{total} entry(ies) across {} service(s), {} ERR, {} WARN",
        by_svc.len(),
        errors.len(),
        warns.len()
    );
    if unparsed > 0 {
        let _ = write!(out, ", {unparsed} unparsed");
    }
    out.push('\n');
    // Each DISTINCT entry once, with how many times it occurred, in the order
    // it first appeared. The first twenty raw entries said `TimeoutError …
    // (×11)` and `… 47 more ERR entries` for an error that happened 33 times:
    // the count was of a window, not of the journal (2026-10-03).
    list_distinct(&mut out, "ERROR", &errors, 20);
    list_distinct(&mut out, "WARN", &warns, 10);
    out.push_str("\n** per-service deduped messages **\n");
    for (svc, msgs) in &by_svc {
        let joined = msgs.join("\n");
        let deduped = logs::compact_generic_logs(&joined);
        let kept = deduped.lines().count();
        let ids: Vec<&String> = pids
            .get(svc)
            .map_or(Vec::new(), |p| p.iter().filter(|p| !p.is_empty()).collect());
        let (name, from) = match ids.as_slice() {
            [one] => (format!("{svc}[{one}]"), String::new()),
            [] => (svc.clone(), String::new()),
            many => (svc.clone(), format!(" from {} pids", many.len())),
        };
        let _ = write!(
            out,
            "\n[{name}]  ({} msg(s){from}, {} unique)\n",
            msgs.len(),
            kept
        );
        for line in deduped.lines().take(15) {
            let _ = writeln!(out, "  {line}");
        }
        if kept > 15 {
            let _ = writeln!(out, "  … {} more unique message(s)", kept - 15);
        }
    }
    render_payloads(&mut out, &payloads);
    if !unmatched.is_empty() {
        out.push_str("\n** unparsed payload lines (kept) **\n");
        out.push_str(&crate::auto::compact_remainder(&unmatched.join("\n")));
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Each distinct multi-line payload once, with how many entries carried it.
/// A stack frame's absolute address (`#4  0x000055bb9d63c459 f (bash +
/// 0x41459)`) moves with every run under ASLR and a thread id with every
/// process: both go, so the same crash reads as one; the function and its
/// offset in the module — what identifies the frame — stay.
fn render_payloads(out: &mut String, payloads: &[(String, String, Vec<String>)]) {
    if payloads.is_empty() {
        return;
    }
    let mut seen: Vec<(Vec<String>, usize, &str, &str)> = Vec::new();
    for (svc, msg, lines) in payloads {
        let norm = alike_threads_once(lines.iter().map(|l| normalise_payload_line(l)).collect());
        match seen.iter_mut().find(|(n, ..)| *n == norm) {
            Some((_, k, ..)) => *k += 1,
            None => seen.push((norm, 1, svc, msg)),
        }
    }
    let _ = writeln!(out, "\n** multi-line payloads ({} entries, {} distinct; frame addresses and thread ids dropped) **", payloads.len(), seen.len());
    for (lines, k, svc, msg) in &seen {
        let times = if *k > 1 {
            format!("  [×{k}, first:]")
        } else {
            String::new()
        };
        let _ = writeln!(out, "\n[{svc}] {msg}{times}");
        for l in lines {
            let _ = writeln!(out, "  {l}");
        }
    }
}

/// A multi-threaded dump lists every thread: a real one ran to 75 KB, seven
/// hundred frames, for ONE crash. The first stack is the thread that crashed
/// (systemd-coredump prints it first) and is kept whole; the others are
/// where they were parked, counted by their top frame (`futex_wait ×31`).
fn alike_threads_once(lines: Vec<String>) -> Vec<String> {
    const HEAD: &str = "Stack trace of thread:";
    let mut out: Vec<String> = Vec::new();
    let mut others: Vec<(String, usize)> = Vec::new();
    let mut n_threads = 0usize;
    let mut in_block = false;
    let mut post: Vec<String> = Vec::new();
    for l in lines {
        if l == HEAD {
            n_threads += 1;
            in_block = true;
            if n_threads == 1 {
                out.push(l);
            }
            continue;
        }
        if in_block && l.starts_with('#') {
            if n_threads == 1 {
                out.push(l);
            } else if let Some(rest) = l.strip_prefix("#0 ") {
                let top = rest.split(" + ").next().unwrap_or(&l).to_string();
                match others.iter_mut().find(|(t, _)| *t == top) {
                    Some((_, k)) => *k += 1,
                    None => others.push((top, 1)),
                }
            }
            continue;
        }
        in_block = false;
        if n_threads == 0 {
            out.push(l);
        } else {
            post.push(l);
        }
    }
    if n_threads > 1 {
        let census: Vec<String> = others
            .iter()
            .map(|(t, k)| {
                if *k > 1 {
                    format!("{t}) ×{k}")
                } else {
                    format!("{t})")
                }
            })
            .collect();
        out.push(format!(
            "{} other thread(s), by top frame: {}",
            n_threads - 1,
            census.join(" · ")
        ));
    }
    out.extend(post);
    out
}

fn normalise_payload_line(l: &str) -> String {
    if let Some(rest) = l.strip_prefix("Stack trace of thread ") {
        if rest
            .trim_end_matches(':')
            .bytes()
            .all(|b| b.is_ascii_digit())
        {
            return "Stack trace of thread:".to_string();
        }
    }
    let mut it = l.split_whitespace();
    if let (Some(n), Some(addr)) = (it.next(), it.next()) {
        let rest = l[n.len()..].trim_start();
        let addr_ok = addr.len() > 2
            && addr.starts_with("0x")
            && addr[2..].bytes().all(|b| b.is_ascii_hexdigit());
        if n.starts_with('#')
            && n[1..].bytes().all(|b| b.is_ascii_digit())
            && addr_ok
            && rest.starts_with(addr)
        {
            return format!("{n} {}", rest[addr.len()..].trim_start());
        }
    }
    l.to_string()
}

fn list_distinct(out: &mut String, level: &str, entries: &[String], cap: usize) {
    if entries.is_empty() {
        return;
    }
    let mut seen: Vec<(&str, usize)> = Vec::new();
    for e in entries {
        match seen.iter_mut().find(|(t, _)| *t == e.as_str()) {
            Some((_, n)) => *n += 1,
            None => seen.push((e.as_str(), 1)),
        }
    }
    let _ = writeln!(out, "\n** {level}-level entries (verbatim, each once) **");
    for (e, n) in seen.iter().take(cap) {
        if *n > 1 {
            let _ = writeln!(out, "  {e}  [×{n}]");
        } else {
            let _ = writeln!(out, "  {e}");
        }
    }
    if seen.len() > cap {
        let _ = writeln!(
            out,
            "  … {} more distinct {level} entries",
            seen.len() - cap
        );
    }
}

fn compact_dmesg(text: &str) -> String {
    // For dmesg we just template-dedupe the messages (no service grouping).
    let mut msgs = Vec::with_capacity(text.lines().count());
    let mut total = 0usize;
    let mut unparsed = 0usize;
    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        match dmesg_line(raw) {
            Some(msg) => {
                total += 1;
                msgs.push(msg.to_string());
            }
            None => unparsed += 1,
        }
    }
    let joined = msgs.join("\n");
    let deduped = logs::compact_generic_logs(&joined);
    let unique = deduped.lines().count();
    let header = format!(
        "[tokenade:journal] dmesg — {} entry(ies), {} unique (after template-dedup), {} parse-skips\n",
        total, unique, unparsed
    );
    format!("{header}{}", deduped.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_journalctl() {
        let t = "Jan 01 12:34:56 host systemd[1]: Started network.service.\n\
                 Jan 01 12:34:57 host sshd[1234]: Accepted publickey for alice\n\
                 Jan 01 12:34:58 host kernel: usb 1-1: new device\n";
        assert_eq!(detect(t), Flavour::Journalctl);
    }

    /// An exception line is an error, and each distinct error is listed once
    /// with its real count.
    #[test]
    fn exceptions_count_as_errors_listed_once_with_their_totals() {
        let mut t = String::new();
        for i in 0..40 {
            let msg = match i % 4 {
                0 => "TimeoutError: gateway did not answer",
                1 => "java.net.SocketTimeoutException: Read timed out",
                _ => "GET /api/items 200",
            };
            let _ = writeln!(t, "Jan 01 12:34:{:02} host web[42]: {msg}", i);
        }
        let out = crate::journal::compact_journal(&t);
        assert!(out.contains(", 20 ERR,"), "{out}");
        assert!(
            out.contains("web[42]: TimeoutError: gateway did not answer  [×10]"),
            "{out}"
        );
        assert!(
            out.contains("web[42]: java.net.SocketTimeoutException: Read timed out  [×10]"),
            "{out}"
        );
        assert!(
            !names_an_exception("ErrorHandler started") && !names_an_exception("No Error: fine")
        );
    }

    #[test]
    fn detects_dmesg() {
        let t = "[    0.000000] Linux version 6.5.0\n[   12.345678] some module loaded\n[   13.111111] another message\n";
        assert_eq!(detect(t), Flavour::Dmesg);
    }

    #[test]
    fn journal_groups_by_service_drops_chrome() {
        let mut t = String::new();
        for i in 0..50 {
            let _ = writeln!(
                t,
                "Jan 01 12:34:{:02} myhost systemd[1]: Started unit-{i}.service.",
                i % 60
            );
        }
        for _ in 0..20 {
            t.push_str("Jan 01 12:35:00 myhost sshd[1234]: Accepted publickey for alice\n");
        }
        t.push_str("Jan 01 12:35:01 myhost kernel: ERROR: oops at 0xdeadbeef\n");
        let out = compact(&t);
        // 50 systemd + 20 sshd + 1 kernel = 71.
        assert!(out.contains("71 entry(ies)"));
        assert!(out.contains("ERROR-level entries"));
        assert!(out.contains("oops at"));
        assert!(out.contains("[systemd[1]]"));
        assert!(out.contains("[sshd[1234]]"));
        // Hostname dropped.
        assert!(!out.contains("myhost"));
    }

    #[test]
    fn dmesg_dedups_repeated_messages() {
        let mut t = String::new();
        for i in 0..100 {
            let _ = writeln!(t, "[   12.{i:06}] some module: status 0");
        }
        t.push_str("[   13.000000] DIFFERENT message\n");
        let out = compact(&t);
        assert!(out.contains("101 entry(ies)"));
        assert!(out.contains("some module: status 0"));
        assert!(out.contains("DIFFERENT message"));
        // The 100 timestamped duplicates fold to one Option-C marker.
        // dmesg's per-line `[ N.NNNNNN]` prefix is stripped at journal
        // level before compact_generic_logs sees the lines, so the 100
        // bodies are byte-identical → "identical lines" branch.
        assert!(out.contains("[×100 identical lines]"), "got: {out}");
    }

    #[test]
    fn unknown_passes_through() {
        let t = "totally not a journal or dmesg output\njust some prose\n";
        assert_eq!(compact(t), t);
    }

    #[test]
    fn savings_real_shape() {
        let mut t = String::new();
        // 2000 journal entries across 5 services, with heavy repetition.
        for i in 0..2000 {
            let svc = match i % 5 {
                0 => "systemd[1]",
                1 => "sshd[1234]",
                2 => "nginx[5678]",
                3 => "kernel",
                _ => "dockerd[999]",
            };
            let _ = writeln!(
                t,
                "Jan 01 12:{:02}:{:02} prod-srv-01 {svc}: handling request {i}",
                (i / 60) % 60,
                i % 60
            );
        }
        let (out, sav) = compact_with_savings(&t);
        assert!(out.contains("2000 entry(ies)"));
        assert!(sav.percent() > 50.0, "got {:?}", sav);
    }
}

#[cfg(test)]
mod locale_months {
    use super::*;

    fn fr(n: usize) -> String {
        (0..n)
            .map(|i| {
                format!(
                    "août 25 22:28:{i:02} ai postgres[2175255]: WARNING:  collation mismatch {i}\n"
                )
            })
            .collect()
    }

    fn en(n: usize) -> String {
        (0..n)
            .map(|i| {
                format!(
                    "Aug 25 22:28:{i:02} ai postgres[2175255]: WARNING:  collation mismatch {i}\n"
                )
            })
            .collect()
    }

    /// `journalctl` prints the month in the machine's LOCALE. The parser read
    /// exactly one of them — ASCII uppercase, then bytes 3 and 6 indexed as
    /// spaces — so on a French machine `août 25 …` matched nothing, `detect`
    /// answered `Unknown`, and `compact` handed back its input untouched:
    /// 56 KB in, 56 KB out.
    #[test]
    fn a_french_journal_is_recognised_and_folded() {
        let t = fr(40);
        assert!(
            matches!(detect(&t), Flavour::Journalctl),
            "French journalctl unrecognised"
        );
        let out = compact(&t);
        assert!(
            out.len() < t.len() / 2,
            "not folded: {} of {}",
            out.len(),
            t.len()
        );
    }

    /// …and the English form it always handled still works, including the
    /// space-padded single-digit day the fixed offsets only fitted by accident.
    #[test]
    fn the_english_forms_still_work() {
        assert!(matches!(detect(&en(40)), Flavour::Journalctl));
        let padded: String = (0..40)
            .map(|i| format!("Aug  5 10:00:{i:02} host sshd[42]: accepted {i}\n"))
            .collect();
        assert!(
            matches!(detect(&padded), Flavour::Journalctl),
            "a space-padded day was not recognised"
        );
    }

    /// The shape must stay narrow enough not to claim ordinary prose.
    #[test]
    fn prose_is_not_a_journal() {
        let prose = "Sur la branche main\nVotre branche est en avance\nModifications a valider\n";
        assert!(matches!(detect(prose), Flavour::Unknown));
        let md = "# Titre\n\nUn paragraphe ordinaire ici.\n\n- item\n";
        assert!(matches!(detect(md), Flavour::Unknown));
    }

    /// A bibliography numbers its references `[1]`, `[2]`, `[3]` — the exact
    /// shape `dmesg_line` was testing for, because a bare integer passed as
    /// "float-shaped". Three citations in the first 20 lines were enough to read a
    /// Python test module as a kernel ring buffer: measured on sympy's
    /// `test_fp_groups.py`, 10 203 bytes came back as 250, under a banner naming its
    /// own failure ("3 entry(ies) … 217 parse-skips").
    #[test]
    fn a_bibliography_is_not_a_kernel_ring_buffer() {
        let refs = "References\n\
                    ==========\n\
                    \n\
                    [1] Holt, D., Eick, B., O'Brien, E. Handbook of Computational Group Theory\n\
                    [2] John J. Cannon; Lucien A. Dimino; George Havas; Jane M. Watson\n\
                    [3] PROC. SECOND INTERNAT. CONF. THEORY OF GROUPS, CANBERRA 1973\n";
        assert!(
            matches!(detect(refs), Flavour::Unknown),
            "citation markers are prose, not timestamps"
        );
    }

    /// The other half: the fractional part is what the kernel actually prints
    /// (`%5lu.%06lu`), so tightening onto it must leave every real ring buffer
    /// claimed. Mutation proof for the char-class condition lives here too — an
    /// alphabetic stamp is refused for a different reason than a dot-less one.
    #[test]
    fn each_dmesg_stamp_condition_refuses_something_real() {
        let real = "[    0.000000] Linux version 6.1.0\n\
                    [    0.000001] Command line: root=/dev/sda1\n\
                    [    1.234567] usb 1-1: new high-speed USB device\n\
                    [    2.345678] EXT4-fs (sda1): mounted filesystem\n";
        assert!(
            matches!(detect(real), Flavour::Dmesg),
            "real dmesg still claimed"
        );

        // Condition 1: the dot. Without it, the bibliography above is claimed.
        assert!(dmesg_line("[1] Holt, D., Eick, B.").is_none());
        assert!(dmesg_line("[    1.234567] usb 1-1: new device").is_some());

        // Condition 2: the char class. `[HH:MM] ` and `[INFO] ` carry no dot either,
        // so this one needs a stamp that HAS a dot and still is not a number.
        assert!(dmesg_line("[v1.2.3-beta] released").is_none());
        assert!(dmesg_line("[ 12.5] ok").is_some());
    }
}

#[cfg(test)]
mod english_month_tables {
    /// A hard-coded English month table in a PARSER is a locale defect waiting
    /// to happen — one was exactly that, and cost `journalctl` its fold on
    /// every non-English machine. New ones must be justified here.
    ///
    /// The list is short on purpose: the point is that adding a table forces
    /// someone to say WHY the input is locale-invariant, and to have measured
    /// it rather than assumed it.
    #[test]
    fn every_english_month_table_is_justified() {
        /// file → why an English-only month table is safe there.
        const JUSTIFIED: &[(&str, &str)] = &[(
            "gitlog.rs",
            "git does NOT localise its own date output — `git log` prints \
             `Mon Aug 25 …` whatever LC_ALL says. MEASURED 2026-08-25 by \
             running the same `git log` under en_US.utf8 and fr_FR.utf8: byte \
             for byte identical, and both fold to 0.064. Do not 'fix' this one.",
        )];

        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src];
        let mut offenders: Vec<String> = Vec::new();
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|s| s.to_str()) != Some("rs") {
                    continue;
                }
                let name = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                let Ok(t) = std::fs::read_to_string(&p) else {
                    continue;
                };
                // A month TABLE: at least three abbreviations as string
                // literals in the same file, outside of tests. Through
                // `srcsweep`: a cut at the first `#[cfg(test)]` also drops
                // every production line after a mid-file test module, and this
                // tree mounts its tests in the middle.
                let body = tk_base::srcsweep::production_lines(&t)
                    .into_iter()
                    .map(|(_, l)| l)
                    .collect::<Vec<_>>()
                    .join("\n");
                let body = body.as_str();
                let hits = [
                    "\"Jan\"", "\"Feb\"", "\"Mar\"", "\"Apr\"", "\"Jun\"", "\"Jul\"", "\"Aug\"",
                    "\"Sep\"", "\"Oct\"", "\"Nov\"", "\"Dec\"",
                ]
                .iter()
                .filter(|m| body.contains(**m))
                .count();
                if hits >= 3 && !JUSTIFIED.iter().any(|(f, _)| *f == name) {
                    offenders.push(format!("{name} ({hits} month literals)"));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "an English month table was added without saying why the input is \
             locale-invariant — measure it under LC_ALL=fr_FR.utf8 first, then \
             add it to JUSTIFIED with the number:\n  {}",
            offenders.join("\n  ")
        );
    }

    /// `journalctl -b -p err` of a boot with coredumps: one group per
    /// service, each distinct crash once, the same crash under other
    /// addresses counted, idle threads by their top frame.
    #[test]
    fn coredumps_are_grouped_by_service_and_each_crash_said_once() {
        let pad = " ".repeat(48);
        let mut t = String::new();
        for (k, base) in ["7fc011e3", "7f4ebf83", "7f3641e3"].iter().enumerate() {
            t.push_str(&format!("sept. 13 07:38:2{k} ai systemd-coredump[30847{k}]: Process 30846{k} (sh) of user 1000 dumped core.\n{pad}\n"));
            t.push_str(&format!("{pad}Stack trace of thread 30846{k}:\n{pad}#0  0x0000{base}e8fb kill (libc.so.6 + 0x3e8fb)\n{pad}#1  0x0000{base}0021 kill_builtin (bash + 0xc3021)\n"));
            if k == 2 {
                for w in 0..3 {
                    t.push_str(&format!("{pad}\n{pad}Stack trace of thread 9{w}:\n{pad}#0  0x00007fbee671e4ed syscall (libc.so.6 + 0x1e4ed)\n{pad}#1  0x00007fbee671e4ee park (app + 0x10)\n"));
                }
            }
            t.push_str(&format!(
                "{pad}ELF object binary architecture: AMD x86-64\n"
            ));
        }
        let out = crate::journal::compact_journal(&t);
        assert!(out.contains("3 entry(ies) across 1 service(s)"), "{out}");
        assert!(
            out.contains("[systemd-coredump]  (3 msg(s) from 3 pids"),
            "{out}"
        );
        assert!(out.contains("3 entries, 2 distinct"), "{out}");
        assert!(out.contains("[×2, first:]"), "{out}");
        assert!(
            out.contains("  #1 kill_builtin (bash + 0xc3021)\n"),
            "{out}"
        );
        assert!(
            out.contains("3 other thread(s), by top frame: syscall (libc.so.6) ×3"),
            "{out}"
        );
        assert!(
            !out.contains("park"),
            "only the crashed thread keeps its frames:\n{out}"
        );
        assert!(!out.contains("unparsed"), "{out}");
    }
}
