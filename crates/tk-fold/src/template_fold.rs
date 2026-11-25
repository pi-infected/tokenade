//! Fold lines that are the SAME sentence with one value changed.
//!
//! Byte-equality folding (`<line> (×N)`) is the honest floor, and on real
//! output it almost never fires: a build log does not repeat a line, it repeats
//! a *sentence* — one warning per route, one per file, one per module — with a
//! path or an identifier swapped in. Measured on a real Next.js build: 0.1%
//! saved by exact folding, because no two lines were ever byte-equal.
//!
//! The rule here is deliberately narrow, because the failure mode of template
//! folding is severe: "retained for 400 days" and "retained for 30 days" share
//! a template and mean opposite things. So the varying values are never
//! dropped, only the repeated sentence is:
//!
//! ```text
//!  ⚠ cannot resolve `{}` in "{}"  (×24)
//!    dynamic · /api/a/route
//!    runtime · /api/a/route
//! ```
//!
//! Every value, and every value PAIRING, survives in source order. What is
//! saved is the sentence said 23 extra times.

/// Values are what changes between two instances of the same sentence: quoted
/// strings, backticked identifiers, paths, and numbers.
fn slot_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    // A MIXED-CASE identifier — a name given by a machine.
    //
    // Two forms, and both are needed: a lowercase letter followed later by an
    // UPPERCASE one (`AppleACPIPlatformExpert`), or at least two capitals
    // followed by a lowercase letter (`IORegistryEntry`, `IOPMrootDomain`). A
    // first version required regular humps `[A-Z][a-z0-9]+` and
    // missed precisely these names, which carry acronyms.
    //
    // A hexadecimal literal (`0x100000114`) is a value too, and
    // it is the one that told each `ioreg` node apart from its neighbour: without
    // it, two otherwise identical lines stayed two templates.
    //
    // Both bounds matter: `Unknown` and `Services` have no INTERNAL
    // capital, `PID` and `COMM` have no lowercase. A table header does not
    // become a value, and neither does a word of prose.
    //
    // MEASURED 2026-09-09: `ioreg` carries **42 % repeated text** (1,593 of
    // 3,769 bytes, seven words present on more than four lines out of five) and
    // did not fold by a single byte, because the node name and its class
    // were recognised as values by nothing.
    //
    // The fold keeps ALL values: widening the alphabet cannot make
    // information disappear, only group two sentences — and the
    // template must be identical everywhere else for them to be grouped.
    RE.get_or_init(|| regex::Regex::new(&format!(r#"("[^"]*"|'[^']*'|`[^`]*`|(?:\./|\.\./|/)[^\s,;:"'`)\]]+|{}|\b(?:[A-Za-z][a-z0-9]*[a-z][A-Za-z0-9]*[A-Z][A-Za-z0-9]*|[A-Z]{{2,}}[a-z][A-Za-z0-9]*)\b|\b\d+(?:\.\d+)*(?:ms|s|kB|MB|GB|%)?\b)"#, crate::regexes::HEX_LITERAL)).expect("static pattern"))
}

/// The template a line belongs to, plus the values it filled in.
fn shape(line: &str) -> (String, Vec<String>) {
    let mut key = String::with_capacity(line.len());
    let mut values = Vec::new();
    let mut last = 0;
    for m in slot_re().find_iter(line) {
        key.push_str(&line[last..m.start()]);
        key.push('\u{1}');
        values.push(m.as_str().to_string());
        last = m.end();
    }
    key.push_str(&line[last..]);
    (key, values)
}

/// Minimum repeats before folding is worth the explanation it costs.
const MIN_RUN: usize = 3;
/// A template shorter than this saves nothing once the values are re-listed —
/// and short lines are where a false grouping would be hardest to notice.
const MIN_TEMPLATE_CHARS: usize = 30;

/// Fold repeated sentences, keeping every value. Returns the text unchanged
/// when nothing repeats enough to pay for itself.
pub fn fold_templated_runs(text: &str) -> String {
    fold_templated_runs_counted(text).0
}

/// As [`fold_templated_runs`], plus how many repetitions were absorbed.
///
/// The count is in REPETITIONS, not lines: folding trades N long lines for one
/// template plus N short value lines, so the line count can rise while the
/// output shrinks. Anything gating on this must weigh bytes, not lines.
pub fn fold_templated_runs_counted(text: &str) -> (String, usize) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < MIN_RUN {
        return (text.to_string(), 0);
    }
    // Group by template, in first-appearance order.
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    let mut shapes: Vec<(String, Vec<String>)> = Vec::with_capacity(lines.len());
    for line in &lines {
        let (key, values) = shape(line);
        let trimmed = key.replace('\u{1}', "");
        // Our own elision marker says WHERE a line is missing; gathered at the
        // first one's place it says nothing — the rule the uniform-marker fold
        // follows too (`elision::fold_uniform_markers`, 2026-10-02). OURS only:
        // a process that folded nothing wrote no marker, and a document that
        // QUOTES ours folds like any repeated sentence (`fold_count`'s rule).
        let ours = crate::elision::gap_of_marker(line).is_some()
            && crate::fold_count::folded_ceiling() > 0;
        let foldable = !values.is_empty() && trimmed.trim().len() >= MIN_TEMPLATE_CHARS && !ours;
        if foldable {
            let idx = shapes.len();
            groups.entry(key.clone()).or_insert_with(|| {
                order.push(key.clone());
                Vec::new()
            });
            groups.get_mut(&key).unwrap().push(idx);
        }
        shapes.push((if foldable { key } else { String::new() }, values));
    }
    // **An instance that heads a BODY stays with its body.** A line followed by
    // a more indented line of another shape is a header — `error[E0616]: field
    // `x` of struct …` over its ` --> file:line`, a tsc error over `Types of
    // property …`. Gathered at the first one's place, the headers leave their
    // bodies behind, and which field fails at which line is gone: measured
    // 2026-10-02, 39 of 736 template groups in three weeks of real outputs had
    // this shape. The value-per-line pairing this fold promises does not cover
    // the line BELOW.
    let indent_of = |l: &str| l.len() - l.trim_start().len();
    let heads_a_body = |k: &String| {
        groups[k].iter().any(|&i| {
            lines.get(i + 1).is_some_and(|next| {
                !next.trim().is_empty()
                    && indent_of(next) > indent_of(lines[i])
                    && shapes[i + 1].0 != *k
            })
        })
    };
    let folded: std::collections::HashSet<&String> = order
        .iter()
        .filter(|k| groups[*k].len() >= MIN_RUN && !heads_a_body(k))
        .collect();
    if folded.is_empty() {
        return (text.to_string(), 0);
    }
    let absorbed: usize = folded.iter().map(|k| groups[*k].len() - 1).sum();
    let mut emitted: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let key = &shapes[i].0;
        if !folded.contains(key) {
            out.push((*line).to_string());
            continue;
        }
        if !emitted.insert(key) {
            continue; // already folded at its first occurrence
        }
        let members = &groups[key];
        let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
        let all_same = members.iter().all(|m| shapes[*m].1 == shapes[members[0]].1);
        if all_same {
            // Identical lines after all — say it once with the count.
            crate::fold_count::note_folded(members.len());
            out.push(format!("{line}  (×{})", members.len()));
            continue;
        }
        // **A slot whose value NEVER changes is not a
        // variable.** Without this, `2026-09-08 agent a lu …` comes out as
        // `{}-{}-{} agent a lu …` with `2026 · 09 · 08` repeated under every line:
        // the reader loses a date for nothing, and the output GROWS by the weight
        // of a repeated constant. Constants go back into the template,
        // and only the values that vary are listed.
        let first = &shapes[members[0]].1;
        let varies: Vec<bool> = (0..first.len())
            .map(|k| members.iter().any(|m| shapes[*m].1[k] != first[k]))
            .collect();
        // What ALL the values of a slot have in common belongs
        // to the template, not to each line — see `shared_affix`.
        let affix: Vec<(usize, usize)> = (0..first.len())
            .map(|k| {
                if !varies[k] {
                    return (0, 0);
                }
                let vals: Vec<&str> = members.iter().map(|m| shapes[*m].1[k].as_str()).collect();
                shared_affix(&vals)
            })
            .collect();
        let mut template = String::with_capacity(key.len());
        for (slot, part) in key.split('\u{1}').enumerate() {
            if slot > 0 {
                let k = slot - 1;
                if varies[k] {
                    let (head, tail) = affix[k];
                    let v = &first[k];
                    template.push_str(&v[..head]);
                    template.push_str("{}");
                    template.push_str(&v[v.len() - tail..]);
                } else {
                    template.push_str(&first[k]);
                }
            }
            template.push_str(part);
        }
        crate::fold_count::note_folded(members.len());
        out.push(format!("{template}  (×{})", members.len()));
        for m in members {
            let vals: Vec<&str> = shapes[*m]
                .1
                .iter()
                .enumerate()
                .filter(|(k, _)| varies[*k])
                .map(|(k, v)| {
                    let (head, tail) = affix[k];
                    &v[head..v.len() - tail]
                })
                .collect();
            out.push(format!("{indent}  {}", vals.join(VALUE_SEPARATOR)));
        }
    }
    (out.join("\n"), absorbed)
}

