//! **One entry: wrap it, recognise it, restore it.**
//!
//! The operation must be IDEMPOTENT — re-wrapping an already wrapped entry
//! does nothing — and REVERSIBLE: the original command is stored under
//! `tokenadeOriginal`, and that is what uninstalling gives back,
//! verbatim. Without that key, unwrapping would be guessing.

use super::*;

/// Sentinel metadata key under which we stash the original command +
/// args before rewriting. Picked to be unique enough that no real
/// MCP config field would collide.
pub const ORIGINAL_KEY: &str = "tokenadeOriginal";

/// The wrapped `command` fallback. Resolved from `env::current_exe()`
/// at wrap time (see [`wrapper_command_string`]); this bare-name
/// fallback is used only when the absolute path can't be determined
/// (tests, exotic platforms). Detection of entries WE wrapped keys off
/// [`ORIGINAL_KEY`], not the command string — so the command can vary across
/// re-installs without breaking idempotency. That rule stands; it just does
/// not cover the other way an entry can already be routed through tokenade,
/// which is a user writing `tokenade mcp-proxy -- <cmd>` by hand because the
/// CLAUDE.md block tokenade ships tells them to. Those carry no ORIGINAL_KEY,
/// so [`classify_command`] asks the narrower question about the command
/// string, and only that one.
pub const WRAPPER_COMMAND: &str = "tokenade";

/// Does this entry ALREADY launch its server through tokenade?
///
/// Two ways an entry can be routed through us, and only one was recognised.
/// `mcp-wrap install` records the original command under [`ORIGINAL_KEY`] and
/// skips anything carrying it. The other is the hand-written form above.
/// Measured 2026-09-03, a hand-routed github server came back as
/// `tokenade mcp-proxy -- tokenade mcp-proxy -- npx -y @…/server-github`:
/// every tool result folded twice, an extra process per call, and
/// `tokenade mcp-stats` reporting the server as unwrapped — asking the user to
/// wrap what was already wrapped.
///
/// Keyed on the command's BASENAME, because a config may spell it
/// `/usr/local/bin/tokenade` — the same reason the hook is basename-keyed. The
/// self-skip beside every call site asks whether the SERVER is named
/// "tokenade", which is a different question and answers no here.
/// What is this entry's command, as far as wrapping is concerned?
///
/// Two tokenade-shaped commands, two different answers, and the difference is
/// the first argument:
///   * `tokenade mcp-proxy -- <cmd>` is a third-party server the user routed
///     through us BY HAND, because the CLAUDE.md block tokenade ships tells
///     them to. It carries no `ORIGINAL_KEY`, so it was wrapped AGAIN —
///     measured 2026-09-03,
///     `tokenade mcp-proxy -- tokenade mcp-proxy -- npx -y @…/server-github`:
///     every tool result folded twice, an extra process per call, and
///     `tokenade mcp-stats` reporting it as unwrapped while asking the user to
///     wrap it. It counts as WRAPPED.
///   * any other `tokenade …` is tokenade's own server. Wrapping it would nest
///     us in ourselves, and it is not a third-party server, so it must not
///     count toward the wrapped fraction either. The self-skip beside each
///     call site catches this only when the entry is NAMED "tokenade"; renamed,
///     it fell through to `wrap_entry`.
///
/// Keyed on the command's BASENAME — a config may spell it
/// `/usr/local/bin/tokenade`, the same reason the hook is basename-keyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryCommand {
    /// Not tokenade — wrap it.
    ThirdParty,
    /// `tokenade mcp-proxy -- …`: already routed through us by hand.
    RoutedByHand,
    /// tokenade itself, doing something else.
    OurOwn,
    /// A server the HOST APP installed and manages: its executable lives in
    /// the app's own runtime (the Codex desktop app writes `node_repl` —
    /// browser and computer-use — from `…\OpenAI\Codex\runtimes\…` into
    /// `~/.codex/config.toml`, measured 2026-10-06 on Windows) or in an app
    /// package/bundle. The app rewrites that entry when it updates, and its
    /// results are the app's own protocol: not ours to wrap.
    HostApp,
}

/// Is this executable inside a desktop app's own install or runtime?
pub(crate) fn lives_in_a_host_app(cmd: &str) -> bool {
    let p = cmd
        .trim_matches(|c| c == '\'' || c == '"')
        .replace('\\', "/")
        .to_ascii_lowercase();
    [
        "/openai/codex/",
        // Linux and macOS: the app's runtime root (`~/.cache/codex-runtimes`,
        // read in the ChatGPT 26.930 Linux bundle) and the Linux packages.
        "/.cache/codex-runtimes/",
        "/usr/lib/chatgpt/",
        "/usr/lib/claude-desktop/",
        "/windowsapps/",
        ".app/contents/",
        "/anthropicclaude/",
        "/packages/claude_",
    ]
    .iter()
    .any(|m| p.contains(m))
}

