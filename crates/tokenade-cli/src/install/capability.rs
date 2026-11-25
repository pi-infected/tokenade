#![allow(unused_imports)]
//! Install-time agent-capability probes.
//!
//! The WebSearch PostToolUse compactor rides Claude Code's
//! `hookSpecificOutput.updatedToolOutput` (replace-tool-output) channel,
//! which shipped in Claude Code **2.1.121** (changelog: "PostToolUse hooks
//! can now replace tool output for all tools via
//! `hookSpecificOutput.updatedToolOutput`"). Older versions ignore the
//! field — the hook would burn a process per WebSearch for zero effect —
//! so the installer probes the LOCAL binary and only wires the hook when
//! the capability is proven. Re-running install (the update path) re-runs
//! the probe, so the entry appears/disappears as the user's Claude Code
//! moves across the boundary.
//!
//! This module is the seed of a future per-agent capability matrix:
//! additional `pub(crate) fn <agent>_supports_<capability>() -> bool`
//! probes belong here, keyed the same way (feature probe first, version
//! floor as fallback, conservative false).
use super::*;
use crate::*;
use std::path::{Path, PathBuf};

/// The byte marker probed inside the Claude Code binary. The engine's zod
/// schema names the field verbatim (`updatedToolOutput: E.unknown()…`), so
/// a plain substring scan is reliable (verified locally: 12 hits in the
/// 2.1.205 Bun executable; the npm `cli.js` distribution carries the same
/// source text).
const UPDATED_TOOL_OUTPUT_MARKER: &[u8] = b"updatedToolOutput";

/// Version floor for the SECONDARY probe (see module docs).
const UPDATED_TOOL_OUTPUT_MIN_VERSION: (u64, u64, u64) = (2, 1, 121);

/// Does the locally installed Claude Code honour
/// `hookSpecificOutput.updatedToolOutput` on PostToolUse hooks?
///
/// Probe order:
///   1. `TOKENADE_CAP_UPDATED_TOOL_OUTPUT` env — "1" forces true, "0"
///      forces false (TEST-ONLY override, used by the install tests).
///   2. Feature probe: resolve the `claude` binary from PATH (symlinks
///      followed), scan its bytes for [`UPDATED_TOOL_OUTPUT_MARKER`]
///      (chunked, capped at 256 MB). A hit is authoritative-true; a miss
///      on a LARGE file (the real Bun executable / bundled cli.js) is
///      authoritative-false. A miss on a small file is a launcher shim
///      (npm `.cmd`/`.ps1`/shell wrapper, or a Windows versions-dir
///      trampoline) — inconclusive, fall through.
///   3. Version fallback: `claude --version` parses to x.y.z →
///      compare against 2.1.121.
///   4. Unresolvable / unreadable and no version → false (conservative:
///      never install a hook we can't prove works).
pub(crate) fn claude_supports_updated_tool_output() -> bool {
    // The override is read EVERY time and never cached: it is the knob tests
    // and operators turn, and a cached answer would make turning it useless.
    match std::env::var("TOKENADE_CAP_UPDATED_TOOL_OUTPUT")
        .ok()
        .as_deref()
    {
        Some("1") => return true,
        Some("0") => return false,
        _ => {}
    }
    // Everything below is a fact about the machine, and it cannot change while
    // this process runs — so it is resolved once. It is not cheap: the local
    // Claude binary is 247 905 800 bytes on this machine and
    // `file_contains_marker` scans up to 256 MB of it. Measured 2026-08-27:
    // 0,08 s per scan with the file warm in page cache, and worse cold.
    // `hookwrap` consults this predicate, so an uncached answer put a
    // quarter-gigabyte read on a per-tool-call path.
    static RESOLVED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *RESOLVED.get_or_init(probe_updated_tool_output)
}