/// What all the values of a slot share at the head and at the tail,
/// in BYTES, ready to be lifted into the template.
///
/// The template says the sentence once and the values follow, one per line —
/// but if all fifteen values start with `/var/lib/snapd/snaps/`,
/// that root is rewritten fifteen times under a sentence that is itself
/// written only once. MEASURED on the real `mount` capture: the two columns
/// of the fifteen `squashfs` mounts share 21 and 20 characters, i.e. **615
/// bytes** out of the 5,864 this capture delivers. It is the "the root is said
/// once" fold applied to the value list of the template
/// fold, which did not say it once.
///
/// Three safeguards, each against one way of doing harm:
/// - a short affix does not pay for its reading effort: at least
///   [`MIN_AFFIX`] characters;
/// - a value must never become EMPTY — `abc` and `abcdef` share
///   `abc`, and an empty cell under a template cannot be read back;
/// - the cut falls on a character boundary, otherwise the output is no longer
///   UTF-8.
fn shared_affix(vals: &[&str]) -> (usize, usize) {
    if vals.len() < 2 {
        return (0, 0);
    }
    let head = common_len(vals, false);
    let tail = common_len(vals, true);
    // The two must not overlap nor empty a value.
    let shortest = vals.iter().map(|v| v.len()).min().unwrap_or(0);
    let (head, tail) = if head + tail >= shortest {
        (0, 0)
    } else {
        (head, tail)
    };
    // **The cut falls on a SEPARATOR, never in the middle of a word.**
    //
    // Without this, eleven `ioreg` identifiers that all start with `0x1000001`
    // come out as template `0x1000001{}` and values `1a`, `21a`…: nothing is
    // lost, and yet `0x10000011a` no longer exists anywhere in the
    // output, so a reader searching for it does not find it. The capture test
    // requires precisely that every identifier in the capture comes out WHOLE, and
    // it caught this on the first shot.
    let head = cut_at_separator(vals[0], head, false);
    let tail = cut_at_separator(vals[0], tail, true);
    let head = if head >= MIN_AFFIX { head } else { 0 };
    let tail = if tail >= MIN_AFFIX { tail } else { 0 };
    (head, tail)
}

