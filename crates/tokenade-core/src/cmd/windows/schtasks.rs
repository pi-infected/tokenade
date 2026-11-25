//! Windows `schtasks /query` (default table form).
//!
//! ```text
//! Folder: \Microsoft\Windows\Defrag
//! TaskName                                 Next Run Time          Status
//! ======================================== ====================== ===============
//! ScheduledDefrag                          N/A                    Ready
//!
//! Folder: \Microsoft\Windows\Device Setup
//! INFO: There are no scheduled tasks presently available at your access level.
//! ```
//!
//! Every folder repeats the header and its rule. Measured 2026-10-02 on a real
//! Windows 10 (fr-FR): 705 lines, 116 folders, 231 tasks — 232 of the lines
//! were the header and its rule again, and the head/tail cut it went through
//! kept the first 50 lines, so most folders and their tasks were gone. Given
//! once, with every task respaced under its folder, the whole list fits.
//! Folders the user may not read are named on one line with the tool's own
//! message. Any other form (`/fo LIST`, `/v`) is `None`.

use crate::tabular::{ruler_cell, ruler_spans};
use std::fmt::Write;

/// `Folder: \…` / `Dossier : \…` — the label is the locale's, the path is not.
fn folder_of(line: &str) -> Option<&str> {
    let (label, path) = line.split_once(':')?;
    let path = path.trim();
    (path.starts_with('\\') && !label.contains('\\') && label.chars().count() <= 20).then_some(path)
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().all(|c| c == '=' || c == ' ')
}

/// A table row leaves blank the gaps between the rule's segments; a sentence
/// runs across them.
fn crosses_a_gap(line: &str, spans: &[(usize, usize)]) -> bool {
    let chars: Vec<char> = line.chars().collect();
    spans.windows(2).any(|w| (w[0].1..w[1].0).any(|i| chars.get(i).is_some_and(|c| !c.is_whitespace())))
}

struct Folder<'a> {
    path: &'a str,
    rows: Vec<String>,
    note: Option<&'a str>,
}

pub fn try_compact(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut folders: Vec<Folder> = Vec::new();
    let mut title: Option<(Vec<char>, Vec<(usize, usize)>)> = None;
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if let Some(path) = folder_of(line) {
            folders.push(Folder { path, rows: Vec::new(), note: None });
            spans.clear(); // each folder has its own header and rule
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let folder = folders.last_mut()?;
        if is_rule(line) {
            spans = ruler_spans(line);
            if title.is_none() {
                title = Some((lines[i.checked_sub(1)?].chars().collect(), spans.clone()));
            }
        } else if lines.get(i + 1).is_some_and(|next| is_rule(next)) {
            // The header: given once, from `title`.
        } else if spans.is_empty() || crosses_a_gap(line, &spans) {
            // Not a row: the tool's own message (`INFO: There are no
            // scheduled tasks…`), written under the rule across its columns.
            folder.note = Some(line.trim());
        } else {
            let chars: Vec<char> = line.chars().collect();
            let cells: Vec<String> = (0..spans.len()).map(|k| ruler_cell(&chars, &spans, k)).collect();
            folder.rows.push(cells.join("  "));
        }
    }
    let (title, title_spans) = title?;
    let mut out = (0..title_spans.len()).map(|k| ruler_cell(&title, &title_spans, k)).collect::<Vec<_>>().join("  ");
    out.push('\n');
    let mut unreadable: Vec<&str> = Vec::new();
    let mut note_text: Option<&str> = None;
    for f in &folders {
        if f.rows.is_empty() {
            unreadable.push(f.path);
            note_text = note_text.or(f.note);
            continue;
        }
        let _ = writeln!(out, "{}", f.path);
        for r in &f.rows {
            let _ = writeln!(out, "  {r}");
        }
    }
    if !unreadable.is_empty() {
        let _ = writeln!(out, "[{} folder(s) without a listed task — {}] {}", unreadable.len(), note_text.unwrap_or("no task"), unreadable.join(" · "));
    }
    Some(out.trim_end().to_string())
}

pub fn compact(text: &str) -> String {
    try_compact(text).unwrap_or_else(|| text.to_string())
}

pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "TaskName                                 Next Run Time          Status         \n======================================== ====================== ===============\n";

    fn sample() -> String {
        let mut s = String::from("\nFolder: \\\n");
        s.push_str(HEAD);
        s.push_str("Git for Windows Updater                  10/3/2026 8:32:55 PM   Ready          \n\n");
        s.push_str("Folder: \\Microsoft\\Windows\\Defrag\n");
        s.push_str(HEAD);
        s.push_str("ScheduledDefrag                          N/A                    Ready          \n\n");
        s.push_str("Folder: \\Empty\n");
        s.push_str(HEAD);
        s.push('\n');
        s.push_str("Folder: \\Microsoft\\Windows\\Device Setup\n");
        s.push_str(HEAD);
        s.push_str("INFO: There are no scheduled tasks presently available at your access level.\n");
        s
    }

    #[test]
    fn every_task_under_its_folder_and_the_header_once() {
        let out = try_compact(&sample()).expect("a schtasks table");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines, [
            "TaskName  Next Run Time  Status",
            "\\",
            "  Git for Windows Updater  10/3/2026 8:32:55 PM  Ready",
            "\\Microsoft\\Windows\\Defrag",
            "  ScheduledDefrag  N/A  Ready",
            "[2 folder(s) without a listed task — INFO: There are no scheduled tasks presently available at your access level.] \\Empty · \\Microsoft\\Windows\\Device Setup",
        ]);
    }

    #[test]
    fn another_form_is_left_alone() {
        assert_eq!(try_compact("HostName: PC\nTaskName: \\X\nStatus: Ready\n"), None);
    }
}
