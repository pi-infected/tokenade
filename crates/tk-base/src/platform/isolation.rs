//! **Isolate a test without shaking the suite.**
//!
//! `env` and the current directory are PROCESS-global; sixty-six
//! files in this tree mutate them. `ENV_LOCK` is therefore not
//! `#[cfg(test)]` — that attribute becomes FALSE as soon as the layer is a
//! separate crate, as the extraction of `tk-base` demonstrated. The RAII guards here
//! restore the original state even on panic.

use super::*;

/// Remove regular files DIRECTLY under `dir` whose mtime is older than
/// `ttl_secs`. Non-recursive, best-effort, silent on errors. Returns the
/// number of files removed. This is the shared disk-hygiene primitive for
/// the scratch directories that would otherwise grow without bound in normal
/// use (per-session state, per-project image-read cache): a file untouched
/// for the TTL belongs to a finished session / stale read and is safe to
/// drop, while anything the current session just wrote carries a fresh mtime
/// and is never swept.
/// Wall-clock seconds since the Unix epoch, 0 if the clock is before it.
///
/// One definition. It was written FIFTEEN times across the two crates under
/// three names — `now_secs`, `now_secs_global`, `now_unix_secs` — with
/// byte-identical bodies, one per module that happened to need a timestamp.
/// Mined from `repowise` (2026-08-29), whose own commit is the same shape:
/// "five byte-identical copies … are now one function".
///
/// The `unwrap_or(0)` is the contract, not an accident: every caller stamps a
/// record or compares a TTL, and a clock behind the epoch must degrade to
/// "very old" rather than panic in a hook.
pub fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Unix time in milliseconds. One declarer: `search::now_millis`,
/// `gain::weigh::now_ms` and `updater::state::now_ms` each spelled it out
/// and now re-export this one under their old names.
pub fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn evict_files_older_than(dir: &std::path::Path, ttl_secs: u64) -> usize {
    let now = std::time::SystemTime::now();
    let mut removed = 0usize;
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
        if !is_file {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .map(|age| age.as_secs() > ttl_secs)
            .unwrap_or(false);
        if stale && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Drop stale entries (paths that no longer exist) from
/// `~/.tokenade/projects-registry.txt`. The file is append-only — every
/// scaffolded project adds a line and nothing ever removed them, so on
/// machines that scaffold many ephemeral dirs (CI, `/tmp` checkouts) it grew
/// unbounded. Stale lines are already filtered on READ, so pruning them is
/// pure hygiene. Best-effort; returns the number of lines dropped. Shared by
/// the automatic SessionStart sweep and the manual `tokenade evict-stale`.
pub fn prune_projects_registry() -> usize {
    let Some(home) = home_dir() else {
        return 0;
    };
    let registry = home.join(".tokenade").join("projects-registry.txt");
    let Ok(content) = std::fs::read_to_string(&registry) else {
        return 0;
    };
    let mut kept = Vec::with_capacity(content.lines().count());
    let mut dropped = 0usize;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if std::path::Path::new(trimmed).exists() {
            kept.push(trimmed.to_string());
        } else {
            dropped += 1;
        }
    }
    if dropped == 0 {
        return 0;
    }
    let new_body = if kept.is_empty() {
        String::new()
    } else {
        format!("{}\n", kept.join("\n"))
    };
    if std::fs::write(&registry, new_body).is_err() {
        return 0;
    }
    dropped
}

/// User config directory (XDG-style).
///
/// Resolution order:
///   1. `$XDG_CONFIG_HOME` (any platform — explicit override wins).
///   2. `$APPDATA` (Windows native default).
///   3. `$HOME/.config` (Unix XDG default).
///   4. `$USERPROFILE/AppData/Roaming` (Windows fallback).
///   5. Falls back to the current dir so callers can blindly `join()`.
pub fn config_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("APPDATA") {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("HOME") {
        return PathBuf::from(p).join(".config");
    }
    if let Some(p) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(p).join("AppData").join("Roaming");
    }
    PathBuf::from(".")
}

