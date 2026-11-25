//! TAP (Test Anything Protocol) — what `node --test` writes when its output
//! is not a terminal, and `--reporter tap` everywhere.
//!
//! ```text
//! TAP version 13
//! # Subtest: t0
//! ok 1 - t0
//!   ---
//!   duration_ms: 0.8
//!   ...
//! # Subtest: t5
//! not ok 6 - t5
//!   ---
//!   error: |-
//!     Expected values to be strictly deep-equal:
//!   ...
//! 1..50
//! # pass 49
//! # fail 1
//! ```
//!
//! Nothing read TAP: it went to the pytest compactor, which kept 4 769 of
//! 5 037 bytes on a real 50-test run (2026-10-03), and the shape fold that
//! then beat it on size regrouped the failure's YAML BY SHAPE — `a: 1` of
//! `actual` moved under `expected`, both arrays merged into one `<N1>: <N2>`
//! line, `actual:` left empty. A passing test is one line of answer; a
//! failing one is the whole block, as printed.

/// `ok 3 - name` / `not ok 6 - name`, possibly indented (nested subtests).
fn test_line(line: &str) -> Option<bool> {
    let t = line.trim_start();
    let (passed, rest) = if let Some(r) = t.strip_prefix("not ok ") {
        (false, r)
    } else {
        let r = t.strip_prefix("ok ")?;
        (true, r)
    };
    rest.starts_with(|c: char| c.is_ascii_digit()).then_some(passed)
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// TAP: a `TAP version` line, or at least three test lines.
pub fn detect(text: &str) -> bool {
    text.lines().take(5).any(|l| l.trim_start().starts_with("TAP version "))
        || text.lines().filter(|l| test_line(l).is_some()).count() >= 3
}

/// A failure's `stack: |-` lists the test's own frame, then the RUNNER's:
/// `Test.runInAsyncScope (node:async_hooks…)`, `Test.run (node:internal/
/// test_runner/test:1047:25)`… — six of seven frames on a real `node --test`
/// failure, the same six under every failing test. The test's frames stay;
/// the runtime's are counted on one line (never all of them: a failure raised
/// inside the runtime keeps its first frame).
fn without_runtime_frames<'a>(block: &[&'a str]) -> Vec<std::borrow::Cow<'a, str>> {
    let mut out: Vec<std::borrow::Cow<'a, str>> = Vec::with_capacity(block.len());
    let mut i = 0;
    while i < block.len() {
        let line = block[i];
        out.push(line.into());
        i += 1;
        if line.trim() != "stack: |-" {
            continue;
        }
        let frames_at = i;
        while i < block.len() && indent(block[i]) > indent(line) && !block[i].trim().is_empty() {
            i += 1;
        }
        let frames = &block[frames_at..i];
        let own: Vec<&str> = frames.iter().copied().filter(|f| !crate::stacktrace::is_runtime_js_frame(f)).collect();
        let kept: Vec<&str> = if own.is_empty() { frames.iter().take(1).copied().collect() } else { own };
        let dropped = frames.len() - kept.len();
        out.extend(kept.into_iter().map(Into::into));
        if dropped > 0 {
            let pad = " ".repeat(indent(frames[0]));
            out.push(format!("{pad}[{dropped} runtime frame(s) not shown]").into());
        }
    }
    out
}

/// TAP read from the OUTPUT alone, whoever ran it — `pnpm test` over
/// `tsx --test`, `npm test`, a Makefile. The argv route above never saw these:
/// a real 81-test `pnpm test:affiliate` (2026-10-05) went to the markdown
/// outline, which took every `# Subtest:` for a heading and the closing
/// `# tests 81` / `# pass 81` for two more. Stricter than [`detect`]: the
/// version line near the top (a script runner prints its own banner first),
/// or test lines AND the `1..N` plan.
pub fn detect_by_content(text: &str) -> bool {
    text.lines().take(10).any(|l| l.trim_start().starts_with("TAP version "))
        || (text.lines().filter(|l| test_line(l).is_some()).count() >= 3
            && text.lines().any(|l| {
                let t = l.trim();
                t.strip_prefix("1..").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            }))
}

