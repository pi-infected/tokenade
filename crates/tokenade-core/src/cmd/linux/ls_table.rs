//! Shared engine for the long `ls -l` family.
//!
//! Observed real output (machine locale fr_FR.UTF-8):
//! ```text
//! total 11044
//! -rw-r--r--  1 user user   2063 17 juin  08:10 ab_apache.rs
//! drwxr-xr-x 25 user user 110592 17 juin  08:54 .
//! ```
//! The actionable content for a coding agent is perms, owner, size and
//! name. The link count is noise and goes; the owner and group are almost
//! always uniform, so the pair the listing repeats is named ONCE in the
//! banner and kept on the rows that differ — a `root:wheel` entry in a
//! directory of `dev:admin` is the answer to a permission error, and
//! dropping the column outright lost it. They sit at fixed positions
//! (tokens 1/2/3, right after the perms word) regardless of
//! `--time-style`, so they are read by POSITION, never by label —
//! locale-independent.
//!
//! Crucially this elides COLUMNS, not ROWS: every file line survives, so
//! no filename is ever decapitated (the failure mode of a generic
//! head-fold on a directory listing).

use std::fmt::Write;

/// A leading token like `-rw-r--r--`, `drwxr-xr-x`, `lrwxrwxrwx`,
/// optionally suffixed with an ACL/xattr marker (`+`, `@`, `.`).
fn is_perms(tok: &str) -> bool {
    let b = tok.as_bytes();
    if b.len() < 10 {
        return false;
    }
    if !matches!(b[0], b'-' | b'd' | b'l' | b'b' | b'c' | b'p' | b's') {
        return false;
    }
    // The 9 permission chars.
    b[1..10]
        .iter()
        .all(|&c| matches!(c, b'r' | b'w' | b'x' | b's' | b'S' | b't' | b'T' | b'-'))
        // Anything trailing must be an ACL marker only.
        && b[10..].iter().all(|&c| matches!(c, b'+' | b'@' | b'.'))
}

/// Byte offset just past the `n`th whitespace-delimited token (1-based),
/// or `None` if the line has fewer than `n` tokens.
fn nth_token_end(line: &str, n: usize) -> Option<usize> {
    let mut count = 0;
    let mut in_tok = false;
    for (i, c) in line.char_indices() {
        if c.is_whitespace() {
            if in_tok {
                count += 1;
                if count == n {
                    return Some(i);
                }
                in_tok = false;
            }
        } else {
            in_tok = true;
        }
    }
    if in_tok {
        count += 1;
        if count == n {
            return Some(line.len());
        }
    }
    None
}

/// True when the text contains at least one `ls -l`-shaped line.
pub fn detect(text: &str) -> bool {
    text.lines()
        .any(|l| l.split_whitespace().next().is_some_and(is_perms))
}

/// Listing lines needed before naming the dominant perms in the banner pays
/// for the banner itself.
const PERMS_FOLD_MIN_LINES: usize = 8;
/// Share of lines that must carry the same perms for it to count as "the"
/// perms of this listing, in percent.
const PERMS_FOLD_MIN_SHARE: usize = 80;

/// The permission word almost every line repeats, if there is one.
///
/// A source directory is a hundred lines of `-rw-r--r--`: eleven characters
/// of pure repetition per line, and the column stops being informative the
/// moment it stops varying. Naming it once and keeping it only where it
/// DIFFERS preserves exactly the signal an agent reads this listing for — the
/// directory, the symlink, the executable bit — and drops the rest.
fn dominant_perms(text: &str) -> Option<String> {
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut total = 0usize;
    for line in text.lines() {
        let mut toks = line.split_whitespace();
        if let Some(p) = toks.next() {
            if is_perms(p) && toks.clone().count() >= 5 {
                *counts.entry(p).or_default() += 1;
                total += 1;
            }
        }
    }
    if total < PERMS_FOLD_MIN_LINES {
        return None;
    }
    // Ties broken on the perms text so the choice cannot depend on hash order.
    let (best, n) = counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))?;
    (n * 100 / total >= PERMS_FOLD_MIN_SHARE).then(|| (*best).to_string())
}

/// The `owner:group` pair most rows carry. Always named — exceptions keep
/// theirs, so naming the commonest is lossless at any listing size.
/// Ties broken on the text so the choice cannot depend on hash order.
fn dominant_owner(text: &str) -> Option<String> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for line in text.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() >= 6 && is_perms(toks[0]) {
            *counts.entry(format!("{}:{}", toks[2], toks[3])).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(o, _)| o)
}

