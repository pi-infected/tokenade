//! **The same instruction, written twice.**
//!
//! A cascade is loaded at EVERY session: a duplicated sentence costs its
//! tokens on every turn, and worse, it gives two voices to a rule that
//! deserved only one.
//!
//! The exact duplicate is decidable; the SEMANTIC duplicate is not, hence
//! a threshold and an apply step that goes through a dry run. Removing an
//! instruction the user wrote is irreversible for them.

use super::*;

/// Result of applying cascade deduplication to disk.
#[derive(Debug, Default)]
pub struct ApplyResult {
    /// Display paths of files that were actually rewritten.
    pub files_modified: Vec<String>,
    /// Total cascade token count before deduplication.
    pub tokens_before: usize,
    /// Total cascade token count after deduplication.
    pub tokens_after: usize,
}

/// Second-pass semantic deduplication using embedding cosine similarity.
///
/// After the exact-match pass has already rewritten `entries`, this pass
/// embeds every surviving qualified line, and for each cross-file pair
/// whose cosine similarity ≥ `SEMANTIC_SIM_THRESHOLD` it removes the
/// occurrence in the less-specific (lower-index) file.
///
/// Silently returns an empty vec if the embedding model fails (the exact
/// pass already ran, so the cascade is cleaner even on model error).
pub(crate) fn semantic_dedup_pass(entries: &mut [(PathBuf, String)]) -> Vec<String> {
    use std::collections::{HashMap, HashSet};

    if entries.len() < 2 {
        return vec![];
    }

    // Collect all qualifying lines: (file_idx, raw_line, norm).
    let mut raw_items: Vec<(usize, String, String)> = Vec::new();
    for (file_idx, (_, content)) in entries.iter().enumerate() {
        for raw in content.lines() {
            if let Some(norm) = normalize(raw) {
                raw_items.push((file_idx, raw.to_string(), norm));
            }
        }
    }
    if raw_items.len() < 2 {
        return vec![];
    }

    // Unique norms for batch embedding.
    let mut seen_norms: HashSet<String> = HashSet::new();
    let unique_norms: Vec<String> = raw_items
        .iter()
        .filter_map(|(_, _, norm)| {
            if seen_norms.insert(norm.clone()) {
                Some(norm.clone())
            } else {
                None
            }
        })
        .collect();

    // Embed; skip this pass silently on any model error.
    let embeddings = match crate::embedding::embed_batch(&unique_norms) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    // norm -> embedding index.
    let norm_idx: HashMap<String, usize> = unique_norms
        .iter()
        .enumerate()
        .map(|(i, n)| (n.clone(), i))
        .collect();

    // Attach embed index; type: Vec<(file_idx, raw, norm, embed_idx)>.
    let items: Vec<(usize, String, String, usize)> = raw_items
        .into_iter()
        .filter_map(|(fi, raw, norm)| norm_idx.get(&norm).copied().map(|ei| (fi, raw, norm, ei)))
        .collect();

    // For each cross-file pair with similarity ≥ threshold, remove the
    // occurrence in the less-specific (lower file_idx) file.
    let mut to_remove: HashSet<(usize, String)> = HashSet::new();

    for i in 0..items.len() {
        let (fi, raw_i, norm_i, ei) = (items[i].0, &items[i].1, &items[i].2, items[i].3);
        if to_remove.contains(&(fi, raw_i.clone())) {
            continue;
        }
        for item_j in items.iter().skip(i + 1) {
            let (fj, raw_j, norm_j, ej) = (item_j.0, &item_j.1, &item_j.2, item_j.3);
            if fi == fj {
                continue; // same file — not our concern
            }
            if norm_i == norm_j {
                continue; // exact match — handled by pass 1
            }
            if to_remove.contains(&(fj, raw_j.clone())) {
                continue;
            }
            let sim = crate::embedding::cosine(&embeddings[ei], &embeddings[ej]);
            if sim >= SEMANTIC_SIM_THRESHOLD {
                // Remove from the less-specific (lower-index) file.
                if fi < fj {
                    to_remove.insert((fi, raw_i.clone()));
                    break; // item i is gone, no need to compare further
                } else {
                    to_remove.insert((fj, raw_j.clone()));
                }
            }
        }
    }

    if to_remove.is_empty() {
        return vec![];
    }

    // Rewrite affected files.
    let mut files_modified: Vec<String> = Vec::new();
    for (file_idx, (path, content)) in entries.iter_mut().enumerate() {
        let remove_set: HashSet<&str> = to_remove
            .iter()
            .filter(|(fi, _)| *fi == file_idx)
            .map(|(_, raw)| raw.as_str())
            .collect();
        if remove_set.is_empty() {
            continue;
        }
        let mut kept: Vec<&str> = Vec::new();
        let mut removed_any = false;
        for raw in content.lines() {
            if remove_set.contains(raw) {
                removed_any = true;
            } else {
                kept.push(raw);
            }
        }
        if removed_any {
            let new_content = if content.ends_with('\n') {
                format!("{}\n", kept.join("\n"))
            } else {
                kept.join("\n")
            };
            if std::fs::write(&*path, &new_content).is_ok() {
                files_modified.push(path.display().to_string());
                *content = new_content;
            }
        }
    }
    files_modified
}