/// One-shot best-effort hardening of tokenade's storage dirs to
/// owner-only perms (0700 dirs, 0600 sensitive files). Defense-
/// in-depth against secret-leak vectors that survive our redaction
/// pipeline (paired-flag misses, content-cached blobs, stash files
/// that captured a credential before redaction kicked in).
///
/// Targets:
///   * `~/.tokenade/`             — debug.log, dedup.db, gain.jsonl, index.db, semantic.db
///   * `~/.cache/tokenade/stash/` — progressive-disclosure cache (may contain secret-bearing
///     shell commands captured pre-redaction)
///
/// Silent on Windows (no Unix perms) and on permission errors
/// (don't break the user's tooling if they intentionally widened
/// the perms for sharing).
/// The ONE lock every test that mutates process environment must hold.
///
/// Process env is global: `set_var("HOME", …)` in one test is visible to every
/// other test in the same binary. Modules each grew their own lock —
/// `debug_log::ENV_LOCK`, `agent_hermes::HERMES_HOME_LOCK`, one inside
/// `platform`'s own test module — and a lock only serialises the tests that
/// take it, so tests in different modules never waited for each other. That is
/// why the release gate has to pass `--test-threads=1`.
///
/// It lives here because this is where `HOME`/`USERPROFILE` resolution lives
/// ([`pick_home`]). Take it for the FULL lifetime of the mutation *and* of
/// everything that reads the environment afterwards, and restore what you found.
///
/// `pub`, not `pub(crate)`: `tokenade-cli`'s tests mutate the same process-wide
/// variables (`TOKENADE_STASH_DIR`, `HOME`) against the same real state, and a
/// lock only serialises the tests that can take it. Keeping it crate-private
/// meant the CLI suite had no way to serialise at all — which is how
/// `a_heavily_folded_mcp_result_leaves_a_pointer_that_returns_the_bytes` came
/// to fail once and pass on the re-run: it writes to the REAL stash and another
/// test swept it mid-flight.
///
/// It is also no longer `#[cfg(test)]`. That attribute is FALSE here when
/// another crate's test binary links this one — the third time that trap has
/// cost something today — so the lock simply did not exist for the callers that
/// needed it most. An unlocked `Mutex<()>` costs nothing in a release build.
pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Points the stash and the redaction ledger at scratch paths, via their own
/// dedicated knobs — NOT by mutating `HOME`.
///
/// The distinction is deliberate and this crate already made it: see
/// `every_store_path_honours_an_env_override`, whose doc records that per-store
/// env overrides are "the mechanism this crate settled on for isolation,
/// deliberately, after process-wide `HOME` mutation stalled the suite". So a
/// store that HAS a knob must be isolated with that knob.
/// [`ScratchHome`] is for the remaining case only: a path derived from `HOME`
/// with no knob of its own, such as the global `~/.config/git/ignore` that
/// `project_scaffold::ensure` appends to.
///
/// Holds [`ENV_LOCK`] for its whole life, same contract as [`ScratchHome`].
pub struct StoreIsolation {
    _env: std::sync::MutexGuard<'static, ()>,
    prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
    dir: PathBuf,
}