/// Moves the cut back to the nearest separator, or cancels it.
fn cut_at_separator(v: &str, mut n: usize, from_end: bool) -> usize {
    const SEP: &[u8] = b"/.:=,@# ";
    let b = v.as_bytes();
    while n > 0 {
        let edge = if from_end { b[b.len() - n] } else { b[n - 1] };
        if SEP.contains(&edge) {
            return n;
        }
        n -= 1;
    }
    0
}

/// The common prefix (or suffix), truncated at a character boundary.
fn common_len(vals: &[&str], from_end: bool) -> usize {
    let first = vals[0];
    let mut n = first.len();
    for v in &vals[1..] {
        let (a, b): (&[u8], &[u8]) = (first.as_bytes(), v.as_bytes());
        let mut i = 0usize;
        while i < n && i < b.len() {
            let (x, y) = if from_end {
                (a[a.len() - 1 - i], b[b.len() - 1 - i])
            } else {
                (a[i], b[i])
            };
            if x != y {
                break;
            }
            i += 1;
        }
        n = i;
    }
    // Move back to a character boundary of the FIRST (they coincide,
    // since the compared bytes are equal).
    while n > 0 && !first.is_char_boundary(if from_end { first.len() - n } else { n }) {
        n -= 1;
    }
    n
}

/// Below this number of characters, lifting an affix into the template makes
/// the reader work for almost nothing.
const MIN_AFFIX: usize = 4;

/// What separates two values under a template.
///
/// Declared here rather than spelled out in the `join`: it is a STRUCTURE, not a
/// decoration, and another fold must be able to refuse to cross it. The case:
/// `phrase_fold` replaced `· /tmp/x` — separator included — with a reference,
/// and the line `A · B` came out as `A«1»`. Nothing was lost and yet the
/// list no longer showed the same number of values, whereas the template's
/// legend promises exactly the opposite.
pub const VALUE_SEPARATOR: &str = " · ";

/// Fold only where a document is repetitive enough that the fold pays for
/// itself several times over. Ordinary prose merely rhymes: sentences that
/// share a shape without being generated from one. Folding those would cost a
/// reader the sentence and save almost nothing, so the text is returned
/// untouched unless the fold removes a tenth of it.
pub fn fold_if_it_pays(text: &str) -> String {
    match fold_run_if_it_pays(text, TEMPLATE_LEGEND.len()) {
        Some(folded) => format!("{TEMPLATE_LEGEND}{folded}"),
        None => text.to_string(),
    }
}