/// Internal helper: apply dedup given explicit `(PathBuf, content)` pairs.
/// Kept public so tests can call it directly without touching the filesystem
/// for path resolution.
pub fn apply_dedup_entries(entries: Vec<(std::path::PathBuf, String)>) -> ApplyResult {
    apply_dedup_entries_opts(entries, true)
}

/// Like [`apply_dedup_entries`] but with explicit control over the
/// semantic (paraphrase) pass. The exact-match pass is always run; the
/// semantic pass — fuzzy 0.85-cosine removal — is gated behind
/// `run_semantic` because it is too aggressive to run unattended (it can
/// silently drop a deliberately-restated user instruction from the
/// less-specific file). The auto SessionStart path runs exact-only; the
/// explicit `audit-claude-md --apply` CLI runs both.
pub fn apply_dedup_entries_opts(
    mut entries: Vec<(std::path::PathBuf, String)>,
    run_semantic: bool,
) -> ApplyResult {
    use std::collections::{HashMap, HashSet};
    let tokens_before: usize = entries
        .iter()
        .map(|(_, c)| crate::tokenizer::estimate(c))
        .sum();
    if entries.len() < 2 {
        return ApplyResult {
            files_modified: vec![],
            tokens_before,
            tokens_after: tokens_before,
        };
    }
    // For each normalized line, record the index of the LAST (most-specific) file.
    let mut last_file_for_line: HashMap<String, usize> = HashMap::new();
    for (idx, (_, content)) in entries.iter().enumerate() {
        let mut seen: HashSet<String> = HashSet::new();
        for line in content.lines() {
            if let Some(norm) = normalize(line) {
                if seen.insert(norm.clone()) {
                    last_file_for_line.insert(norm, idx);
                }
            }
        }
    }
    let mut files_modified = Vec::new();
    for (idx, (path, content)) in entries.iter_mut().enumerate() {
        let mut kept: Vec<&str> = Vec::new();
        let mut removed_any = false;
        let mut seen: HashSet<String> = HashSet::new();
        for line in content.lines() {
            let keep = match normalize(line) {
                None => true,
                Some(norm) => {
                    if !seen.insert(norm.clone()) {
                        true // intra-file repeat: not our concern
                    } else {
                        last_file_for_line.get(&norm).copied().unwrap_or(idx) == idx
                    }
                }
            };
            if keep {
                kept.push(line);
            } else {
                removed_any = true;
            }
        }
        if removed_any {
            let new_content = if content.ends_with('\n') {
                format!("{}\n", kept.join("\n"))
            } else {
                kept.join("\n")
            };
            if std::fs::write(&*path, &new_content).is_ok() {
                files_modified.push(path.display().to_string());
                *content = new_content;
            }
        }
    }

    // Pass 2: semantic dedup — catches same instruction phrased differently.
    // Gated: skipped on the unattended auto path (see `run_semantic`).
    if run_semantic {
        let sem_modified = semantic_dedup_pass(&mut entries);
        for p in sem_modified {
            if !files_modified.contains(&p) {
                files_modified.push(p);
            }
        }
    }

    // Final token count from the rewritten content.
    let tokens_after: usize = entries
        .iter()
        .map(|(_, c)| crate::tokenizer::estimate(c))
        .sum();
    ApplyResult {
        files_modified,
        tokens_before,
        tokens_after,
    }
}

/// Remove duplicate lines from the CLAUDE.md cascade, writing modified
/// files back to disk automatically.
///
/// Lines that appear in multiple files are **kept** in the most-specific
/// (last in load order = project/cwd) file and **removed** from earlier,
/// less-specific files. The global `~/.claude/CLAUDE.md` is where
/// cross-project boilerplate accumulates and is the primary target.
///
/// Idempotent: a second call on an already-clean cascade returns
/// `files_modified: []` with `tokens_before == tokens_after`.
/// Runs both exact + semantic passes — for the explicit `audit-claude-md
/// --apply` CLI. The unattended SessionStart hook uses
/// [`apply_dedup_exact`] instead (exact-only, see its doc).
pub fn apply_dedup(start: &Path, home: Option<&Path>) -> ApplyResult {
    let entries = existing_cascade(start, home);
    apply_dedup_entries(entries)
}

/// Exact-match-only cascade dedup for the unattended SessionStart path.
///
/// Same as [`apply_dedup`] but skips the fuzzy semantic pass — see
/// [`apply_dedup_entries_opts`] for why the auto path must not run
/// 0.85-cosine paraphrase removal against user files. Idempotent and
/// silent: a clean cascade (or <2 existing files) is a no-op.
pub fn apply_dedup_exact(start: &Path, home: Option<&Path>) -> ApplyResult {
    let entries = existing_cascade(start, home);
    apply_dedup_entries_opts(entries, false)
}
