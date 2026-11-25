//! **What is the residue MADE of?**
//!
//! A rate says how much remains; it never says what. An earlier pass costed by
//! hand what the `npm` compactor left — 102 route lines for 5,826
//! bytes, 20 `.pnpm/` paths for 3,802 — and **19,000 of the 28,649 remaining
//! bytes were attributed to nothing**. An axis you cannot name cannot be
//! worked on, and it does not close either.
//!
//! This census folds a capture through the real entry point
//! (`filter_command`), then groups the RESIDUE lines by SHAPE: the
//! numbers, quoted strings, hashes and paths are replaced by
//! a marker, so two lines that differ only by their values
//! fall into the same group. The report gives the heaviest groups,
//! with an example — a number that cannot be re-read is an opinion.
//!
//! ## WHAT IT MEASURES, AND WHAT IT DOES NOT
//!
//! **A single layer.** It calls `filter_command`, which is the command
//! compactor — and the agent receives the result of a SECOND layer
//! (`disclose::build_summary`) that cuts a middle again. The two
//! interact: one change folded 3,569 bytes here and the next measured, on
//! the delivery path, that the output GREW from 16,122 to 20,397 bytes
//! — removing lines at the first layer takes the document below the threshold
//! of the second, which then stops cutting.
//!
//! So: by default this report names targets, it does not cost a gain.
//!
//! **`--delivered` takes the number from BOTH layers**, by running the real CLI
//! (`tokenade filter --deliver <prog>` on standard input) with an isolated
//! state. It is the only number one is allowed to put in a patch note,
//! and the instrument those changes were missing: without it, a first-layer
//! fold is judged on a quantity the agent never receives.
//!
//! Yet it lost it for eight iterations. It was first obtained through
//! `wrap "cat <capture>"`; a later fix corrected that routing — it measured the
//! `cat` compactor — by going through `filter <prog>`, which has only ONE layer.
//! A correct fix took away a capability, and this comment kept
//! announcing two layers. The report's sentence now comes from the path
//! actually taken (`Stages::banner`), not from here. `--one-stage` returns the old
//! number, for whoever wants to see the gap between the two.
//!
//! `cargo run -q -p tokenade-bench --bin residue_census [--fixture <key>] [--limit N]
//!  [--delivered [--one-stage]] [--verbatim]`
//!
//! Without `--fixture`, it surveys ALL command captures and ranks the
//! shapes by remaining bytes, all compactors combined.

use std::collections::BTreeMap;
use tokenade_bench::{captures_on, Route, Stages};

