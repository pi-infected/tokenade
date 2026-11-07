//! **Remove the binary and the data, according to how we were installed.**
//!
//! A system package is not uninstalled like a copy dropped in by hand, and
//! deleting a tree while following symlinks can step outside our own files.
//! The install method is DETECTED rather than assumed.

use super::*;

/// Which package manager delivered the currently-running binary. Decides
/// whether a FULL uninstall auto-removes the binary or just tells the user
/// the exact command their package manager needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallMethod {
    /// Installed via `npm i -g @tokenade/cli` (binary lives under the npm
    /// global prefix's node_modules). npm removes the binary on its own
    /// `npm uninstall` — we must NOT delete it out from under npm.
    Npm,
    /// `cargo install` → `~/.cargo/bin/tokenade`.
    Cargo,
    /// Homebrew formula → `.../Cellar/...` or `/opt/homebrew/...`.
    Brew,
    /// curl-installer / manual copy into `~/.local/bin`, `/usr/local/bin`, …
    /// Nobody else manages this file, so uninstall removes it directly.
    Standalone,
}

/// Classify the install method from the running executable's path. Pure (takes
/// the path, no I/O) so it's unit-testable with synthetic paths. Order matters:
/// npm's prefix can itself sit under a `bin/`, so the package-manager-unique
/// markers are matched before the generic standalone fallback.
pub(crate) fn detect_install_method(exe: &Path) -> InstallMethod {
    // Normalise separators so Windows paths match the same substrings.
    let s = exe.to_string_lossy().replace('\\', "/");
    let low = s.to_lowercase();
    if low.contains("/node_modules/") || low.contains("/@tokenade/") {
        return InstallMethod::Npm;
    }
    if low.contains("/.cargo/bin/") || low.contains("/cargo/bin/") {
        return InstallMethod::Cargo;
    }
    // Homebrew: intel `/usr/local/Cellar/…`, arm `/opt/homebrew/…`, or a
    // linuxbrew `…/.linuxbrew/…` / `…/Homebrew/…` prefix.
    if low.contains("/cellar/") || low.contains("/homebrew/") || low.contains("linuxbrew") {
        return InstallMethod::Brew;
    }
    InstallMethod::Standalone
}

/// FAIL-OPEN recursive delete that NEVER follows symlinks: a symlink entry is
/// unlinked (the link itself is removed), never traversed — so a link inside
/// `~/.tokenade` that points OUTSIDE can't delete anything off-target. This is
/// the same guarantee std's post-CVE-2022-21658 `remove_dir_all` gives, spelled
/// out explicitly so the safety property is obvious and version-independent.
pub(crate) fn remove_tree_no_follow(path: &Path) -> std::io::Result<()> {
    // Every target we delete is built as `<home>/.tokenade`-style — always
    // absolute. A RELATIVE path here means a caller resolved an empty/bogus
    // home and we are about to recursively delete `./tokenade` out of whatever
    // directory the user happens to stand in (reproduced with `HOME=`: real
    // user files destroyed). Nothing legitimate reaches this function relative,
    // so refuse rather than guess. `is_absolute` is cfg-correct here: unlike the
    // pure home-resolution policy, this runs on the real platform.
    if !path.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "refusing to recursively delete a relative path: {}",
                path.display()
            ),
        ));
    }
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        // Unlink the link itself; do NOT descend into its target.
        return fs::remove_file(path);
    }
    if !meta.is_dir() {
        return fs::remove_file(path);
    }
    // Best-effort per-entry: collect the first error but keep deleting the rest
    // so one un-removable file doesn't strand the whole tree.
    let mut first_err: Option<std::io::Error> = None;
    for entry in fs::read_dir(path)? {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                first_err.get_or_insert(e);
                continue;
            }
        };
        let p = entry.path();
        // `file_type()` here is lstat-based (does not follow symlinks).
        let is_symlink = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        let res = if is_symlink {
            fs::remove_file(&p)
        } else if p.is_dir() {
            remove_tree_no_follow(&p)
        } else {
            fs::remove_file(&p)
        };
        // Gone already is the goal reached: a hook or a background write
        // running during uninstall can rename its temp file away between
        // `read_dir` and the unlink. CI hit exactly that ("No such file or
        // directory" on a tree that ended up absent).
        if let Err(e) = res.or_else(gone_is_ok) {
            first_err.get_or_insert(e);
        }
    }
    // A writer still running (a hook, a background index or log write) can
    // drop a new file in after the walk: the directory is then "not empty"
    // although everything we saw is gone (CI, 2.0.8). Walk it again, a
    // bounded number of times, before calling the delete failed.
    let mut last = fs::remove_dir(path).or_else(gone_is_ok);
    for _ in 0..NOT_EMPTY_RETRIES {
        match &last {
            Err(e) if first_err.is_none() && e.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                std::thread::sleep(std::time::Duration::from_millis(50));
                last = remove_contents_then_dir(path);
            }
            _ => break,
        }
    }
    match last {
        Ok(()) => match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        },
        Err(e) => Err(first_err.unwrap_or(e)),
    }
}

