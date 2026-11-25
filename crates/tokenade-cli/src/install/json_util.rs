#![allow(unused_imports)]
//! Mechanical module split of install.rs (no behaviour change). Wired
//! back into the `install` namespace by mod.rs via `pub(crate) use <name>::*;`.
use super::*;
use crate::hooks::*;
use crate::wrap::*;
use crate::*;
use std::fs;
use std::path::{Path, PathBuf};

/// Get-or-create `obj[key]` as an object, replacing a non-object value. A
/// user file may hold `"hooks": []` or `null`; unwrapping `as_object_mut()`
/// on what `entry().or_insert()` returned panicked the installer on it.
pub(crate) fn ensure_object_entry<'a>(
    obj: &'a mut serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> &'a mut serde_json::Map<String, serde_json::Value> {
    let v = obj.entry(key).or_insert(serde_json::json!({}));
    if !v.is_object() {
        *v = serde_json::json!({});
    }
    v.as_object_mut().expect("ensured object two lines up")
}

/// Defensive `entry().or_insert([]).as_array_mut().unwrap()` (#486).
/// If the user has hand-edited `~/.claude/settings.json` and parked a
/// non-array value (object, string, number) at `hooks.PreToolUse`
/// etc., the naive `as_array_mut().unwrap()` panics. Replace any
/// non-array existing value with `[]` so the installer can still
/// extend it. Clobbers user-set non-array values — they were almost
/// certainly typos or stale config from a prior unrelated tool.
pub(crate) fn ensure_array_entry<'a>(
    obj: &'a mut serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> &'a mut Vec<serde_json::Value> {
    let v = obj.entry(key).or_insert(serde_json::json!([]));
    if !v.is_array() {
        *v = serde_json::json!([]);
    }
    // Safe: just guaranteed it's an array on the line above.
    v.as_array_mut().expect("ensured array two lines up")
}

/// Decide whether `group` is a tokenade-installed hook entry of the kind
/// identified by `tag` (e.g. `"tokenade-pretool"`) and `subcmd` (the
/// dispatch verb, e.g. `"hookwrap-pretool"`). Returns
/// `(matched, has_tag)`:
///   - `matched=true` when the group is recognisably a tokenade entry of
///     this kind, either via `_tag == tag` (preferred) OR via the inner
///     command ending in the dispatch subcommand.
///   - `has_tag=false` for the untagged case — caller should treat as
///     stale + replace, so the rewrite re-adds the `_tag` marker.
///
/// Why the fallback matters (#775): Claude Code's settings editor (and
/// some pre-#604 install paths) drops unknown JSON fields like our
/// `_tag` when re-serializing `settings.json`. Without the fallback,
/// the next `tokenade install` would not recognise the existing entry
/// and would push a duplicate.
pub(crate) fn group_matches_tokenade_hook(
    group: &serde_json::Value,
    tag: &str,
    subcmd: &str,
) -> (bool, bool) {
    // Inspect EVERY entry of the group, not just the first: a tokenade
    // entry merged second into a user's group was invisible to both
    // idempotence (→ duplicate installs) and drift-heal.
    let Some(entries) = group.get("hooks").and_then(|h| h.as_array()) else {
        return (false, false);
    };
    if entries.iter().any(|inner| {
        inner
            .get("_tag")
            .and_then(|t| t.as_str())
            .map(|s| s == tag)
            .unwrap_or(false)
    }) {
        return (true, true);
    }
    // Empty `subcmd` would expand to `format!(" {} ", "")` == `"  "` →
    // matches ANY command with a space (i.e. almost everything). Caller
    // bug: forgot to map a new tag to its dispatch subcommand. Refuse
    // to false-positive — return `(false, false)` so the caller pushes
    // a fresh tagged entry rather than treating an unrelated hook as
    // stale. Pinned by `group_matches_tokenade_hook_empty_subcmd_safe`.
    if subcmd.is_empty() {
        return (false, false);
    }
    // Untagged: match on the dispatch subcommand. Anchor on a LEADING
    // space (binary-path → subcommand boundary) so we don't false-
    // positive on suffix overlap (e.g. `hookwrap` vs `hookwrap-pretool`).
    // Allow shell-quoted (`'`/`"`) and bare (` ` / EOL) trailing delimiters.
    let by_space = format!(" {subcmd} ");
    let by_squote = format!(" {subcmd}'");
    let by_dquote = format!(" {subcmd}\"");
    let by_end = format!(" {subcmd}");
    let matched_subcmd = entries.iter().any(|inner| {
        let cmd = inner.get("command").and_then(|c| c.as_str()).unwrap_or("");
        cmd.contains(&by_space)
            || cmd.contains(&by_squote)
            || cmd.contains(&by_dquote)
            || cmd.ends_with(&by_end)
    });
    (matched_subcmd, false)
}