fn probe_updated_tool_output() -> bool {
    // Without `claude` on PATH, the Claude Code that will read these hooks is
    // the one in Claude Desktop's Code tab: it reads the same `~/.claude`.
    // Measured 2026-10-06 on a Windows machine: no `claude` on PATH, 2.1.286
    // bundled by the app — and the WebSearch/MCP hooks were skipped.
    if resolve_claude_binary().is_none() {
        if let Some(bin) =
            tokenade_core::platform::home_dir().and_then(|h| desktop_claude_binary(&h))
        {
            return file_contains_marker(&bin, UPDATED_TOOL_OUTPUT_MARKER) == Some(true);
        }
    }
    if let Some(bin) = resolve_claude_binary() {
        match file_contains_marker(&bin, UPDATED_TOOL_OUTPUT_MARKER) {
            Some(true) => return true,
            Some(false) => {
                // Only trust a negative scan on a file big enough to BE
                // the engine — a small file is a shim whose bytes prove
                // nothing about the real binary it launches.
                let big_enough = std::fs::metadata(&bin)
                    .map(|m| m.len() >= 1024 * 1024)
                    .unwrap_or(false);
                if big_enough {
                    return false;
                }
            }
            None => {} // unreadable → version fallback
        }
    }
    claude_version_at_least(UPDATED_TOOL_OUTPUT_MIN_VERSION).unwrap_or(false)
}

/// Walk PATH for the `claude` executable (plus `.exe`/`.cmd`/`.bat`/`.ps1`
/// on Windows — npm installs a `.cmd`/`.ps1` shim there) and resolve
/// symlinks to the real file (npm's `claude` is typically a symlink into
/// `node_modules`, and native installs may point into a versions dir).
/// The newest Claude Code an installed Claude Desktop carries:
/// `<app data>/Claude/claude-code/<version>/[<hash>/]claude[.exe]`, the app
/// data being `~/.config` (Linux), `~/Library/Application Support` (macOS),
/// `%APPDATA%`, or the Store build's redirected `Packages\Claude_*` copy.
pub(crate) fn desktop_claude_binary(home: &Path) -> Option<PathBuf> {
    let mut roots = vec![
        home.join(".config").join("Claude"),
        home.join("Library")
            .join("Application Support")
            .join("Claude"),
        home.join("AppData").join("Roaming").join("Claude"),
    ];
    if let Ok(rd) = std::fs::read_dir(home.join("AppData").join("Local").join("Packages")) {
        roots.extend(
            rd.flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("Claude_"))
                .map(|e| e.path().join("LocalCache").join("Roaming").join("Claude")),
        );
    }
    let version = |n: &str| -> Option<Vec<u64>> { n.split('.').map(|p| p.parse().ok()).collect() };
    let exe = |d: &Path| {
        ["claude", "claude.exe"]
            .iter()
            .map(|n| d.join(n))
            .find(|p| p.is_file())
    };
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for ver_dir in roots
        .iter()
        .filter_map(|r| std::fs::read_dir(r.join("claude-code")).ok())
        .flatten()
        .flatten()
    {
        let Some(v) = version(&ver_dir.file_name().to_string_lossy()) else {
            continue;
        };
        let found = exe(&ver_dir.path()).or_else(|| {
            std::fs::read_dir(ver_dir.path())
                .ok()?
                .flatten()
                .find_map(|h| exe(&h.path()))
        });
        if let Some(bin) = found {
            if best.as_ref().is_none_or(|(b, _)| v > *b) {
                best = Some((v, bin));
            }
        }
    }
    best.map(|(_, p)| p)
}

pub(crate) fn resolve_claude_binary() -> Option<PathBuf> {
    resolve_agent_binary("claude")
}

/// The same walk, for any agent CLI. `cursor_supports_mcp_post_replace` needs
/// it and a second copy of a PATH walk is how two of them end up disagreeing
/// about Windows extensions.
pub(crate) fn resolve_agent_binary(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let mut candidates = vec![dir.join(name)];
        if cfg!(windows) {
            for ext in ["exe", "cmd", "bat", "ps1"] {
                candidates.push(dir.join(format!("{name}.{ext}")));
            }
        }
        for c in candidates {
            if c.is_file() {
                return Some(std::fs::canonicalize(&c).unwrap_or(c));
            }
        }
    }
    None
}