impl StoreIsolation {
    pub fn new(label: &str) -> Self {
        let env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "tokenade-store-iso-{}-{}",
            std::process::id(),
            label
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let vars: [(&'static str, PathBuf); 2] = [
            ("TOKENADE_STASH_DIR", dir.join("stash")),
            ("TOKENADE_REDACT_LEDGER", dir.join("redact.jsonl")),
        ];
        let mut prev = Vec::new();
        for (k, v) in vars {
            prev.push((k, std::env::var_os(k)));
            // SAFETY: env vars are process-wide; ENV_LOCK is held for the whole
            // guard lifetime, which outlives every read of these values.
            unsafe {
                std::env::set_var(k, &v);
            }
        }
        StoreIsolation {
            _env: env,
            prev,
            dir,
        }
    }
}

impl Drop for StoreIsolation {
    fn drop(&mut self) {
        for (k, v) in &self.prev {
            // SAFETY: still inside the ENV_LOCK guard's lifetime.
            unsafe {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Points `HOME` at a scratch directory for the guard's lifetime.
///
/// Tests that exercise a code path reaching the user's HOME write the REAL
/// files: measured 2026-08-04 by running the core test binary under an
/// isolated HOME, `cargo test` produced the developer's global
/// `~/.config/git/ignore`, five `.tokenade/*.db` stores, `redact.jsonl` and
/// six stash blobs. The stash half already had a known cost — see the note on
/// [`ENV_LOCK`] above, where a test failed once and passed on re-run because
/// "it writes to the REAL stash and another test swept it mid-flight".
///
/// Holds [`ENV_LOCK`] for its whole life, per that lock's contract: the
/// mutation AND the window in which the value is read.
///
/// `pub`, not `#[cfg(test)]`, for the reason [`ENV_LOCK`] gives: that attribute
/// is false when another crate's test binary links this one. `pub` also means
/// no dead-code warning in a release build, where nothing constructs it.
/// Hold [`ENV_LOCK`] and restore every variable you touched, on drop.
///
/// Three test modules each rolled their own `Mutex<()>` to serialise
/// environment mutations (`hookwrap`'s `LOCK`, `wrap`'s `ENV_GUARD`, and
/// `read_pending_lock`), which is the shape that cannot work: a lock only
/// serialises the callers that take THE SAME lock, so three private mutexes
/// let three tests mutate the same process-wide state at once. That is the
/// classic flake, once per mutex.
///
/// They also each spelled the save-and-restore by hand, and mostly did not
/// restore at all. Both halves live here now, and `ScratchHome` is written in
/// terms of this rather than repeating it.
///
/// NOT for the current directory: `set_current_dir` is a different
/// process-global with a different lock (`cwd_lock`). Merging distinct
/// resources under one mutex buys nothing and serialises tests that never
/// conflict.
pub struct ScopedEnv {
    _env: std::sync::MutexGuard<'static, ()>,
    saved: Vec<(std::ffi::OsString, Option<std::ffi::OsString>)>,
}

impl Default for ScopedEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl ScopedEnv {
    pub fn new() -> Self {
        ScopedEnv {
            _env: ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner()),
            saved: Vec::new(),
        }
    }

    fn remember(&mut self, key: &str) {
        if self.saved.iter().any(|(k, _)| k == key) {
            return; // the FIRST value seen is the one to restore
        }
        self.saved.push((key.into(), std::env::var_os(key)));
    }

    pub fn set(&mut self, key: &str, value: impl AsRef<std::ffi::OsStr>) -> &mut Self {
        self.remember(key);
        // SAFETY: env is process-wide; ENV_LOCK is held for this guard's whole
        // lifetime, which outlives every read of the value.
        unsafe {
            std::env::set_var(key, value);
        }
        self
    }

    pub fn unset(&mut self, key: &str) -> &mut Self {
        self.remember(key);
        // SAFETY: see `set`.
        unsafe {
            std::env::remove_var(key);
        }
        self
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        // SAFETY: still inside the ENV_LOCK guard's lifetime. Restored in
        // reverse so a key touched twice returns to what it was first.
        for (key, prev) in self.saved.iter().rev() {
            // SAFETY: ENV_LOCK is still held (this guard's lifetime), so no other thread reads
            // the environment.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

pub struct ScratchHome {
    env: ScopedEnv,
    dir: PathBuf,
}

impl ScratchHome {
    /// `label` only distinguishes concurrent scratch dirs; any string works.
    pub fn new(label: &str) -> Self {
        let mut env = ScopedEnv::new();
        let dir = std::env::temp_dir().join(format!(
            "tokenade-scratch-home-{}-{}",
            std::process::id(),
            label
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        // BOTH, and the second one is the whole point on Windows.
        //
        // `home_dir` above makes `USERPROFILE` authoritative there and `HOME`
        // the fallback — deliberately, to match what the agent reads — so a
        // scratch home that sets only `HOME` redirects nothing on Windows:
        // every test using it reads the developer's REAL profile, writes its
        // fixtures into the scratch, and then asserts against a directory that
        // never saw them.
        //
        // MEASURED in the 1.1.7 release build: seven Windows failures, six of
        // them this one line — five `user_filters` tests reporting `got: []`
        // where a filter had just been written, and
        // `project_root_never_climbs_past_home` walking past a boundary that
        // was never moved. The e2e harness has set both since it was written
        // (`tests/e2e/main.rs`: `.env("HOME", …).env("USERPROFILE", …)`); this
        // helper is the door that drifted.
        env.set("HOME", &dir);
        env.set("USERPROFILE", &dir);
        // And the third variable that decides where a test looks, for exactly
        // the same reason as the second. `CLAUDE_CONFIG_DIR` overrides
        // `$HOME/.claude` outright (see `claude_config_dir_for`), so a
        // developer or a CI runner who has moved their Claude Code config
        // would have every scratch home in the suite read and write the REAL
        // one — fixtures written into the scratch, assertions made against a
        // directory that never saw them, which is the shape the USERPROFILE
        // line above was added for. Absent, not redirected: the scratch home
        // IS the answer, and `claude_config_dir_for` falls back to it.
        env.unset("CLAUDE_CONFIG_DIR");
        ScratchHome { env, dir }
    }

    /// The scratch directory now standing in for HOME.
    pub fn path(&self) -> &std::path::Path {
        &self.dir
    }

    /// Set another variable under the SAME guard. Taking a second guard would
    /// deadlock — `Mutex` is not reentrant — so this is the way to combine.
    pub fn env(&mut self) -> &mut ScopedEnv {
        &mut self.env
    }
}

impl Drop for ScratchHome {
    fn drop(&mut self) {
        // `self.env` restores HOME and USERPROFILE when it drops, right after this.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
