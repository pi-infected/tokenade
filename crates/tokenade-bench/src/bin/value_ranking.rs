//! Regenerates the MEASURED value hierarchy (a local, untracked report).
//!
//! It exists because a declared hierarchy is an opinion. This one comes out
//! of the real usage ledger (`tokenade gain --json`), so it says what the
//! agents really take and what it still costs.
//!
//! ## The first ranking was wrong, and its first use showed it
//!
//! It sorted the "expensive and inefficient paths" by bytes still delivered,
//! without relating them to the total. `sysinfo` therefore came first with 59,307
//! uses — and 3,879,492 input bytes, i.e. **65 bytes per call**. The remaining
//! gain there was worth 1.6 MB out of 833 MB saved: 0.2 %. Sorting on an
//! unrelated total over-weights very frequent operations with a tiny load.
//!
//! Corrected, the picture flips: **76 % of everything still delivered comes from
//! TWO operations** (`html-to-markdown` 40.9 %, `auto-compact` 35.0 %), already at
//! −95 % but so voluminous that their residue dominates. Gaining one point on
//! `html-to-markdown` pays more than perfecting `sysinfo` entirely.
//!
//! ## And my own probes must be set aside
//!
//! `a11y-compact` weighed 11.2 % of what was delivered — 5 calls, all on the same day. It is
//! a probe from a past iteration, not usage. The ledger carries
//! `heaviest_day_share`: an operation concentrated on a single day AND little
//! used is noise I made.

use tokenade_bench::Bench;
// `portrait` and not `focus`: the view is named after the `focus` function,
// and a module of the same name would make `focus(name)` unreachable from the trunk.
// `#[path]` because this file is the ROOT of its binary: `mod x;`
// looks there for `src/bin/x.rs`, next to the sixteen other benches, and not
// `src/bin/value_ranking/x.rs`. Without it, three very common names
// (`sessions`, `exclusions`) would camp in the binaries directory.
#[path = "value_ranking/exclusions.rs"]
mod exclusions;
#[path = "value_ranking/portrait.rs"]
mod portrait;
#[path = "value_ranking/sessions.rs"]
mod sessions;
pub(crate) use exclusions::*;
pub(crate) use portrait::*;
pub(crate) use sessions::*;

/// The share of the residue a view could ATTRIBUTE, over the total it knows
/// exists. It is the denominator that makes a session share honest, and it
/// is computed at two places — the `op` table, which counts the rollup
/// buckets in the total, and the `--focus` view, which counts them apart. A single
/// spelling, because the two have already contradicted each other: `--focus` set it
/// to one, which is true of the ROWS it lists and false of the OP it
/// talks about.
/// The AVERAGE residue per call — the quantity a compactor can
/// act on, which the ranking never displayed although it held both its
/// factors.
///
/// MEASURED 2026-09-07: `sysinfo` is 3rd in the pool with 1,608,905 bytes, and
/// **59,307 calls**, i.e. **27 bytes per call**. The document says to read it
/// from top to bottom, and its third row has nothing to take — while
/// `npm`, 4th, leaves 10,446 per call. The ranking sorted on the total and
/// read like a queue of targets.
fn residue_per_call(rest: u64, count: u64) -> Option<u64> {
    (count > 0).then(|| rest / count)
}

/// Is there still something to fold on ONE call?
///
/// Folding returns at best the residue per call MINUS the banner: at equality the
/// gain is exactly zero. The bound is therefore `<=`, and it lived in the
/// markdown rendering — a decision in a function that writes a document,
/// so untestable and surviving mutation.
fn has_headroom(per: Option<u64>, floor: Option<u64>) -> bool {
    match (per, floor) {
        (Some(p), Some(f)) => p > f,
        _ => true,
    }
}

/// What an operation STILL delivers — the only quantity that says where there is
/// still something to gain. `percent` alone over-weights the tiny, `count` alone the frequent.
fn still_delivered(before: u64, saved: u64) -> u64 {
    before.saturating_sub(saved)
}