/// Chunked substring scan of `path` for `marker`. Reads at most 256 MB;
/// each chunk carries a `marker.len()-1` byte overlap so a marker
/// straddling a chunk boundary is still found. Returns None when the file
/// can't be opened/read (caller decides the fallback), Some(found)
/// otherwise.
pub(crate) fn file_contains_marker(path: &Path, marker: &[u8]) -> Option<bool> {
    use std::io::Read;
    const CHUNK: usize = 8 * 1024 * 1024;
    const MAX_SCAN_BYTES: u64 = 256 * 1024 * 1024;
    if marker.is_empty() {
        return Some(true);
    }
    let mut f = std::fs::File::open(path).ok()?;
    let overlap = marker.len() - 1;
    let mut buf = vec![0u8; overlap + CHUNK];
    let mut filled = 0usize; // bytes currently valid at the front of buf
    let mut scanned: u64 = 0;
    loop {
        if scanned >= MAX_SCAN_BYTES {
            return Some(false);
        }
        let n = f.read(&mut buf[filled..]).ok()?;
        if n == 0 {
            return Some(false);
        }
        scanned += n as u64;
        let window = &buf[..filled + n];
        if window.windows(marker.len()).any(|w| w == marker) {
            return Some(true);
        }
        // Keep the tail as overlap for the next round.
        let keep = window.len().min(overlap);
        let start = window.len() - keep;
        buf.copy_within(start..start + keep, 0);
        filled = keep;
    }
}

/// Run `claude --version` and compare the first x.y.z triple against
/// `min`. None when the command can't run / prints no version.
fn claude_version_at_least(min: (u64, u64, u64)) -> Option<bool> {
    binary_version_at_least("claude", min)
}

/// How long a capability answer is trusted before re-probing.
///
/// The answer only changes when the user upgrades the agent's CLI, so a week
/// is generous rather than risky: the worst case is a week of not using a
/// capability that just became available, and the hook is re-asserted on every
/// session anyway. Mirrors `platform::system_lang`, which caches a PowerShell
/// probe for exactly the same reason — a per-invocation process spawn on a path
/// that runs constantly.
const CAP_CACHE_TTL_SECS: u64 = 7 * 86_400;

fn cap_cache_path(name: &str) -> std::path::PathBuf {
    tokenade_core::platform::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".tokenade")
        .join(name)
}

/// Answer from `path` when it is fresh, otherwise run `probe` and record it.
///
/// Split out with the path injected so the policy is testable without touching
/// `HOME` or spawning anything.
///
/// This exists because the uncached probe was the single most expensive thing
/// tokenade did. Profiled 2026-08-03 on a real install: SessionStart cost
/// 1 024 ms, of which **977 ms was this one `gemini --version` spawn** — a Node
/// CLI, paid on every session, every resume and every `/clear`, to answer a
/// question whose answer changes when the user upgrades Gemini and at no other
/// time. Ten bisection experiments had failed to find it; a profiler found it
/// in two runs.
fn cached_probe(path: &std::path::Path, ttl_secs: u64, probe: impl FnOnce() -> bool) -> bool {
    let fresh = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|age| age.as_secs() < ttl_secs)
        .unwrap_or(false);
    if fresh {
        if let Ok(v) = std::fs::read_to_string(path) {
            match v.trim() {
                "1" => return true,
                "0" => return false,
                _ => {} // corrupt: fall through and re-probe
            }
        }
    }
    let answer = probe();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, if answer { "1" } else { "0" });
    answer
}

/// How long a `<bin> --version` probe may run before we give up and
/// treat the capability as unproven. Node CLIs answer in well under a
/// second; 5 s absorbs a cold FS cache without hanging an install.
const VERSION_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Run `<bin> --version` (bounded by [`VERSION_PROBE_TIMEOUT`]) and
/// compare the first x.y.z triple against `min`. None when the command
/// can't run, times out, or prints no version.
fn binary_version_at_least(bin: &str, min: (u64, u64, u64)) -> Option<bool> {
    version_probe_output(bin).and_then(|text| parse_semver_triple(&text).map(|v| v >= min))
}