/// The shape of a line: what remains when the VALUES are erased.
///
/// Deliberately coarse. The goal is not to recognise a grammar but
/// to make lines that differ only by their data fall together —
/// which is exactly what a template fold could absorb.
fn shape_of(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // A whole quoted string becomes a marker.
            '"' | '\'' | '`' => {
                let quote = c;
                out.push('§');
                for n in chars.by_ref() {
                    if n == quote {
                        break;
                    }
                }
            }
            // An integer, whatever it is.
            '0'..='9' => {
                out.push('#');
                while chars
                    .peek()
                    .is_some_and(|n| n.is_ascii_digit() || *n == '.')
                {
                    chars.next();
                }
            }
            // A path: anything containing a `/` is crushed up to the whitespace.
            '/' => {
                out.push('¶');
                while chars.peek().is_some_and(|n| !n.is_whitespace()) {
                    chars.next();
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[derive(Default)]
struct Group {
    lines: usize,
    /// The distinct raw lines, with their count — what the shape erases.
    raw: BTreeMap<String, usize>,
    bytes: usize,
    /// In how many DISTINCT captures this shape appears.
    ///
    /// Without this column, a total does not say whether it describes a CLASS or a
    /// single file — and that is exactly the difference between a target and a
    /// local artifact. A fold was once built then removed because the
    /// shape only bit on three fixtures; the capture count answers
    /// that question BEFORE writing the code.
    files: std::collections::BTreeSet<String>,
    example: String,
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// **What the product returns ALMOST WHOLE.**
///
/// The ranking above sorts on ABSOLUTE residue: a small capture
/// returned as is never appears in it, whatever its rate. It is the
/// same mistake in the other direction — there a total hid a rate, here a total
/// hides a FAILURE. A compactor that recognises an output and returns it whole
/// is a defect of the same family as the lockfile one, and it has no minimum
/// size.
///
/// The bar is at 90 % returned: below that, something bit.
const RETURNED_WHOLE_PCT: usize = 90;

/// **And a floor size, without which the view says nothing.**
///
/// First version without it: 157 captures listed, including `macos/awk_field2`
/// (2 bytes), `macos/brew_list` (8) and `linux/date_now` (33) — returned at
/// 100 % because there is NOTHING to fold in them. Sorting on the percentage
/// brings up the smallest ones, exactly when the percentage stops
/// meaning anything. It is the same mistake in the other direction: there a total
/// hid a rate, here a rate hides a size.
///
/// A thousand bytes: below that, a fold would return less than what its announcement
/// costs on most paths.
const RETURNED_WHOLE_MIN_BYTES: usize = 1_000;

/// The selection, PURE: what weighs and that the product returns almost whole,
/// sorted on bytes still delivered.
pub fn returned_almost_whole(
    rows: &[(String, usize, usize)],
    pct: usize,
    min_bytes: usize,
) -> Vec<&(String, usize, usize)> {
    let mut kept: Vec<&(String, usize, usize)> = rows
        .iter()
        .filter(|(_, b, a)| *b >= min_bytes && a * 100 / b.max(&1) >= pct)
        .collect();
    // Sorted on BYTES still delivered, not on the percentage: what we
    // would recover by fixing is bytes.
    kept.sort_by_key(|y| std::cmp::Reverse(y.2));
    kept
}

fn print_returned_almost_whole(rows: &[(String, usize, usize)], limit: usize) {
    let kept = returned_almost_whole(rows, RETURNED_WHOLE_PCT, RETURNED_WHOLE_MIN_BYTES);
    if kept.is_empty() {
        println!(
            "no capture of at least {RETURNED_WHOLE_MIN_BYTES} bytes is returned \
             at more than {RETURNED_WHOLE_PCT} %: everything that weighs has been bitten.\n"
        );
        return;
    }
    let total: usize = kept.iter().map(|r| r.2).sum();
    println!(
        "returned at more than {RETURNED_WHOLE_PCT} % and weighing at least \
             {RETURNED_WHOLE_MIN_BYTES} bytes — {} capture(s), {} bytes delivered:",
        kept.len(),
        thousands(total)
    );
    for (key, b, a) in kept.iter().take(limit) {
        println!(
            "  {:>3} %  {:>9} → {:>9}  {key}",
            a * 100 / b.max(&1),
            thousands(*b),
            thousands(*a)
        );
    }
    println!();
}

/// The value of a `--name <value>` flag.
fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

/// One table per capture, re-readable: `key<TAB>before<TAB>after`.
fn save_rows(rows: &[(String, usize, usize)], path: &str) {
    let mut s = String::new();
    for (k, b, a) in rows {
        s.push_str(&format!("{k}\t{b}\t{a}\n"));
    }
    match std::fs::write(path, s) {
        Ok(()) => println!("  ↳ {} line(s) written to {path}", rows.len()),
        Err(e) => eprintln!("  ✗ writing {path}: {e}"),
    }
}

/// **Which capture GREW?**
///
/// The project's rules require this question for every fold change — it was measured
/// that a layer-1 fold can defuse layer 2 and make the delivered output grow — and
/// the instrument did not ask it. Two iterations asked it by hand, with
/// an analysis script written for the occasion, which is exactly the throwaway
/// probe the rules forbid. It is here, and the report NAMES the
/// captures that go up before announcing a total.
///
/// A capture absent from the baseline is not a regression: the corpus
/// grows. It is counted separately rather than passed over in silence.
fn report_against_baseline(rows: &[(String, usize, usize)], path: &str) {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  ✗ baseline {path} unreadable: {e}");
            return;
        }
    };
    let mut base = std::collections::HashMap::new();
    for l in raw.lines() {
        let mut it = l.split('\t');
        if let (Some(k), Some(_), Some(a)) = (it.next(), it.next(), it.next()) {
            if let Ok(a) = a.parse::<usize>() {
                base.insert(k.to_string(), a);
            }
        }
    }
    let mut up = Vec::new();
    let mut down = Vec::new();
    let mut unseen = 0usize;
    for (k, _, a) in rows {
        match base.get(k) {
            None => unseen += 1,
            Some(&b) if *a > b => up.push((k.clone(), b, *a)),
            Some(&b) if *a < b => down.push((k.clone(), b, *a)),
            _ => {}
        }
    }
    println!("against {path}:");
    if up.is_empty() && down.is_empty() {
        println!("  no capture moves.");
    }
    up.sort_by_key(|r| r.1 as i64 - r.2 as i64);
    for (k, b, a) in &up {
        println!(
            "  ↑ {:>9} → {:>9}  (+{})  {k}",
            thousands(*b),
            thousands(*a),
            a - b
        );
    }
    down.sort_by_key(|r| r.2 as i64 - r.1 as i64);
    for (k, b, a) in &down {
        println!(
            "  ↓ {:>9} → {:>9}  (−{})  {k}",
            thousands(*b),
            thousands(*a),
            b - a
        );
    }
    if unseen > 0 {
        println!("  {unseen} capture(s) absent from the baseline — the corpus has grown.");
    }
    // The TOTAL comes after the names: a net gain hiding a regression is
    // exactly what was paid for once by reading a total instead of the diffs.
    let net: i64 = down.iter().map(|r| r.2 as i64 - r.1 as i64).sum::<i64>()
        + up.iter().map(|r| r.2 as i64 - r.1 as i64).sum::<i64>();
    println!(
        "  net {net:+} byte(s) · {} up, {} down",
        up.len(),
        down.len()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    tokenade_bench::reject_unknown_args(
        &[
            "--fixture",
            "--limit",
            "--verbatim",
            "--delivered",
            "--one-stage",
            "--program-audit",
            "--pages",
            "--serp",
            "--annotations",
            "--save",
            "--baseline",
        ],
        &args,
    );
    let want = args
        .iter()
        .position(|a| a == "--fixture")
        .and_then(|i| args.get(i + 1))
        .cloned();
    // `--verbatim` shows the RAW lines of the heaviest group.
    //
    // Added because I did without it once too often: the shape erases
    // the digits, so `[20 lines elided]` and `[37 lines elided]` fall
    // into the SAME group, and the example line shows only one of them. I read
    // "110 holes of 20 lines" where the group only said "110 holes".
    // An instrument that summarises must be able to show what it summarised.
    let verbatim = args.iter().any(|a| a == "--verbatim");
    // **The heaviest family of the usage ranking was measured
    // nowhere.** `html-to-markdown` returns 21.5 MB to agents, 40.8 % of the deposit,
    // over 6,536 real calls — and the forty-one pages of the corpus were
    // outside the sweep, as the FORMAT captures were before.
    if args.iter().any(|a| a == "--pages") {
        page_census(limit_of(&args), Route::Page, "web");
        return;
    }
    // **The RESULTS pages route was walked by nothing.** Six
    // real pages, 1.1 MB, in the repo since July. Classifying it was
    // refused before it had been costed; here it is, costable.
    if args.iter().any(|a| a == "--serp") {
        page_census(limit_of(&args), Route::Serp, "search");
        return;
    }
    // `--annotations`: what the folds SAY, capture by capture.
    if args.iter().any(|a| a == "--annotations") {
        annotation_census();
        return;
    }
    // `--program-audit`: under WHICH name is each capture measured, and is that
    // name a line a user can type?
    if args.iter().any(|a| a == "--program-audit") {
        program_audit();
        return;
    }
    let delivered = args.iter().any(|a| a == "--delivered");
    // `--delivered` means BOTH layers, which its name always promised
    // and which it no longer did after a routing fix. `--one-stage` keeps the old
    // number for whoever wants to compare the two — it is the gap between them that
    // explains the earlier growth.
    let stages = if args.iter().any(|a| a == "--one-stage") {
        Stages::CommandOnly
    } else {
        Stages::Delivered
    };
    let limit: usize = limit_of(&args);

    // The DELIVERY path, measured through the real CLI. Built before the
    // loop: one bench per capture would open a fresh state each time, and
    // it is the shared state that keeps dedup alive — so each capture is
    // written under a UNIQUE name, otherwise the second measurement reads a duplicate.
    let bench = match delivered.then(tokenade_bench::Delivery::open).transpose() {
        Ok(b) => b,
        Err(note) => {
            eprintln!("{note}");
            std::process::exit(2);
        }
    };
    let mut delivered_rows: Vec<(String, usize, usize)> = Vec::new();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let (mut before, mut after, mut seen) = (0usize, 0usize, 0usize);
    for c in captures_on(Route::Command) {
        if want.as_ref().is_some_and(|w| &c.key != w) {
            continue;
        }
        // **Under the command that produced it**, not under a made-up name:
        // `tasklist_v_win` was measured under `tasklist-v`, which exists
        // nowhere, and came FIRST in this ranking with 7,185 bytes where
        // `tasklist /v` delivers 2,862. The refusal names the capture rather than
        // guessing.
        let prog = match tokenade_bench::command_of(&c.key) {
            Ok(Some(p)) => p,
            Ok(None) => continue,
            Err(note) => {
                eprintln!("{note}");
                std::process::exit(2);
            }
        };
        let folded =
            tokenade_core::filter::filter_command(std::slice::from_ref(&prog), &c.body).output;
        seen += 1;
        before += c.body.len();
        after += folded.len();
        if let Some(d) = bench.as_ref() {
            // UNDER ITS PROGRAM, through `filter <prog>`. Measuring `cat <fixture>`
            // measured `cat`'s routing: eight captures were reported at
            // 100 % although their compactor exists and folds.
            match d.measure_stages(&prog, &c.body, stages) {
                Ok(out) => delivered_rows.push((c.key.clone(), c.body.len(), out)),
                Err(note) => {
                    eprintln!("{note}");
                    std::process::exit(2);
                }
            }
        }
        for line in folded.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let e = groups.entry(shape_of(line)).or_default();
            e.lines += 1;
            e.bytes += line.len() + 1;
            e.files.insert(c.key.clone());
            *e.raw.entry(line.to_string()).or_default() += 1;
            if e.example.is_empty() {
                e.example = line.to_string();
            }
        }
    }
    if seen == 0 {
        eprintln!(
            "residue_census: no capture{}.\n\
             \x20 The instrument measured nothing — this is not \"no residue\".",
            want.map(|w| format!(" named `{w}`")).unwrap_or_default()
        );
        std::process::exit(2);
    }

    if !delivered_rows.is_empty() {
        let db: usize = delivered_rows.iter().map(|r| r.1).sum();
        let da: usize = delivered_rows.iter().map(|r| r.2).sum();
        print!(
            "{}",
            tokenade_bench::Measured {
                stages,
                items: delivered_rows.len(),
                unit: "capture(s)",
                before: db,
                after: da,
                via: match stages {
                    Stages::Delivered => "filter --deliver <prog>",
                    Stages::CommandOnly => "filter <prog>",
                    Stages::PageOnly => "web <file>",
                },
            }
            .header()
        );
        let mut worst = delivered_rows.clone();
        worst.sort_by_key(|b| std::cmp::Reverse(b.2));
        for (key, b, a) in worst.iter().take(limit) {
            println!("  {:>9} → {:>9}  {key}", thousands(*b), thousands(*a));
        }
        println!();
        // The layer-growth question, asked by the instrument and not by hand.
        if let Some(path) = flag_value(&args, "--baseline") {
            report_against_baseline(&delivered_rows, &path);
        }
        if let Some(path) = flag_value(&args, "--save") {
            save_rows(&delivered_rows, &path);
        }
        print_returned_almost_whole(&delivered_rows, limit);
    }

    let mut rows: Vec<(&String, &Group)> = groups.iter().collect();
    rows.sort_by_key(|b| std::cmp::Reverse(b.1.bytes));
    println!(
        "{seen} capture(s) · {} → {} bytes (−{} %)\n\
             \x20 {} distinct shape(s) in the residue\n\
             \x20 WARNING: ONE layer only (`filter_command`). The agent receives two —\n\
             \x20 a fold from here can make what is delivered GROW by defusing the\n\
             \x20 second. This report names targets, it does not cost a gain.\n",
        thousands(before),
        thousands(after),
        100 * before.saturating_sub(after) / before.max(1),
        thousands(rows.len())
    );
    print_shape_ranking(&groups, after, limit, verbatim);
}

#[cfg(test)]
mod returned_whole_tests {
    use super::*;

    fn row(k: &str, b: usize, a: usize) -> (String, usize, usize) {
        (k.to_string(), b, a)
    }

    /// **A percentage without a floor brings up the smallest captures,
    /// exactly where it stops meaning anything.**
    ///
    /// First version of this view: 157 captures listed, including
    /// `macos/awk_field2` (2 bytes) and `linux/date_now` (33) returned at 100 %
    /// because there is NOTHING to fold in them. It is the same mistake in the other
    /// direction — there a total hid a rate, here a rate hides a size.
    #[test]
    fn the_floor_keeps_the_tiny_captures_out_and_the_order_is_in_bytes() {
        let rows = vec![
            row("tiny_returned_whole", 33, 33),
            row("big_returned_whole", 3_769, 3_769),
            row("medium_almost_whole", 5_165, 5_049),
            row("big_well_folded", 48_436, 5_136),
        ];
        let kept = returned_almost_whole(&rows, 90, 1_000);
        let names: Vec<&str> = kept.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(
            names,
            vec!["medium_almost_whole", "big_returned_whole"],
            "the floor or the order has changed"
        );
        // The well-folded capture is not a target, and neither is the tiny one.
        assert!(!names.contains(&"big_well_folded"));
        assert!(!names.contains(&"tiny_returned_whole"));
        // Without a floor, the tiny one comes up — that is the original defect.
        assert!(returned_almost_whole(&rows, 90, 0)
            .iter()
            .any(|r| r.0 == "tiny_returned_whole"));
    }
}

/// The captures the ranking measures under a made-up name.
fn program_audit() {
    let bench = match tokenade_bench::Delivery::open() {
        Ok(b) => b,
        Err(note) => {
            eprintln!("{note}");
            std::process::exit(2);
        }
    };
    let mut rows: Vec<(String, String, String, usize, usize)> = Vec::new();
    for c in captures_on(Route::Command) {
        let (Some(fused), Ok(Some(real))) = (
            tokenade_bench::program_of(&c.key),
            tokenade_bench::command_of(&c.key),
        ) else {
            continue;
        };
        if fused == real {
            continue;
        }
        let a = bench.measure_stages(&fused, &c.body, Stages::Delivered);
        let b = bench.measure_stages(&real, &c.body, Stages::Delivered);
        if let (Ok(a), Ok(b)) = (a, b) {
            rows.push((c.key.clone(), fused, real, a, b));
        }
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.3.saturating_sub(r.4)));
    let over: usize = rows.iter().map(|r| r.3.saturating_sub(r.4)).sum();
    println!(
        "{} capture(s) measured under a fused name · {} byte(s) of overestimated residue",
        rows.len(),
        over
    );
    for (key, fused, real, a, b) in rows {
        println!("  {a:>7} under `{fused}`  ->  {b:>7} under `{real}`   {key}");
    }
}

/// The report's line ceiling.
fn limit_of(args: &[String]) -> usize {
    args.iter()
        .position(|a| a == "--limit")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(15)
}

/// **What a real PAGE still lets through to the agent.**
///
/// Measured by `web <file>`, the real path: a page passed through stdin
/// reaches no extractor, and measuring it under `read` would measure the read
/// view, not extraction. ONE layer only — the caller says so rather than
/// letting a complete delivery be assumed.
/// **The ranking by SHAPE, written once.**
///
/// It lived in `main`, so the PAGES route did not have it — and it is what
/// named an earlier target. A census of bytes says which entries
/// are heavy; only a census of shapes says what REPEATS, and a
/// repetition is a class no entry-by-entry report can name.
/// Same family as before: a general mechanism wired onto a single route.
fn print_shape_ranking(
    groups: &BTreeMap<String, Group>,
    total: usize,
    limit: usize,
    verbatim: bool,
) {
    let mut ranked: Vec<(&String, &Group)> = groups.iter().collect();
    ranked.sort_by_key(|b| std::cmp::Reverse(b.1.bytes));
    // REPEATED shapes first: a shape seen only once is
    // content, not a template, and folding it would save nothing.
    let repeated: usize = ranked
        .iter()
        .filter(|(_, g)| g.lines > 1)
        .map(|(_, g)| g.bytes)
        .sum();
    println!(
        "shapes seen MORE THAN ONCE weigh {} bytes, i.e. {} % of the residue:",
        thousands(repeated),
        100 * repeated / total.max(1)
    );
    for (shape, g) in ranked.iter().filter(|(_, g)| g.lines > 1).take(limit) {
        let ex: String = g.example.chars().take(72).collect();
        println!(
            "  ×{:<5} {:>7} B  in {:>3} capture(s)  {}",
            g.lines,
            thousands(g.bytes),
            g.files.len(),
            ex
        );
        // Few captures: NAME them. "in 2 captures" teaches nothing as long
        // as one does not know which, and that is where one goes to read.
        if g.files.len() <= 3 {
            println!(
                "        {}",
                g.files.iter().cloned().collect::<Vec<_>>().join(", ")
            );
        }
        if verbatim {
            let mut raw: Vec<(&String, &usize)> = g.raw.iter().collect();
            raw.sort_by(|a, b| b.1.cmp(a.1));
            println!(
                "        {} distinct raw line(s) under this shape:",
                raw.len()
            );
            for (line, n) in raw.iter().take(8) {
                let t: String = line.chars().take(64).collect();
                println!("          ×{n:<4} {t}");
            }
        }
        let _ = shape;
    }
}

/// The census of an HTML route: pages through `web`, RESULTS
/// pages through `search`.
///
/// Parameterised rather than duplicated, because measuring a SERP through `web` is the
/// wrong route — 47,853 bytes against 20,709 through `search`, and an output
/// that carries the engine's navigation bar. It is the "measuring a
/// capture under `cat`" mistake one door further.
fn page_census(limit: usize, route: Route, verb: &str) {
    let bench = match tokenade_bench::Delivery::open() {
        Ok(b) => b,
        Err(note) => {
            eprintln!("{note}");
            std::process::exit(2);
        }
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("tokenade-core").join("tests").join("fixtures"))
        .unwrap_or_default();
    let mut rows: Vec<(String, usize, usize)> = Vec::new();
    // **The ranking by SHAPE, on pages too.**
    //
    // It only existed on the commands route, and it is what named an
    // earlier target — a census of bytes says which pages are heavy,
    // only a census of shapes says what REPEATS, which no
    // page-by-page report can name. Same family as before: a general
    // mechanism wired onto a single route.
    // `via` names the LINE the caller would type. Static because
    // `Measured` keeps it by reference, and the only two HTML routes are
    // known here.
    let via: &'static str = if route == Route::Serp {
        "search <file>"
    } else {
        "web <file>"
    };
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for c in captures_on(route) {
        let full = root.join(&c.key);
        let file = full.parent().unwrap_or(&root).join(&c.file_name);
        let args = [verb, &*file.to_string_lossy()];
        let text = match bench.render_args(&args) {
            Ok(t) => t,
            Err(note) => {
                eprintln!("{note}");
                std::process::exit(2);
            }
        };
        rows.push((c.key.clone(), c.body.len(), text.len()));
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let e = groups.entry(shape_of(line)).or_default();
            e.lines += 1;
            e.bytes += line.len() + 1;
            e.files.insert(c.key.clone());
            *e.raw.entry(line.to_string()).or_default() += 1;
            if e.example.is_empty() {
                e.example = line.to_string();
            }
        }
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.2));
    let before: usize = rows.iter().map(|r| r.1).sum();
    let after: usize = rows.iter().map(|r| r.2).sum();
    print!(
        "{}",
        tokenade_bench::Measured {
            stages: Stages::PageOnly,
            items: rows.len(),
            unit: if route == Route::Serp {
                "results page(s)"
            } else {
                "page(s)"
            },
            before,
            after,
            via,
        }
        .header()
    );
    for (key, b, a) in rows.iter().take(limit) {
        println!(
            "  {:>9} → {:>9}  {key}",
            tokenade_bench::thousands(*b as u64),
            tokenade_bench::thousands(*a as u64)
        );
    }
    // **The KEPT share, not only the bytes.** A page whose extraction
    // returns a tenth of what it received is not comparable to another
    // that returns a hundredth, and ranking by bytes alone puts first
    // the pages that are simply big. The same pair as before: a total without
    // a rate, and a rate without a floor, are each wrong on their own side.
    let mut by_share: Vec<&(String, usize, usize)> = rows
        .iter()
        .filter(|(_, b, a)| kept_share_is_high(*b, *a))
        .collect();
    by_share.sort_by_key(|(_, b, a)| std::cmp::Reverse(a * 1000 / b.max(&1)));
    if by_share.is_empty() {
        println!("\nno page keeps more than {KEPT_SHARE_PCT} % of what it received.");
    } else {
        println!(
            "\nkeep more than {KEPT_SHARE_PCT} % of the page and weigh at least {} bytes — \
             {} page(s):",
            tokenade_bench::thousands(KEPT_FLOOR_BYTES as u64),
            by_share.len()
        );
        for (key, b, a) in by_share.iter().take(limit) {
            println!(
                "  {:>3} %  {:>9} → {:>9}  {key}",
                a * 100 / b.max(&1),
                tokenade_bench::thousands(*b as u64),
                tokenade_bench::thousands(*a as u64)
            );
        }
    }
    // The layer-growth question and the per-entry table, on pages too: they
    // were only wired on the commands route, and without them a
    // web fix is judged on a TOTAL — which has already been paid for once.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(path) = flag_value(&args, "--baseline") {
        report_against_baseline(&rows, &path);
    }
    if let Some(path) = flag_value(&args, "--save") {
        save_rows(&rows, &path);
    }
    // The same ranking as the commands route, through the same declarer.
    let after: usize = rows.iter().map(|r| r.2).sum();
    println!();
    print_shape_ranking(&groups, after, limit, false);
}