/// Passing tests needed before folding them pays for the banner.
const MIN_PASSING: usize = 5;

pub fn compact(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<std::borrow::Cow<str>> = Vec::with_capacity(lines.len());
    let mut pending_subtest: Option<&str> = None;
    let mut passing = 0usize;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with("# Subtest:") {
            if let Some(p) = pending_subtest.replace(line) {
                out.push(p.into());
            }
            i += 1;
            continue;
        }
        let Some(passed) = test_line(line) else {
            if let Some(p) = pending_subtest.take() {
                out.push(p.into());
            }
            out.push(line.into());
            i += 1;
            continue;
        };
        // The block: this line, and the YAML diagnostic under it.
        let mut end = i + 1;
        if lines.get(end).is_some_and(|l| l.trim() == "---" && indent(l) > indent(line)) {
            while end < lines.len() && lines[end].trim() != "..." {
                end += 1;
            }
            end = (end + 1).min(lines.len());
        }
        if passed {
            // A suite reports its own `ok` line once its tests passed: it is
            // not a test. Counting it said "107 passing" over a run whose own
            // summary says `# tests 81` (26 suites, 2026-10-05).
            if !lines[i..end].iter().any(|l| l.trim() == "type: 'suite'") {
                passing += 1;
            }
            pending_subtest = None;
        } else {
            if let Some(p) = pending_subtest.take() {
                out.push(p.into());
            }
            out.extend(without_runtime_frames(&lines[i..end]));
        }
        i = end;
    }
    if let Some(p) = pending_subtest {
        out.push(p.into());
    }
    if passing < MIN_PASSING {
        return None;
    }
    let out = without_empty_scaffolding(out);
    Some(format!(
        "[tokenade:tap] {passing} passing test(s) folded — every failure and the summary as printed\n{}",
        out.join("\n").trim_end()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real failure block survives byte for byte; the passing ones go.
    #[test]
    fn passing_tests_fold_and_the_failure_block_is_kept_as_printed() {
        let mut t = String::from("TAP version 13\n");
        for i in 0..6 {
            t.push_str(&format!("# Subtest: t{i}\nok {} - t{i}\n  ---\n  duration_ms: 0.1\n  ...\n", i + 1));
        }
        let failure = "# Subtest: t6\nnot ok 7 - t6\n  ---\n  duration_ms: 0.9\n  expected:\n    a: 1\n    b:\n      0: 1\n      1: 3\n  actual:\n    a: 1\n    b:\n      0: 1\n      1: 2\n  ...\n";
        t.push_str(failure);
        t.push_str("1..7\n# pass 6\n# fail 1\n");
        let out = compact(&t).unwrap();
        assert_eq!(
            out,
            format!("[tokenade:tap] 6 passing test(s) folded — every failure and the summary as printed\nTAP version 13\n{failure}1..7\n# pass 6\n# fail 1")
        );
    }

    #[test]
    fn detects_node_tap() {
        assert!(detect("TAP version 13\n# Subtest: a\nok 1 - a\n"));
        assert!(!detect("collected 3 items\n...\n"));
    }

    /// A real `node --test` failure: the test's frame stays, the runner's six go.
    #[test]
    fn a_failure_keeps_its_own_frames_and_counts_the_runtime_ones() {
        let block = [
            "not ok 151 - case 150",
            "  ---",
            "  stack: |-",
            "    TestContext.<anonymous> (x.test.js:154:33)",
            "    Test.runInAsyncScope (node:async_hooks:214:14)",
            "    Test.run (node:internal/test_runner/test:1047:25)",
            "    async Test.processPendingSubtests (node:internal/test_runner/test:744:7)",
            "  ...",
        ];
        let out: Vec<String> = without_runtime_frames(&block).into_iter().map(|c| c.into_owned()).collect();
        assert_eq!(
            out,
            ["not ok 151 - case 150", "  ---", "  stack: |-", "    TestContext.<anonymous> (x.test.js:154:33)", "    [3 runtime frame(s) not shown]", "  ..."]
        );
        // Raised inside the runtime: its first frame is kept, not nothing.
        let inner = ["  stack: |-", "    Test.run (node:internal/test_runner/test:1047:25)", "    node:internal/process/task_queues:95:5", "  ..."];
        let out: Vec<String> = without_runtime_frames(&inner).into_iter().map(|c| c.into_owned()).collect();
        assert_eq!(out[1], "    Test.run (node:internal/test_runner/test:1047:25)");
        assert_eq!(out[2], "    [1 runtime frame(s) not shown]");
    }
}

#[cfg(test)]
mod suites {
    /// The banner counts what the run's own summary counts: tests, not the
    /// suites that group them (a real `tsx --test` run: 81 tests, 26 suites).
    #[test]
    fn a_passing_suite_is_not_counted_as_a_test() {
        let text = include_str!("../../../../tests/fixtures/linux/pnpm_tsx_tap.txt");
        assert!(super::detect_by_content(text));
        let out = super::compact(text).unwrap();
        assert!(out.starts_with("[tokenade:tap] 81 passing test(s)"), "{}", &out[..120]);
        assert!(out.contains("# tests 81"));
        // A green run leaves no empty suite header and no nested plan.
        assert!(!out.contains("# Subtest:"), "{out}");
        assert!(!out.lines().any(|l| l.starts_with(" ") && l.trim().starts_with("1..")), "{out}");
        assert!(out.lines().any(|l| l == "1..26"), "{out}");
    }

    /// A failure inside a suite keeps the headers that lead to it.
    #[test]
    fn a_failure_keeps_the_headers_that_lead_to_it() {
        let mut t = String::from("TAP version 13\n");
        for s in 0..3 {
            t.push_str(&format!("# Subtest: suite{s}\n"));
            for i in 0..3 {
                let ok = if s == 1 && i == 2 { "not ok" } else { "ok" };
                t.push_str(&format!("    # Subtest: t{s}{i}\n    {ok} {} - t{s}{i}\n      ---\n      duration_ms: 0.1\n      ...\n", i + 1));
            }
            let ok = if s == 1 { "not ok" } else { "ok" };
            t.push_str(&format!("    1..3\n{ok} {} - suite{s}\n  ---\n  duration_ms: 0.4\n  type: \x27suite\x27\n  ...\n", s + 1));
        }
        t.push_str("1..3\n# tests 9\n# pass 8\n# fail 1\n");
        let out = super::compact(&t).unwrap();
        for kept in ["# Subtest: suite1", "    # Subtest: t12", "    not ok 3 - t12", "not ok 2 - suite1", "# fail 1"] {
            assert!(out.lines().any(|l| l == kept), "{kept}: {out}");
        }
        assert!(!out.contains("suite0") && !out.contains("t00"), "{out}");
    }
}

/// What folding the passing tests leaves behind with nothing under it: the
/// nested `1..N` plan of each suite and the `# Subtest:` header of a suite
/// that passed. On a real 81-test run that was 52 lines out of 75 kept
/// (2026-10-05). A header stays when what follows belongs to it — the
/// failing test at its own indent, or deeper lines; the top-level plan and
/// the `# tests` summary are never touched.
fn without_empty_scaffolding(lines: Vec<std::borrow::Cow<'_, str>>) -> Vec<std::borrow::Cow<'_, str>> {
    let is_plan = |l: &str| l.trim().strip_prefix("1..").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let lines: Vec<_> = lines.into_iter().filter(|l| !(indent(l) > 0 && is_plan(l))).collect();
    // Backwards, so a suite whose children all went is seen empty in turn.
    let mut kept: Vec<std::borrow::Cow<'_, str>> = Vec::with_capacity(lines.len());
    for line in lines.into_iter().rev() {
        if line.trim_start().starts_with("# Subtest:") {
            let owns_next = kept.last().is_some_and(|next| {
                let (ni, li) = (indent(next), indent(&line));
                ni > li || (ni == li && test_line(next).is_some())
            });
            if !owns_next {
                continue;
            }
        }
        kept.push(line);
    }
    kept.reverse();
    kept
}
