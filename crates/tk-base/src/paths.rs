//! One-shot migration of on-disk paths after the `sts` → `tokenade`
//! rename: rename `<root>/.sts/` to `<root>/.tokenade/` when an old
//! install is detected. Idempotent — does nothing if the new dir already
//! exists or the old one is missing.
//!
//! Called from the CLI's [`run`] entry point so every invocation
//! (`tokenade query`, `tokenade gain`, `tokenade mcp`, `tokenade
//! hookwrap`, …) has a consistent view of where state lives. Cheap
//! when migration is unnecessary (one `metadata` call).

use std::fs;
use std::path::Path;

/// Old (pre-rename) per-project data directory.
pub const LEGACY_DIR_NAME: &str = ".sts";
/// New per-project data directory.
pub const DIR_NAME: &str = ".tokenade";

/// Outcome of [`migrate_legacy_dir`] — used by tests and for the
/// install UX to surface that a migration happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// Nothing to do — the old dir doesn't exist.
    NotPresent,
    /// New dir already exists; we left the old one alone to avoid
    /// clobbering anything the user may have intentionally split.
    /// (We log a one-liner suggesting they merge by hand.)
    AlreadyMigrated,
    /// Renamed `.sts` → `.tokenade` atomically.
    Renamed,
    /// Tried to rename but the OS rejected (e.g. cross-device).
    Failed(String),
}

/// Inspect `<root>` and, if a legacy `.sts/` is found AND no `.tokenade/`
/// exists, rename the former to the latter. Safe to call once per
/// process; cheap on subsequent calls (just a `metadata` probe).
/// Replace `path`'s contents so that a concurrent reader sees either the old
/// bytes or the new ones, never a half-written or EMPTY file.
///
/// `fs::write` truncates before it writes. MEASURED 2026-08-04 with two writers
/// and three readers on one small state file: **69 % of reads came back
/// empty**. That matters because every caller of these files parses them and
/// falls back to a zero state on failure — so a torn read does not look like an
/// error, it looks like "this counter has never been used", and the counter
/// silently restarts.
///
/// The temporary name carries the pid AND a per-process sequence: a fixed
/// `.tmp` sibling (the pattern this replaces) lets two writers share one
/// scratch file, so one can rename the other's half-written bytes into place —
/// First symlinked component of `path` at or below `root`, if any.
///
/// The repo-scoped counterpart to following a link. A user-scope path may be
/// a link the user chose — a dotfiles manager publishing `~/.gemini/settings.json`
/// — and writing through it is correct (see `mcp_wrap::write_locked`). A path
/// INSIDE a checked-out repository is a different animal: a hostile repo can
/// ship `.claude/CLAUDE.md` as a link to `~/.ssh/authorized_keys`, and a
/// scaffold write would go straight through it.
///
/// Components ABOVE `root` are the user's own filesystem and are not our
/// business — only the part of the path the repository controls is checked.
/// Mirrors `atomic_io::first_symlink_below` in tokenade-cli, which guards the
/// project-scope hook writer for the same reason.
pub fn first_symlink_below(root: &Path, path: &Path) -> Option<std::path::PathBuf> {
    let rest = path.strip_prefix(root).ok()?;
    let mut cur = root.to_path_buf();
    for part in rest.iter() {
        cur.push(part);
        // `symlink_metadata` does NOT follow the final component — that is the
        // whole point; `metadata` would report the target's type instead.
        match fs::symlink_metadata(&cur) {
            Ok(md) if md.file_type().is_symlink() => return Some(cur),
            _ => continue,
        }
    }
    None
}

/// [`write_atomic`] for a path inside a repository: refuses to write when any
/// component at or below `root` is a symlink.
///
/// `Err(AlreadyExists)` names the offending link so the caller can say why it
/// skipped. Deliberately a REFUSAL, not a follow: the scaffold is a
/// convenience, and no convenience is worth writing through a link a cloned
/// repo chose for us.
pub fn write_atomic_repo_scoped(root: &Path, path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(link) = first_symlink_below(root, path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("refusing to write through repo symlink {}", link.display()),
        ));
    }
    write_atomic(path, contents)
}

