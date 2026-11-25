//! **The portrait of ONE compactor** — the `--focus` view.
//!
//! The ranking says where there is still something to gain; this view says why a row
//! is there. It reads the RAW ledger, not the aggregate, because the aggregate has
//! already thrown away the sessions — and it names what it could not detail,
//! since "no row for `network`" on a compactor the table
//! credits with 232,205 bytes is a lie of the view, not an absence.

use super::*;

/// What `--focus` reads from the ledger: the attributable rows, and what it
/// had to set aside.
///
/// Extracted to be TESTABLE, as `aggregate_by_cmd` already is: the
/// decision lived in a function that reads the disk, so the two
/// mutations that neutralised the coverage survived there — the pure function
/// was covered, its callers were not.
pub(crate) type FocusScan = (
    Vec<(u64, u64, i64, Option<String>)>,
    u64,
    u64,
    usize,
    std::collections::BTreeMap<i64, u64>,
);

pub(crate) fn focus_scan(text: &str, name: &str) -> FocusScan {
    let mut rows: Vec<(u64, u64, i64, Option<String>)> = Vec::new();
    // What this view CANNOT detail. A rollup bucket carries the sum
    // of a day and no session, so it is set aside from the rows — but
    // setting it aside without SAYING so makes it answer "no row for `network`" on
    // a compactor the ranking credits with 232,205 bytes and 1,173
    // uses. An instrument that cannot see must say so, not return zero.
    let (mut rolled_rows, mut rolled_after, mut rolled_calls) = (0usize, 0u64, 0u64);
    // And per DAY. The day rule judges on `per_day`, which the TABLE
    // fills with the named buckets — so its denominator is the whole
    // residue. Here the map was built on the attributable rows only:
    // same rule, two denominators, and `webfetch_compact` got set aside
    // on "a single day carries 100 % of the residue" where that day carries
    // 1,342 bytes out of 491,215.
    let mut rolled_by_day: std::collections::BTreeMap<i64, u64> = Default::default();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if !focus_matches(&v, name) {
            continue;
        }
        if v["rollup"].as_bool().unwrap_or(false) {
            rolled_rows += 1;
            rolled_after += tokenade_core::gain::delivered_of(&v);
            rolled_calls += v["rollup_calls"].as_u64().unwrap_or(1);
            *rolled_by_day
                .entry(v["ts"].as_i64().unwrap_or(0) / 86_400)
                .or_default() += tokenade_core::gain::delivered_of(&v);
            continue;
        }
        rows.push((
            v["before"].as_u64().unwrap_or(0),
            tokenade_core::gain::delivered_of(&v),
            v["ts"].as_i64().unwrap_or(0),
            residue_owner(&v).map(|s| s.chars().take(8).collect()),
        ));
    }
    (rows, rolled_after, rolled_calls, rolled_rows, rolled_by_day)
}

/// The rule lives in the PRODUCT (`gain::compactor_of`): it is what writes
/// these rows, and a second spelling of the same rule here would drift as soon
/// as a form is added. The bench reads it, it does not restate it.
pub(crate) fn compactor_name(v: &serde_json::Value) -> Option<&str> {
    tokenade_core::gain::compactor_of(v)
}

/// Does this row talk about the compactor we are looking at?
///
/// Two places name it. `cmd`/`compactor`/`label` on the rows of the
/// CLI path; but the hook writes `op:"auto:ini"` and puts the COMMAND
/// LINE in `label`, so that the compactor there is the `op`. Looking only
/// for the first made `--focus auto:ini` mute on 209 real uses: a
/// new instrument with half the ledger in its blind spot.
pub(crate) fn focus_matches(v: &serde_json::Value, name: &str) -> bool {
    compactor_name(v) == Some(name) || v["op"].as_str() == Some(name)
}