/// Above this, the extraction keeps an unusual share of the page.
const KEPT_SHARE_PCT: usize = 5;

/// Below this, the share says nothing: a two-kilobyte page kept at
/// half does not weigh. Both directions of the same mistake, written once.
const KEPT_FLOOR_BYTES: usize = 4_000;

fn kept_share_is_high(input: usize, delivered: usize) -> bool {
    delivered >= KEPT_FLOOR_BYTES && input > 0 && delivered * 100 >= input * KEPT_SHARE_PCT
}

#[cfg(test)]
mod page_census_tests {
    /// **The share AND the floor**, each on both sides.
    ///
    /// Ranking by bytes alone puts first the pages that are simply
    /// big; ranking by SHARE alone puts first the tiny ones.
    #[test]
    fn a_high_kept_share_needs_a_floor_too() {
        // 10 % of a 200 KB page: flagged.
        assert!(super::kept_share_is_high(200_000, 20_000));
        // 50 % of a 2 KB page: it is small, we do not flag it.
        assert!(!super::kept_share_is_high(2_000, 1_000));
        // Just under the share, above the floor: nothing.
        assert!(!super::kept_share_is_high(200_000, 9_999));
        // Just above the share: flagged.
        assert!(super::kept_share_is_high(200_000, 10_000));
        // An empty input is not a talkative extraction.
        assert!(!super::kept_share_is_high(0, 5_000));
    }
}

