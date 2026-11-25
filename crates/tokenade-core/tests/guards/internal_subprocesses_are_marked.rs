//! Tokenade's own plumbing is never mistaken for work an agent asked for.
//!
//! `pack` asks `git ls-files` which files exist; `web` asks `curl` for a page;
//! the grep hook runs `rg`; a media read runs `ffprobe`. Tokenade reads all of
//! that ITSELF — none of it reaches an agent. Put any interception layer
//! between tokenade and those programs (the PATH shim shipped before 0.9.5,
//! which the installer removes but an install that never upgraded still
//! carries; a wrapper script; a shell function) and two things go wrong at
//! once. MEASURED 2026-09-04 with such a `git` first on PATH: one `tokenade
//! pack` booked `wrap / git-ls-files 8059 -> 249` plus a `recovery:offered`
//! row — a saving on 2,699 file paths nobody was going to pay for — and pack's
//! own answer came back 57 bytes different, because it had parsed the FOLD
//! instead of the file list.
//!
//! `process_util::internal_command` is the one rule. This sweep is what keeps a
//! new spawn site from quietly opting out of it.

use std::path::{Path, PathBuf};

/// Sites that spawn a program for the AGENT or the USER, each with whose work
/// it is. An entry here is a claim that a human read the call and decided.
const NOT_OUR_OWN_PLUMBING: &[(&str, &str)] = &[
    (
        "crates/tokenade-cli/src/wrap/spawn.rs",
        "the agent's command — compacting it is the product",
    ),
    (
        "crates/tokenade-cli/src/wrap/route.rs",
        "the agent's command, probed to know whether it exists before launching it",
    ),
    (
        "crates/tokenade-cli/src/wrap/bypass.rs",
        "`raw`, the user's escape hatch, and the uncompacted runner this sweep exists for",
    ),
    (
        "crates/tokenade-core/src/sandbox.rs",
        "the script `tokenade exec` was given to run",
    ),
    (
        "crates/tokenade-cli/src/mcp_proxy.rs",
        "the wrapped MCP server, whose results are folded on the way back",
    ),
    (
        "crates/tokenade-cli/src/diagnostics_cli/cleanup.rs",
        "a command the user typed, and `sudo` for it",
    ),
    (
        "crates/tokenade-core/src/updater.rs",
        "the newly downloaded binary, probed and then re-executed",
    ),
    (
        "crates/tk-base/src/process_util.rs",
        "our own executable, detached — and `internal_command` itself",
    ),
    ("crates/tokenade-cli/src/main.rs", "our own executable"),
    (
        "crates/tokenade-cli/src/commands/index_cmds.rs",
        "our own executable — the index rebuild handed to a child",
    ),
    (
        "crates/tokenade-cli/src/dashboard_cli/statusline.rs",
        "our own executable, and the status-line command the USER configured",
    ),
    (
        "crates/tokenade-cli/src/install/shell_proxy.rs",
        "the user's command, passed through when the proxy declines to wrap it",
    ),
    (
        "crates/tokenade-cli/src/install/legacy_shim_removal.rs",
        "the user's real command, re-run once after a stale shim is removed",
    ),
    (
        "crates/tokenade-cli/src/lifecycle/browser.rs",
        "the desktop browser opener",
    ),
    (
        "crates/tokenade-cli/src/bench_cli.rs",
        "the benchmark harness's own shell",
    ),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_sources(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

/// The unmarked spawn sites in one file: `(the file spawns anything at all,
/// the undeclared ones)`.
///
/// PRODUCTION lines only, and code only — naming `Command::new` in a doc
/// comment is prose, not a spawn.
fn unmarked_spawn_sites(rel: &str, body: &str) -> (bool, Vec<String>) {
    // A test harness spawns whatever it is testing. `production_lines` strips
    // `#[cfg(test)]` MODULES, and these are whole FILES carrying no attribute
    // of their own, so the path is what says so.
    if rel.contains("/tests/") || rel.ends_with("_tests.rs") {
        return (false, Vec::new());
    }
    // A declared exemption applies to the MODULE, not to the single file:
    // `updater.rs` declared, `updater/swap.rs` accused, although it is the
    // same module and the same reason. A split would lose the declaration and
    // it would be copied again — that is the same class of defect.
    let declared = NOT_OUR_OWN_PLUMBING.iter().any(|(p, _)| {
        rel.ends_with(p)
            || p.strip_suffix(".rs")
                .is_some_and(|trunk| rel.contains(&format!("{trunk}/")))
    });
    let mut any = false;
    let mut out = Vec::new();
    for (i, line) in tokenade_core::srcsweep::production_lines(body) {
        let code = line.split("//").next().unwrap_or("");
        if !code.contains("Command::new(") {
            continue;
        }
        any = true;
        if declared {
            continue;
        }
        out.push(format!("{rel}:{i}: {}", line.trim()));
    }
    (any, out)
}

/// The rule itself, on source that is not in the tree — so weakening the sweep
/// shows up here even when the tree is clean.
#[test]
fn the_rule_reports_an_unmarked_spawn_and_spares_a_declared_one() {
    let src = "fn f() {\n    let mut c = std::process::Command::new(\"git\");\n}\n";
    let (any, sites) = unmarked_spawn_sites("crates/tokenade-cli/src/somewhere.rs", src);
    assert!(any);
    assert_eq!(
        sites.len(),
        1,
        "an unmarked spawn must be reported: {sites:?}"
    );

    let (any, sites) = unmarked_spawn_sites("crates/tokenade-cli/src/mcp_proxy.rs", src);
    assert!(any);
    assert!(sites.is_empty(), "a declared one must be spared: {sites:?}");

    // …and its CHILDREN with it. A declared module that gets split keeps its
    // reason: `updater.rs` exempted, `updater/swap.rs` is too, otherwise the
    // split would bring back an accusation already settled.
    let (any, sites) = unmarked_spawn_sites("crates/tokenade-core/src/updater/swap.rs", src);
    assert!(any);
    assert!(
        sites.is_empty(),
        "the child of a declared module must be declared too: {sites:?}"
    );
    // The rule bears on the module's DIRECTORY, not on its prefix: a file
    // starting with the same name is not its child.
    let (_, sites) = unmarked_spawn_sites("crates/tokenade-core/src/updater_extra.rs", src);
    assert!(!sites.is_empty(), "a namesake neighbour is not exempted");

    // A test module's own spawns are its business, whether it is a `/tests/`
    // file or a `*_tests.rs` one.
    for t in [
        "crates/tokenade-cli/src/tests/misc.rs",
        "crates/tokenade-cli/src/llm_proxy/live_agent_tests.rs",
    ] {
        let (any, sites) = unmarked_spawn_sites(t, src);
        assert!(!any, "{t} is a test harness");
        assert!(sites.is_empty());
    }

    // The helper is not a spawn site, and neither is prose about one.
    let benign = "fn f() {\n    let mut c = internal_command(\"git\");\n    // was Command::new(\"git\")\n}\n";
    let (any, sites) = unmarked_spawn_sites("crates/tokenade-cli/src/somewhere.rs", benign);
    assert!(!any, "nothing here spawns anything directly");
    assert!(sites.is_empty());
}

#[test]
fn every_subprocess_tokenade_reads_itself_goes_through_internal_command() {
    let root = workspace_root();
    let mut files = Vec::new();
    for d in ["crates/tokenade-core/src", "crates/tokenade-cli/src"] {
        rust_sources(&root.join(d), &mut files);
    }
    assert!(files.len() > 100, "the sweep found no sources to read");

    let mut offenders = Vec::new();
    let mut seen_any = false;
    for path in &files {
        let Ok(body) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let (found, mut sites) = unmarked_spawn_sites(&rel, &body);
        seen_any |= found;
        offenders.append(&mut sites);
    }
    assert!(
        seen_any,
        "the sweep matched no spawn at all, so it proves nothing"
    );
    assert!(
        offenders.is_empty(),
        "a subprocess is spawned unmarked; if tokenade reads its output itself, use \
         `process_util::internal_command`, and if it runs what the agent or the user asked \
         for, declare the file in NOT_OUR_OWN_PLUMBING with whose work it is:\n  {}",
        offenders.join("\n  ")
    );
}