pub(crate) fn classify_command(command: Option<&str>, first_arg: Option<&str>) -> EntryCommand {
    let Some(cmd) = command else {
        return EntryCommand::ThirdParty;
    };
    if lives_in_a_host_app(cmd) {
        return EntryCommand::HostApp;
    }
    let base = cmd
        .trim_matches(|c| c == '\'' || c == '"')
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(cmd);
    let base = base.strip_suffix(".exe").unwrap_or(base);
    if base != "tokenade" && base != "tokenade-mcp" {
        return EntryCommand::ThirdParty;
    }
    match first_arg.map(|a| a.trim_matches(|c| c == '\'' || c == '"')) {
        Some("mcp-proxy") => EntryCommand::RoutedByHand,
        _ => EntryCommand::OurOwn,
    }
}

pub(super) fn wrapper_command_string() -> String {
    // `stable_exe`, not `current_exe`. The comment above reasons that this
    // should "match the install asymmetry: same absolute path everywhere" —
    // true, and the path `install` used was itself the bug.
    // Measured 2026-08-08: `tokenade mcp-proxy install` run from a build
    // binary rewrote a real MCP entry to `…/target/release/tokenade`, so one
    // `cargo clean` and the agent's MCP servers stop launching.
    crate::platform::stable_exe()
}

/// First arg the wrapper inserts to identify the proxy subcommand.
pub const WRAPPER_SUBCOMMAND: &str = "mcp-proxy";

/// The entry's environment key naming the app whose config was wrapped. An
/// environment variable and not a flag: an older tokenade that meets it in a
/// config ignores it, where an unknown flag would stop the server.
pub const AGENT_ENV: &str = "TOKENADE_MCP_AGENT";

pub(super) fn put_agent_env(obj: &mut serde_json::Map<String, Value>, agent: Option<&str>) {
    let Some(agent) = agent else { return };
    if let Value::Object(env) = obj
        .entry("env")
        .or_insert_with(|| Value::Object(Default::default()))
    {
        env.insert(AGENT_ENV.into(), Value::String(agent.into()));
    }
}

fn drop_agent_env(obj: &mut serde_json::Map<String, Value>) {
    if let Some(Value::Object(env)) = obj.get_mut("env") {
        env.remove(AGENT_ENV);
        if env.is_empty() {
            obj.remove("env");
        }
    }
}

/// Optional separator the proxy uses to delimit its own flags (none
/// today) from the target command — same convention as `--` in
/// most CLI tools.
pub const PROXY_ARGV_SEPARATOR: &str = "--";