/// Read a JSON config we intend to REWRITE. Absent file → empty object.
/// UNPARSABLE file (hand-edit, merge conflict) → Err, so the caller bails
/// without writing: the old `.ok()…unwrap_or(json!({}))` pattern silently
/// replaced the user's whole file with a tokenade-only document. Valid
/// JSON that isn't an object → Err for the same reason.
/// Read a TEXT config we intend to REWRITE. Absent file → empty string, which
/// is what every caller wants for a first install. Present but unreadable →
/// `Err`, because these callers all build `original + our block` and write the
/// result: an empty `original` truncates the user's file down to our block
/// alone. Text twin of [`read_json_object_or_bail`], and the same reasoning —
/// the only failure that legitimately means "nothing there yet" is NotFound.
pub(crate) fn read_text_or_bail(path: &std::path::Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(r) => Ok(r),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!(
            "cannot read {} — left untouched: {e}",
            path.display()
        )),
    }
}

pub(crate) fn read_json_object_or_bail(
    path: &std::path::Path,
) -> Result<serde_json::Value, String> {
    let raw = match std::fs::read_to_string(path) {
        // ABSENT is the only failure that legitimately means "empty config".
        // Every other one — permission denied, non-UTF-8 bytes, a directory in
        // the way, EIO — means the file EXISTS and holds keys we could not see,
        // and answering `{}` hands the caller a document it will write straight
        // back, destroying every one of them. That is the same silent-clobber
        // this function's own doc comment was written to close; it was closed on
        // the PARSE path and left open on the READ path.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(serde_json::json!({})),
        Err(e) => {
            return Err(format!(
                "cannot read {} — left untouched: {e}",
                path.display()
            ))
        }
        Ok(r) => r,
    };
    // UTF-8 BOM (PowerShell Set-Content default on old versions) makes
    // strict serde reject an otherwise-valid file — strip it (audit).
    let raw_stripped = raw.trim_start_matches('\u{feff}');
    match serde_json::from_str::<serde_json::Value>(raw_stripped) {
        Err(e) => Err(format!(
            "unparsable {} — left untouched: {e}",
            path.display()
        )),
        Ok(v) if !v.is_object() => Err(format!(
            "{} is valid JSON but not an object — left untouched",
            path.display()
        )),
        Ok(v) => Ok(v),
    }
}

#[cfg(test)]
mod json_util_tests {
    use super::read_json_object_or_bail;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tokenade-jsonutil-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_absent_file_is_a_legitimately_empty_config() {
        let p = tmp("absent").join("settings.json");
        assert_eq!(read_json_object_or_bail(&p).unwrap(), serde_json::json!({}));
    }

    /// A config that EXISTS but cannot be decoded must never read as `{}`.
    /// Every caller of this function does read-modify-write on an agent's
    /// settings, so an empty answer is written back and the user's keys are
    /// gone. Non-UTF-8 bytes (a latin-1 hand edit, a truncated write) are the
    /// root-proof way to reach that arm.
    #[test]
    fn a_present_but_undecodable_file_bails_instead_of_reading_as_empty() {
        let p = tmp("badbytes").join("settings.json");
        std::fs::write(&p, [0xff, 0xfe, 0x00, 0x80, 0x81]).unwrap();
        let err = read_json_object_or_bail(&p).expect_err("must refuse to guess");
        assert!(
            err.contains("cannot read") && err.contains("left untouched"),
            "unhelpful refusal: {err}"
        );
    }

    /// Same rule for a file we are not allowed to open. Skipped when the test
    /// runs as root, which can read it anyway.
    #[cfg(unix)]
    #[test]
    fn a_present_but_unreadable_file_bails_instead_of_reading_as_empty() {
        use std::os::unix::fs::PermissionsExt;
        let p = tmp("noperm").join("settings.json");
        std::fs::write(&p, "{\"userKey\":1}").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_to_string(&p).is_ok() {
            return; // root: the premise does not hold here
        }
        assert!(
            read_json_object_or_bail(&p).is_err(),
            "an unreadable config read as an empty one — the caller would write \
             it back and drop every user key"
        );
    }
}