/// the very failure the rename was added to prevent.
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    // Third writer to reach an agent config, and the FIRST one to run during
    // an install — measured 2026-08-19, the pristine copy taken by the other
    // two already held `{}` because this one had been there before them. A
    // rule that lives in one place has to be CALLED from every place.
    //
    // Skipped for this product's own state (`~/.tokenade/…`): a ledger has no
    // "user's own bytes" to protect and would just gain a sibling copy.
    // Resolve links BEFORE choosing where to rename. A dotfiles manager
    // (chezmoi, stow, yadm) publishes `~/.cursor/mcp.json` or `~/.aider.conf.yml`
    // as a link into its own tree; renaming a tmp file onto the link REPLACES
    // the link, so the agent reads bytes outside the user's dotfiles, their
    // managed copy is orphaned silently, and the next `apply` reverts us.
    // `mcp_wrap::write_locked` and `atomic_io::write_json_atomic` both
    // canonicalize and say why — this writer, the first to run during an
    // install, did not. Measured 2026-08-25: the link came back a regular file.
    //
    // Missing destination → keep the path as given, so a first write still
    // creates it. Note the asymmetry with `write_atomic_repo_scoped`, which
    // REFUSES a link rather than following it: scope decides. A link a cloned
    // repo chose for us is an attack; a link the user's own dotfiles manager
    // chose IS the file.
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // Our own state is judged on BOTH spellings: `~/.tokenade` may itself be a
    // link, and canonicalizing it away would start littering a ledger directory
    // with pristine backups of rows nobody needs to protect.
    if !is_tokenade_state(path) && !is_tokenade_state(&resolved) {
        backup_once(&resolved);
        if let Some(original) = pristine_if_equivalent(&resolved, contents) {
            return write_atomic_inner(&resolved, &original);
        }
    }
    write_atomic_inner(&resolved, contents)
}

/// Is this one of this product's own files rather than one of the user's?
///
/// Two homes, not one. `~/.tokenade` holds the ledgers and the indexes; the
/// disclosure stash lives under the OS cache directory as `…/tokenade/stash`,
/// whose component is `tokenade` without the dot. Testing only the dotted
/// spelling made every stash file look like a user file, so a rewrite of one
/// took a pristine copy beside it — and the only command that ever rewrites a
/// stash file is `tokenade scrub-secret`. Measured 2026-09-03: scrubbing a
/// secret out of a stashed blob reported "scrubbed 3002 occurrence(s)" and
/// left 3000 of them in `<blob>.tokenade-original`, created by the scrub
/// itself, in the same directory. A backup of the bytes a scrub exists to
/// destroy is the one backup that must not be taken.
fn is_tokenade_state(path: &Path) -> bool {
    if path.components().any(|c| c.as_os_str() == ".tokenade") {
        return true;
    }
    // `<cache>/tokenade/**` — the stash and anything else we keep there.
    match crate::platform::cache_dir() {
        Some(cache) => path.starts_with(cache.join("tokenade")),
        None => false,
    }
}

fn write_atomic_inner(path: &Path, contents: &str) -> std::io::Result<()> {
    write_atomic_bytes(path, contents.as_bytes())
}