/// Auto-heal an already-wrapped entry whose `command` still points at
/// the bare wrapper name `WRAPPER_COMMAND` ("tokenade"). Returns true
/// when a refresh was needed AND applied. Idempotent: subsequent calls
/// see the absolute path and return false.
///
/// Handles both wrap shapes (object with `command` as string, and Codex
/// array shape where `command` is an array starting with the wrapper).
/// Preserves `args` / `original` / every other key verbatim.
pub(super) fn refresh_stale_wrapper(obj: &mut serde_json::Map<String, Value>) -> bool {
    let wrapper = wrapper_command_string();
    // Refresh is only meaningful when the wrapper is NOT already the
    // absolute path (`current_exe` lookup failed and we'd be replacing
    // bare with bare — no win, skip).
    if wrapper == WRAPPER_COMMAND {
        return false;
    }
    match obj.get_mut("command") {
        Some(Value::String(s)) if s == WRAPPER_COMMAND => {
            *s = wrapper;
            true
        }
        Some(Value::Array(arr)) => {
            if let Some(Value::String(first)) = arr.first_mut() {
                if first == WRAPPER_COMMAND {
                    *first = wrapper;
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// Rewrite a single MCP entry object in place. Returns `Err(())` when
/// the entry shape is unrecognised (caller logs + skips).
pub(super) fn wrap_entry(
    obj: &mut serde_json::Map<String, Value>,
    agent: Option<&str>,
) -> Result<(), ()> {
    // Two known shapes:
    //   (A) object: `{ "command": "<bin>", "args": [<args>], "env": {...}? }`
    //   (B) Codex:  `{ "command": ["<bin>", <args>...] }`
    // We always rewrite to the SAME shape we found.
    // Use absolute path so wrap entries work for GUI agents that
    // don't inherit user PATH.
    let wrapper = wrapper_command_string();
    if let Some(cmd) = obj.get("command").cloned() {
        match cmd {
            Value::String(orig_bin) => {
                let orig_args = obj
                    .get("args")
                    .and_then(|a| a.as_array())
                    .cloned()
                    .unwrap_or_default();
                // Build original-payload snapshot.
                let mut original = serde_json::Map::new();
                original.insert("command".into(), Value::String(orig_bin.clone()));
                original.insert("args".into(), Value::Array(orig_args.clone()));
                // Rewrite to wrapper form.
                obj.insert("command".into(), Value::String(wrapper));
                let mut new_args: Vec<Value> = vec![
                    Value::String(WRAPPER_SUBCOMMAND.into()),
                    Value::String(PROXY_ARGV_SEPARATOR.into()),
                    Value::String(orig_bin),
                ];
                new_args.extend(orig_args);
                obj.insert("args".into(), Value::Array(new_args));
                obj.insert(ORIGINAL_KEY.into(), Value::Object(original));
                put_agent_env(obj, agent);
                Ok(())
            }
            Value::Array(orig_argv) if !orig_argv.is_empty() => {
                let mut original = serde_json::Map::new();
                original.insert("command".into(), Value::Array(orig_argv.clone()));
                let mut new_argv: Vec<Value> = vec![
                    Value::String(wrapper),
                    Value::String(WRAPPER_SUBCOMMAND.into()),
                    Value::String(PROXY_ARGV_SEPARATOR.into()),
                ];
                new_argv.extend(orig_argv);
                obj.insert("command".into(), Value::Array(new_argv));
                obj.insert(ORIGINAL_KEY.into(), Value::Object(original));
                put_agent_env(obj, agent);
                Ok(())
            }
            _ => Err(()),
        }
    } else {
        Err(())
    }
}

/// The launch command an unwrap should restore, read from the LIVE wrapped
/// entry rather than from the snapshot taken when it was wrapped.
///
/// A wrapped entry spells the original command out in full, after the `--`:
/// `["mcp-proxy", "--", "npx", "-y", "@scope/server"]`. That tail is what the
/// server actually launches today. `tokenadeOriginal` is what it launched the
/// day it was wrapped, and the two part company the moment the user edits the
/// entry — which is exactly how an MCP server is upgraded, by bumping its
/// version in the config.
///
/// Measured 2026-09-01: wrap a server, bump it to `@2.0.0` and raise its
/// timeout the way a user would, then unwrap. `env` and `timeout` came back
/// intact — they are not ours to rewrite — and `args` were silently reverted
/// to the snapshot, so the server went back to the old version with nothing in
/// the output saying an edit had been discarded. Same shape as the addon
/// registry that replaced the user's credentials wholesale on upgrade: the
/// defect is not in the first run, it is in what happens the second time.
///
/// The snapshot stays as the fallback, for an entry whose tail cannot be read
/// (no `--`, or nothing after it) — and it is still what proves the entry is
/// ours to unwrap in the first place.
pub(super) fn original_from_live_tail(args: &[Value]) -> Option<(Value, Value)> {
    // The FIRST standalone `--` is the separator the wrapper inserted; any
    // later one belongs to the server's own arguments and must survive.
    let sep = args.iter().position(|v| v.as_str() == Some("--"))?;
    let tail = &args[sep + 1..];
    let (bin, rest) = tail.split_first()?;
    if !bin.is_string() {
        return None;
    }
    Some((bin.clone(), Value::Array(rest.to_vec())))
}

/// The TOML spelling of the same choice [`original_from_live_tail`] makes for
/// JSON: read the live wrapped `args`, and fall back to the snapshot when the
/// entry carries no readable tail.
pub(super) fn toml_live_or_snapshot(
    entry: &toml::value::Table,
    snapshot_bin: &str,
    snapshot_args: Vec<String>,
) -> (String, Vec<String>) {
    let live = entry
        .get("args")
        .and_then(|a| a.as_array())
        .and_then(|a| {
            let vals: Vec<Value> = a
                .iter()
                .map(|v| Value::String(v.as_str().unwrap_or_default().to_string()))
                .collect();
            original_from_live_tail(&vals)
        })
        .and_then(|(bin, rest)| {
            let bin = bin.as_str()?.to_string();
            let rest: Vec<String> = rest
                .as_array()?
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect();
            Some((bin, rest))
        });
    live.unwrap_or_else(|| (snapshot_bin.to_string(), snapshot_args))
}

pub(super) fn unwrap_entry(obj: &mut serde_json::Map<String, Value>) -> Result<(), ()> {
    let Some(orig_val) = obj.remove(ORIGINAL_KEY) else {
        return Err(());
    };
    drop_agent_env(obj);
    let Value::Object(orig) = orig_val else {
        // Malformed backup — re-stash and bail rather than corrupt.
        obj.insert(ORIGINAL_KEY.into(), orig_val);
        return Err(());
    };
    // Prefer what the entry launches TODAY over what it launched when it was
    // wrapped; see `original_from_live_tail`. Object-shape entries only: an
    // array-shape entry keeps its whole command line under `command`, and that
    // case is restored from the snapshot below exactly as before.
    let live = obj
        .get("args")
        .and_then(|a| a.as_array())
        .and_then(|a| original_from_live_tail(a));
    if let (Some((bin, rest)), true) = (live, orig.get("args").is_some()) {
        obj.insert("command".into(), bin);
        obj.insert("args".into(), rest);
        return Ok(());
    }
    if let Some(cmd) = orig.get("command").cloned() {
        obj.insert("command".into(), cmd);
    } else {
        // Audit: bailing here had already dropped the backup — the entry
        // stayed wrapped AND lost its original forever. Re-stash first.
        obj.insert(ORIGINAL_KEY.into(), Value::Object(orig));
        return Err(());
    }
    // `args` is only present in object-shape entries.
    if let Some(args) = orig.get("args").cloned() {
        obj.insert("args".into(), args);
    } else {
        // Array-shape entries don't carry a separate args field —
        // drop the wrapper-only one if it was added.
        obj.remove("args");
    }
    Ok(())
}

#[cfg(test)]
mod host_app_tests {
    use super::*;

    /// The app is named in the wrapped entry's env, and only our key leaves
    /// on unwrap: the user's own variables stay, an env we created goes.
    #[test]
    fn the_app_is_named_in_the_entry_and_unwrap_takes_only_that_back() {
        let mut with_env: serde_json::Map<String, Value> =
            serde_json::from_str(r#"{"command":"node","args":["x.js"],"env":{"A":"1"}}"#).unwrap();
        wrap_entry(&mut with_env, Some("claude-desktop")).unwrap();
        assert_eq!(with_env["env"][AGENT_ENV], "claude-desktop");
        unwrap_entry(&mut with_env).unwrap();
        assert_eq!(
            Value::Object(with_env.clone())["env"],
            serde_json::json!({"A":"1"})
        );
        let mut bare: serde_json::Map<String, Value> =
            serde_json::from_str(r#"{"command":"node","args":["x.js"]}"#).unwrap();
        wrap_entry(&mut bare, Some("cursor")).unwrap();
        unwrap_entry(&mut bare).unwrap();
        assert!(!bare.contains_key("env"), "{bare:?}");
        let mut unnamed: serde_json::Map<String, Value> =
            serde_json::from_str(r#"{"command":"node","args":[]}"#).unwrap();
        wrap_entry(&mut unnamed, None).unwrap();
        assert!(!unnamed.contains_key("env"));
    }

    /// The Codex desktop app's own `node_repl` (Windows, measured), an MSIX
    /// package and a macOS bundle are the app's; a server the user installed
    /// is wrapped as before.
    #[test]
    fn a_server_the_host_app_manages_is_not_wrapped() {
        for cmd in [
            r"C:\Users\V\AppData\Local\OpenAI\Codex\runtimes\cua_node\7d8d\bin\node_repl.exe",
            r"C:\Program Files\WindowsApps\Claude_2.1_x64__pzs8\app\resources\x.exe",
            "/Applications/Codex.app/Contents/Resources/node_repl",
            "/home/v/.cache/codex-runtimes/cua_node/7d8d/bin/node_repl",
            "/usr/lib/chatgpt/resources/codex",
            "/usr/lib/claude-desktop/resources/chrome-native-host",
        ] {
            assert_eq!(
                classify_command(Some(cmd), None),
                EntryCommand::HostApp,
                "{cmd}"
            );
        }
        for cmd in [
            "node",
            "npx",
            r"C:\Users\V\AppData\Roaming\npm\mcp-server.cmd",
            "/usr/local/bin/uvx",
        ] {
            assert_eq!(
                classify_command(Some(cmd), None),
                EntryCommand::ThirdParty,
                "{cmd}"
            );
        }
    }
}