/// Rewrite each `ls -l` line as `<perms> <size> <date… name>`, dropping the
/// link column and naming the dominant `owner:group` once (rows that differ
/// keep theirs). Non-matching lines (e.g. `total 11044`) are
/// kept verbatim. Returns `None` if nothing was elided.
/// A permission word repeated on nearly every line is named once in the
/// banner and dropped from those rows; rows that differ keep theirs.
pub fn fold(text: &str, label: &str) -> Option<String> {
    let whole = render(text, label, usize::MAX)?;
    if tk_base::delivery::fits(&whole) {
        return Some(whole);
    }
    // **A listing that does not fit keeps its NAMES.** `ls -la /etc` (296
    // entries, measured 2026-10-03) rendered 5 314 tokens, just over the
    // threshold, and the downstream cut kept thirty rows: 260 names gone,
    // the very thing an `ls` is read for. Past the first rows, each entry is
    // its name alone (`dir/` for a directory), on one line per run.
    render(text, label, FULL_ROWS_WHEN_CUT)
}

/// Rows kept whole before the rest of an oversized listing goes to names.
const FULL_ROWS_WHEN_CUT: usize = 40;

/// The name an `ls -l` row ends with: everything after the size and the
/// three date tokens, a directory marked `/`.
fn row_name(line: &str, perms: &str) -> Option<String> {
    let name = line[nth_token_end(line, 8)?..].trim_start();
    Some(if perms.starts_with('d') {
        format!("{name}/")
    } else {
        name.to_string()
    })
}

fn render(text: &str, label: &str, full_rows: usize) -> Option<String> {
    let dominant = dominant_perms(text);
    let owner = dominant_owner(text).unwrap_or_default();
    let mut out = String::with_capacity(text.len());
    let mut banner_done = false;
    let mut elided = 0usize;
    let mut names: Vec<String> = Vec::new();
    let flush_names = |out: &mut String, names: &mut Vec<String>| {
        if !names.is_empty() {
            let _ = writeln!(out, "[{} more, names only] {}", names.len(), names.join(" · "));
            names.clear();
        }
    };
    for line in text.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() >= 6 && is_perms(toks[0]) && elided >= full_rows {
            if let Some(name) = row_name(line, toks[0]) {
                names.push(name);
                elided += 1;
                continue;
            }
        }
        if toks.len() >= 6 && is_perms(toks[0]) {
            // toks: perms(0) links(1) owner(2) group(3) size(4) date… name
            let size = toks[4];
            // Verbatim tail (date + name, original spacing) starts after size.
            let rest = match nth_token_end(line, 5) {
                Some(end) => line[end..].trim_start(),
                None => {
                    out.push_str(line);
                    out.push('\n');
                    continue;
                }
            };
            if !banner_done {
                match &dominant {
                    Some(p) => {
                        let _ = writeln!(
                            out,
                            "[tokenade:{label}] link column elided; owner {owner} and perms {p} \
                             unless shown (size·date·name kept)"
                        );
                    }
                    None => {
                        let _ = writeln!(
                            out,
                            "[tokenade:{label}] link column elided; owner {owner} \
                             unless shown (perms·size·date·name kept)"
                        );
                    }
                }
                banner_done = true;
            }
            if dominant.as_deref() != Some(toks[0]) {
                let _ = write!(out, "{} ", toks[0]);
            }
            let this_owner = format!("{}:{}", toks[2], toks[3]);
            if this_owner != owner {
                let _ = write!(out, "{this_owner} ");
            }
            let _ = writeln!(out, "{} {}", size, rest);
            elided += 1;
        } else {
            flush_names(&mut out, &mut names);
            out.push_str(line);
            out.push('\n');
        }
    }
    flush_names(&mut out, &mut names);
    if elided == 0 {
        return None;
    }
    Some(out.trim_end().to_string())
}