/// What the folded shape MEANS, said once in the output rather than only in
/// this file's own documentation.
///
/// The fold rewrites a run into `sentence (×N)` followed by the values, one
/// line each. Measured 2026-09-03 on a 68 000-token handbook answered through
/// `read --prompt`: 121 lines out, **not one of them** saying what `{}` stands
/// for or what the indented numbers under it are — a reader can take
/// `(paragraph {})` for literal text. Every other surface that reorders or
/// rewrites its input explains itself on the spot; this one did not.
///
/// **It speaks for ITS fold, not for the output.** It said "nothing was
/// dropped", and another stage may have dropped something in the same output:
/// measured 2026-10-02 on 413 delivered template-folded results, 4 also carried
/// a loss marker — `… [15 lines elided by tokenade] …` under a banner that
/// denied any loss.
pub(crate) const TEMPLATE_LEGEND: &str =
    "[tokenade:template-fold] a line ending `(×N)` is one sentence said N times with `{}` \
     swapped; the values follow it, one per line, in source order — this fold dropped nothing.\n";

/// The fold itself, with no legend. `reserve` is charged against the win so a
/// caller that will prepend the legend cannot end up bigger than the input.
fn fold_run_if_it_pays(text: &str, reserve: usize) -> Option<String> {
    let (folded, absorbed) = fold_templated_runs_counted(text);
    if absorbed == 0 || !crate::fold_gate::pays(text.len(), folded.len(), reserve) {
        None
    } else {
        Some(folded)
    }
}

/// [`fold_if_it_pays`], but never inside a fenced *or indented* code block.
///
/// A generated document — a changelog, an operations journal, a ticket export —
/// is one templated entry repeated per item, and that is exactly what folding
/// is for. The code samples between those entries are not: two adjacent lines of
/// a snippet share a shape all the time, and rewriting them into a template plus
/// a value list turns code the reader was meant to run into something they
/// cannot. Fences and CommonMark indented runs are copied through verbatim and
/// each prose run is folded on its own, so a run only folds when IT pays — one
/// repetitive section cannot drag an unrelated one along.
pub fn fold_if_it_pays_outside_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prose = String::new();
    let mut fence: Option<String> = None;
    let lines: Vec<&str> = text.lines().collect();

    // The legend belongs ONCE at the top of the document, not once per prose
    // run: a changelog with five fenced samples has six runs, and six copies
    // of the same sentence is the noise the fold exists to remove.
    let mut folded_anything = false;
    let flush = |prose: &mut String, out: &mut String, folded_anything: &mut bool| {
        if !prose.is_empty() {
            match fold_run_if_it_pays(prose, TEMPLATE_LEGEND.len()) {
                Some(f) => {
                    *folded_anything = true;
                    out.push_str(&f);
                }
                None => out.push_str(prose),
            }
            prose.clear();
        }
    };

    let mut i = 0usize;
    let mut blank_run = 1usize;
    let mut list_run = 0usize;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        match &fence {
            // Inside a fence: verbatim until the matching marker closes it.
            Some(marker) => {
                out.push_str(line);
                out.push('\n');
                if trimmed.starts_with(marker.as_str()) {
                    fence = None;
                    blank_run = 1;
                    list_run = 0;
                }
                i += 1;
            }
            None if trimmed.starts_with("```") || trimmed.starts_with("~~~") => {
                flush(&mut prose, &mut out, &mut folded_anything);
                fence = Some(trimmed[..3].to_string());
                out.push_str(line);
                out.push('\n');
                i += 1;
            }
            // **A table row is not a sentence.** Folded, the rows a question
            // retrieved — `| svc-417 | eu-west-0 | 517 ms | team-4 |` — came
            // back as `| svc-{} | eu-west-{} | {} ms | team-{} |` and a list of
            // `417 · 0 · 517 · 4`: every value kept, and the column each one
            // belongs to left for the reader to count (2026-10-03). A table
            // already says each value once, under its header.
            None if trimmed.starts_with('|') => {
                flush(&mut prose, &mut out, &mut folded_anything);
                out.push_str(line);
                out.push('\n');
                i += 1;
                blank_run = 0;
                list_run = 0;
            }
            None => {
                if let Some(end) =
                    crate::fold_gate::indented_code_run_end(&lines, i, list_run, blank_run)
                {
                    flush(&mut prose, &mut out, &mut folded_anything);
                    for l in &lines[i..end] {
                        out.push_str(l);
                        out.push('\n');
                    }
                    i = end;
                    blank_run = 1;
                    list_run = 0;
                    continue;
                }
                prose.push_str(line);
                prose.push('\n');
                if line.trim().is_empty() {
                    blank_run += 1;
                } else if crate::fold_gate::is_list_item(line) {
                    list_run = 1;
                    blank_run = 0;
                } else {
                    list_run = 0;
                    blank_run = usize::from(crate::fold_gate::is_atx_heading(line));
                }
                i += 1;
            }
        }
    }
    flush(&mut prose, &mut out, &mut folded_anything);

    // `lines()` drops a trailing newline; only re-add one the input had.
    if !text.ends_with('\n') {
        while out.ends_with('\n') {
            out.pop();
        }
    }
    if folded_anything {
        out.insert_str(0, TEMPLATE_LEGEND);
    }
    out
}
#[cfg(test)]
include!("template_fold_tests.rs");