pub fn write_atomic_bytes(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stem = path.file_name().and_then(|s| s.to_str()).unwrap_or("state");
    let tmp = path.with_file_name(format!(".{stem}.{}.{n}.tmp", std::process::id()));
    fs::write(&tmp, contents)?;
    // `rename` gives the destination the TEMPORARY file's permissions, which
    // are whatever the umask allows. Several callers rewrite files the user
    // owns and may have tightened — agent settings that can hold an API key,
    // a trust store. Widening those to 0644 as a side effect of a save is not
    // something a save is allowed to do, so carry the destination's mode over
    // when it already exists. Best-effort: a metadata failure must not turn a
    // successful write into a lost one.

    if let Ok(meta) = fs::metadata(path) {
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

pub fn migrate_legacy_dir(root: &Path) -> MigrationOutcome {
    let legacy = root.join(LEGACY_DIR_NAME);
    let new = root.join(DIR_NAME);
    let legacy_exists = legacy.is_dir();
    let new_exists = new.exists();
    match (legacy_exists, new_exists) {
        (false, _) => MigrationOutcome::NotPresent,
        (true, true) => MigrationOutcome::AlreadyMigrated,
        (true, false) => match fs::rename(&legacy, &new) {
            Ok(()) => MigrationOutcome::Renamed,
            Err(e) => MigrationOutcome::Failed(format!(
                "could not rename {} → {}: {e}",
                legacy.display(),
                new.display()
            )),
        },
    }
}

/// The name of the pristine copy kept beside a config this product rewrites.
///
/// Deliberately NOT one of the `<name>.tokenade-backup-<ts>` files. Those are a
/// ROLLING history: `diagnostics_cli::backup_file` prunes the oldest past a
/// cap, and its timestamp is in whole SECONDS — so a pristine copy taken in the
/// same second as a routine backup was overwritten by it, byte for byte, by
/// `fs::copy`. Measured 2026-08-19: after an install the only surviving copy of
/// `~/.claude/settings.json` held the INSTALLED state, and the user's own bytes
/// were gone within the same second they were saved.
///
/// A pristine copy is a different KIND of thing from a rolling backup: there is
/// exactly one, it is never pruned, and it is never rewritten.
pub const PRISTINE_SUFFIX: &str = ".tokenade-original";

fn pristine_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(PRISTINE_SUFFIX);
    std::path::PathBuf::from(p)
}

/// Capture the file's bytes ONCE, before this product first rewrites them.
///
/// Without this there is nothing to put back, and "reversible" is a claim about
/// meaning rather than about bytes.
pub fn backup_once(path: &std::path::Path) {
    let dst = pristine_path(path);
    if dst.exists() {
        return;
    }
    if let Ok(current) = std::fs::read(path) {
        let _ = std::fs::write(dst, current);
    }
}

/// `now` with every empty object/array pruned that the `original` does not
/// carry at the same place. Recursive, so a container nested inside one the
/// user does own is still reachable.
///
/// Deliberately narrow: only an EMPTY container, and only one the original had
/// no key for. Anything else is a content difference and must survive, because
/// the caller uses equality with the original as its licence to overwrite.
fn prune_empty_containers_absent_from(
    now: &serde_json::Value,
    original: &serde_json::Value,
) -> serde_json::Value {
    match (now, original) {
        (serde_json::Value::Object(n), serde_json::Value::Object(o)) => {
            let mut out = serde_json::Map::new();
            for (k, v) in n {
                let orig = o.get(k);
                let is_empty_container = match v {
                    serde_json::Value::Object(m) => m.is_empty(),
                    serde_json::Value::Array(a) => a.is_empty(),
                    _ => false,
                };
                if is_empty_container && orig.is_none() {
                    continue;
                }
                match orig {
                    Some(ov) => {
                        out.insert(k.clone(), prune_empty_containers_absent_from(v, ov));
                    }
                    None => {
                        out.insert(k.clone(), v.clone());
                    }
                }
            }
            serde_json::Value::Object(out)
        }
        _ => now.clone(),
    }
}

/// Delete every `*.tokenade-original` under `home`.
///
/// Separate from [`restore_pristine_files`], and called LATER, because doing it
/// in the same walk does not work: measured 2026-08-19, the copies were gone
/// and back by the end of the uninstall, since a writer that runs after the
/// restore takes a fresh one through `backup_once`. Same shape as the restore
/// itself — the only moment a "did you leave anything behind" question has an
/// answer is when nobody else is going to write.
pub fn forget_pristine_files(home: &Path) -> usize {
    let mut removed = 0;
    let mut stack = vec![home.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if p.file_name().is_some_and(|n| n == "node_modules") {
                    continue;
                }
                stack.push(p);
                continue;
            }
            if p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(PRISTINE_SUFFIX))
                && fs::remove_file(&p).is_ok()
            {
                removed += 1;
            }
        }
    }
    removed
}