/// **What the folds ASSERT, over the whole corpus, in one command.**
///
/// A census of bytes says that a fold saves; it does not say what it returns.
/// `phrase_fold` was shipped measured and green, and three successive readings
/// found three structure corruptions in it — the value separator of the
/// neighbouring fold, a replacement biting inside words, then
/// column padding and a table rule declared as "sentences".
/// None was visible in a total, all were in the
/// FIRST line of the output.
///
/// This view prints, for each capture that carries any, the banners this
/// product writes — it is the reading made cheap enough to be done
/// before every commit that touches a fold.
fn annotation_census() {
    let bench = match tokenade_bench::Delivery::open() {
        Ok(b) => b,
        Err(note) => {
            eprintln!("{note}");
            std::process::exit(2);
        }
    };
    // No `Measured` here: this view measures nothing, it READS. A header
    // "0 → 0 bytes (−0 %)" is the all-is-well for zero measurements that has
    // already been paid for once.
    println!(
        "{}\nthis report READS what the folds assert; it costs nothing. (filter --deliver <prog>)",
        Stages::Delivered.banner()
    );
    let mut seen = 0usize;
    for c in captures_on(Route::Command) {
        let Ok(prog) = tokenade_core::corpus_key::command_of(&c.key) else {
            continue;
        };
        let Some(prog) = prog else { continue };
        let Ok(out) = bench.render(&prog, &c.body, Stages::Delivered) else {
            continue;
        };
        let notes: Vec<&str> = out
            .lines()
            .filter(|l| l.trim_start().starts_with("[tokenade:") || l.contains("» = "))
            .collect();
        if notes.is_empty() {
            continue;
        }
        seen += 1;
        println!("\n{}", c.key);
        for n in notes {
            println!("  {}", &n[..n.len().min(300)]);
        }
    }
    println!("\n{seen} capture(s) carry an annotation.");
}
