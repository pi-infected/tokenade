//! Installing tokenade into each agent that hosts plugins.
//!
//! One function per agent (Hermes, pi, OpenCode, Kilo, Cursor, Cline,
//! OpenClaw): where its plugin directory lives, and what to write there.

mod bridges;
mod cursor;
pub(crate) use bridges::*;
pub(crate) use cursor::*;

use super::*;

/// Re-write every plugin bridge that is ALREADY installed, silently.
///
/// The upgrade path refreshed Claude Code's hooks and Gemini's, and nothing
/// else: `PLUGIN_INSTALLS` had exactly one caller, the interactive `tokenade
/// install`. So a Claude Code user got every bridge fix on upgrade and an
/// opencode / pi / openclaw / cline / kilo / hermes user kept the bridge from
/// the day they installed — for ever, unless they happened to re-run install by
/// hand. Every fix ever made to these files, including redaction correctness,
/// stopped at the release.
///
/// Same rule as the hook refresh, and it is what makes this safe: refresh only
/// where a prior install is PROVEN. The dropped file is the marker (it is
/// already the marker `any_agent_has_tokenade_in` uses), so this never creates
/// an install the user did not ask for — it only replaces bytes tokenade wrote.
///
/// Registration steps are deliberately NOT re-run. `cline plugin install` and
/// `openclaw plugins install` spawn a process and print to stderr; they belong
/// to an interactive install. What goes stale on upgrade is the file content,
/// and the registration still points at the same directory.
pub(crate) fn refresh_installed_plugin_bridges(home: &Path) {
    for (_label, marker, dir, assets) in plugin_bridges(home) {
        if !dir.join(marker).exists() {
            continue;
        }
        // Best-effort and silent, like every other surface this path refreshes.
        let _ = install_plugin_files(dir, false, assets, _label, |_| "plugin", |_| true);
    }
}

/// The file whose presence proves each plugin bridge is installed, its
/// directory, and what belongs in it. ONE list: the upgrade refresh walks it,
/// and [`any_agent_has_tokenade_in`] reads its markers.
///
/// They were two lists, and they had already drifted — the badge's copy never
/// knew about hermes, so a hermes-only user was told tokenade was wired
/// nowhere. Same drift as `PLUGIN_INSTALLS` versus the refresh table.
pub(crate) fn plugin_bridges(
    home: &Path,
) -> [(
    &'static str,
    &'static str,
    PathBuf,
    &'static [(&'static str, &'static str, bool)],
); 6] {
    let hermes_dir = hermes_home_dir(home).join("plugins").join("tokenade");
    let pi_dir = home.join(".pi").join("agent").join("extensions");
    let openclaw_dir = home.join(".tokenade").join("openclaw-plugin");
    let cline_dir = home.join(".tokenade").join("cline-plugin");
    let bridges: [(
        &'static str,
        &'static str,
        PathBuf,
        &'static [(&'static str, &'static str, bool)],
    ); 6] = [
        (
            "hermes",
            "__init__.py",
            hermes_dir,
            &[
                ("__init__.py", HERMES_PLUGIN_INIT, true),
                ("plugin.yaml", HERMES_PLUGIN_YAML, false),
            ],
        ),
        (
            "pi",
            "tokenade.ts",
            pi_dir,
            &[("tokenade.ts", PI_EXTENSION_TS, true)],
        ),
        (
            "opencode",
            "tokenade.ts",
            opencode_plugin_dir(home),
            &[("tokenade.ts", OPENCODE_PLUGIN_TS, true)],
        ),
        (
            "kilo-code",
            "tokenade.ts",
            kilo_plugin_dir(home),
            &[("tokenade.ts", OPENCODE_PLUGIN_TS, true)],
        ),
        (
            "openclaw",
            "index.js",
            openclaw_dir,
            &[
                ("index.js", OPENCLAW_PLUGIN_INDEX, true),
                ("package.json", OPENCLAW_PLUGIN_PKG, false),
                ("openclaw.plugin.json", OPENCLAW_PLUGIN_MANIFEST, false),
            ],
        ),
        (
            "cline",
            "tokenade.ts",
            cline_dir,
            &[
                ("tokenade.ts", CLINE_PLUGIN_TS, true),
                ("package.json", CLINE_PLUGIN_PKG, false),
            ],
        ),
    ];
    bridges
}

/// Cursor reads programmatic hooks from `~/.cursor/hooks.json`.
pub(crate) fn cursor_hooks_path(home: &Path) -> PathBuf {
    home.join(".cursor").join("hooks.json")
}

/// Dashboard hook badge: is tokenade wired into ANY supported agent on this
/// machine — not just Claude Code? Returns `Some(true)` if any agent carries
/// the tokenade hook/plugin, `Some(false)` if a known agent config exists but
/// none carry it, and `None` if no supported agent was found at all (badge
/// hidden). Fail-open: unreadable paths are skipped, never an error.
pub(crate) fn any_agent_has_tokenade() -> Option<bool> {
    let home = tokenade_core::platform::home_dir()?;
    any_agent_has_tokenade_in(&home)
}