/// Put back every config whose live content now MEANS what the user's did.
///
/// A final pass rather than a rule inside a writer, and that is the lesson
/// rather than a shortcut: four different functions write
/// `~/.claude/settings.json` during one install/uninstall cycle
/// (`atomic_io::write_json_atomic`, `mcp_wrap::write_locked`,
/// `paths::write_atomic`, and a raw `fs::write` in `install/uninstall.rs`), and
/// wiring the rule into each of them still left the file reformatted, because
/// there was always one more. Asking the question ONCE, at the end, when the
/// answer is finally knowable, cannot be outflanked by a fifth writer.
///
/// Walks for the `.tokenade-original` copies rather than for a list of known
/// agent paths: the list would need editing every time an agent is added, and
/// the copies are exactly the files this product admits having rewritten.
pub fn restore_pristine_files(home: &Path) -> usize {
    // Guarded here TOO, and not only at the uninstall entry: this walk rewrites
    // the user's config files from copies, so it is the sharpest edge in the
    // module and the one most likely to be called from somewhere new.
    if refuse_real_home_from_a_test_binary(home, "restore config files").is_err() {
        return 0;
    }
    let mut restored = 0;
    let mut stack = vec![home.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if p.file_name().is_some_and(|n| n == "node_modules") {
                    continue;
                }
                stack.push(p);
                continue;
            }
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(stem) = name.strip_suffix(PRISTINE_SUFFIX) else {
                continue;
            };
            let live = p.with_file_name(stem);
            let (Ok(now), Ok(original)) = (fs::read_to_string(&live), fs::read_to_string(&p))
            else {
                continue;
            };
            if now == original {
                continue;
            }
            let (Ok(a), Ok(b)) = (
                serde_json::from_str::<serde_json::Value>(&now),
                serde_json::from_str::<serde_json::Value>(&original),
            ) else {
                continue;
            };
            // Equal as JSON → the only difference was formatting, restore.
            //
            // Not equal → try once more with the EMPTY CONTAINERS WE CREATED
            // pruned. An uninstall removes the entries it added and leaves the
            // object it put them in: measured 2026-08-19 on Gemini CLI and Qwen
            // Code, a file that arrived as `{"compat":"seed"}` came back as
            // `{"compat":"seed","hooks":{}}`. That is not formatting, so the
            // rule above refused it — correctly, and the fix belongs here
            // rather than in each writer, for the reason this whole pass
            // exists: four functions touch one file in a cycle and the only
            // moment the question has an answer is once nobody else will write.
            //
            // An empty container holds no user data, and one absent from the
            // original was ours. If the user's own `hooks` had entries, pruning
            // ours leaves a document that no longer matches and nothing is
            // restored — which is the correct refusal.
            let pruned = prune_empty_containers_absent_from(&a, &b);
            if (a == b || pruned == b) && write_atomic_inner(&live, &original).is_ok() {
                restored += 1;
            }
        }
    }
    restored
}

/// The PRISTINE bytes of a config, when the text about to be written says
/// exactly the same thing.
///
/// Uninstall removes what install added and then re-serialises, so a config
/// that arrived as `{"mcpServers":{}}` on one line went back out as three
/// indented lines with a trailing newline. Nothing of the user's had changed
/// and their `git diff` said otherwise — measured 2026-08-19 on Claude Code,
/// Gemini CLI and Qwen Code, whose settings this product owns.
///
/// Two writers reach these files (`atomic_io::write_json_atomic` in the CLI,
/// `mcp_wrap::write_locked` here), so the rule lives in ONE place and both call
/// it. A second implementation of the same gesture is how three separate cells
/// lied about the product earlier the same day.
///
/// Only ever returns something when the two parse to the SAME JSON: a
/// FORMATTING difference is the one thing this may hide, a content difference
/// must always be written.
pub fn pristine_if_equivalent(path: &std::path::Path, body: &str) -> Option<String> {
    let original = std::fs::read_to_string(pristine_path(path)).ok()?;
    if original == body {
        return None;
    }
    let a: serde_json::Value = serde_json::from_str(body).ok()?;
    let b: serde_json::Value = serde_json::from_str(&original).ok()?;
    (a == b).then_some(original)
}