// --- Shared-helper stubs (see df_table.rs for the rationale) ---------------
pub fn try_compact(_text: &str) -> Option<String> {
    None
}
pub fn compact(text: &str) -> String {
    crate::archetypes::passthrough(text)
}
pub fn compact_with_savings(text: &str) -> (String, crate::Savings) {
    crate::archetypes::savings_of(text, compact)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source directory is a hundred lines of one permission word. Naming
    /// it once and keeping it only where it DIFFERS is the whole point: the
    /// directory, the symlink and the executable bit — the reasons anyone
    /// reads this column — all survive because they are the exceptions.
    #[test]
    fn a_constant_permission_column_is_named_once_and_exceptions_are_kept() {
        let mut t = String::from("total 5904\n");
        for i in 0..40 {
            let _ = writeln!(
                t,
                "-rw-r--r--  1 username username {i}0536 29 juil. 16:50 mod{i}.rs"
            );
        }
        t.push_str("drwxr-xr-x  5 username username 12288 2 août  20:24 subdir\n");
        t.push_str("lrwxrwxrwx  1 username username     9 2 août  20:24 link -> mod0.rs\n");
        let out = fold(&t, "ls-long").unwrap();

        assert!(
            out.contains("perms -rw-r--r-- unless shown"),
            "the dominant value has to be named:\n{out}"
        );
        assert!(
            out.lines().any(|l| l.starts_with("drwxr-xr-x")),
            "the directory kept its perms:\n{out}"
        );
        assert!(
            out.lines().any(|l| l.starts_with("lrwxrwxrwx")),
            "the symlink kept its perms:\n{out}"
        );
        assert!(
            !out.contains("-rw-r--r-- 00536"),
            "the repeated value is gone from the rows:\n{out}"
        );
        // Every file still has its line — this elides COLUMNS, never ROWS.
        for i in 0..40 {
            assert!(out.contains(&format!("mod{i}.rs")), "lost mod{i}.rs");
        }
        assert!(out.len() < t.len(), "and it is smaller");
    }

    /// `ls -la /etc` (296 entries) did not fit and the cut downstream kept
    /// thirty rows. Past the first rows, every entry keeps its name.
    #[test]
    fn a_listing_that_does_not_fit_keeps_every_name() {
        let _g = tk_base::delivery::threshold_on_this_thread(500);
        let mut t = String::from("total 2120\n");
        for i in 0..120 {
            let _ = writeln!(t, "-rw-r--r--  1 root root  {i}0 13 juil. 22:24 conf{i}.d");
        }
        t.push_str("drwxr-xr-x  4 root root 4096 13 juil. 22:24 ssl certs\n");
        let out = fold(&t, "ls-long").unwrap();
        assert!(out.contains("-rw-r--r-- 390 13 juil. 22:24 conf39.d") || out.contains("390 13 juil. 22:24 conf39.d"), "{out}");
        assert!(out.contains("[81 more, names only] conf40.d · conf41.d · "), "{out}");
        assert!(out.ends_with(" · conf119.d · ssl certs/"), "{out}");
    }

    #[test]
    fn recognises_perms() {
        assert!(is_perms("-rw-r--r--"));
        assert!(is_perms("drwxr-xr-x"));
        assert!(is_perms("lrwxrwxrwx"));
        assert!(is_perms("-rw-r--r--+")); // ACL marker
        assert!(!is_perms("total"));
        assert!(!is_perms("username"));
    }

    #[test]
    fn detects_french_locale_long_listing() {
        let t = "total 11044\n-rw-r--r--  1 username username   2063 17 juin  08:10 ab_apache.rs\n";
        assert!(detect(t));
    }

    #[test]
    fn elides_owner_group_keeps_name_and_size() {
        let t = "total 11044\n\
                 -rw-r--r--  1 username username   2063 17 juin  08:10 ab_apache.rs\n\
                 drwxr-xr-x 25 username username 110592 17 juin  08:54 .\n";
        let out = fold(t, "ls-long").unwrap();
        assert!(out.contains("total 11044")); // non-file line verbatim
        assert!(out.contains("ab_apache.rs"));
        assert!(out.contains("2063"));
        assert!(out.contains("17 juin")); // date kept
        // the uniform owner/group is named once, not on every row.
        assert_eq!(out.matches("username").count(), 2, "{out}");
        assert!(out.contains("owner username:username unless shown"), "{out}");
    }

    /// macOS `/Applications` mixes `root:wheel` and `user:admin`: the owner
    /// of an entry is what a permission error asks about, so the rows that
    /// differ from the named pair keep theirs.
    #[test]
    fn an_owner_that_differs_is_kept_on_its_row() {
        let t = "total 16\n\
                 drwxrwxr-x   7 root  admin   224 Mar 10  2026 .\n\
                 drwxrwxr-x@  3 user  admin    96 Mar 10  2026 Google Chrome.app\n\
                 drwxr-xr-x   3 root  wheel    96 Sep 24  2025 Utilities\n\
                 -rw-r--r--   1 root  wheel     0 Sep 24  2025 .localized\n\
                 drwxr-xr-x  22 root  wheel   704 Sep 24  2025 ..\n";
        let out = fold(t, "ls-long").unwrap();
        assert!(out.contains("owner root:wheel unless shown"), "{out}");
        let row = |name: &str| out.lines().find(|l| l.ends_with(name)).unwrap().to_string();
        assert_eq!(row("Google Chrome.app"), "drwxrwxr-x@ user:admin 96 Mar 10  2026 Google Chrome.app");
        assert_eq!(row(" ."), "drwxrwxr-x root:admin 224 Mar 10  2026 .");
        assert_eq!(row("Utilities"), "drwxr-xr-x 96 Sep 24  2025 Utilities");
    }

    #[test]
    fn preserves_name_with_spaces() {
        let t = "-rw-r--r-- 1 u g 42 Jan  1 12:00 my file.txt\n\
                 -rw-r--r-- 1 u g 99 Jan  1 12:00 other.txt\n";
        let out = fold(t, "ls-long").unwrap();
        assert!(out.contains("my file.txt"));
    }

    #[test]
    fn preserves_symlink_arrow() {
        let t = "lrwxrwxrwx 1 u g 7 Jan  1 12:00 link -> target\n\
                 -rw-r--r-- 1 u g 9 Jan  1 12:00 real.txt\n";
        let out = fold(t, "ls-long").unwrap();
        assert!(out.contains("link -> target"));
    }

    #[test]
    fn non_listing_returns_none() {
        assert!(fold("just some prose\nmore prose\n", "ls-long").is_none());
    }
}