const NOT_EMPTY_RETRIES: usize = 5;

/// One more pass over what appeared since: every entry, then the directory.
fn remove_contents_then_dir(path: &Path) -> std::io::Result<()> {
    let entries = match fs::read_dir(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        r => r?,
    };
    for entry in entries {
        let p = entry?.path();
        remove_tree_no_follow(&p).or_else(gone_is_ok)?;
    }
    fs::remove_dir(path).or_else(gone_is_ok)
}

fn gone_is_ok(e: std::io::Error) -> std::io::Result<()> {
    if e.kind() == std::io::ErrorKind::NotFound {
        Ok(())
    } else {
        Err(e)
    }
}

/// FULL-uninstall data wipe: delete tokenade's OWN data directories —
/// `~/.tokenade/` (gain ledger, dedup.db, index.db, semantic.db, debug.log,
/// license.key, install sentinels) and the disclosure-stash parent
/// `<cache>/tokenade/` (stash, downloaded model, fastembed cache). ONLY these
/// two tokenade-owned trees are ever touched — never anything else in HOME or
/// the cache dir. FAIL-OPEN: a tree that can't be removed logs a line and never
/// aborts the uninstall. `--keep-data` skips this call entirely; `--dry-run`
/// lists what WOULD be removed without touching disk.
///
/// `cache_dir` is the *parent* cache dir (e.g. `~/.cache`); we join `tokenade`
/// onto it — matching [`tokenade_core::disclose::stash_dir`]'s `<cache>/tokenade`.
pub(crate) fn remove_tokenade_data(
    home: Option<&Path>,
    cache_dir: Option<&Path>,
    dry_run: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut targets: Vec<PathBuf> = Vec::new();
    // A home directory is absolute BY DEFINITION, and this function deletes
    // trees. MEASURED 2026-08-04: with `HOME=.` the dry-run offered
    // `would delete ./.tokenade` — a directory in whatever the CWD happens to
    // be, not the user's home. That is the residue of the 0.9.5 data-loss
    // incident: `HOME=""` was made to refuse, a relative HOME was not, and it
    // reaches the same place by a different route. Refusing the whole class
    // beats refusing one value.
    let usable = |p: &Path| p.is_absolute();
    if let Some(h) = home.filter(|h| usable(h)) {
        targets.push(h.join(".tokenade"));
    }
    if let Some(c) = cache_dir.filter(|c| usable(c)) {
        let t = c.join("tokenade");
        // Guard against a degenerate cache_dir == home that would double-list.
        if !targets.contains(&t) {
            targets.push(t);
        }
    }
    // Nothing could be LOCATED — not "nothing to remove". With `HOME` unset or
    // degenerate (`""`, `.`, `..`) `home_dir()` returns None, so this function
    // used to produce zero lines and the run still ended on "the agent configs
    // no longer route through tokenade", exit 0. Verified 2026-08-04: the data
    // guard itself holds (a canary file beside the CWD survived every
    // degenerate value), so this is not a deletion bug — it is a SILENT no-op
    // that reads as success on a command a user runs once and never re-checks.
    if targets.is_empty() {
        out.push(
            "could not locate tokenade's data directory (no usable HOME or cache dir) — nothing was removed; re-run with HOME set, or delete ~/.tokenade by hand"
                .to_string(),
        );
        return out;
    }
    for dir in targets {
        // Present-check that also treats a dangling symlink as present (so we
        // still unlink it) but skips a genuinely absent path.
        let present = dir.exists() || fs::symlink_metadata(&dir).is_ok();
        if !present {
            continue;
        }
        // Say HOW MUCH. `~/.tokenade` holds the model weights the local-agent
        // feature downloads — 20 GB on a dogfood install — and the line used to
        // read `would delete /home/x/.tokenade` with no hint of that. In a
        // dry-run especially: that is the moment someone is deciding, and a
        // path alone does not tell them what they are deciding about.
        let size = tokenade_core::platform::human_bytes(tokenade_core::platform::dir_bytes(&dir));
        if dry_run {
            out.push(format!("would delete {} ({size})", dir.display()));
            continue;
        }
        match remove_tree_no_follow(&dir) {
            Ok(()) => out.push(format!("deleted {} ({size} reclaimed)", dir.display())),
            Err(e) => out.push(format!(
                "couldn't fully delete {} ({e}) — remove it by hand",
                dir.display()
            )),
        }
    }
    out
}

/// Detect how the running binary was installed and either remove it
/// (Standalone only) or return the exact command the user must run. Returns the
/// detected method plus a human-readable line for the uninstall report.
/// FAIL-OPEN: a failed self-delete degrades to a "remove it by hand" line, it
/// never aborts. NOT called in `--headless` mode (npm removes its own binary).
pub(crate) fn handle_binary_removal(dry_run: bool) -> (InstallMethod, String) {
    let exe = std::env::current_exe().unwrap_or_default();
    binary_removal_action(&exe, dry_run)
}