#[cfg(test)]
mod the_legend_speaks_only_for_its_own_fold {
    /// Another stage may have elided in the same output (4 delivered outputs
    /// out of 413): so the legend denies no loss on behalf of the output.
    #[test]
    fn it_vouches_for_nothing_about_the_whole_output() {
        assert!(super::TEMPLATE_LEGEND.contains("this fold dropped nothing"));
        assert!(!super::TEMPLATE_LEGEND.contains("nothing was dropped"));
    }
}

#[cfg(test)]
mod our_markers_stay_in_place {
    /// Merged into the place of the first one, 43 "1 line" markers no longer said
    /// which line of which conversation was missing (2026-10-02).
    #[test]
    fn an_elision_marker_is_not_a_sentence_to_fold() {
        let _g = crate::fold_count::isolate_folded_ceiling();
        // A document that QUOTES the marker, in a process that folded nothing: content.
        let quoted: String = (0..4)
            .map(|i| format!("ligne {i}\n{}\n", crate::elision::marker(20)))
            .collect();
        assert!(super::fold_templated_runs(&quoted).contains("(×4)"));
        crate::fold_count::note_folded(9);
        let mut text = String::new();
        for i in 0..6 {
            text.push_str(&format!(
                "conversation {i} line\n{}\n",
                crate::elision::marker(1 + i % 2)
            ));
        }
        let out = super::fold_templated_runs(&text);
        assert_eq!(out.matches("lines elided by tokenade").count(), 6, "{out}");
        assert!(!out.contains("(×"), "{out}");
    }
}

#[cfg(test)]
mod a_header_keeps_its_body {
    /// `cargo build`: six merged `error[E0616]` left their ` --> file:line`
    /// behind — which field fails at which line was no longer readable.
    #[test]
    fn errors_followed_by_their_location_are_not_merged() {
        let mut text = String::new();
        for (f, l) in [("alpha", 12), ("beta", 40), ("gamma", 77), ("delta", 91)] {
            text.push_str(&format!(
                "error[E0616]: field `{f}` of struct `sessions::SessionConcentration` is private\n"
            ));
            text.push_str(&format!("  --> crates/bench/src/ranking.rs:{l}:9\n\n"));
        }
        assert_eq!(super::fold_templated_runs(&text), text);
        // The same sentences without a body still fold.
        let flat: String = ["alpha", "beta", "gamma", "delta"]
            .iter()
            .map(|f| format!("warning: unused import `{f}` in module crates::bench::ranking\n"))
            .collect();
        assert!(super::fold_templated_runs(&flat).contains("(×4)"));
    }
}

#[cfg(test)]
mod a_table_row_is_not_a_sentence {
    /// Twenty rows of one shape are a table, said once each under a header:
    /// folding them would hand back values without their columns.
    #[test]
    fn table_rows_are_kept_verbatim() {
        let rows: String = (0..30)
            .map(|i| {
                format!(
                    "| service-{i} | europe-west-{} | {} milliseconds | platform-team-{} |\n",
                    i % 3,
                    100 + i,
                    i % 7
                )
            })
            .collect();
        let text = format!("| name | region | timeout | owner |\n|---|---|---|---|\n{rows}");
        assert_eq!(super::fold_if_it_pays_outside_code(&text), text);
        // The same values as prose sentences still fold.
        let prose: String = (0..20)
            .map(|i| {
                format!(
                    "Service svc-{i} runs in region eu-west-{} with a timeout of {} ms.\n",
                    i % 3,
                    100 + i
                )
            })
            .collect();
        assert_ne!(super::fold_if_it_pays_outside_code(&prose), prose);
    }
}