/// What a `by_op` row really delivered. `delivered` (written by
/// `gain --json`) removes the outputs that a stash or a dedup of the same call
/// replaced; `before − saved` counted them — measured 2026-10-02: 168,378
/// phantom tokens on `auto:json`, hoisted to 3rd. An older ledger without the
/// field keeps the old rule.
fn op_delivered(o: &serde_json::Value) -> u64 {
    o["delivered"].as_u64().unwrap_or_else(|| {
        still_delivered(
            o["before"].as_u64().unwrap_or(0),
            o["saved"].as_u64().unwrap_or(0),
        )
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    tokenade_bench::reject_unknown_args(&["--focus"], &args);
    if let Some(i) = args.iter().position(|a| a == "--focus") {
        match args.get(i + 1) {
            Some(name) => {
                focus(name);
                return;
            }
            None => {
                eprintln!("usage: value_ranking --focus <compactor>");
                std::process::exit(2);
            }
        }
    }
    let bench = match Bench::reading_real_state() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("value_ranking: no tokenade binary to measure: {e}");
            std::process::exit(2);
        }
    };
    let run = match bench.run(&["gain", "--json"]) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("value_ranking: `gain --json` could not run: {e}");
            std::process::exit(2);
        }
    };
    if !run.survived() {
        eprintln!("value_ranking: `gain --json` was killed by a signal");
        std::process::exit(2);
    }
    let v: serde_json::Value = match serde_json::from_slice(&run.stdout) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("value_ranking: unreadable `gain --json` output: {e}");
            std::process::exit(2);
        }
    };

    let (by_cmd, rolled_up, cmd_probes, cmd_probe_rows) = per_command_rows();
    // **Which family still has a named PIECE to act on?**
    //
    // `op` is a family; the unit we fix is the compactor. A
    // family ALL of whose compactors are set aside — concentrated, or with a
    // fabricated baseline — offers no target, and the table above
    // ranked it anyway. Measured 2026-09-08: `npm` is 4th in the pool with
    // 9,775 bytes per call, and not one of its compactor rows
    // survives. Two iterations went off on it only to come back
    // with "the named target was already folded".
    let ops_with_a_named_target: std::collections::BTreeSet<&str> =
        by_cmd.iter().map(|(op, ..)| op.as_str()).collect();
    let sess_by_op = session_shares();
    let ops = v["by_op"].as_array().cloned().unwrap_or_default();
    // A single window: `span.days`. Frozen at 104, the STALE prose
    // contradicted the header the day the ledger passed 104 (115
    // on 2026-09-19).
    let window_days = v["span"]["days"].as_u64().unwrap_or(0);
    // The denominator must CONTAIN the rows it divides. The header's `after`
    // sums everything EXCEPT the code-intel ops, which `by_op` does
    // carry: measured 2026-09-08 on this ledger, 52,464,200 versus
    // 52,701,930, the gap being exactly the 237,730 bytes of those six
    // rows — two of which appeared in the first fifteen with a "share
    // of delivered" computed on a total that did not count them.
    let delivered_total = ops.iter().map(op_delivered).sum::<u64>().max(1);
    let mut rows: Vec<(String, u64, u64, f64, f64, Option<Excluded>)> = ops
        .iter()
        .map(|o| {
            let name = o["op"].as_str().unwrap_or("?").to_string();
            let count = o["count"].as_u64().unwrap_or(0);
            let saved = o["saved"].as_u64().unwrap_or(0);
            let pct = o["percent"].as_f64().unwrap_or(0.0);
            let share = o["heaviest_day_share"].as_f64().unwrap_or(0.0);
            let ssh = sess_by_op.get(&name).copied().unwrap_or_default();
            // Two rules, a single channel. The second arrived later:
            // a row whose comparison baseline is the
            // published ratio of `code_intel_heuristic` folded nothing, and its
            // `percent` IS that ratio. Reading it as a measured rate makes
            // one take as a target an answer that never had an input.
            let excluded = not_a_target(
                &name,
                o["baseline"].as_str(),
                o["borrowed_saved"].as_u64().unwrap_or(0),
                saved,
                op_delivered(o),
                count,
                ssh,
                share,
                ops_with_a_named_target.contains(name.as_str()),
                today_day(),
                window_days,
            );
            (name, count, op_delivered(o), pct, share, excluded)
        })
        .collect();
    rows.sort_by_key(|b| std::cmp::Reverse(b.2));

    let span = &v["span"];
    let mut md = String::new();
    md.push_str("# Value ranking — derived from REAL usage\n\n");
    md.push_str(&format!(
        "Source: `tokenade gain --json`. **{} operations**, {} days, from {} to {}.\n\
         Regenerated by `cargo run -p tokenade-bench --bin value_ranking`.\n\n",
        v["operations"].as_u64().unwrap_or(0),
        span["days"].as_u64().unwrap_or(0),
        span["first_iso"]
            .as_str()
            .unwrap_or("?")
            .get(..10)
            .unwrap_or("?"),
        span["last_iso"]
            .as_str()
            .unwrap_or("?")
            .get(..10)
            .unwrap_or("?"),
    ));
    md.push_str(
        "> A DECLARED value hierarchy is an opinion; this one is measured.\n\
         > Refresh it if it is more than 7 days old, otherwise read it and work from top to bottom.\n\n",
    );
    md.push_str(&format!(
        "**Total still delivered to agents: {} bytes.** This is the pool, the sum of the rows \
         of `by_op` — the very ones the table ranks.\n\n",
        thousands(delivered_total)
    ));

    md.push_str(
        "## Where there is still something to gain (and where a regression costs the most)\n\n",
    );
    md.push_str("| # | op | uses | still delivered | share of delivered | rate | residue/call |\n");
    md.push_str("|---|---|---|---|---|---|---|\n");
    let mut i = 0;
    let mut no_headroom: Vec<String> = Vec::new();
    let mut headless: Vec<String> = Vec::new();
    // The floor is MEASURED on the corpus's real folds, it is not chosen.
    let floor = tokenade_bench::announcement_cost();
    // **Two policies on a single question.** A row without a named piece
    // stays in the table with a mark: its residue is real, and
    // `html-to-markdown` weighs 40.9 % of the pool — hiding it would cost more
    // than the warning. The three other reasons set aside.
    let is_marked = |e: &Option<Excluded>| e.as_ref().is_some_and(|x| x.kind == HEADLESS);
    for (name, count, rest, pct, _share, probe) in
        rows.iter().filter(|r| r.5.is_none() || is_marked(&r.5))
    {
        i += 1;
        if i > 15 {
            break;
        }
        let _ = probe;
        let per = residue_per_call(*rest, *count);
        if is_marked(probe) {
            headless.push(name.clone());
        }
        md.push_str(&format!(
            "| {i} | `{name}`{} | {}× | {} | {:.1}% | −{:.0}% | {} |\n",
            if is_marked(probe) { " ✖" } else { "" },
            thousands(*count),
            thousands(*rest),
            100.0 * *rest as f64 / delivered_total as f64,
            pct,
            match per {
                Some(p) => format!(
                    "{} B{}",
                    thousands(p),
                    if has_headroom(per, floor) { "" } else { " ⌀" }
                ),
                None => "?".into(),
            }
        ));
        if !has_headroom(per, floor) {
            no_headroom.push(format!(
                "`{name}` — {} bytes over {} calls, i.e. {} per call",
                thousands(*rest),
                thousands(*count),
                thousands(per.unwrap_or(0))
            ));
        }
    }
    if let (false, Some(f)) = (no_headroom.is_empty(), floor) {
        md.push_str(&format!(
            "\n**⌀ — no headroom PER CALL.** The total is a pool, but one does not fold \
             a pool, one folds a call: every fold ANNOUNCES itself, and the shortest banner this product emits \
             on the real corpus measures {f} bytes. Folding therefore returns at best the residue \
             MINUS {f}: at equality the gain is exactly zero, below it the output grows. \
             {} row(s) above are in this case — {}. Reading them from top to bottom as \
             targets is what this ranking made one say about `sysinfo`: 3rd in the pool, and \
             nothing to take per call.\n",
            no_headroom.len(),
            no_headroom.join(" ; ")
        ));
    }
    if !headless.is_empty() {
        md.push_str(&format!(
            "\n**✖ — no named PIECE to act on.** `op` is a family; the unit \
             we fix is the compactor, and the table below names one per row. \
             {} row(s) above no longer have ANY row there: all their compactors are \
             set aside — concentrated on one day or one session, or with a fabricated \
             comparison baseline. Their residue is real and it is in rollup buckets \
             that name nobody, so there is nothing to open: {}. Two iterations went \
             off on `npm` only to come back with \"the named target was already \
             folded\"; this sign is what would have said so before leaving.\n",
            headless.len(),
            headless
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(" ; ")
        ));
    }
    // ── Opening the families ───────────────────────────────────────────────
    //
    // `op` is a FAMILY (`sysinfo` covers `lsblk`, `uname`, `hostname`…;
    // `npm` covers seven compactors), and the unit we fix is the
    // compactor. The ranking therefore stopped one level too high: it said
    // "npm returns −55 %" without being able to say WHICH of the seven. The ledger already carries
    // `cmd` on each row, so we read it — with the product's own reader
    // (`gain::ledger_with_rollup`), so as not to re-implement the
    // rotation handling in a bench that would then drift.

    md.push_str("\n## Inside the families: which compactor exactly\n\n");
    md.push_str(
        "`op` is a family; it is the compactor we fix. Without this table, the \
         ranking stops at \"npm returns −55 %\" without saying which of the seven.\n\n",
    );
    md.push_str(
        "The name comes from `cmd` (the folded program) or, on a row born from CONTENT \
         rather than from a command — a `read`, an MCP answer, a page — from `label`. \
         Reading only `cmd` made the ranking's second family disappear \
         entirely.\n\n",
    );
    md.push_str(&format!(
        "**{} bytes still delivered are not in this table**: they come from rollup \
         rows, one bucket per day and per op that names no compactor. They are \
         counted in the table above, which reads `gain --json`.\n\n",
        thousands(rolled_up)
    ));
    md.push_str(&format!(
        "**{} more bytes were set aside as MY probes**: a compactor almost all of whose \
         residue falls on a single day — or more than 80 % of which a single SESSION \
         wrote, when at least half the residue is attributable to a \
         session — is traffic I just made while measuring, whatever its \
         number of calls. The coverage is in the rule: a rollup bucket carries no \
         session's name, and a share computed on a third of the residue cannot \
         condemn the whole.\n\n",
        thousands(cmd_probes)
    ));
    md.push_str(&render_excluded(
        "The rows set aside, and on what proof",
        &cmd_probe_rows,
    ));
    md.push_str("| op | cmd | uses | still delivered | rate |\n|---|---|---|---|---|\n");
    for (op, cmd, count, rest, before) in by_cmd.iter().take(15) {
        let pct = if *before > 0 {
            100.0 * (*before - *rest) as f64 / *before as f64
        } else {
            0.0
        };
        md.push_str(&format!(
            "| `{op}` | `{cmd}` | {}× | {} | −{pct:.0}% |\n",
            thousands(*count),
            thousands(*rest)
        ));
    }

    let probes: Vec<&String> = rows
        .iter()
        .filter(|r| r.5.as_ref().is_some_and(|e| e.kind == CONCENTRATED))
        .map(|r| &r.0)
        .collect();
    let borrowed_rows: Vec<String> = rows
        .iter()
        .filter_map(|(name, count, rest, _, _, ex)| {
            let e = ex.as_ref()?;
            (e.kind == BORROWED).then(|| {
                format!(
                    "`{name}` — {count} uses, {} bytes · {}",
                    thousands(*rest),
                    e.why
                )
            })
        })
        .collect();
    let borrowed_bytes: u64 = rows
        .iter()
        .filter(|r| r.5.as_ref().is_some_and(|e| e.kind == BORROWED))
        .map(|r| r.2)
        .sum();
    if !borrowed_rows.is_empty() {
        md.push_str(&format!(
            "\n## Set aside: nothing to fold, the answer IS the output\n\n\
             **{} bytes** over {} row(s) compute their \"before\" FROM what they \
             delivered — the published ratio of `code_intel_heuristic` (57 %, CodeGraph 0.9.4) \
             for the code-intelligence ops, a native multiplier or a flat turn \
             credit from `coefficients` for the others — and not from an input that \
             this machine read. `structure_map` BUILDS a map of the repo and \
             `web-search` a list of results: there is no raw output to fold, \
             so their rate IS the constant and their \"residue per call\" is the size \
             of an answer. The terminal table has always partitioned the code-intel ops; \
             it does not see the others, `gain --json` did not say so, and this \
             ranking filed them among measured rates.\n\n",
            thousands(borrowed_bytes),
            borrowed_rows.len()
        ));
        md.push_str(&render_excluded(
            "Which ones, and on what proof",
            &borrowed_rows,
        ));
    }
    md.push_str(
        "\n## Set aside: concentrated, so not spread usage\n\nA residue that falls \
         on a single day, or almost all of which a single SESSION wrote, does not say \
         where an improvement would pay: it is a single piece of work, often a probe \
         I wrote while measuring — but not always mine, and the rule does not \
         claim so.\n\n",
    );
    let probe_rows: Vec<String> = rows
        .iter()
        .filter_map(|(name, count, rest, _pct, _share, ex)| {
            let e = ex.as_ref()?;
            (e.kind == CONCENTRATED).then(|| {
                format!(
                    "`{name}` — {count} uses, {} bytes · {}",
                    thousands(*rest),
                    e.why
                )
            })
        })
        .collect();
    md.push_str(&render_excluded(
        "Which ones, and on what proof",
        &probe_rows,
    ));
    // **A SILENT removal is half of the defect** — the earlier rule,
    // applied to the two headings added later. A row that disappears
    // from the table without anything saying so makes one look for why the ranking
    // changed, which costs more than the row was worth.
    for (kind, title, preamble) in [
        // A single heading here: `HEADLESS` MARKS the row instead of
        // setting it aside (see `is_marked`), so it is already stated at the top and a
        // second list would repeat it.
        (
            STALE,
            "Set aside: nothing arrives there any more",
            &format!(
                "{}. A compactor fixed this week still appears there with its \
                 BEFORE rate and presents itself as a target: that is what sent an iteration \
                 onto `read_code_strip_lite`, 7th at −43 %, whose last event \
                 preceded the fix to the read-view ladder.",
                ranking_covers(window_days).replacen("the ", "The ", 1)
            ),
        ),
        (
            HISTORY,
            "Set aside: ranked on their history",
            &format!(
                "Live, but at least {:.0} % of their residue sleeps in rollup buckets \
                 older than {STALE_AFTER_DAYS} days: their rank says what they used to deliver, \
                 not what they deliver. Measured 2026-10-02: `sysinfo` 2nd with 1,576,872 \
                 bytes, of which 2,978 since July.",
                HISTORY_SHARE * 100.0
            ),
        ),
    ] {
        let rows: Vec<String> = rows
            .iter()
            .filter_map(|(name, count, rest, _, _, ex)| {
                let e = ex.as_ref()?;
                (e.kind == kind).then(|| {
                    format!(
                        "`{name}` — {count} uses, {} bytes · {}",
                        thousands(*rest),
                        e.why
                    )
                })
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        md.push_str(&format!("\n## {title}\n\n{preamble}\n\n"));
        md.push_str(&render_excluded("Which ones, and on what proof", &rows));
    }
    md.push_str(
        "\n## How to use it\n\n\
         - The table says two things at once: where a **regression** costs the most, and where an \
         **improvement** still pays.\n\
         - An already excellent rate on a huge volume beats a mediocre rate on a small one: gaining \
         one point on the first row pays more than perfecting the fourth.\n\
         - Do not sort on the rate alone (over-weights the tiny) nor on the number of uses alone \
         (over-weights the frequent): that is the mistake the first version of this file made.\n",
    );

    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/value-ranking.md");
    if let Err(e) = tokenade_bench::write_generated_doc(&out, &md) {
        eprintln!("value_ranking: cannot write: {e}");
        std::process::exit(2);
    }
    println!(
        "wrote {} ({} op, {} set aside)",
        out.display(),
        rows.len(),
        probes.len()
    );
}

/// `(op, cmd, uses, still delivered, incoming bytes)`, from most delivered to least.
///
/// Reads the ledger through the PRODUCT's reader, not through a copy: rotation
/// moves rows into a rollup file, and a bench that ignored it
/// would report the week's ranking instead of the installation's.
fn per_command_rows() -> (Vec<(String, String, u64, u64, u64)>, u64, u64, Vec<String>) {
    let Some(home) = tokenade_core::platform::home_dir() else {
        return (Vec::new(), 0, 0, Vec::new());
    };
    let path = home.join(".tokenade").join("gain.jsonl");
    aggregate_by_cmd(&tokenade_core::gain::ledger_with_rollup(&path))
}

/// The aggregation itself, separated from reading to be testable on
/// text rather than on the machine's ledger.
fn aggregate_by_cmd(text: &str) -> (Vec<(String, String, u64, u64, u64)>, u64, u64, Vec<String>) {
    use std::collections::BTreeMap;
    let mut acc: BTreeMap<(String, String), (u64, u64, u64)> = BTreeMap::new();
    // The residue, per compactor AND per day: it is its CONCENTRATION that
    // distinguishes usage from a probe of mine. See `is_self_probe`.
    let mut per_day: BTreeMap<(String, String), BTreeMap<i64, u64>> = BTreeMap::new();
    // The SESSION judges better than the day, and this pass already reads the row
    // that carries it: not keeping it forced the table to decide on the
    // weaker of the two signals.
    let mut per_sess: BTreeMap<(String, String), BTreeMap<String, u64>> = BTreeMap::new();
    // And the bytes NO session carries: a NAMED rollup bucket enters
    // this table but not the per-session
    // split. Ignoring them in the denominator amounts to setting a coverage of one
    // that is true of the rows kept and false of the key in question —
    // the exact error `--focus` carried, measured at 0.3 % of the bytes on
    // `webfetch_compact`.
    let mut per_key_unattributed: BTreeMap<(String, String), u64> = BTreeMap::new();
    // And the share of the savings whose baseline was FABRICATED. The same rule as
    // the `op` table, here too: this table shows a rate, and the first fix
    // only corrected the other one — `structure_map / map (repo)` stayed there at
    // −57 %, set aside from one table and presented in the next.
    let mut per_key_borrowed: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut rolled_up_after = 0u64;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(op) = v["op"].as_str() else {
            continue;
        };
        // A ROLLUP row is a day bucket: it carries the sum of
        // `rollup_calls` calls and no compactor's name (`(rolled-up)`).
        // Counting it here would put "html-to-markdown / (rolled-up), 6 uses,
        // 21 MB" at the top of a table that exists to name the compactor.
        // It is counted one level UP, by `gain --json` itself.
        // A rollup bucket is an aggregated day. Since it carries
        // `compactor`, it KNOWS whom it belongs to and enters the table
        // with its real cardinality (`rollup_calls`). An older bucket,
        // folded before that field existed, belongs to nobody: it
        // stays out of the table, and its weight is stated.
        let is_rollup = v["rollup"].as_bool().unwrap_or(false);
        if is_rollup && compactor_name(&v).is_none() {
            rolled_up_after += tokenade_core::gain::delivered_of(&v);
            continue;
        }
        // `cmd` names the PROGRAM whose output was folded; it only exists
        // on rows that came from a command. A row born from content —
        // a `read`, an MCP answer, a page — carries its compactor under
        // `label` instead. Keeping only `cmd` made the ranking's
        // SECOND family disappear entirely: `auto-compact`, 3,315
        // rows and 35 % of everything still delivered, in a table whose
        // reason for being is to say "which of the seven exactly".
        let Some(cmd) = compactor_name(&v) else {
            continue;
        };
        let before = v["before"].as_u64().unwrap_or(0);
        let after = tokenade_core::gain::delivered_of(&v);
        let key = (op.to_string(), cmd.to_string());
        let e = acc.entry(key.clone()).or_default();
        e.0 += if is_rollup {
            v["rollup_calls"].as_u64().unwrap_or(1)
        } else {
            1
        };
        e.1 += after;
        e.2 += before;
        if tokenade_core::coefficients::baseline_is_borrowed(v["class"].as_str(), op, before, after)
        {
            *per_key_borrowed.entry(key.clone()).or_default() += before.saturating_sub(after);
        }
        let day = v["ts"].as_i64().unwrap_or(0) / 86_400;
        *per_day
            .entry(key.clone())
            .or_default()
            .entry(day)
            .or_default() += after;
        match residue_owner(&v) {
            Some(sess) => {
                *per_sess
                    .entry(key)
                    .or_default()
                    .entry(sess.to_string())
                    .or_default() += after;
            }
            None => *per_key_unattributed.entry(key).or_default() += after,
        }
    }
    let mut probe_after = 0u64;
    let mut probe_rows: Vec<String> = Vec::new();
    let mut out: Vec<(String, String, u64, u64, u64)> = acc
        .into_iter()
        .filter(|(key, (n, rest, before))| {
            let share = per_day.get(key).map(top_share).unwrap_or(0.0);
            let ssh = per_sess
                .get(key)
                .map(|m| {
                    SessionConcentration::measured(
                        m,
                        per_key_unattributed.get(key).copied().unwrap_or(0),
                    )
                })
                .unwrap_or_default();
            // A zero-byte residue has nothing to give up: let it through
            // rather than declare it a probe on a share computed over nothing.
            // Two rules, in the same order as the `op` table: a fabricated
            // baseline first, then the concentration.
            let borrowed = per_key_borrowed.get(key).copied().unwrap_or(0);
            let saved = before.saturating_sub(*rest);
            if let Some(e) = borrowed_reason(
                if borrowed == 0 {
                    Some("measured")
                } else {
                    Some("mixed")
                },
                &key.0,
                borrowed,
                saved,
                *rest,
                *n,
            ) {
                probe_after += *rest;
                probe_rows.push(format!(
                    "{}/{} — {} uses, {} bytes · {}",
                    key.0,
                    key.1,
                    n,
                    thousands(*rest),
                    e.why
                ));
                return false;
            }
            let keep = *rest == 0 || !is_self_probe(share, ssh, *n);
            if !keep {
                probe_after += *rest;
                // A SILENT removal is half of the defect: that is what
                // was recorded when setting aside the rollup buckets, and the
                // same rule holds here. The row says who leaves and on what
                // proof, so that a gap with `--focus` shows instead of being
                // guessed.
                let why = self_probe_reason(share, ssh, *n)
                    .unwrap_or_else(|| "reason lost — the rule changed under the list".into());
                probe_rows.push(format!(
                    "{}/{} — {} uses, {} bytes · {why}",
                    key.0,
                    key.1,
                    n,
                    thousands(*rest)
                ));
            }
            keep
        })
        .map(|((op, cmd), (n, rest, before))| (op, cmd, n, rest, before))
        .collect();
    out.sort_by_key(|b| std::cmp::Reverse(b.3));
    probe_rows.sort();
    (out, rolled_up_after, probe_after, probe_rows)
}