/// Test seam for [`any_agent_has_tokenade`] over an explicit home dir.
pub(crate) fn any_agent_has_tokenade_in(home: &Path) -> Option<bool> {
    // Plugin/extension agents: the dropped file IS the install marker. Read
    // from `plugin_bridges` rather than a second copy — the copy that used to
    // live here had no hermes entry, so a hermes-only user was told tokenade
    // was wired nowhere while its bridge sat on disk.
    if plugin_bridges(home)
        .iter()
        .any(|(_, marker, dir, _)| dir.join(marker).exists())
    {
        return Some(true);
    }
    // Hook/settings agents: the config exists (agent present) and we look for
    // a tokenade hook command inside it (`hookwrap*` or `gemini-hook` family).
    // Substring is enough — those names are unambiguous across every agent's
    // config shape.
    //
    // Same reasoning: this list is `hook_agent_configs`, which the upgrade
    // refresh also walks. Its private copy here knew nothing about Droid,
    // Copilot or Reasonix, so those three answered "not wired" too.
    let mut any_present = false;
    for (c, _) in hook_agent_configs(home) {
        if let Ok(s) = std::fs::read_to_string(&c) {
            any_present = true;
            if crate::hook_health::config_proves_a_tokenade_hook(&s) {
                return Some(true);
            }
        }
    }
    any_present.then_some(false)
}

/// An installer with the shape every lifecycle hook agent shares.
pub(crate) type LifecycleInstaller = fn(&str, Option<&str>, Option<&Path>, bool) -> HookOutcome;

fn install_qwen_adapter(
    exe: &str,
    _dbg: Option<&str>,
    home: Option<&Path>,
    dry_run: bool,
) -> HookOutcome {
    install_qwen_cli_hook(exe, home, dry_run)
}

#[cfg(test)]
mod agent_cli_tests {
    use super::{agent_cli, install_cline_plugin};
    use std::path::Path;

    fn nowhere() -> &'static Path {
        Path::new("/nonexistent-home-for-agent-cli-tests")
    }

    /// A CLI that is not on PATH must come back `None`, so the caller can say
    /// "not installed here" instead of "the agent refused us". The two notes
    /// send the reader to different places.
    #[test]
    fn a_cli_nobody_installed_is_none() {
        assert!(agent_cli("tokenade-no-such-agent-cli-xyzzy", nowhere()).is_none());
    }

    /// And one that IS there resolves. On Windows this is the whole point:
    /// every npm-installed agent CLI is a `.cmd`, which `Command::new` alone
    /// never finds — measured on the Windows VM, where openclaw was installed
    /// and the bridge registration failed on every run.
    #[cfg(unix)]
    #[test]
    fn a_cli_on_path_resolves() {
        assert!(
            agent_cli("sh", nowhere()).is_some(),
            "sh is on PATH on every unix"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_batch_shim_runs_through_the_command_interpreter() {
        // `where.exe` is an .exe, so it must NOT be wrapped; the wrapping is
        // reserved for .cmd/.bat, which the loader cannot execute.
        let c = agent_cli("where", nowhere()).expect("where.exe is on PATH");
        assert!(!c
            .get_program()
            .to_string_lossy()
            .eq_ignore_ascii_case("cmd"));
    }

    /// **The registration must land in the home being installed into, not in
    /// the one this process happens to run from.**
    ///
    /// A fake `cline` on PATH writes down the `$HOME` it was handed. Without
    /// the pin it writes the ambient one, which is how 3 688 copies of the
    /// tokenade plugin — 129 MB — ended up under a developer's real
    /// `~/.cline/plugins/_installed/local` while every test involved was using
    /// a temp directory. Asserting on the CHILD's view is the only way to see
    /// it: the parent's own bookkeeping was correct throughout.
    #[cfg(unix)]
    #[test]
    fn a_spawned_registration_is_confined_to_the_target_home() {
        let base = std::env::temp_dir().join(format!("tok-pinhome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let bin = base.join("bin");
        let target_home = base.join("home");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&target_home).unwrap();
        let witness = base.join("child-home.txt");
        std::fs::write(
            bin.join("cline"),
            format!(
                "#!/bin/sh\nprintf '%s\\n%s' \"$HOME\" \"$USERPROFILE\" > {}\nexit 0\n",
                witness.display()
            ),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join("cline"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        // PATH is process-global; this test module is the only one that
        // touches it, and it restores what it found.
        let saved = std::env::var_os("PATH");
        // SAFETY: single-threaded test process; restored below.
        unsafe {
            std::env::set_var("PATH", &bin);
        }
        let registered = install_cline_plugin(&target_home, false);
        // SAFETY: test-only environment mutation. CI runs the suite under nextest, one process
        // per test, so no other thread reads the environment while it changes.
        unsafe {
            match &saved {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
        assert_eq!(registered, Ok(true), "the fake cline exits 0");

        let seen = std::fs::read_to_string(&witness)
            .expect("the fake cline never ran — the registration did not spawn it");
        let mut lines = seen.split('\n');
        let home_seen = lines.next().unwrap_or_default();
        // `USERPROFILE` is what Windows resolves a home from. A machine that
        // reads both must not see them disagree, so the child gets both — and
        // a unix test can still hold that line, because the child reports what
        // it was handed either way.
        let userprofile_seen = lines.next().unwrap_or_default();
        assert_eq!(
            Path::new(home_seen),
            target_home,
            "the registration was handed HOME={home_seen}, not the home being installed into"
        );
        assert_eq!(
            Path::new(userprofile_seen),
            target_home,
            "the registration was handed USERPROFILE={userprofile_seen}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
