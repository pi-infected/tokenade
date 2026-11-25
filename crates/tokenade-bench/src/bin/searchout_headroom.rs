//! What remains in a search output AFTER `searchout::compact`.
//!
//! A COSTING, not a fix — in the spirit of `web_headroom` and
//! `padding_headroom`: name what we would gain, per axis, BEFORE writing a
//! line of compactor. The `auto:grep` row of the ranking returns −41 % for
//! 636 bytes per call, and the ledger's worst event folds 41,702 bytes
//! into 24,816. The question is: what are those bytes made of?
//!
//! Three axes, measured separately on the ALREADY folded output:
//!
//! - **duplicates** — two matches whose text is identical. A grep
//!   on a symbol returns the same line in thirty files.
//! - **prefix** — the path common to all listed files, rewritten whole
//!   on every header.
//! - **long lines** — what exceeds a per-line cap. The compactor
//!   has NONE, so a minified line goes in whole.
//!
//! The three partly overlap (a long line can be a duplicate);
//! the report gives them separately and does not add them up.
//!
//! `cargo run -q -p tokenade-bench --bin searchout_headroom`

use std::collections::BTreeMap;
use tokenade_bench::{captures, Capture};

/// The per-line cap we COST. Not a decision: the report says what it
/// would return, and nothing else.
const LINE_CAP: usize = 120;

#[derive(Default)]
struct Headroom {
    folded: usize,
    duplicates: usize,
    prefix: usize,
    long_lines: usize,
    /// The heaviest repetitions, so that we SEE what we propose to
    /// fold. A headroom number that cannot be re-read is an opinion.
    worst: Vec<(usize, usize, String)>,
}

/// The content of the match lines of the folded output, in the
/// `  <n>: <content>` format `searchout::compact` writes.
fn match_contents(folded: &str) -> Vec<&str> {
    folded
        .lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("  ")?;
            let (num, content) = rest.split_once(": ")?;
            num.chars().all(|c| c.is_ascii_digit()).then_some(content)
        })
        .collect()
}

/// The paths of the file headers: `<path> (<n>):`.
fn file_headers(folded: &str) -> Vec<&str> {
    folded
        .lines()
        .filter_map(|l| {
            if l.starts_with(' ') {
                return None;
            }
            let path = l.strip_suffix("):")?;
            let (path, n) = path.rsplit_once(" (")?;
            n.chars().all(|c| c.is_ascii_digit()).then_some(path)
        })
        .collect()
}

/// The longest common prefix, cut at the last `/` — a prefix that stops
/// in the middle of a file name cannot be factored out.
fn common_prefix<'a>(paths: &[&'a str]) -> &'a str {
    let Some(first) = paths.first() else {
        return "";
    };
    let mut end = first.len();
    for p in &paths[1..] {
        let common = first
            .as_bytes()
            .iter()
            .zip(p.as_bytes())
            .take_while(|(a, b)| a == b)
            .count();
        end = end.min(common);
    }
    match first[..end].rfind('/') {
        Some(i) => &first[..=i],
        None => "",
    }
}

fn headroom_of(body: &str) -> Option<Headroom> {
    if !tokenade_core::searchout::detect(body) {
        return None;
    }
    let folded = tokenade_core::searchout::compact(body);
    let contents = match_contents(&folded);
    if contents.is_empty() {
        return None;
    }

    // Duplicates: what keeping ONE copy and counting the others would return.
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for c in &contents {
        *seen.entry(c).or_default() += 1;
    }
    // What a fold WOULD DELIVER, not what the repetition WEIGHS. Keeping the
    // first occurrence with a `[k]` label attached, and returning the
    // following ones as `[k]`, costs the label twice — so a short line
    // repeated twice saves nothing. Counting `len * (n-1)` would have
    // announced a headroom the fold cannot hold: an opportunity
    // once measured 8 out of 8 returned 0 once delivered.
    let mut duplicates = 0usize;
    let mut tag = 0usize;
    for (c, n) in seen.iter().filter(|(_, n)| **n > 1) {
        tag += 1;
        let label = format!("[{tag}]").len();
        let before = c.len() * n;
        let after = c.len() + label + label * (n - 1);
        duplicates += before.saturating_sub(after);
    }

    let headers = file_headers(&folded);
    let prefix = common_prefix(&headers);
    let prefix = if headers.len() > 1 {
        prefix.len() * (headers.len() - 1)
    } else {
        0
    };

    let long_lines: usize = contents
        .iter()
        .map(|c| c.len().saturating_sub(LINE_CAP))
        .sum();

    let mut worst: Vec<(usize, usize, String)> = seen
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(c, n)| (c.len() * (n - 1), *n, (*c).to_string()))
        .collect();
    worst.sort_by_key(|b| std::cmp::Reverse(b.0));
    worst.truncate(5);
    Some(Headroom {
        folded: folded.len(),
        duplicates,
        prefix,
        long_lines,
        worst,
    })
}

fn pct(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        100.0 * part as f64 / whole as f64
    }
}

fn main() {
    tokenade_bench::reject_unknown_args(&[], &std::env::args().skip(1).collect::<Vec<_>>());
    let mut rows: Vec<(String, Headroom)> = Vec::new();
    for Capture { key, body, .. } in captures() {
        if let Some(h) = headroom_of(&body) {
            rows.push((key, h));
        }
    }
    if rows.is_empty() {
        eprintln!(
            "searchout_headroom: NO capture in the corpus is a search output.\n\
             \x20 The instrument measured nothing — this is not \"no headroom\"."
        );
        std::process::exit(2);
    }
    rows.sort_by(|a, b| {
        (b.1.duplicates + b.1.prefix + b.1.long_lines)
            .cmp(&(a.1.duplicates + a.1.prefix + a.1.long_lines))
    });

    let tot: usize = rows.iter().map(|r| r.1.folded).sum();
    let dup: usize = rows.iter().map(|r| r.1.duplicates).sum();
    let pre: usize = rows.iter().map(|r| r.1.prefix).sum();
    let lng: usize = rows.iter().map(|r| r.1.long_lines).sum();
    println!(
        "{} search capture(s) · {tot} bytes still delivered after folding\n",
        rows.len()
    );
    println!("  duplicates     {dup:>8} B  ({:.1} %)", pct(dup, tot));
    println!("  common prefix  {pre:>8} B  ({:.1} %)", pct(pre, tot));
    println!(
        "  > {LINE_CAP} B/line  {lng:>8} B  ({:.1} %)",
        pct(lng, tot)
    );
    println!("\nper capture:");
    for (key, h) in rows.iter().take(15) {
        println!(
            "  {key:<34} {:>7} B · dup {:>6} · pref {:>5} · long {:>6}",
            h.folded, h.duplicates, h.prefix, h.long_lines
        );
        for (bytes, n, text) in &h.worst {
            let t: String = text.chars().take(64).collect();
            println!("      ×{n:<3} {bytes:>5} B  {t}");
        }
    }
}