/// Pure core of `handle_binary_removal`, split out so the build-artifact
/// guard is unit-testable without depending on where the test binary lives.
pub(crate) fn binary_removal_action(exe: &Path, dry_run: bool) -> (InstallMethod, String) {
    let method = detect_install_method(exe);
    let line = match method {
        InstallMethod::Npm => {
            "the binary is managed by npm — remove it with: npm uninstall -g @tokenade/cli"
                .to_string()
        }
        InstallMethod::Cargo => {
            "the binary is managed by cargo — remove it with: cargo uninstall tokenade-cli"
                .to_string()
        }
        InstallMethod::Brew => {
            "the binary is managed by Homebrew — remove it with: brew uninstall tokenade"
                .to_string()
        }
        InstallMethod::Standalone => {
            let norm = exe.to_string_lossy().replace('\\', "/");
            if exe.as_os_str().is_empty() {
                "couldn't locate the running binary — delete it by hand".to_string()
            } else if norm.contains("/target/debug/") || norm.contains("/target/release/") {
                // A cargo build artifact (dev checkout / e2e harness), NOT an
                // installed binary. Self-deleting it would nuke a developer's
                // build — and in the e2e suite it would delete the shared
                // binary the other tests spawn, cascading into spurious
                // "unknown command" / NotFound failures. Report, never touch.
                format!(
                    "running from a build tree ({}) — not deleting; remove it with cargo/rm yourself",
                    exe.display()
                )
            } else if dry_run {
                format!("would delete the standalone binary {}", exe.display())
            } else if cfg!(windows) {
                // A running .exe is LOCKED on Windows: it can't delete itself,
                // and a detached `del` race is fragile. Safest, per the PO
                // directive, is to hand the user the exact command rather than
                // risk a half-deleted binary.
                format!(
                    "the binary can't self-delete on Windows — remove it with: del \"{}\"",
                    exe.display()
                )
            } else {
                // Unix: unlinking the running executable is safe — the inode is
                // freed only when the last handle (this process) exits, so the
                // command finishes normally and the file is gone afterward.
                match fs::remove_file(exe) {
                    Ok(()) => format!("deleted the standalone binary {}", exe.display()),
                    Err(e) => format!(
                        "couldn't delete {} ({e}) — remove it by hand",
                        exe.display()
                    ),
                }
            }
        }
    };
    (method, line)
}

/// Remove the auto-compact override IF WE OWN IT (`_tokenadeAutoCompact`
/// marker present). Installed by `set_autocompact_window`; without this,
/// Claude Code kept compacting at tokenade's threshold forever after the
/// product was uninstalled. A hand-set `autoCompactWindow` (no marker) is
/// left untouched.
pub(crate) fn strip_autocompact_override(home: Option<&Path>, dry_run: bool) -> Option<String> {
    let home = home?;
    let path = home.join(".claude").join("settings.json");
    let raw = fs::read_to_string(&path).ok()?;
    let mut settings: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let obj = settings.as_object_mut()?;
    obj.remove("_tokenadeAutoCompact")?;
    obj.remove("autoCompactWindow");
    if let Some(env) = obj.get_mut("env").and_then(|e| e.as_object_mut()) {
        env.remove("CLAUDE_CODE_AUTO_COMPACT_WINDOW");
    }
    // Drop a now-empty env map rather than leaving `"env": {}` litter.
    if obj
        .get("env")
        .and_then(|e| e.as_object())
        .is_some_and(|m| m.is_empty())
    {
        obj.remove("env");
    }
    if dry_run {
        return Some(format!(
            "would remove auto-compact override from {}",
            path.display()
        ));
    }
    let pretty = serde_json::to_string_pretty(&settings).ok()?;
    match fs::write(&path, pretty + "\n") {
        Ok(()) => Some(format!(
            "removed auto-compact override from {}",
            path.display()
        )),
        Err(e) => Some(format!("couldn't rewrite {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A writer that keeps dropping files into the tree while it is deleted
    /// (a hook still running during uninstall) must not make it fail.
    #[test]
    fn a_tree_written_to_while_deleted_still_goes() {
        let root = std::env::temp_dir().join(format!("tokenade-rmtree-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub")).unwrap();
        for i in 0..200 {
            fs::write(root.join(format!("f{i}")), b"x").unwrap();
        }
        let w = root.clone();
        let writer = std::thread::spawn(move || {
            for i in 0..40 {
                let _ = fs::write(w.join(format!("late{i}")), b"x");
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        });
        std::thread::sleep(std::time::Duration::from_millis(5));
        let res = remove_tree_no_follow(&root);
        writer.join().unwrap();
        // The writer may outlive the delete and recreate nothing but its own
        // files under a missing parent — those writes fail, which is fine.
        assert!(res.is_ok(), "{res:?}");
        let _ = fs::remove_dir_all(&root);
    }
}