/// Refuse destructive work on the operator's REAL home from a test binary.
///
/// Measured 2026-08-19, on the machine this was written on: a test exercising
/// the uninstall path ran with no `HOME` override, so `platform::home_dir()`
/// answered the developer's own home and the uninstall did exactly what it is
/// built to do — to their live install. `~/.claude/settings.json` went from
/// 4 975 to 832 bytes, all six hook events gone, and the statusline restored to
/// the pre-tokenade command it had wrapped. The user noticed because their
/// savings figure disappeared; nothing else said a word. Sessions between that
/// moment and the repair were never compacted.
///
/// This is the third time this class has been paid for — `cargo test` writing
/// into the real `~/.tokenade` was found on 2026-08-04 across five databases,
/// and the fix then was to redirect the stores. Redirection only helps the
/// paths somebody remembered to redirect. This one asks the opposite question,
/// once, at the door.
///
/// The discriminator is NOT `cfg!(test)`: that is per-crate, and this crate is
/// compiled without it when the CLI's tests run — which is precisely the case
/// that did the damage. It is the running executable's own path. A test or
/// bench harness lives under `target/**/deps/`; an installed product never
/// does.
///
/// The "real" home comes from the passwd database and not from `$HOME`, so a
/// test that DOES set `HOME` to a temp directory passes: its home differs from
/// the account's, which is the whole point of setting it.
pub fn refuse_real_home_from_a_test_binary(home: &Path, what: &str) -> Result<(), String> {
    if !running_from_a_test_binary() {
        return Ok(());
    }
    let Some(real) = account_home() else {
        return Ok(());
    };
    if home == real {
        return Err(format!(
            "refusing to {what} against the account's real home ({}) from a test binary.\n\
             Set HOME (and USERPROFILE on Windows) to a temporary directory for this test.\n\
             This guard exists because a test without that override uninstalled a live \
             install on 2026-08-19.",
            real.display()
        ));
    }
    Ok(())
}

/// Is this executable a test or bench harness?
fn running_from_a_test_binary() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    // `<target-dir>/<profile>/deps/<name>-<16 hex>` — whatever the target
    // directory is called. Requiring a component named `target` switched this
    // guard OFF under any `CARGO_TARGET_DIR` (2026-10-03, a clean-clone run
    // built into `target-clean/`): the one setup where a test is least watched
    // was the one it stopped protecting.
    //
    // Cargo 1.100 moved them to `<build-dir>/<profile>/build/<pkg>/<hash>/out/
    // <name>-<hash>`, and the guard went off on every CI run with it: the
    // directory above `out` carrying the binary's own hash is that layout.
    let stem = exe.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let Some(hash) = stem
        .rsplit_once('-')
        .map(|(_, h)| h)
        .filter(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit()))
    else {
        return false;
    };
    fn name(p: Option<&std::path::Path>) -> Option<&str> {
        p?.file_name()?.to_str()
    }
    let dir = exe.parent();
    name(dir) == Some("deps")
        || (name(dir) == Some("out") && name(dir.and_then(|d| d.parent())) == Some(hash))
}

/// The account's home, from the OS rather than from `$HOME`.
///
/// `$HOME` is what a test overrides; the passwd entry is what it cannot.
#[cfg(unix)]
fn account_home() -> Option<std::path::PathBuf> {
    // No libc dependency for one lookup: the same answer is in the passwd file,
    // and `getent` is not needed either — `/etc/passwd` is world-readable and
    // this only has to be right on a developer machine.
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()?;
    let text = fs::read_to_string("/etc/passwd").ok()?;
    for line in text.lines() {
        let mut f = line.split(':');
        if f.next()? == user {
            return f.nth(4).map(std::path::PathBuf::from);
        }
    }
    None
}

#[cfg(not(unix))]
fn account_home() -> Option<std::path::PathBuf> {
    // Windows has no passwd file; `USERPROFILE` is the closest stable answer
    // and a test that overrides it is doing the right thing anyway.
    std::env::var_os("USERPROFILE").map(std::path::PathBuf::from)
}
#[cfg(test)]
include!("paths_tests.rs");