/// The portrait of ONE compactor: is it real usage, where does its residue go, and
/// what do its worst events look like.
///
/// Exists because I reconstructed it twice by hand before
/// realising that the target at the top of the ranking was my own measurement.
/// A question asked twice belongs to the bench.
pub(crate) fn focus(name: &str) {
    let Some(home) = tokenade_core::platform::home_dir() else {
        eprintln!("value_ranking: no home directory");
        std::process::exit(2);
    };
    let path = home.join(".tokenade").join("gain.jsonl");
    let text = tokenade_core::gain::ledger_with_rollup(&path);
    let (rows, rolled_after, rolled_calls, rolled_rows, rolled_by_day) = focus_scan(&text, name);
    if rows.is_empty() {
        if rolled_rows > 0 {
            println!(
                "`{name}` — nothing to detail: its {} bytes over {rolled_calls} calls are \n\
                 all in {rolled_rows} ROLLUP bucket(s), which carry the sum of a day \n\
                 and no session's name. This view cannot say more about them; the \n\
                 ranking, for its part, counts them.",
                thousands(rolled_after)
            );
        } else {
            println!("no row for `{name}`");
        }
        return;
    }
    let before: u64 = rows.iter().map(|r| r.0).sum();
    let after: u64 = rows.iter().map(|r| r.1).sum();
    let pct = if before > 0 {
        100.0 * (before - after.min(before)) as f64 / before as f64
    } else {
        0.0
    };
    println!(
        "{name} — {} rows, {} → {} bytes (−{pct:.0}%)",
        rows.len(),
        thousands(before),
        thousands(after)
    );
    let total = after + rolled_after;
    if rolled_rows > 0 {
        // The COVERAGE, stated here as the probe rule states it elsewhere:
        // everything that follows — days, sessions, worst events — speaks
        // only of the detailable part.
        println!(
            "  {} MORE bytes ({:.0} % of the total) are in {rolled_rows} rollup bucket(s), \
             over {rolled_calls} calls: nothing that follows talks about them\n  \
             these buckets are {}",
            thousands(rolled_after),
            100.0 * rolled_after as f64 / total.max(1) as f64,
            day_span(rolled_by_day.keys().copied(), today_day())
        );
    }

    let mut by_day = rolled_by_day;
    for (_, a, ts, _) in &rows {
        *by_day.entry(ts / 86_400).or_default() += a;
    }
    // A row WITHOUT a session is detailable — it has its bytes, its date, its
    // worst case — and it is attributable to nobody. It therefore goes into the
    // days and never into the sessions.
    let (by_sess, sessionless, sess_share) =
        SessionConcentration::of_focus_rows(&rows, rolled_after);
    // SESSIONS are stated as a share of the attributable — it is all they
    // cover — and DAYS as a share of the total, buckets included.
    let share = |v: u64| {
        if after == 0 {
            0.0
        } else {
            100.0 * v as f64 / after as f64
        }
    };
    let day_pct = |v: u64| 100.0 * v as f64 / total.max(1) as f64;
    let mut days: Vec<_> = by_day.iter().map(|(d, v)| (*v, *d)).collect();
    days.sort_by_key(|b| std::cmp::Reverse(b.0));
    let day_share = top_share(&by_day);
    println!(
        "  {} days; the heaviest carries {:.0}% of the residue",
        by_day.len(),
        day_pct(days[0].0)
    );
    // FROM WHEN do these events date? The question decides whether the target is
    // still alive, and the view did not ask it: the ranking covers
    // `span.days` days, so a defect fixed last week appears there with its
    // previous rate and presents itself as a target. An iteration nearly took up
    // `auto:grep` again, already fixed, on rows that all predated the fix.
    println!("  {}", day_span(by_day.keys().copied(), today_day()));
    if sessionless > 0 {
        println!(
            "  {} bytes ({:.0} % of the total) are on rows WITHOUT a session: \
             detailable, attributable to nobody",
            thousands(sessionless),
            100.0 * sessionless as f64 / total.max(1) as f64
        );
    }
    let mut sessions: Vec<_> = by_sess.iter().map(|(s, v)| (*v, s.clone())).collect();
    sessions.sort_by_key(|b| std::cmp::Reverse(b.0));
    for (v, sname) in sessions.iter().take(3) {
        println!(
            "  session {sname:<10} {:>10} bytes  {:.0}%",
            thousands(*v),
            share(*v)
        );
    }
    // This view PRINTED the sessions and gave no verdict: an iteration
    // got sent onto its own measurement with these rows right in front of it.
    // It says so now, with the same rule as the two tables.
    if let Some(why) = self_probe_reason(day_share, sess_share, rows.len() as u64) {
        // The reason DISPLAYED must be the one that decided. The first version
        // always talked about the session, and printed "a single session carries
        // 13 %" on `read_code_strip_lite`, set aside by the DAY rule
        // (100 %): an instrument that explains its decision badly is
        // rightly disobeyed, and 13 % disqualifies nothing.
        println!(
            "  WARNING  concentrated, so not spread usage: {why}.\n\
             \x20          The ranking sets it aside; do not take this target."
        );
    }

    let mut afters: Vec<u64> = rows.iter().map(|r| r.1).collect();
    afters.sort_unstable();
    let q = |f: f64| afters[((afters.len() - 1) as f64 * f) as usize];
    println!(
        "  residue per call: median {} · p90 {} · max {}",
        thousands(q(0.5)),
        thousands(q(0.9)),
        thousands(q(1.0))
    );

    let mut worst = rows.clone();
    worst.sort_by_key(|b| std::cmp::Reverse(b.1));
    println!("  worst events (input → delivered):");
    for (b, a, ts, sname) in worst.iter().take(5) {
        // **The DATE of the event, not only that of the row.** An earlier iteration
        // dated the row; a "live" row can carry its worst cases
        // in a distant past, and it is the worst case we are going to fix.
        // MEASURED 2026-09-08: `auto:archive` had only one day of inactivity and
        // its two biggest events did not date from the same month.
        // `no session` rather than a `?`: the invented name was the defect.
        println!(
            "    {:>10} → {:<10} {}  {}",
            thousands(*b),
            thousands(*a),
            iso_day(ts / 86_400),
            match sname {
                Some(s) => format!("session {s}"),
                None => "no session".into(),
            }
        );
    }
}