fn thousands(n: u64) -> String {
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

#[cfg(test)]
mod tests {

    /// A ledger where most of the residue names NO session: neither a
    /// rollup, nor a session's work — unknown. The view counted it
    /// as a session named `?` and refused the target on that basis.
    fn a_ledger_mostly_sessionless() -> String {
        [
            // `auto:json` and not `web-search`: the latter is now
            // set aside for ANOTHER reason (its baseline is a multiplier, not
            // a reading), and the fixture would then talk about two rules at
            // once in a test that questions only one.
            r#"{"op":"auto:json","cmd":"search","before":30000,"after":9000,"ts":86400}"#,
            r#"{"op":"auto:json","cmd":"search","before":30000,"after":9000,"ts":172800}"#,
            r#"{"op":"auto:json","cmd":"search","before":4000,"after":1000,"ts":172800,"sess":"s1"}"#,
        ]
        .join("\n")
    }

    /// A row without `sess` is attributable to NOBODY. MEASURED 2026-09-07:
    /// `auto:json` — the first actionable row of the ranking — has 601,667 of
    /// its 624,430 bytes (95 %) on rows without a session, and `--focus`
    /// answered "a single session carries 95 % of the residue, the ranking
    /// sets it aside" on the row that appears in that very table.
    #[test]
    fn a_row_without_a_session_belongs_to_nobody() {
        let rollup = serde_json::json!({"rollup": true, "sess": "s1", "after": 10});
        assert_eq!(residue_owner(&rollup), None, "a bucket names no session");
        let bare = serde_json::json!({"after": 10});
        assert_eq!(residue_owner(&bare), None, "no sess field: nobody");
        let owned = serde_json::json!({"after": 10, "sess": "s1"});
        assert_eq!(residue_owner(&owned), Some("s1"));
    }

    /// The SITE: `focus` must neither invent the session, nor declare it
    /// concentrated on bytes nobody carries.
    #[test]
    fn focus_does_not_invent_a_session_for_rows_that_carry_none() {
        let (rows, rolled_after, ..) = focus_scan(&a_ledger_mostly_sessionless(), "search");
        assert_eq!(rows.len(), 3);
        let (by_sess, sessionless, c) = SessionConcentration::of_focus_rows(&rows, rolled_after);
        assert_eq!(sessionless, 18_000, "two rows out of three name nobody");
        assert_eq!(
            by_sess.keys().collect::<Vec<_>>(),
            vec!["s1"],
            "no invented session must appear here"
        );
        assert_eq!(by_sess["s1"], 1_000);
        assert!(
            c.coverage < 0.5,
            "5 % attributable cannot condemn the whole: {:.2}",
            c.coverage
        );
        assert!(
            self_probe_reason(0.50, c, rows.len() as u64).is_none(),
            "no proof: it is not a proof of concentration"
        );
    }

    /// The other site, on the SAME ledger: the table already counts these bytes
    /// as unattributed, and the two views must therefore agree.
    #[test]
    fn the_table_and_focus_agree_on_who_carries_nothing() {
        let (rows, _, probe_after, why) = aggregate_by_cmd(&a_ledger_mostly_sessionless());
        assert_eq!(
            rows.len(),
            1,
            "wrongly set aside: {why:?} ({probe_after} bytes)"
        );
        assert_eq!(rows[0].3, 19_000);
    }

    /// The ranking held its TWO factors and never divided them.
    /// MEASURED: `sysinfo` is 3rd in the pool (1,608,905 bytes) and 59,307
    /// calls — 27 bytes per call, versus 10,445 for `npm` just below.
    #[test]
    fn the_ranking_says_what_is_left_on_one_call() {
        assert_eq!(residue_per_call(1_608_905, 59_307), Some(27));
        assert_eq!(residue_per_call(1_044_587, 100), Some(10_445));
        // Zero calls: nothing to divide, and above all nothing to CLAIM.
        assert_eq!(residue_per_call(1_000, 0), None);
    }

    /// The floor is measured on real folds, it is not chosen — and it
    /// is worth 27 bytes, exactly `sysinfo`'s residue per call.
    #[test]
    fn the_floor_comes_from_the_corpus_not_from_a_constant() {
        let f = tokenade_bench::announcement_cost().expect("measurable banners");
        assert!(f > 0);
        // At EQUALITY the gain is exactly zero: the bound is `<=`, not `<`.
        assert!(
            !has_headroom(Some(f), Some(f)),
            "at equality, folding returns nothing"
        );
        assert!(
            !has_headroom(Some(f - 1), Some(f)),
            "below it, folding lengthens"
        );
        assert!(has_headroom(Some(f + 1), Some(f)));
        // Without a measured floor, claim nothing: no row is marked.
        assert!(has_headroom(Some(1), None));
        assert!(has_headroom(None, Some(f)));
    }

    /// The synthetic ledger of `webfetch_compact` in miniature: most
    /// of the residue in a NAMED rollup bucket, a single session row
    /// opposite. Two days, so that the DAY rule does not decide in
    /// place of the session one.
    fn a_ledger_mostly_in_rollup() -> String {
        [
            r#"{"op":"web","cmd":"webfetch_compact","rollup":true,"rollup_calls":60,"before":200000,"after":50000,"ts":86400}"#,
            r#"{"op":"web","cmd":"webfetch_compact","rollup":true,"rollup_calls":60,"before":200000,"after":50000,"ts":172800}"#,
            r#"{"op":"web","cmd":"webfetch_compact","before":4000,"after":1000,"ts":172800,"sess":"s1"}"#,
        ]
        .join("\n")
    }

    /// The coverage can no longer be asserted by a site: it is MEASURED,
    /// and the only constructor requires what no session carries.
    #[test]
    fn a_concentration_measures_what_no_session_carries() {
        let mut per_sess = std::collections::BTreeMap::new();
        per_sess.insert("s1".to_string(), 1_000u64);
        let c = SessionConcentration::measured(&per_sess, 100_000);
        assert_eq!(
            c.share, 1.0,
            "a single session carries everything attributable"
        );
        assert!(
            c.coverage < 0.02,
            "and that is only {:.4} of the residue",
            c.coverage
        );
        // Without rollup, the view sees everything: the coverage is full.
        assert_eq!(SessionConcentration::measured(&per_sess, 0).coverage, 1.0);
    }

    /// The SITE, not just the helper: the table declared a probe any op one of whose
    /// sessions carries 80 % of the rows, even when those rows are 1 % of the bytes.
    #[test]
    fn the_table_does_not_discard_an_op_it_can_barely_see() {
        let (rows, _, probe_after, why) = aggregate_by_cmd(&a_ledger_mostly_in_rollup());
        assert_eq!(
            rows.len(),
            1,
            "set aside on 1 % of its bytes: {why:?} ({probe_after} bytes)"
        );
        assert_eq!(rows[0].1, "webfetch_compact");
        assert_eq!(rows[0].3, 101_000, "the NAMED buckets count in the residue");
    }

    /// The other site. `--focus` answered "no row" on a compactor that
    /// the ranking credits with 232,205 bytes, because it skipped the rollup
    /// buckets and rendered that absence as emptiness.
    #[test]
    fn focus_counts_the_rollup_it_cannot_detail() {
        let (rows, rolled_after, rolled_calls, rolled_rows, rolled_by_day) =
            focus_scan(&a_ledger_mostly_in_rollup(), "webfetch_compact");
        assert_eq!(rows.len(), 1);
        assert_eq!((rolled_rows, rolled_after, rolled_calls), (2, 100_000, 120));
        // And the DAYS map carries the buckets, otherwise the day rule
        // judges on 1 % of the bytes and returns "100 % of the residue".
        assert_eq!(rolled_by_day.len(), 2);
        assert_eq!(rolled_by_day.values().sum::<u64>(), 100_000);
        let day_share = top_share(&rolled_by_day);
        assert!(day_share < 0.95, "two days: {day_share:.2}");
        let mut by_sess = std::collections::BTreeMap::new();
        by_sess.insert("s1".to_string(), rows[0].1);
        let c = SessionConcentration::measured(&by_sess, rolled_after);
        assert!(
            self_probe_reason(0.505, c, rows.len() as u64).is_none(),
            "a view that sees only 1 % of the bytes has nothing to declare"
        );
    }
    use super::*;

    /// The sort that sent the first iteration onto a 0.2 % target.
    /// A SILENT removal is half of the defect. The list of `op`s
    /// set aside lived as 125 bare names in the document that steers every
    /// iteration: impossible to check a single exclusion there, and that is what
    /// made a mere comparison between two counts pass for a
    /// disagreement.
    #[test]
    fn an_exclusion_carries_its_evidence_or_it_is_not_rendered() {
        let out = render_excluded("Set aside", &["`x` — 2 uses, 3 bytes".to_string()]);
        assert!(
            out.contains("Set aside (1)"),
            "the count must be stated: {out}"
        );
        assert!(
            out.contains("`x` — 2 uses, 3 bytes"),
            "the proof must be there: {out}"
        );
        // Nothing to say, nothing to render: an empty section would be noise.
        assert_eq!(render_excluded("Set aside", &[]), "");
    }

    /// The share of the biggest contributor, once for the three views.
    #[test]
    fn the_top_share_is_the_biggest_over_the_whole() {
        let m: std::collections::BTreeMap<&str, u64> =
            [("a", 8u64), ("b", 1), ("c", 1)].into_iter().collect();
        assert!((top_share(&m) - 0.8).abs() < 1e-9);
        let empty: std::collections::BTreeMap<&str, u64> = Default::default();
        assert_eq!(
            top_share(&empty),
            0.0,
            "dividing by nothing does not return 1"
        );
        let zeros: std::collections::BTreeMap<&str, u64> =
            [("a", 0u64), ("b", 0)].into_iter().collect();
        assert_eq!(top_share(&zeros), 0.0);
    }

    #[test]
    fn what_is_still_delivered_is_volume_times_inefficiency() {
        // Frequent but tiny: 59,307 calls of 65 bytes.
        assert_eq!(still_delivered(3_879_492, 2_270_587), 1_608_905);
        // Rare but huge: beats the previous one despite an excellent rate.
        assert!(still_delivered(430_000_000, 408_000_000) > still_delivered(3_879_492, 2_270_587));
        // A `saved` inconsistent with `before` must not overflow.
        assert_eq!(still_delivered(10, 99), 0);
    }

    /// The reason DISPLAYED must be the one that decided.
    ///
    /// The first version always talked about the session, and printed "a
    /// single session carries 13 %" on `read_code_strip_lite`, set aside by the
    /// DAY rule (100 % over 8 uses). An instrument that explains its decision badly
    /// is rightly disobeyed — and 13 % disqualifies nothing.
    /// The COVERAGE is the denominator that makes a session share honest,
    /// and it was computed at two places with different answers.
    ///
    /// `--focus` set it to one, on the belief that its rows are all
    /// attributable: true of the ROWS, false of the OP it talks about. MEASURED —
    /// `webfetch_compact` has **489,873 of its 491,215 bytes** in rollup
    /// buckets, and the view gave its probe verdict on the remaining 1,342,
    /// i.e. 0.3 % of the bytes.
    #[test]
    fn coverage_counts_what_no_session_carries() {
        assert_eq!(
            coverage_of(0, 0),
            0.0,
            "dividing by nothing does not return 1"
        );
        assert_eq!(coverage_of(10, 10), 1.0);
        assert_eq!(
            coverage_of(0, 100),
            0.0,
            "all in rollup: nothing attributable"
        );
        assert!((coverage_of(1_342, 491_215) - 0.0027).abs() < 1e-3);

        // And the rule uses it: below half attributable, the session
        // clause can no longer condemn, whatever its share.
        let thin = SessionConcentration {
            share: 1.0,
            coverage: coverage_of(1_342, 491_215),
            last_day: 0,
            rolled_last_day: 0,
            rolled_share: 0.0,
        };
        assert!(
            self_probe_reason(0.40, thin, 199).is_none(),
            "condemned on 0.3 % of its bytes"
        );
        let full = SessionConcentration {
            share: 1.0,
            coverage: coverage_of(400, 500),
            last_day: 0,
            rolled_last_day: 0,
            rolled_share: 0.0,
        };
        assert!(self_probe_reason(0.40, full, 199).is_some());
    }

    #[test]
    fn the_verdict_names_the_clause_that_fired() {
        let none = SessionConcentration::default();
        let mine = SessionConcentration {
            share: 0.90,
            coverage: 0.9,
            last_day: 0,
            rolled_last_day: 0,
            rolled_share: 0.0,
        };

        // Day + few calls: it is the day that must be read.
        let why = self_probe_reason(1.0, none, 8).expect("set aside");
        assert!(why.contains("day"), "{why}");
        assert!(
            why.contains("8 uses"),
            "the number of calls is part of the reason: {why}"
        );
        assert!(
            !why.contains("session"),
            "reason borrowed from the other clause: {why}"
        );

        // Day alone, very concentrated: the reason no longer talks about calls,
        // because this clause does not look at them.
        let why = self_probe_reason(0.99, none, 4_000).expect("set aside");
        assert!(why.contains("day") && !why.contains("uses"), "{why}");

        // Session: the reason states the share AND its coverage, otherwise it
        // would claim more than the measurement supports.
        let why = self_probe_reason(0.40, mine, 100).expect("set aside");
        assert!(why.contains("session"), "{why}");
        assert!(
            why.contains("90 %") && why.contains("attributable"),
            "{why}"
        );

        // And nothing to say when nothing decides.
        assert!(self_probe_reason(
            0.40,
            SessionConcentration {
                share: 0.35,
                coverage: 0.9,
                last_day: 0,
                rolled_last_day: 0,
                rolled_share: 0.0
            },
            100
        )
        .is_none());
    }

    #[test]
    fn a_one_day_burst_of_a_few_calls_is_my_own_probe() {
        // No known session: it is the day that must decide, as
        // before, otherwise hardening on sessions would lose the old rule.
        let none = SessionConcentration::default();
        assert!(
            is_self_probe(1.0, none, 5),
            "5 calls on the same day: a probe"
        );
        assert!(
            !is_self_probe(0.61, none, 12),
            "few calls but spread over several days: not a probe"
        );
        // The number of calls does not protect: what we rank are
        // BYTES. MEASURED 2026-09-07 — `auto:html`, 409 uses, 4,694,843
        // bytes still delivered, FIRST row of the per-compactor table, and
        // 98 % of those bytes written by the current session in the
        // preceding minutes. The rule "concentrated AND rare" let
        // exactly that through.
        assert!(
            is_self_probe(0.98, none, 409),
            "almost all the residue on one day is my exhaust, however many the calls"
        );
        // And truly spread usage stays usage, whatever the volume:
        // it is the half that must not be carried away when hardening.
        assert!(
            !is_self_probe(0.72, none, 4162),
            "a residue spread over several days is usage, not a probe"
        );
    }

    /// The SESSION judges better than the day — a day carries real
    /// work next to a probe — and the `--focus` view had this data
    /// in front of it without ever using it: it printed "session
    /// 447cf2a1 · 83 %" and let the reader conclude.
    ///
    /// But the session share means nothing without its COVERAGE. A rollup
    /// bucket carries no session's name, so on `auto-compact`
    /// only 6.5 MB of the 18.5 MB still delivered are attributable: concluding
    /// "it is my exhaust" of the 18.5 because 83 % of the 6.5 is
    /// is precisely the over-claim this guard must prevent.
    #[test]
    fn a_session_that_wrote_almost_everything_is_my_own_probe() {
        let mine = SessionConcentration {
            share: 0.83,
            coverage: 1.0,
            last_day: 0,
            rolled_last_day: 0,
            rolled_share: 0.0,
        };
        assert!(
            is_self_probe(0.40, mine, 4164),
            "a session writes 83 % of the attributable residue: it is my measurement"
        );
        // Spread over several sessions: usage, even if very concentrated on one
        // working day.
        let spread = SessionConcentration {
            share: 0.35,
            coverage: 1.0,
            last_day: 0,
            rolled_last_day: 0,
            rolled_share: 0.0,
        };
        assert!(!is_self_probe(0.40, spread, 4164));
        // The SAME concentration, but computed on a third of the residue: we do not
        // conclude. It is the real case of `auto-compact`, two thirds of which
        // are in rollup buckets that accuse nobody.
        let thin = SessionConcentration {
            share: 0.83,
            coverage: 0.35,
            last_day: 0,
            rolled_last_day: 0,
            rolled_share: 0.0,
        };
        assert!(
            !is_self_probe(0.40, thin, 4164),
            "a share computed on a third of the residue cannot condemn the whole"
        );
        // And a massively used compactor is set aside by no
        // session: `sysinfo` makes 59,307 uses over 103 days.
        assert!(!is_self_probe(0.40, mine, 59_307));
    }

    /// `op` is a family, the compactor is what we fix. The aggregation
    /// must tell them apart, name a row born from CONTENT by its `label`,
    /// set aside the rollup buckets while SAYING how much they weigh, and survive
    /// an unreadable row — a real ledger carries some.
    #[test]
    fn the_aggregation_separates_the_compactor_from_its_family() {
        // `lsblk` is the real case: MANY uses, almost nothing to deliver
        // (59,307 calls of 65 bytes in the real ledger). It must come
        // AFTER `pnpm`, which is called less often and delivers much more. A
        // fixture where both sorts give the same order proves nothing.
        // Each row carries a `ts` from a DIFFERENT day: without it all
        // the residue falls on the same day and the whole table reads as
        // a probe. A fixture without timestamps does not measure this filter, it
        // triggers it.
        const DAY: i64 = 86_400;
        let mut lines: Vec<String> = (0..10)
            .map(|i| {
                format!(
                    r#"{{"op":"sysinfo","cmd":"lsblk","ts":{},"before":{},"after":{}}}"#,
                    DAY * (i + 1),
                    30 + i,
                    20 + i
                )
            })
            .collect();
        lines.push(format!(
            r#"{{"op":"npm","cmd":"pnpm","ts":{},"before":1000,"after":200}}"#,
            DAY
        ));
        lines.push(format!(
            r#"{{"op":"npm","cmd":"pnpm","ts":{},"before":1000,"after":300}}"#,
            DAY * 2
        ));
        // A row born from CONTENT has no `cmd`: it carries its
        // compactor under `label`. It is the shape of `auto-compact`, second
        // family of the ranking — 3,315 rows, 35 % of everything still
        // delivered — that "no `cmd`, no entry" made disappear
        // ENTIRELY from a table whose reason for being is to name the compactor.
        lines.push(format!(
            r#"{{"op":"auto-compact","label":"auto:html","ts":{},"before":2000,"after":350}}"#,
            DAY * 3
        ));
        lines.push(format!(
            r#"{{"op":"auto-compact","label":"auto:html","ts":{},"before":2000,"after":350}}"#,
            DAY * 4
        ));
        // A rollup bucket names no compactor and is worth thousands
        // of calls: counted here, it would occupy the first row with a
        // "1 use".
        lines.push(
            r#"{"op":"html-to-markdown","label":"(rolled-up)","rollup":true,"rollup_calls":6524,"before":90000,"after":9000}"#
                .to_string(),
        );
        // Neither one nor the other: nothing to name, nothing to display.
        lines.push(r#"{"op":"sysinfo","cmd":"","before":900,"after":100}"#.to_string());
        lines.push("pas du json".to_string());
        let (rows, rolled_up, _probes, _why) = aggregate_by_cmd(&lines.join("\n"));
        assert_eq!(
            rows.len(),
            3,
            "a row with no name at all does not make an entry: {rows:?}"
        );
        assert_eq!(
            rolled_up, 9000,
            "the set-aside rollup must be QUANTIFIED, not hushed"
        );
        assert!(
            !rows.iter().any(|r| r.1 == "(rolled-up)"),
            "a rollup bucket slipped into the table: {rows:?}"
        );
        // Sorted by what is STILL DELIVERED — not by uses: `lsblk` has five
        // times more than `pnpm` and still comes after.
        assert_eq!(
            rows[0],
            ("auto-compact".into(), "auto:html".into(), 2, 700, 4000),
            "the row named by its `label` must be there, and a single use that \
             delivers 700 goes ahead of two uses that deliver 500"
        );
        assert_eq!(rows[1], ("npm".into(), "pnpm".into(), 2, 500, 2000));
        assert_eq!(
            rows[1].2 * 5,
            rows[2].2,
            "the fixture must oppose the two sorts"
        );
        assert_eq!(rows[2].1, "lsblk");
    }

    /// The ranking must not send me onto my own exhaust.
    ///
    /// MEASURED 2026-09-07: `auto:html` occupied the FIRST row of the
    /// per-compactor table — 409 uses, 4,694,843 bytes still delivered — and 391 of
    /// those rows had been written by the current session in the
    /// preceding minutes, while measuring. 98 % of the residue on a single day.
    #[test]
    fn a_compactor_whose_residue_lands_on_one_day_is_set_aside_and_counted() {
        const DAY: i64 = 86_400;
        let mut lines: Vec<String> = (0..60)
            .map(|i| {
                format!(
                    r#"{{"op":"auto-compact","label":"auto:probe","ts":{},"before":20000,"after":10000}}"#,
                    DAY * 900 + i
                )
            })
            .collect();
        // The same compactor did serve on other days — but so little that
        // the measuring day takes everything. That is the real shape.
        lines.push(format!(
            r#"{{"op":"auto-compact","label":"auto:probe","ts":{},"before":200,"after":100}}"#,
            DAY * 400
        ));
        // A REAL, spread neighbour, which must survive the filter: without it, a
        // rule that sets everything aside would pass this test.
        for d in 0..5 {
            lines.push(format!(
                r#"{{"op":"npm","cmd":"pnpm","ts":{},"before":9000,"after":2000}}"#,
                DAY * (500 + d)
            ));
        }
        let (rows, _rolled, probe_after, _why) = aggregate_by_cmd(&lines.join("\n"));
        assert!(
            !rows.iter().any(|r| r.1 == "auto:probe"),
            "the concentrated compactor stayed in the table: {rows:?}"
        );
        assert_eq!(
            probe_after, 600_100,
            "the bytes set aside must be QUANTIFIED — otherwise they vanish silently"
        );
        assert_eq!(
            rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(),
            vec!["pnpm"],
            "the spread neighbour must survive the filter"
        );
    }

    /// A rollup bucket that KNOWS whom it belongs to enters the table,
    /// with its real cardinality; an anonymous bucket stays out and its weight
    /// is quantified. Otherwise 84 % of the pool — MEASURED 2026-09-07:
    /// 44,411,043 bytes out of 52,690,500 — belongs to nobody.
    #[test]
    fn an_attributed_rollup_bucket_enters_the_table_with_its_real_call_count() {
        const DAY: i64 = 86_400;
        let mut lines = vec![
            format!(
                r#"{{"op":"auto-compact","label":"(rolled-up)","compactor":"auto:ini","rollup":true,"rollup_calls":209,"ts":{},"before":50000,"after":26000}}"#,
                DAY * 100
            ),
            format!(
                r#"{{"op":"auto-compact","label":"(rolled-up)","compactor":"auto:ini","rollup":true,"rollup_calls":100,"ts":{},"before":50000,"after":26000}}"#,
                DAY * 101
            ),
            // A bucket from before the field: anonymous, so outside.
            format!(
                r#"{{"op":"sysinfo","label":"(rolled-up)","rollup":true,"rollup_calls":900,"ts":{},"before":9000,"after":4000}}"#,
                DAY * 102
            ),
        ];
        lines.push(format!(
            r#"{{"op":"auto-compact","cmd":"pnpm","ts":{},"before":100,"after":10}}"#,
            DAY * 103
        ));
        let (rows, rolled_up, _probes, _why) = aggregate_by_cmd(&lines.join("\n"));
        assert_eq!(
            rolled_up, 4000,
            "the anonymous bucket must stay quantified outside"
        );
        let ini = rows
            .iter()
            .find(|r| r.1 == "auto:ini")
            .expect("the attributed bucket must enter the table");
        assert_eq!(
            ini.2, 309,
            "a bucket is worth `rollup_calls` calls, not one"
        );
        assert_eq!(ini.3, 52_000);
        assert!(
            !rows.iter().any(|r| r.1 == "(rolled-up)"),
            "the sentinel is nobody's name: {rows:?}"
        );
    }

    /// `--focus` must reach both ways of naming a compactor.
    #[test]
    fn focus_reaches_a_compactor_named_by_its_op() {
        // CLI path: the name is in `label`.
        let cli: serde_json::Value =
            serde_json::from_str(r#"{"op":"auto-compact","label":"auto:html"}"#).unwrap();
        assert!(focus_matches(&cli, "auto:html"));
        // HOOK path: the name is the `op`, and `label` carries the command.
        let hook: serde_json::Value =
            serde_json::from_str(r#"{"op":"auto:ini","label":"Bash: cat /etc/hosts"}"#).unwrap();
        assert!(
            focus_matches(&hook, "auto:ini"),
            "the compactor named by its op remains unreachable"
        );
        // And it must not answer for the neighbour.
        assert!(!focus_matches(&hook, "auto:html"));
        assert!(!focus_matches(&cli, "auto-compact-not-a-name"));
    }

    #[test]
    fn thousands_groups_from_the_right() {
        assert_eq!(thousands(1), "1");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1 000");
        assert_eq!(thousands(52_688_864), "52 688 864");
    }

    /// A row whose rate comes from the published ratio is not a target: there
    /// is no input, the answer IS the output. An iteration lost its
    /// opening looking for a fold in `structure_map`, 13th in the ranking at
    /// −56 % — a number that is the constant itself.
    #[test]
    fn a_borrowed_baseline_is_excluded_and_names_its_share() {
        // `borrowed`: all borrowed.
        let e = borrowed_reason(Some("borrowed"), "structure_map", 1_326, 1_326, 1_000, 4)
            .expect("set aside");
        assert_eq!(e.kind, BORROWED);
        assert!(
            e.why.contains("100 %"),
            "the share must be named: {}",
            e.why
        );
        assert!(e.why.contains("code_intel_heuristic"), "{}", e.why);
        // `mixed`: the REAL case of this recording, because the
        // `ci_heuristic` stamp arrived after part of the history. Keeping
        // only `borrowed` would have left `structure_map` in the table.
        let m = borrowed_reason(
            Some("mixed"),
            "structure_map",
            114_439,
            122_569,
            94_818,
            424,
        )
        .expect("set aside");
        assert_eq!(m.kind, BORROWED);
        assert!(
            m.why.contains("93 %"),
            "the measured share must come out: {}",
            m.why
        );
        assert!(m.why.contains("94 818"), "{}", m.why);
    }

    /// And `skeleton` stays: it tokenises the file it is handed, its baseline
    /// is measured. The terminal table partitions ALL the
    /// code-intel ops and therefore loses the measured half of the bucket, the biggest.
    #[test]
    fn a_measured_baseline_stays_in_the_table() {
        assert!(borrowed_reason(Some("measured"), "skeleton", 0, 8_600, 400, 3).is_none());
        // A JSON older than the field does not make the row disappear.
        assert!(borrowed_reason(None, "skeleton", 0, 8_600, 400, 3).is_none());
        // And a zero share wins over a talkative label.
        assert!(borrowed_reason(Some("mixed"), "skeleton", 0, 8_600, 400, 3).is_none());
    }

    /// The reason is CARRIED, not recomputed. The list of set-aside rows
    /// recomputed it, with an `unwrap_or("reason lost")` that itself said
    /// what could happen; a second rule would have had to be spelled at the
    /// two places.
    #[test]
    fn the_two_exclusion_rules_share_one_channel() {
        let concentrated = self_probe_reason(1.0, SessionConcentration::default(), 3)
            .map(|why| Excluded {
                kind: CONCENTRATED,
                why,
            })
            .expect("a day at 100 % over 3 uses is set aside");
        assert_eq!(concentrated.kind, CONCENTRATED);
        assert_ne!(concentrated.kind, BORROWED, "the two headings are distinct");
    }

    /// The date decides whether a target is still alive. Civil rendering is the
    /// kind of computation one believes correct: anchored on known days.
    #[test]
    fn a_day_number_renders_as_a_civil_date() {
        assert_eq!(iso_day(0), "1970-01-01");
        assert_eq!(iso_day(19_723), "2024-01-01");
        // A leap year, the day after 29 February.
        assert_eq!(iso_day(19_783), "2024-03-01");
        assert_eq!(iso_day(20_704), "2026-09-08");
    }

    /// **The reason names the constant that decided.** The first version returned the
    /// code-intel ratio for any row set aside; `web-search`, the first one the
    /// widened rule gained, was rendered under that reason although its baseline
    /// is an entirely different multiplier.
    #[test]
    fn the_reason_names_the_constant_that_decided() {
        let ci = borrowed_reason(Some("mixed"), "structure_map", 90, 100, 1_000, 4)
            .expect("set aside")
            .why;
        assert!(ci.contains("CodeGraph"), "{ci}");
        assert_eq!(
            fabricated_from("structure_map"),
            fabricated_from("skeleton")
        );
        assert_ne!(
            fabricated_from("structure_map"),
            fabricated_from("web-search")
        );
        assert!(!ci.contains("SERP"), "{ci}");
        let web = borrowed_reason(Some("mixed"), "web-search", 100, 100, 1_000, 4)
            .expect("set aside")
            .why;
        let web_reason = fabricated_from("web-search");
        // The GENERIC reason also contains "native multiplier": looking
        // for it distinguishes nothing, and the mutation that removes the branch
        // survived on that. It is the WHOLE sentence that must be asserted.
        assert_eq!(web_reason, fabricated_from("web-search"));
        assert_ne!(
            web_reason,
            fabricated_from("feed"),
            "the generic one answered"
        );
        assert!(web.contains("SERP"), "{web}");
        assert!(!web.contains("CodeGraph"), "{web}");
        // A stamped row that belongs to neither of the two known families says
        // it comes from a coefficient, without inventing which.
        let other = borrowed_reason(Some("borrowed"), "feed", 100, 100, 1_000, 4)
            .expect("set aside")
            .why;
        assert!(other.contains("turn credit"), "{other}");
        assert!(!other.contains("SERP"), "{other}");
        assert!(!other.contains("CodeGraph"), "{other}");
    }

    /// The second table shows a rate, so it applies the same rule. The
    /// first fix only corrected the first one: `structure_map / map (repo)` stayed
    /// in the compactor table at −57 %, set aside from one table and presented
    /// in the next.
    #[test]
    fn the_compactor_table_excludes_a_fabricated_baseline_too() {
        let ledger = [
            // An ordinary measurement, which stays.
            r#"{"op":"auto:json","compactor":"auto:json","before":9000,"after":400,"ts":1788000000,"sess":"s1"}"#,
            r#"{"op":"auto:json","compactor":"auto:json","before":9000,"after":400,"ts":1788086400,"sess":"s2"}"#,
            // A baseline stamped at write time.
            r#"{"op":"structure_map","compactor":"map (repo)","class":"ci_heuristic","before":2326,"after":1000,"ts":1788000000,"sess":"s1"}"#,
            r#"{"op":"structure_map","compactor":"map (repo)","class":"ci_heuristic","before":2326,"after":1000,"ts":1788086400,"sess":"s2"}"#,
            // And a row PREDATING the stamp, recognised by its op.
            r#"{"op":"web-search","compactor":"search","before":3000,"after":1000,"ts":1788000000,"sess":"s1"}"#,
            r#"{"op":"web-search","compactor":"search","before":3000,"after":1000,"ts":1788086400,"sess":"s2"}"#,
        ]
        .join("\n");
        let (rows, _, _, why) = aggregate_by_cmd(&ledger);
        let ops: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(
            ops,
            vec!["auto:json"],
            "only the measurement stays: {ops:?}"
        );
        assert!(
            why.iter().any(|w| w.contains("structure_map")),
            "the removal must be NAMED: {why:?}"
        );
        assert!(
            why.iter().any(|w| w.contains("web-search")),
            "including for a row without a stamp: {why:?}"
        );
    }

    /// **A dead row is not a target.** The window is `span.days`
    /// (104 on 2026-09-08): `network` was 8th in the pool with 232,205
    /// bytes, and its last event dated from 56 days earlier.
    #[test]
    fn a_line_that_stopped_receiving_is_named_and_dated() {
        let today = 20_704; // 2026-09-08
        let window = 104;
        let idle = |d: i64| SessionConcentration {
            share: 0.1,
            coverage: 1.0,
            last_day: today - d,
            rolled_last_day: 0,
            rolled_share: 0.0,
        };
        // The bound is at THREE days, and it is tested on both sides.
        assert!(
            stale_reason(idle(3), today, window).is_none(),
            "three days stay alive"
        );
        let e = stale_reason(idle(4), today, window).expect("four days, dead");
        assert_eq!(e.kind, STALE);
        assert!(e.why.contains("4 day(s)"), "{}", e.why);
        assert!(e.why.contains(&ranking_covers(window)), "{}", e.why);
        // And the DATE, not only the age: it is what is compared with the date
        // of a fix.
        assert!(e.why.contains(&iso_day(today - 4)), "{}", e.why);
        // A row without a timestamp is not declared dead on an absence of
        // proof: `last_day` is 0 when the ledger carries none.
        assert!(stale_reason(SessionConcentration::default(), today, window).is_none());
    }

    /// **The window written is `span.days`, not 104.** Measured 2026-09-19:
    /// the header said 115 days and every dead row still said 104.
    #[test]
    fn the_stale_copy_names_the_span_not_a_frozen_width() {
        let today = 20_704;
        let idle = SessionConcentration {
            share: 0.1,
            coverage: 1.0,
            last_day: today - 4,
            rolled_last_day: 0,
            rolled_share: 0.0,
        };
        let e104 = stale_reason(idle, today, 104).expect("dead");
        assert!(e104.why.contains("104 days"), "{}", e104.why);
        assert!(!e104.why.contains("115 days"), "{}", e104.why);
        let e115 = stale_reason(idle, today, 115).expect("dead");
        assert!(e115.why.contains("115 days"), "{}", e115.why);
        assert!(!e115.why.contains("104 days"), "{}", e115.why);
        assert_eq!(ranking_covers(115), "the ranking covers 115 days");
    }

    /// **A single spelling of the question, and the reason returned is the one that
    /// DECIDED.** Four rules, arrived one by one, each because an
    /// iteration had gone off on a target that was not one.
    #[test]
    fn the_four_rules_answer_in_one_place_and_the_strongest_wins() {
        let today = 20_704;
        let live = SessionConcentration {
            share: 0.1,
            coverage: 1.0,
            last_day: today,
            rolled_last_day: 0,
            rolled_share: 0.0,
        };
        let dead = SessionConcentration {
            share: 0.1,
            coverage: 1.0,
            last_day: today - 40,
            rolled_last_day: 0,
            rolled_share: 0.0,
        };
        let concentrated = SessionConcentration {
            share: 0.99,
            coverage: 1.0,
            last_day: today - 40,
            rolled_last_day: 0,
            rolled_share: 0.0,
        };
        // Nothing to object: alive, spread, measured, with a named piece.
        assert!(not_a_target(
            "auto:json",
            Some("measured"),
            0,
            900,
            100,
            60,
            live,
            0.3,
            true,
            today,
            104
        )
        .is_none());
        // A fabricated baseline wins over everything: there is nothing to fold.
        let e = not_a_target(
            "web-search",
            Some("mixed"),
            90,
            100,
            100,
            60,
            concentrated,
            0.99,
            false,
            today,
            104,
        )
        .expect("set aside");
        assert_eq!(e.kind, BORROWED, "the most decisive reason must decide");
        // Then the concentration, before staleness: a probe from last
        // week is a probe, not a dead row.
        let e = not_a_target(
            "auto:json",
            Some("measured"),
            0,
            900,
            100,
            3,
            concentrated,
            0.99,
            false,
            today,
            104,
        )
        .expect("set aside");
        assert_eq!(e.kind, CONCENTRATED);
        // Then staleness, before the absence of a piece: a dead row is not
        // explained by its compactors.
        let e = not_a_target(
            "auto:json",
            Some("measured"),
            0,
            900,
            100,
            60,
            dead,
            0.3,
            false,
            today,
            104,
        )
        .expect("set aside");
        assert_eq!(e.kind, STALE);
        assert!(e.why.contains(&ranking_covers(104)), "{}", e.why);
        // And last, the family without a named piece.
        let e = not_a_target(
            "npm",
            Some("measured"),
            0,
            900,
            100,
            60,
            live,
            0.3,
            false,
            today,
            104,
        )
        .expect("set aside");
        assert_eq!(e.kind, HEADLESS);
        assert!(e.why.contains("rollup"), "{}", e.why);
    }

    /// **From when to when, and is it over?** A single spelling, because
    /// three places asked it and the third — the rollup buckets —
    /// did NOT ask it. That is what cost the middle of an iteration:
    /// `html-to-markdown` carries 40 % of the pool in six anonymous buckets, and
    /// nothing said whether they dated from yesterday or from July.
    #[test]
    fn a_span_says_from_when_to_when_and_whether_it_is_over() {
        let today = 20_704; // 2026-09-08
        let live = day_span([today - 3, today - 1, today], today);
        assert_eq!(live, "from 2026-09-05 to 2026-09-08");
        let over = day_span([today - 56, today - 49], today);
        assert_eq!(
            over,
            "from 2026-07-14 to 2026-07-21 — NOTHING for 49 day(s)"
        );
        // An absent day (`0`) is not 1 January 1970: a row without a
        // timestamp must not make the window start at the epoch.
        assert_eq!(day_span([0, today], today), "from 2026-09-08 to 2026-09-08");
        assert_eq!(day_span([], today), "undated");
        assert_eq!(day_span([0, 0], today), "undated");
    }

    /// And the ✖ mark DATES the blindness it signals: naming without dating makes
    /// one look for a fix where there is only history.
    #[test]
    fn the_headless_mark_dates_the_blindness() {
        let today = 20_704;
        let old_rollup = SessionConcentration {
            share: 0.1,
            coverage: 0.02,
            last_day: today,
            rolled_last_day: today - 49,
            rolled_share: 0.0,
        };
        let e = not_a_target(
            "html-to-markdown",
            Some("measured"),
            0,
            900,
            100,
            60,
            old_rollup,
            0.3,
            false,
            today,
            104,
        )
        .expect("set aside");
        assert_eq!(e.kind, HEADLESS);
        assert!(e.why.contains("2026-07-21"), "{}", e.why);
        assert!(e.why.contains("49 day(s)"), "{}", e.why);
        assert!(e.why.contains("HISTORY"), "{}", e.why);
        // Without a dated bucket, the sentence claims nothing.
        let no_rollup = SessionConcentration {
            rolled_last_day: 0,
            ..old_rollup
        };
        let e = not_a_target(
            "npm",
            Some("measured"),
            0,
            900,
            100,
            60,
            no_rollup,
            0.3,
            false,
            today,
            104,
        )
        .expect("set aside");
        assert!(!e.why.contains("HISTORY"), "{}", e.why);
    }

    /// The pass that dates the buckets per `op` is checked on a known ledger.
    #[test]
    fn the_scan_dates_the_rollup_buckets_of_each_op() {
        let ledger = [
            // An ordinary row from today, with its session.
            r#"{"op":"auto:json","after":100,"ts":1788000000,"sess":"s1"}"#,
            // An older ROLLUP bucket: it is what makes a family
            // anonymous, and its DATE says whether the blindness is live or historical.
            r#"{"op":"auto:json","after":900,"ts":1783000000,"rollup":true}"#,
            // And an op with no bucket at all: it must not inherit a date.
            r#"{"op":"grep","after":50,"ts":1788000000,"sess":"s2"}"#,
        ]
        .join("\n");
        let by_op = shares_from(&ledger);
        let j = by_op.get("auto:json").expect("auto:json");
        assert_eq!(j.rolled_last_day, 1_783_000_000 / 86_400);
        assert_eq!(
            j.last_day,
            1_788_000_000 / 86_400,
            "the last day stays the most recent"
        );
        assert_eq!(by_op.get("grep").expect("grep").rolled_last_day, 0);
    }

    /// `--focus` and the per-session shares read `after`: the row of a
    /// compactor that a stash of the same call replaced still counted there —
    /// measured 2026-10-02, `--focus auto:json` gave 309,160 bytes and a
    /// "worst event" of 108,078 the model never received, versus
    /// 147,107 in the ranking. A single "delivered", for all views.
    #[test]
    fn focus_and_session_shares_count_what_was_delivered() {
        let ledger = [
            r#"{"op":"auto:json","cmd":"acme-cli","before":118532,"after":108078,"delivered":0,"ts":1788000000,"sess":"s1"}"#,
            r#"{"op":"auto:json","cmd":"acme-cli","before":900,"after":100,"ts":1788000000,"sess":"s1"}"#,
            r#"{"op":"auto:json","cmd":"acme-cli","before":900,"after":100,"ts":1788000000,"sess":"s2"}"#,
        ]
        .join("\n");
        let (rows, ..) = focus_scan(&ledger, "auto:json");
        let delivered: Vec<u64> = rows.iter().map(|r| r.1).collect();
        assert_eq!(delivered, vec![0, 100, 100]);
        // s1 and s2 delivered as much: the phantom must not make s1 the
        // session that "carries" the residue.
        let share = shares_from(&ledger)["auto:json"].share;
        assert!((share - 0.5).abs() < 1e-9, "{share}");
    }

    /// A LIVE row ranked on its history: `sysinfo` 2nd on
    /// 2026-10-02 with 1,576,872 bytes, of which 1,573,894 in buckets from
    /// May–July. `stale_reason` does not see it (an event today).
    #[test]
    fn a_live_line_weighed_by_its_old_rollups_is_history() {
        let today = 20_728;
        let ledger = [
            r#"{"op":"sysinfo","rollup":true,"after":1573894,"ts":1784505600}"#.to_string(),
            format!(
                r#"{{"op":"sysinfo","after":2978,"ts":{},"sess":"s1"}}"#,
                today * 86_400
            ),
        ]
        .join("\n");
        let sess = shares_from(&ledger)["sysinfo"];
        assert!(sess.rolled_share > 0.99, "{}", sess.rolled_share);
        let e = not_a_target(
            "sysinfo",
            Some("measured"),
            0,
            1,
            1_576_872,
            59_276,
            sess,
            0.1,
            true,
            today,
            128,
        )
        .expect("history");
        assert_eq!(e.kind, HISTORY);
        assert!(e.why.contains("2978 bytes"), "{}", e.why);
        // Half alive: a real target.
        let half = SessionConcentration {
            rolled_share: 0.5,
            ..sess
        };
        assert!(history_reason(half, 1_000, today).is_none());
        // Recent buckets: not history yet.
        let fresh = SessionConcentration {
            rolled_last_day: today - 1,
            ..sess
        };
        assert!(history_reason(fresh, 1_000, today).is_none());
    }
}