/// Spawn `<bin> --version`, poll with `try_wait` until exit or the probe
/// timeout (killed on expiry), and return its stdout. A `--version`
/// banner is far below the OS pipe buffer, so the child can never block
/// on a full pipe while we poll.
fn version_probe_output(bin: &str) -> Option<String> {
    use std::io::Read as _;
    let mut cmd = tokenade_core::process_util::internal_command(bin);
    cmd.arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    tokenade_core::process_util::configure_no_window(&mut cmd);
    let mut child = cmd.spawn().ok()?;
    let deadline = std::time::Instant::now() + VERSION_PROBE_TIMEOUT;
    loop {
        match child.try_wait().ok()? {
            Some(status) => {
                if !status.success() {
                    return None;
                }
                break;
            }
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out)
}

/// First `x.y.z` triple in `s` ("2.1.205 (Claude Code)" → (2,1,205)).
pub(crate) fn parse_semver_triple(s: &str) -> Option<(u64, u64, u64)> {
    for tok in s.split(|c: char| !(c.is_ascii_digit() || c == '.')) {
        let parts: Vec<&str> = tok.split('.').filter(|p| !p.is_empty()).collect();
        if parts.len() >= 3 {
            if let (Ok(a), Ok(b), Ok(c)) = (parts[0].parse(), parts[1].parse(), parts[2].parse()) {
                return Some((a, b, c));
            }
        }
    }
    None
}

/// Serializes every test that mutates `TOKENADE_CAP_UPDATED_TOOL_OUTPUT`.
///
/// An ALIAS for [`tokenade_core::platform::ENV_LOCK`], not a second lock. It
/// was a `Mutex` of its own until 2026-08-27, and process env has exactly one
/// owner: two locks over the same variable do not exclude each other, so a
/// test holding this one raced a test holding `ENV_LOCK` through `ScopedEnv`.
/// Caught as a real flake — `hookwrap::copilot_tests::
/// a_host_without_the_field_gets_no_replacement` passed alone and failed in
/// the full suite. Same class as the flake that named `ENV_LOCK` the single
/// home.
#[cfg(test)]
pub(crate) fn cap_env_lock() -> std::sync::MutexGuard<'static, ()> {
    tokenade_core::platform::ENV_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Claude Desktop's own Claude Code is found on all three layouts, the
    /// newest version wins, and a version dir without a binary is skipped.
    #[test]
    fn the_desktop_apps_claude_code_is_found_newest_first() {
        let h = std::env::temp_dir().join(format!("tk-deskclaude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        let put = |rel: &str| {
            let p = h.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"x").unwrap();
        };
        assert!(desktop_claude_binary(&h).is_none());
        put("AppData/Local/Packages/Claude_pzs8/LocalCache/Roaming/Claude/claude-code/2.1.205/1b3f/claude.exe");
        put("AppData/Local/Packages/Claude_pzs8/LocalCache/Roaming/Claude/claude-code/2.1.286/635c/claude.exe");
        std::fs::create_dir_all(h.join(".config/Claude/claude-code/2.1.999")).unwrap();
        let b = desktop_claude_binary(&h).unwrap();
        assert!(b.to_string_lossy().contains("2.1.286"), "{}", b.display());
        put("Library/Application Support/Claude/claude-code/2.1.300/claude");
        assert!(desktop_claude_binary(&h)
            .unwrap()
            .to_string_lossy()
            .contains("2.1.300"));
        let _ = std::fs::remove_dir_all(&h);
    }

    /// The env override must NEVER be cached: it is the knob tests and
    /// operators turn, and the machine probe behind it is memoised. Flip it
    /// both ways in one process and both answers must come back.
    #[test]
    fn the_capability_override_is_read_every_time() {
        let _g = cap_env_lock();
        // SAFETY: process-wide env, and the shared lock is held for the test.
        unsafe {
            std::env::set_var("TOKENADE_CAP_UPDATED_TOOL_OUTPUT", "1");
        }
        assert!(claude_supports_updated_tool_output());
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::set_var("TOKENADE_CAP_UPDATED_TOOL_OUTPUT", "0");
        }
        assert!(!claude_supports_updated_tool_output());
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::set_var("TOKENADE_CAP_UPDATED_TOOL_OUTPUT", "1");
        }
        assert!(
            claude_supports_updated_tool_output(),
            "the override was cached — turning it back on did nothing"
        );
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            std::env::remove_var("TOKENADE_CAP_UPDATED_TOOL_OUTPUT");
        }
    }

    /// Std-only tempdir (tokenade-cli has no tempfile dev-dep): unique
    /// per test invocation, left for the OS tmp cleaner.
    fn tmpdir(tag: &str) -> std::path::PathBuf {
        // The clock alone does not make this unique: `SystemTime` granularity
        // is coarse on some platforms (~15 ms on Windows), so parallel tests
        // can read the same nanos, and `create_dir_all` adopts an existing
        // directory silently. A process-wide counter makes the name unique by
        // construction; `create_dir` fails loudly on any residual collision.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "tokenade-test-{tag}-{}-{}-{seq}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir(&p).expect("test tempdir name must be unique");
        p
    }

    #[test]
    fn marker_scan_finds_and_misses() {
        let dir = tmpdir("cap-scan");
        let with = dir.join("with.bin");
        let without = dir.join("without.bin");
        // Marker buried mid-file among binary noise.
        let mut bytes = vec![0u8; 4096];
        bytes.extend_from_slice(b"xx updatedToolOutput yy");
        bytes.extend_from_slice(&[7u8; 4096]);
        std::fs::write(&with, &bytes).unwrap();
        std::fs::write(&without, vec![7u8; 8192]).unwrap();
        assert_eq!(
            file_contains_marker(&with, b"updatedToolOutput"),
            Some(true)
        );
        assert_eq!(
            file_contains_marker(&without, b"updatedToolOutput"),
            Some(false)
        );
        // Missing file → None (unreadable, not "absent capability").
        assert_eq!(
            file_contains_marker(&dir.join("nope"), b"updatedToolOutput"),
            None
        );
    }

    #[test]
    fn marker_straddling_chunk_overlap_logic_is_sound() {
        // Can't cheaply write an 8 MB file in a unit test; instead verify
        // the overlap arithmetic on the public scan with a marker at the
        // very end of the file (exercises the tail-keep path).
        let dir = tmpdir("cap-tail");
        let p = dir.join("tail.bin");
        let mut bytes = vec![b'a'; 100_000];
        bytes.extend_from_slice(b"updatedToolOutput");
        std::fs::write(&p, &bytes).unwrap();
        assert_eq!(file_contains_marker(&p, b"updatedToolOutput"), Some(true));
    }

    #[test]
    fn env_override_forces_both_ways() {
        let _g = cap_env_lock();
        std::env::set_var("TOKENADE_CAP_UPDATED_TOOL_OUTPUT", "1");
        assert!(claude_supports_updated_tool_output());
        std::env::set_var("TOKENADE_CAP_UPDATED_TOOL_OUTPUT", "0");
        assert!(!claude_supports_updated_tool_output());
        std::env::remove_var("TOKENADE_CAP_UPDATED_TOOL_OUTPUT");
    }

    #[test]
    fn version_probe_missing_binary_is_none() {
        // A binary that can't spawn is None → the probe's unwrap_or(false)
        // makes the capability conservative-false.
        assert_eq!(
            binary_version_at_least("tokenade-no-such-binary-xyz", (0, 26, 0)),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn version_probe_compares_gemini_floor() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmpdir("cap-gemver");
        let write_fake = |name: &str, ver: &str| -> String {
            let p = dir.join(name);
            std::fs::write(&p, format!("#!/bin/sh\necho \"{ver}\"\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p.to_string_lossy().into_owned()
        };
        // Command::new accepts an absolute path — no PATH mutation needed.
        let old = write_fake("gemini-old", "0.25.3");
        let floor = write_fake("gemini-floor", "0.26.0");
        let new = write_fake("gemini-new", "1.2.0 (gemini-cli)");
        let junk = write_fake("gemini-junk", "no version here");
        assert_eq!(binary_version_at_least(&old, (0, 26, 0)), Some(false));
        assert_eq!(binary_version_at_least(&floor, (0, 26, 0)), Some(true));
        assert_eq!(binary_version_at_least(&new, (0, 26, 0)), Some(true));
        assert_eq!(binary_version_at_least(&junk, (0, 26, 0)), None);
    }

    #[test]
    fn semver_triple_parses_version_banners() {
        assert_eq!(
            parse_semver_triple("2.1.205 (Claude Code)"),
            Some((2, 1, 205))
        );
        assert_eq!(
            parse_semver_triple("Claude Code v2.1.121"),
            Some((2, 1, 121))
        );
        assert_eq!(parse_semver_triple("no version here"), None);
        assert!(parse_semver_triple("2.1.121").unwrap() >= (2, 1, 121));
        assert!(parse_semver_triple("2.1.120").unwrap() < (2, 1, 121));
        assert!(parse_semver_triple("2.2.0").unwrap() > (2, 1, 121));
    }
}

/// The key Cursor's CLI reads back from a `postToolUse` hook to REPLACE an MCP
/// tool result.
const CURSOR_MCP_POST_MARKER: &[u8] = b"updated_mcp_tool_output";

/// Does the installed `cursor-agent` honour `updated_mcp_tool_output`?
///
/// The Cursor MCP folding hook was first wired while deliberately NOT letting
/// the MCP proxy step back for it, on a reported claim that Cursor's
/// `postToolUse` does not fire in the CLI. Measured 2026-09-11 against the
/// installed bundle (cursor-agent 2026.08.11), that claim is false. In
/// `190.index.js`, the MCP executor runs
/// `hookExecutor.executeHookForStep(afterMCPExecution, …)` then
/// `firePostToolUse(…)`, and:
///
/// ```text
/// if (void 0 !== (null == a ? void 0 : a.updated_mcp_tool_output)) try {
///   const e = a.updated_mcp_tool_output, t = [];
///   if ("string" == typeof e) t.push(new Me._Z({content:{case:"text", …}}))
///   else if (… "content" in e && Array.isArray(e.content)) …
/// ```
///
/// — it rebuilds the tool result from what the hook returned, string or
/// `{content:[…]}`. So the hook can deliver, and leaving the proxy wrapped on
/// top of it folds the same bytes twice, which is the defect Gemini was
/// carrying.
///
/// **The marker, not a version floor.** Only one cursor-agent version is
/// installed here, so a floor would be a guess dressed as a measurement; the
/// bundle either carries the key or it does not, and that is the capability
/// itself (prefer what the file declares). The launcher on PATH sits in
/// the same directory as the bundle it runs (`NODE_BIN="$SCRIPT_DIR/node"`),
/// so the scan is that directory's `*.js`: 18 MB and 24 ms cold on this
/// machine, and cached for a week afterwards like every other probe here.
pub(crate) fn cursor_supports_mcp_post_replace() -> bool {
    match std::env::var("TOKENADE_CAP_CURSOR_MCP_POST")
        .ok()
        .as_deref()
    {
        Some("1") => return true,
        Some("0") => return false,
        _ => {}
    }
    cached_probe(
        &cap_cache_path("cap-cursor-mcp-post"),
        CAP_CACHE_TTL_SECS,
        probe_cursor_mcp_post,
    )
}

fn probe_cursor_mcp_post() -> bool {
    let Some(launcher) = resolve_agent_binary("cursor-agent") else {
        return false;
    };
    launcher
        .parent()
        .is_some_and(bundle_declares_cursor_mcp_post)
}

/// The scan itself, taking the directory rather than finding it: a test can
/// then build a bundle instead of depending on which agents this machine
/// happens to have installed.
fn bundle_declares_cursor_mcp_post(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    // Bounded: a bundle is dozens of files, and a directory that is not one has
    // no business being walked without end.
    const MAX_FILES: usize = 400;
    for entry in rd.flatten().take(MAX_FILES) {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("js") {
            continue;
        }
        if file_contains_marker(&p, CURSOR_MCP_POST_MARKER) == Some(true) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod cursor_mcp_capability {
    use super::{bundle_declares_cursor_mcp_post, CURSOR_MCP_POST_MARKER};

    fn bundle(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tok-cursorcap-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for (f, body) in files {
            std::fs::write(d.join(f), body).unwrap();
        }
        d
    }

    /// The key is in ONE file of the set, not in the first: the real bundle
    /// has 75 of them and only one carries it.
    #[test]
    fn a_bundle_carrying_the_key_is_recognised() {
        let d = bundle(
            "yes",
            &[
                ("1.index.js", "console.log('nothing here')"),
                ("2.index.js", "const x = 1;"),
                (
                    "190.index.js",
                    "if(void 0!==a.updated_mcp_tool_output){const e=a.updated_mcp_tool_output;}",
                ),
            ],
        );
        assert!(bundle_declares_cursor_mcp_post(&d));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A bundle that does not carry it must answer NO, not "maybe": on that
    /// no, the proxy STAYS in place and MCP results keep being folded. Being
    /// wrong here does not degrade, it switches off.
    #[test]
    fn a_bundle_without_the_key_answers_no() {
        let d = bundle(
            "no",
            &[
                ("1.index.js", "const updated_input = {};"),
                ("2.index.js", "postToolUse('nope')"),
            ],
        );
        assert!(!bundle_declares_cursor_mcp_post(&d));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// And the key in a file that is not JS does not count: a changelog or a
    /// `.d.ts` that MENTIONS it does not prove it is implemented.
    #[test]
    fn the_key_outside_a_js_file_does_not_count() {
        let d = bundle(
            "txt",
            &[("CHANGELOG.md", "added support for updated_mcp_tool_output")],
        );
        assert!(!bundle_declares_cursor_mcp_post(&d));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The marker is the string Cursor reads, to the letter.
    #[test]
    fn the_marker_is_the_one_cursor_reads() {
        assert_eq!(CURSOR_MCP_POST_MARKER, b"updated_mcp_tool_output");
    }
}
