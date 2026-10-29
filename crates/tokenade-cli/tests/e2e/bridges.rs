//! Do the plugin bridges actually FOLD anything?
//!
//! Rank 1bis of the integration ladder (plugin bridges) covers six agents —
//! opencode, kilo, pi, openclaw, cline, hermes — through five shipped asset
//! files. Every test they had asserted on the TEXT of those files: that the
//! redaction signal appears, that the stale-binary fallback is written, that
//! the memo key hashes the whole payload. All useful, and none of them ran a
//! single line of the assets.
//!
//! So the channel that six agents depend on was, until 2026-08-08, verified by
//! reading. These tests load each asset in its real runtime, hand it the shape
//! its host hands it, and check what comes back — with the real tokenade binary
//! behind it.
//!
//! Skipped, loudly, when the runtime is missing: node with type-stripping
//! (Node 22+) for the four JS/TS bridges, python3 for Hermes. A skip prints
//! why, because a bridge test that silently no-ops is worth less than no test.

// The whole file is cfg(unix) — Windows cross-checks the TEST code too
// (D5), and an import used only by gated tests is dead there.
#[cfg(unix)]
use super::*;
use std::path::Path;
#[cfg(unix)]
use std::process::Command;

/// 600 log lines — well past every bridge's `MIN_CHARS` (1000) and shaped like
/// what a shell tool actually returns.
#[cfg(unix)] // only cfg(unix) tests use it (D5 cross-check)
fn log_body() -> String {
    (0..600)
        .map(|i: usize| {
            format!(
                "2026-08-08T10:{:02}:{:02}Z INFO worker[{}] handled path=/api/v1/items status=200 dur={}ms",
                i / 60,
                i % 60,
                i % 8,
                10 + i % 40
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(unix)] // only cfg(unix) tests use it (D5 cross-check)
fn have(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn asset(name: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("asset {}: {e}", p.display()))
}

/// The four JS/TS bridges, each driven through its host's real entry shape.
///
/// One test rather than four: they share a scratch dir, a stub `openclaw`
/// package and a node invocation, and splitting them would pay node's startup
/// four times for no extra signal.
#[cfg(unix)]
#[test]
fn e2e_the_javascript_bridges_actually_fold_a_tool_result() {
    if !have("node", &["--version"]) {
        eprintln!("SKIP: node not on PATH — the JS/TS bridges are untested here");
        return;
    }
    let h = TempHome::new("bridges-js");
    let dir = h.path();
    std::fs::write(dir.join("oc.ts"), asset("opencode_plugin/tokenade.ts")).unwrap();
    std::fs::write(dir.join("pi.ts"), asset("pi_extension/tokenade.ts")).unwrap();
    std::fs::write(dir.join("oclaw.mjs"), asset("openclaw_plugin/index.js")).unwrap();
    std::fs::write(dir.join("cline.ts"), asset("cline_plugin/tokenade.ts")).unwrap();

    // No SDK stub any more, and its absence is the assertion: the rewritten
    // plugin (D9) imports NOTHING from the host. The previous asset imported
    // `definePluginEntry` from `openclaw/plugin-sdk/plugin-entry`, a subpath
    // that openclaw 2026.1.30 does not export — so the old test could only pass
    // because it stubbed the very thing that was broken in production.

    let body = log_body();
    std::fs::write(dir.join("body.txt"), &body).unwrap();
    // `git log --oneline` is the payload that isolates the command HINT:
    // `tokenade read -` leaves it untouched without `--cmd` and folds it ~6x
    // with it. So on this input "did the hint arrive" and "did anything fold"
    // are the same question, which is what makes it a usable probe.
    let git = (0..400)
        .map(|i: usize| {
            format!(
                "{:08x} commit subject number {i} touching a few files",
                i * 2_654_435_761u64 as usize
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("git.txt"), &git).unwrap();
    let driver = r#"
import fs from "node:fs";
const body = fs.readFileSync("./body.txt", "utf8");
const out = {};

const oc = (await import("./oc.ts")).default;
// opencode 2 requires a DEFINITION object ({id, setup}); opencode 1 and Kilo
// Code still call `server()`. Both entry shapes must exist, and both must fold.
out.opencode_v2_definition = typeof oc.id === "string" && typeof oc.setup === "function";
const ocHooks = await oc.server();
const o1 = { output: body };
await ocHooks["tool.execute.after"]({ tool: "bash", args: { command: "journalctl -n 600" } }, o1);
out.opencode = o1.output.length;
// A live terminal session is not a finished result — it must be left alone.
const o2 = { output: body };
await ocHooks["tool.execute.after"]({ tool: "terminal", args: {} }, o2);
out.opencode_terminal = o2.output.length;

// The v2 path: one mutable event instead of (input, output), and hooks are
// registered on their domain. Driven through a stub context — the shape the
// real loader hands `setup`.
{
  const v2 = {};
  await oc.setup({
    tool: { hook: async (n, fn) => { v2[n] = fn; return { dispose: async () => {} }; } },
    event: { subscribe: async function* () {} },
  });
  const e1 = { tool: "bash", sessionID: "s1", input: { command: "journalctl -n 600" },
               status: "completed", result: { output: body } };
  await v2["execute.after"](e1);
  out.opencode_v2 = e1.result.output.length;
  const e2 = { tool: "terminal", sessionID: "s1", input: {}, status: "completed",
               result: { output: body } };
  await v2["execute.after"](e2);
  out.opencode_v2_terminal = e2.result.output.length;
  // The real shell tool: structured `output`, the model reads `content`
  // (payload part + exit notice part).
  const structured = { output: body, exit: 0, truncated: false };
  const e3 = { tool: "shell", sessionID: "s1", input: { command: "journalctl -n 600" },
               status: "completed",
               result: { output: structured, content: [{ type: "text", text: body },
                                                      { type: "text", text: "exit 1" }] } };
  await v2["execute.after"](e3);
  out.opencode_v2_content = e3.result.content[0].text.length;
  out.opencode_v2_notice_kept = e3.result.content[1].text === "exit 1";
  out.opencode_v2_output_kept = e3.result.output === structured && structured.output === body;
  // Content EMPTY and `output` structured: the host falls back to `output`,
  // and that text is what the model reads. An adapter that only handled a
  // STRING `output` folded nothing here.
  const only = { output: body, exit: 0, truncated: false };
  const e4 = { tool: "shell", sessionID: "s1", input: { command: "journalctl -n 600" },
               status: "completed", result: { output: only, content: [] } };
  await v2["execute.after"](e4);
  out.opencode_v2_output_only = only.output.length;
  out.opencode_v2_output_only_kept = e4.result.output === only
    && Array.isArray(e4.result.content) && e4.result.content.length === 0;
}

// The 2.x compaction event. The bus is an ASYNC ITERABLE of `{ type, data }`;
// the old adapter listened for 1.x's `session.compacted` / `properties`, which
// 2.x never emits (the 2.2.1 binary contains no such string at all), so
// `session-tick --reset` was dead code. A stub binary records the argv it
// receives, which is the only observable side of the reset.
{
  const stub = process.cwd() + "/tick-stub.sh";
  const tickLog = process.cwd() + "/tick.log";
  fs.writeFileSync(stub, '#!/bin/sh\nprintf "%s\\n" "$*" >> "$TOKENADE_TICK_LOG"\n');
  fs.chmodSync(stub, 0o755);
  // The stub is scoped to this block: a bridge imported AFTER it resolves the
  // binary on load, so leaving it set would hand `pi` (below) the stub and
  // silently un-fold it.
  const realBin = process.env.TOKENADE_BIN;
  process.env.TOKENADE_BIN = stub;
  process.env.TOKENADE_TICK_LOG = tickLog;
  // A fresh module instance: the one above already resolved the real binary.
  const ocEvent = (await import("./oc.ts?compaction-event")).default;
  const feed = [];
  const waiting = [];
  const send = (ev) => { const w = waiting.shift(); if (w) w(ev); else feed.push(ev); };
  await ocEvent.setup({
    tool: { hook: async () => ({ dispose: async () => {} }) },
    event: { subscribe: () => (async function* () {
      for (;;) {
        const next = feed.length ? feed.shift() : await new Promise((r) => waiting.push(r));
        yield next;
      }
    })() },
  });
  send({ type: "session.compaction.ended", data: { sessionID: "ses_compact" } });
  let tick = "";
  for (let i = 0; i < 100 && !tick; i++) {
    await new Promise((r) => setTimeout(r, 20));
    try { tick = fs.readFileSync(tickLog, "utf8").trim(); } catch {}
  }
  out.opencode_v2_compaction_tick = tick;
  process.env.TOKENADE_BIN = realBin;
}

const pi = (await import("./pi.ts")).default;
let piHandler = null;
pi({ on: (ev, fn) => { if (ev === "tool_result") piHandler = fn; } });
const piOut = await piHandler({ toolName: "bash", args: { command: "journalctl -n 600" },
                                content: [{ type: "text", text: body }] });
out.pi = piOut?.content?.[0]?.text?.length ?? body.length;

// OpenClaw drives `tool_result_persist`, the only hook in 2026.1.30 that can
// REPLACE a result. It is SYNC by contract — the host drops a handler that
// returns a Promise — so `mw` is called without `await` on purpose.
const oclaw = (await import("./oclaw.mjs")).default;
let mw = null;
let before = null;
oclaw.register({ on: (name, fn) => { if (name === "tool_result_persist") mw = fn;
                                     if (name === "before_tool_call") before = fn; },
                 logger: { info: () => {} } });
if (!mw) throw new Error("the openclaw plugin registered no tool_result_persist handler");
if (!before) throw new Error("the openclaw plugin registered no before_tool_call handler");
const clawOut = mw({ toolName: "bash", toolCallId: "c1",
                     message: { role: "toolResult", content: [{ type: "text", text: body }] } });
out.openclaw = clawOut?.message?.content?.[0]?.text?.length ?? body.length;
// A handler that returns a Promise is IGNORED by the host, so returning one
// would make this bridge silently inert exactly like its predecessor.
out.openclaw_is_sync = !(clawOut && typeof clawOut.then === "function");

const cline = (await import("./cline.ts")).default;
let builder = null;
cline.setup({ registerMessageBuilder: (b) => { builder = b; } });
const msgs = [
  { role: "assistant", content: [{ type: "tool_use", id: "t1", name: "bash",
                                   input: { command: "journalctl -n 600" } }] },
  { role: "user", content: [{ type: "tool_result", tool_use_id: "t1",
                              content: [{ type: "text", text: body }] }] },
];
out.cline = builder.build(msgs)[1].content[0].content[0].text.length;

// The command hint, on a payload where it is the whole difference: `git log`
// output does not fold at all without `--cmd`. Driven with a tool named
// `execute_command`, which three of these four bridges did not recognise as a
// shell tool until 2026-08-08.
const git = fs.readFileSync("./git.txt", "utf8");
out.git_before = git.length;
{
  const g1 = { output: git };
  await ocHooks["tool.execute.after"]({ tool: "execute_command", args: { command: "git log --oneline" } }, g1);
  out.git_opencode = g1.output.length;
  // …and a non-shell tool must still be refused: its "command" would name the
  // wrong program to the dispatcher.
  const g2 = { output: git };
  await ocHooks["tool.execute.after"]({ tool: "read", args: { command: "git log --oneline" } }, g2);
  out.git_opencode_read = g2.output.length;
  const gp = await piHandler({ toolName: "execute_command", args: { command: "git log --oneline" },
                               content: [{ type: "text", text: git }] });
  out.git_pi = gp?.content?.[0]?.text?.length ?? git.length;
  // openclaw learns the command one hook EARLIER: `before_tool_call` carries
  // the params, `tool_result_persist` carries only the call id.
  before({ toolName: "execute_command", toolCallId: "g1", params: { command: "git log --oneline" } });
  const gc = mw({ toolName: "execute_command", toolCallId: "g1",
                  message: { role: "toolResult", content: [{ type: "text", text: git }] } });
  out.git_openclaw = gc?.message?.content?.[0]?.text?.length ?? git.length;
  // And with no `before_tool_call` for that id — an older host, or a result
  // whose call we never saw — the fold must still run, hintless, exactly as
  // it did before. `git log` is the payload that does not compact without the
  // hint, so "unchanged" is the right answer here and not a failure.
  const gc2 = mw({ toolName: "execute_command", toolCallId: "unseen",
                   message: { role: "toolResult", content: [{ type: "text", text: git }] } });
  out.git_openclaw_nohint = gc2?.message?.content?.[0]?.text?.length ?? git.length;
  const gmsgs = [
    { role: "assistant", content: [{ type: "tool_use", id: "g1", name: "execute_command",
                                     input: { command: "git log --oneline" } }] },
    { role: "user", content: [{ type: "tool_result", tool_use_id: "g1",
                                content: [{ type: "text", text: git }] }] },
  ];
  out.git_cline = builder.build(gmsgs)[1].content[0].content[0].text.length;
}

// A DOCUMENT read whose host result is EMPTY. The words are in the FILE, and
// every bridge judged that case with the FOLD's rule — "is the extraction
// smaller than what the host handed back?" — which is false for every possible
// extraction when the host handed back nothing. Measured 2026-08-21: the model
// got the empty string while `tokenade read <path>` had the text.
const DOCX = process.env.TOKENADE_E2E_DOCX;
{
  const d1 = { output: "" };
  await ocHooks["tool.execute.after"]({ tool: "read", args: { file_path: DOCX } }, d1);
  out.docx_opencode = d1.output || "";
  const dp = await piHandler({ toolName: "read", args: { file_path: DOCX },
                               content: [{ type: "text", text: "" }] });
  out.docx_pi = dp?.content?.[0]?.text ?? "";
  before({ toolName: "read", toolCallId: "d1", params: { file_path: DOCX } });
  const dc = mw({ toolName: "read", toolCallId: "d1",
                  message: { role: "toolResult", content: [{ type: "text", text: "" }] } });
  out.docx_openclaw = dc?.message?.content?.[0]?.text ?? "";
  const dmsgs = [
    { role: "assistant", content: [{ type: "tool_use", id: "d1", name: "read",
                                     input: { file_path: DOCX } }] },
    { role: "user", content: [{ type: "tool_result", tool_use_id: "d1",
                                content: [{ type: "text", text: "" }] }] },
  ];
  out.docx_cline = dmsgs[1].content[0].content[0].text;
  try { out.docx_cline = builder.build(dmsgs)[1].content[0].content[0].text; } catch {}
}

// The dangerous shape for the fail-open rule: scrubbing makes the result
// LONGER (`AKIA…` becomes `<redacted>`), so a bridge that judges only on
// length throws the scrubbed version away and hands the model the raw key.
// Each of the five bridges implements the `scrubbed()` stderr check itself.
const secret = "AKIA" + "H4KLNPQRSTUV2WXY";
const creds = Array.from({length: 30}, (_, i) =>
  `config entry ${i}: service=api-gateway region=eu-west-1 retries=3 timeout=30s`).join("\n")
  + `\naws_access_key_id=${secret}\n`;
{
  const s1 = { output: creds };
  await ocHooks["tool.execute.after"]({ tool: "bash", args: { command: "cat creds" } }, s1);
  out.leak_opencode = s1.output.includes(secret);
  const sp = await piHandler({ toolName: "bash", args: { command: "cat creds" },
                               content: [{ type: "text", text: creds }] });
  out.leak_pi = (sp?.content?.[0]?.text ?? creds).includes(secret);
  const sc = mw({ toolName: "bash", toolCallId: "s1",
                  message: { role: "toolResult", content: [{ type: "text", text: creds }] } });
  out.leak_openclaw = (sc?.message?.content?.[0]?.text ?? creds).includes(secret);
  const smsgs = [
    { role: "assistant", content: [{ type: "tool_use", id: "s1", name: "bash",
                                     input: { command: "cat creds" } }] },
    { role: "user", content: [{ type: "tool_result", tool_use_id: "s1",
                                content: [{ type: "text", text: creds }] }] },
  ];
  out.leak_cline = builder.build(smsgs)[1].content[0].content[0].text.includes(secret);
  out.creds_bytes = creds.length;
}

// D2 (2026-08-10): the SAME credential in a payload far below MIN_CHARS.
// The floor above existed for folding, and it was silently doing a second job:
// 78 % of real tool results sit under it, so more than three quarters of
// everything an agent read was never scanned for secrets at all. Folding is
// still pointless at this size; redaction is not.
{
  const tiny = `aws_access_key_id=${secret}\nok\n`;
  out.tiny_bytes = tiny.length;
  const t1 = { output: tiny };
  await ocHooks["tool.execute.after"]({ tool: "bash", args: { command: "cat creds" } }, t1);
  out.tiny_leak_opencode = t1.output.includes(secret);
  const tp = await piHandler({ toolName: "bash", args: { command: "cat creds" },
                               content: [{ type: "text", text: tiny }] });
  out.tiny_leak_pi = (tp?.content?.[0]?.text ?? tiny).includes(secret);
  const tc = mw({ toolName: "bash", toolCallId: "y1",
                  message: { role: "toolResult", content: [{ type: "text", text: tiny }] } });
  out.tiny_leak_openclaw = (tc?.message?.content?.[0]?.text ?? tiny).includes(secret);
  const tmsgs = [
    { role: "assistant", content: [{ type: "tool_use", id: "y1", name: "bash",
                                     input: { command: "cat creds" } }] },
    { role: "user", content: [{ type: "tool_result", tool_use_id: "y1",
                                content: [{ type: "text", text: tiny }] }] },
  ];
  out.tiny_leak_cline = builder.build(tmsgs)[1].content[0].content[0].text.includes(secret);
  // A clean payload of the same size must come back BYTE-IDENTICAL: the
  // round-trip is for redaction, and it must not start rewriting small results.
  const clean = "ok\nfile written\n";
  const c1 = { output: clean };
  await ocHooks["tool.execute.after"]({ tool: "bash", args: { command: "cat creds" } }, c1);
  out.clean_unchanged = c1.output === clean;
}

out.before = body.length;
console.log(JSON.stringify(out));
"#;
    std::fs::write(dir.join("driver.mjs"), driver).unwrap();

    let docx = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/docs-bench/fixture/docs-audit/docx.docx");
    let out = Command::new("node")
        .arg("--experimental-strip-types")
        .arg("driver.mjs")
        .current_dir(dir)
        .env("HOME", dir)
        .env("TOKENADE_BIN", binary())
        .env("TOKENADE_FORCE_GAIN", "1")
        .env("TOKENADE_E2E_DOCX", &docx)
        .output()
        .expect("spawn node");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Node 22 is the floor for `--experimental-strip-types`; an older one
    // cannot load a `.ts` asset at all, and that is a skip, not a failure.
    if !out.status.success() && stderr.contains("strip-types") {
        eprintln!("SKIP: this node cannot strip types (needs 22+):\n{stderr}");
        return;
    }
    assert!(
        out.status.success(),
        "the bridge driver failed:\nstdout: {stdout}\nstderr: {stderr}"
    );
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim().lines().last().unwrap_or_default())
            .unwrap_or_else(|e| panic!("driver output was not JSON ({e}):\n{stdout}\n{stderr}"));
    let before = v["before"].as_u64().unwrap();

    for bridge in ["opencode", "pi", "openclaw", "cline"] {
        let after = v[bridge].as_u64().unwrap_or(before);
        assert!(
            after * 4 < before,
            "the {bridge} bridge returned {after} chars for {before} — it did not fold. \
             Six agents ride on these four files and nothing else exercises them."
        );
    }
    assert_eq!(
        v["opencode_terminal"].as_u64().unwrap_or(0),
        before,
        "a live `terminal` session was folded — that is a PTY, not a finished result"
    );

    // opencode 2 default-exports a definition with an `id` and a `setup`
    // function; without it the loader rejects the file outright
    // (`PluginModule.LoadError: Plugin must export a default definition …`)
    // and the bridge folds nothing for the rest of the install.
    assert_eq!(
        v["opencode_v2_definition"].as_bool(),
        Some(true),
        "the opencode asset does not default-export a v2 definition (id + setup) —          opencode 2 refuses to load it"
    );
    let v2_after = v["opencode_v2"].as_u64().unwrap_or(before);
    assert!(
        v2_after * 4 < before,
        "the v2 path of the opencode bridge returned {v2_after} chars for {before} —          the definition loads but its `setup` registers no working hook"
    );
    assert_eq!(
        v["opencode_v2_terminal"].as_u64().unwrap_or(0),
        before,
        "the v2 path folded a live `terminal` session"
    );
    // opencode 2's shell tool returns a structured `output` and puts the text
    // the model reads in `content`: fold the payload part only.
    let v2_content = v["opencode_v2_content"].as_u64().unwrap_or(before);
    assert!(
        v2_content * 4 < before,
        "the v2 path did not fold `result.content` ({v2_content} chars for {before})"
    );
    assert_eq!(
        v["opencode_v2_notice_kept"].as_bool(),
        Some(true),
        "the v2 path overwrote the shell tool's exit notice part"
    );
    assert_eq!(
        v["opencode_v2_output_kept"].as_bool(),
        Some(true),
        "the v2 path replaced the shell tool's structured `output`"
    );
    // `content` empty -> the host falls back to `output`, which for the shell
    // tool is structured. The adapter handled only a STRING `output`, so this
    // payload reached the model unfolded.
    let v2_output_only = v["opencode_v2_output_only"].as_u64().unwrap_or(before);
    assert!(
        v2_output_only * 4 < before,
        "the v2 path did not fold a structured `output` when `content` was empty \
         ({v2_output_only} chars for {before})"
    );
    assert_eq!(
        v["opencode_v2_output_only_kept"].as_bool(),
        Some(true),
        "the v2 path replaced the shell tool's structured `output` object instead of its text field"
    );
    // The compaction reset. 2.x renamed 1.x's `session.compacted` to
    // `session.compaction.ended` and moved `sessionID` under `data`; the bridge
    // listened for the old name, so `session-tick --reset` never ran and the
    // ledger kept counting a conversation the host had already compacted.
    let tick = v["opencode_v2_compaction_tick"].as_str().unwrap_or("");
    assert!(
        tick.contains("--reset") && tick.contains("--session ses_compact"),
        "the v2 `session.compaction.ended` event did not trigger `session-tick --reset` \
         (received {tick:?})"
    );

    // The command hint. Measured 2026-08-08: with a tool named
    // `execute_command`, opencode/pi/openclaw returned all 29 115 bytes of a
    // `git log` — no hint, no fold, and the fail-open rule then declines to
    // replace anything. Cline alone had been patched for that name. The four
    // now share one substring test.
    //
    // openclaw was excluded from this list until 2026-08-21, and the exclusion
    // was asserted in the negative so that the day the host started passing the
    // input the test would fail and someone would come back to it. That day
    // came: `before_tool_call` carries `{ toolName, params, toolCallId }` since
    // openclaw 2026.6.10, the bridge keeps the params under the call id, and
    // `git log` folds here like everywhere else.
    let git_before = v["git_before"].as_u64().unwrap();
    for bridge in ["git_opencode", "git_pi", "git_cline", "git_openclaw"] {
        let after = v[bridge].as_u64().unwrap_or(git_before);
        assert!(
            after * 2 < git_before,
            "{bridge}: {after} chars for {git_before} — the command hint did not reach \
             the binary, and `git log` does not fold without it"
        );
    }
    // The hint is an improvement, never a requirement: an openclaw with no
    // `before_tool_call`, or a result whose call this bridge never saw, must
    // still go through the fold rather than blow up on a missing entry.
    assert_eq!(
        v["git_openclaw_nohint"].as_u64().unwrap_or(git_before),
        git_before,
        "the hintless path changed a `git log` — it has nothing to fold it with, \
         so the only correct answer there is the text it was given"
    );
    assert_eq!(
        v["git_opencode_read"].as_u64().unwrap_or(0),
        git_before,
        "a `read` tool was treated as a shell — its `command` names the wrong program"
    );

    // The document path, with an EMPTY host result. Every bridge judged that
    // case with the FOLD's rule — is the extraction smaller than what the host
    // handed back — which no extraction can satisfy against nothing, so the
    // words were dropped and the model got the empty string. Three of the four
    // additionally refused the payload before reaching the extractor at all, on
    // a floor that exists to skip folding and to bound secret scanning; a
    // document read is neither.
    for bridge in ["docx_opencode", "docx_pi", "docx_openclaw", "docx_cline"] {
        let text = v[bridge].as_str().unwrap_or("");
        assert!(
            text.contains("Prospecteur"),
            "{bridge}: an empty host result for a `.docx` came back as {:?} — the words \
             are in the FILE and `tokenade read <path>` has them",
            &text[..text.len().min(120)]
        );
    }

    // SECURITY: no credential reaches the model through any bridge.
    //
    // What this does NOT do, said plainly because the opposite claim would be
    // the same mistake as a test that asserts nothing: it does not isolate the
    // `scrubbed()` branch. That branch exists for a payload whose SCRUBBED
    // form is longer than its input, where a length-only rule would discard the
    // safe version. Breaking `scrubbed()` on purpose leaves this test green,
    // measured 2026-08-08 — on this fixture the scrubbed output is 2 300 bytes
    // against 2 309 in, so the length rule alone still replaces it.
    //
    // Three attempts to build an inflating fixture failed, and the reason is
    // the redactor being right: it gates short low-entropy values out (a bare
    // `Bearer a0` is left alone by design, because as a plain rule it ate
    // documentation prose), so a secret long enough to be redacted is long
    // enough that `<redacted>` shrinks it. The end property is what is asserted
    // here; `scrubbed()` itself is covered by the unit tests on each asset.
    assert!(
        v["creds_bytes"].as_u64().unwrap_or(0) > 1000,
        "the credential fixture must clear MIN_CHARS or no bridge even looks at it"
    );
    for bridge in ["leak_opencode", "leak_pi", "leak_openclaw", "leak_cline"] {
        assert_eq!(
            v[bridge].as_bool(),
            Some(false),
            "{bridge}: a credential reached the model through this bridge"
        );
    }

    // D9: the openclaw handler must be SYNCHRONOUS. Its host drops a handler
    // that returns a Promise with a warning, which would make this bridge
    // inert exactly the way its predecessor was — silently, and only in
    // production.
    assert_eq!(
        v["openclaw_is_sync"].as_bool(),
        Some(true),
        "the openclaw tool_result_persist handler returned a Promise; the host \
         ignores those, so the bridge would fold nothing"
    );

    // D2: the same credential UNDER the fold floor. This is the case that was
    // open — 78 % of real tool results are this size.
    let tiny = v["tiny_bytes"].as_u64().unwrap_or(0);
    assert!(
        (8..1000).contains(&tiny),
        "the tiny fixture must sit BELOW the fold floor to test anything, got {tiny} bytes"
    );
    for bridge in [
        "tiny_leak_opencode",
        "tiny_leak_pi",
        "tiny_leak_openclaw",
        "tiny_leak_cline",
    ] {
        assert_eq!(
            v[bridge].as_bool(),
            Some(false),
            "{bridge}: a credential under the fold floor reached the model — \
             the floor was doing two jobs and only one of them had a reason"
        );
    }
    assert_eq!(
        v["clean_unchanged"].as_bool(),
        Some(true),
        "a clean small payload came back modified: the redaction round-trip must \
         not start rewriting results that have nothing to redact"
    );
}

/// The Hermes bridge, in python, through the hook name its host registers.
#[cfg(unix)]
#[test]
fn e2e_the_hermes_bridge_actually_folds_a_tool_result() {
    if !have("python3", &["--version"]) {
        eprintln!("SKIP: python3 not on PATH — the Hermes bridge is untested here");
        return;
    }
    let h = TempHome::new("bridges-py");
    let dir = h.path();
    std::fs::write(
        dir.join("hermes_mod.py"),
        asset("hermes_plugin/__init__.py"),
    )
    .unwrap();
    let body = log_body();
    std::fs::write(dir.join("body.txt"), &body).unwrap();
    // The ctx stub captures whatever callable Hermes would register, under
    // whatever method name the asset uses — the registration API is the host's,
    // not ours, so pinning one method name here would test our guess of it.
    let driver = r#"
import importlib.util, json, os, sys
spec = importlib.util.spec_from_file_location("hermes_mod", "hermes_mod.py")
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
body = open("body.txt").read()
captured = {}
class Ctx:
    def __getattr__(self, name):
        def cap(*a, **k):
            for x in list(a) + list(k.values()):
                if callable(x):
                    captured[name] = x
            return None
        return cap
m.register(Ctx())
res = {"before": len(body), "hooks": sorted(captured)}
for name, fn in captured.items():
    out = fn(tool_name="bash", result=body, args={"command": "journalctl -n 600"})
    res[name] = len(out) if isinstance(out, str) else len(body)
secret = "AKIA" + "H4KLNPQRSTUV2WXY"
creds = "\n".join(
    "config entry %d: service=api-gateway region=eu-west-1 retries=3 timeout=30s" % i
    for i in range(30)
) + "\naws_access_key_id=%s\n" % secret
res["leaked"] = False
for name, fn in captured.items():
    out = fn(tool_name="bash", result=creds, args={"command": "cat creds"})
    txt = out if isinstance(out, str) else creds
    if secret in txt:
        res["leaked"] = True
# A DOCUMENT read whose host result is EMPTY: the words are in the FILE, and
# the fold floor plus the fold's own "is it smaller" rule both said no.
res["docx"] = ""
for name, fn in captured.items():
    out = fn(tool_name="read", result="", args={"file_path": os.environ["TOKENADE_E2E_DOCX"]})
    if isinstance(out, str) and out:
        res["docx"] = out
print(json.dumps(res))
"#;
    std::fs::write(dir.join("driver.py"), driver).unwrap();
    let docx = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/docs-bench/fixture/docs-audit/docx.docx");
    let out = Command::new("python3")
        .arg("driver.py")
        .current_dir(dir)
        .env("HOME", dir)
        .env("TOKENADE_BIN", binary())
        .env("TOKENADE_FORCE_GAIN", "1")
        .env("TOKENADE_E2E_DOCX", &docx)
        .output()
        .expect("spawn python3");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the hermes driver failed:\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim().lines().last().unwrap_or_default())
            .unwrap_or_else(|e| panic!("driver output was not JSON ({e}):\n{stdout}"));
    let before = v["before"].as_u64().unwrap();
    let hooks: Vec<String> = v["hooks"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !hooks.is_empty(),
        "`register()` registered no callable — the bridge is inert and nothing said so"
    );
    // Same fix as the four JS bridges: an empty host result for a `.docx` must
    // still bring back the words, because they are in the FILE. Two rules said
    // no here — the fold floor, and the fold's "is it smaller" acceptance.
    let docx_text = v["docx"].as_str().unwrap_or("");
    assert!(
        docx_text.contains("Prospecteur"),
        "hermes: an empty host result for a `.docx` came back as {:?}",
        &docx_text[..docx_text.len().min(120)]
    );
    for hook in &hooks {
        let after = v[hook.as_str()].as_u64().unwrap_or(before);
        assert!(
            after * 4 < before,
            "hermes hook `{hook}` returned {after} chars for {before} — it did not fold"
        );
    }
    // Same security property as its four JS twins: the scrubbed result is
    // LONGER than the input, and a length-only rule would drop it.
    assert_eq!(
        v["leaked"].as_bool(),
        Some(false),
        "a credential reached the model through the hermes bridge"
    );
}

/// The five bridge assets must agree on the constants they all carry.
///
/// Not a style rule — a measured bug class: the
/// shell-tool list existed in five copies, was corrected in one, and no test
/// could see it because every existing bridge test reads one file at a time.
/// The same sweep found Cline's `MIN_CHARS` at 2000 against 1000 everywhere
/// else, justified by a comment that measurement contradicts.
///
/// Deliberately checks VALUES, not text: the four JS/TS files and the Python
/// one are allowed to spell `4_000_000` however their language prefers.
#[test]
fn the_bridge_assets_agree_on_the_constants_they_share() {
    fn num(src: &str, name: &str) -> Option<u64> {
        let at = src.find(&format!("{name} = "))?;
        let rest = &src[at + name.len() + 3..];
        let digits: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '_')
            .filter(|c| *c != '_')
            .collect();
        digits.parse().ok()
    }
    let assets = [
        ("opencode", asset("opencode_plugin/tokenade.ts")),
        ("pi", asset("pi_extension/tokenade.ts")),
        ("openclaw", asset("openclaw_plugin/index.js")),
        ("cline", asset("cline_plugin/tokenade.ts")),
        ("hermes", asset("hermes_plugin/__init__.py")),
    ];
    for (constant, py_name) in [
        ("MIN_CHARS", "_MIN_CHARS"),
        ("MAX_CHARS", "_MAX_CHARS"),
        // Split from MIN_CHARS 2026-08-10 (D2): folding needs 1000, redaction
        // needs none. Five bridges must agree on BOTH or 78 % of tool results
        // go unscanned on whichever one drifts.
        ("REDACT_MIN_CHARS", "_REDACT_MIN_CHARS"),
    ] {
        let values: Vec<(&str, Option<u64>)> = assets
            .iter()
            .map(|(n, s)| {
                let name = if *n == "hermes" { py_name } else { constant };
                (*n, num(s, name))
            })
            .collect();
        let first = values[0].1;
        assert!(
            first.is_some(),
            "{constant} not found in the opencode bridge"
        );
        for (name, v) in &values {
            assert_eq!(
                *v, first,
                "{name}'s {constant} is {v:?}, the others use {first:?} — five copies of one \
                 constant, and this is exactly how the shell-tool list drifted"
            );
        }
    }
    // The stderr contract with the binary: a bridge that does not look for this
    // throws away scrubbed output for being LONGER than what it sent, and hands
    // the model the raw secret.
    for (name, src) in &assets {
        assert!(
            src.contains("[tokenade] redacted:"),
            "the {name} bridge does not look for the redaction signal"
        );
    }

    // The shell-tool vocabulary: identical across the four JS/TS twins, and
    // hermes differs by EXACTLY one word, on purpose.
    //
    // The previous form asserted each bridge CONTAINED `bash`/`shell`/`run`/
    // `execute`/`command` — a floor, which cannot see a bridge that went above
    // it. Hermes has a sixth word, `terminal`, and that is not drift: on the
    // four JS/TS hosts `terminal` is a LIVE PTY session, skipped outright
    // (`SKIP`, asserted by `e2e_the_javascript_bridges_actually_fold_a_tool_result`,
    // which drives `tool: "terminal"` and requires the body to come back
    // untouched). Hermes only ever sees finished results, so it folds them.
    //
    // Pinning the difference beats pinning a floor: if someone aligns the five
    // lists — as was attempted 2026-08-08 — this fails and says why, instead of
    // passing while the four bridges start folding a live terminal.
    let vocab = |src: &str| -> Vec<String> {
        let at = src
            .find("SHELL_WORDS = ")
            .or_else(|| src.find("_SHELL_TOOLS = "))
            .expect("no shell-tool list in this bridge");
        let rest = &src[at..];
        let close = rest.find([']', ')']).expect("unterminated shell-tool list");
        let mut words: Vec<String> = rest[..close]
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect();
        words.sort();
        words
    };
    let js: Vec<(&str, Vec<String>)> = assets
        .iter()
        .filter(|(n, _)| *n != "hermes")
        .map(|(n, s)| (*n, vocab(s)))
        .collect();
    assert_eq!(js.len(), 4, "the JS/TS bridge set changed");
    for (name, words) in &js {
        assert_eq!(
            *words, js[0].1,
            "the {name} bridge's shell-tool vocabulary differs from {}'s — four copies of one \
             list, which is exactly how it drifted before",
            js[0].0
        );
        assert!(
            !words.iter().any(|w| w == "terminal"),
            "the {name} bridge would now treat `terminal` as a finished shell result, but \
             `terminal` is the LIVE PTY session it skips"
        );
    }
    let hermes = vocab(&asset("hermes_plugin/__init__.py"));
    let extra: Vec<&String> = hermes.iter().filter(|w| !js[0].1.contains(w)).collect();
    assert_eq!(
        extra,
        vec![&"terminal".to_string()],
        "hermes' shell-tool list now differs from the JS bridges by something other than the \
         deliberate `terminal` — either it drifted, or the reason changed and this test with it"
    );
    let missing: Vec<&String> = js[0].1.iter().filter(|w| !hermes.contains(w)).collect();
    assert!(
        missing.is_empty(),
        "hermes is missing shell words the other four have: {missing:?}"
    );
}

/// A bridge's saving must arrive in the ledger with the command that produced
/// it, not as an anonymous bucket.
///
/// Measured 2026-08-08: the opencode bridge folded `git log` 29 115 -> 4 569
/// bytes and booked `op=auto-compact, label="auto:unknown"` with no `cmd`
/// field. `--cmd` had been passed and had done the folding — it just never
/// reached the row. That is all six plugin-bridge agents' traffic landing in
/// the Commands panel as one unattributed lump, which is the shape that hid
/// 14.6k Codex shim events until the basename was threaded through.
#[cfg(unix)]
#[test]
fn e2e_a_hinted_fold_records_the_command_that_produced_it() {
    let h = TempHome::new("hinted-credit");
    // `git log --oneline` is the payload that only folds WITH the hint, so a
    // row here can only have come from the hint path.
    let git = (0..400)
        .map(|i: usize| {
            format!(
                "{:08x} commit subject number {i} touching a few files",
                i * 2_654_435_761u64 as usize
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let (out, _err, code) = run_with_stdin(
        &["read", "-", "--cmd", "git log --oneline -n 400"],
        h.path(),
        &git,
    );
    assert_eq!(code, 0);
    assert!(
        out.len() * 2 < git.len(),
        "the hint did not fold, so there is no row to check"
    );

    let log =
        std::fs::read_to_string(h.path().join(".tokenade").join("gain.jsonl")).unwrap_or_default();
    let row: serde_json::Value = log
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON record"))
        .find(|v: &serde_json::Value| v["before"].as_u64().unwrap_or(0) > 0)
        .unwrap_or_else(|| panic!("no ledger row for a fold that happened:\n{log}"));
    assert_eq!(
        row["cmd"].as_str(),
        Some("git"),
        "the row does not name the program the hint identified: {row}"
    );
    assert_eq!(
        row["covered"].as_bool(),
        Some(true),
        "a named compactor IS command-specific coverage: {row}"
    );
    assert!(
        !row["label"]
            .as_str()
            .unwrap_or_default()
            .starts_with("auto:"),
        "the label still says the content-detected kind, not the command: {row}"
    );
}

/// Every agent tokenade covers with a PLUGIN must be driven by one of the
/// bridge tests above — rank 1bis's version of the rank-1 guard.
///
/// Same invisible gap: a seventh bridge agent could be added, its asset
/// written and installed, with nothing loading it in its host's runtime, and
/// every existing test would stay green. That is precisely the state this
/// channel was once found in.
///
/// Kilo is the interesting row. It ships no asset of its own — it installs the
/// OPENCODE asset — so the opencode case genuinely drives kilo's bytes. That
/// justification is ASSERTED below rather than trusted: the day kilo gets its
/// own file, this test says so instead of quietly covering nothing.
#[test]
fn every_plugin_covered_agent_has_a_bridge_test() {
    // (slug, the asset actually loaded on its behalf)
    const DRIVEN: &[(&str, &str)] = &[
        ("opencode", "opencode_plugin/tokenade.ts"),
        ("kilo-code", "opencode_plugin/tokenade.ts"),
        ("pi", "pi_extension/tokenade.ts"),
        ("openclaw", "openclaw_plugin/index.js"),
        ("cline", "cline_plugin/tokenade.ts"),
        ("hermes", "hermes_plugin/__init__.py"),
    ];

    let mut missing = Vec::new();
    let mut plugin_agents = 0;
    for a in tokenade_core::detect::Agent::all() {
        if !matches!(a.coverage(), tokenade_core::detect::Coverage::Plugin) {
            continue;
        }
        plugin_agents += 1;
        if !DRIVEN.iter().any(|(slug, _)| *slug == a.as_str()) {
            missing.push(a.as_str());
        }
    }
    assert!(
        missing.is_empty(),
        "these agents are covered by a plugin but no bridge test loads their asset: {missing:?}"
    );
    let stale: Vec<&str> = DRIVEN
        .iter()
        .map(|(slug, _)| *slug)
        .filter(|slug| {
            !tokenade_core::detect::Agent::all().iter().any(|a| {
                a.as_str() == *slug
                    && matches!(a.coverage(), tokenade_core::detect::Coverage::Plugin)
            })
        })
        .collect();
    assert!(
        stale.is_empty(),
        "DRIVEN lists non-plugin agents: {stale:?}"
    );
    assert_eq!(
        plugin_agents,
        DRIVEN.len(),
        "the plugin-agent count moved; DRIVEN is out of date"
    );

    // Kilo's coverage is inherited, not its own — hold that fact in place.
    let kilo_asset = DRIVEN
        .iter()
        .find(|(s, _)| *s == "kilo-code")
        .map(|(_, a)| *a)
        .expect("kilo-code row");
    assert_eq!(
        kilo_asset, "opencode_plugin/tokenade.ts",
        "kilo no longer shares opencode's asset — it needs a bridge test of its own"
    );
    assert!(
        !Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/kilo_plugin")
            .exists(),
        "a kilo-specific asset appeared; the opencode case no longer drives kilo's bytes"
    );
}

/// Every process launcher in the tree either HOLDS the cancellation guarantee
/// or is listed here with the reason it does not need it.
///
/// This census was first written as prose, and one of its rows went
/// stale — true about the timeout it described, silent about the question the
/// table was asking. Prose cannot fail. This can: a new `Command::new` in a
/// file that neither guards nor appears below breaks the build until somebody
/// answers "who kills this when we are gone?".
///
/// Being on this list is not an accusation. Most of these are brief probes, and
/// three of them detach ON PURPOSE — killing them with their parent would be
/// the defect, not the fix.
#[cfg(unix)]
#[test]
fn every_process_launcher_either_guards_its_work_or_says_why_not() {
    /// `(path UNDER `src/`, reason)`.
    ///
    /// The key does NOT carry the crate name, and that has been deliberate
    /// since 2026-09-13: it used to, `detect` moved into `tk-agents`, and this
    /// guard declared unclassified a module that had been exempted for months.
    /// The same shape bit two other guards the same day. What identifies the
    /// subject is the module, not the crate the refactoring put it in.
    const EXEMPT: &[(&str, &str)] = &[
        // Deliberately outlives us — killing these with the parent is the bug.
        ("agent/local.rs", "spawns the local-serve DAEMON"),
        ("agent/mod.rs", "detached agent run, by design"),
        ("updater.rs", "detached updater; dies with us = no update"),
        ("lifecycle/browser.rs", "opens the USER's browser"),
        // Bounded internal calls: fixed program, fixed args, no user work.
        ("platform.rs", "short powershell/env probe"),
        ("detect.rs", "short agent-detection probe"),
        ("web_fetch.rs", "short internal curl calls"),
        // The index-touch re-exec moved down into `commands/index_cmds.rs`;
        // the trunk of its family is `commands.rs`.
        ("commands.rs", "re-execs ITSELF for index-touch"),
        // Same pattern: tokenade relaunches tokenade, fixed arguments,
        // SYNCHRONOUSLY and to completion. The post-upgrade work belongs to
        // the version just installed, not to the one being replaced —
        // otherwise a fix only takes effect at the NEXT upgrade. The reaper
        // would be backwards here: killing this child because the parent died
        // would leave exactly the half-done install this re-exec exists to
        // avoid.
        (
            "lifecycle/upgrade.rs",
            "re-execs the NEW binary for post-upgrade work, fixed args, synchronous",
        ),
        ("feature_cli/packing.rs", "git with fixed args"),
        ("hooks/grep_pretool.rs", "bounded rg probe"),
        ("healthcheck/checks.rs", "short health probes"),
        ("diagnostics_cli.rs", "short diagnostic probes"),
        ("dashboard_cli/statusline.rs", "short statusline probe"),
        ("install/capability.rs", "short capability probe"),
        ("install/shell_proxy.rs", "install-time only"),
        ("install/uninstall.rs", "uninstall-time only"),
        ("install_cli/agent_plugins.rs", "install-time only"),
        // `systemctl` / `launchctl` / `schtasks` / `sc`, fixed args, run to
        // completion in milliseconds. Guarding them would be backwards: what
        // they install is a SUPERVISOR whose entire job is to outlive us, and
        // the calls themselves are queries and enable/disable verbs, not work.
        (
            "llm_proxy/autostart.rs",
            "short supervisor verbs (systemctl/launchctl/schtasks)",
        ),
        // The agent's own tools: `run` is gated to the TOKENADE_EXPLORE
        // allowlist, and its BASH tool goes through the sandbox, which guards.
        ("agent/tools.rs", "allowlist + sandbox for BASH"),
        // Not shipped.
        ("bench_cli.rs", "bench harness"),
        ("bin/accuracy.rs", "bench harness"),
        (
            "bin/locale_census.rs",
            "bench harness; a closed list of read-only probes, run by hand",
        ),
        (
            "bin/testsel.rs",
            "bench harness; runs `git status` and `tokenade impact`, by hand",
        ),
        (
            "bin/furniture_census.rs",
            "bench harness; runs the tokenade binary over a page corpus, by hand",
        ),
        (
            "bin/rag_eval/labels.rs",
            "bench harness; runs `git log` to mine the retrieval labels, by hand",
        ),
        // Runs `cargo nextest` to completion, by hand, and is not shipped. Its
        // real exposure is the OTHER direction: a SIGKILL mid-run leaves the
        // tree MUTATED, because no `Drop` runs. Its module says so and names
        // the recovery (`git diff`, then `git restore` the one file), which is
        // the honest answer here — a reaper would kill the test run without
        // putting the file back.
        (
            "mutate.rs",
            "bench harness; `cargo nextest` to completion, by hand",
        ),
        ("benches/misc.rs", "bench harness"),
        // `cli/src/tests/…` used to need an entry here because the walk
        // visited `#[cfg(test)]` modules living under `src`. It skips them now,
        // so an entry would be dead — and this guard fails on dead entries,
        // which is how that was noticed.
        // `process_util.rs` is deliberately absent: it DEFINES the reaper, so it
        // contains the name and counts as guarded. Listing it here as well made
        // the staleness check fire, which is the check working.
    ];

    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                // `src/tests/` holds `#[cfg(test)]` modules that happen to live
                // under `src`. They are not shipped, and one of them is the
                // guard that SCANS for `Command::new` — so walking it makes this
                // test flag a test for containing the string it searches for.
                // Same reasoning as the comment below about `tests/`.
                if p.file_name().and_then(|s| s.to_str()) == Some("tests") {
                    continue;
                }
                walk(&p, out);
            } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if n != "tests.rs" && !n.ends_with("_tests.rs") {
                    out.push(p);
                }
            }
        }
    }

    // Only shipped source: `crates/*/src`. A test that spawns a process answers
    // to its own assertions, and walking `tests/` would put this guard in the
    // business of policing itself.
    let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    for e in std::fs::read_dir(crates_dir).expect("crates/").flatten() {
        let src = e.path().join("src");
        if src.is_dir() {
            walk(&src, &mut files);
        }
    }
    assert!(
        files.len() > 100,
        "anti-vacuity: the walk found {} files, so it is not reading the tree",
        files.len()
    );

    let mut guarded = 0usize;
    let mut used: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut unclassified: Vec<String> = Vec::new();

    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        // `internal_command` IS a spawn — it builds a `Command` for tokenade's
        // own plumbing — and this guard's question ("who kills this when
        // tokenade is SIGKILLed?") applies to it exactly as it does to a bare
        // `Command::new`. Matching only the literal made eight files silently
        // leave the sweep on the day they started using the helper.
        if !src.contains("Command::new") && !src.contains("internal_command(") {
            continue;
        }
        // Normalise to a stable suffix: `crates/tokenade-<x>/…` → `<x>/…`.
        // Normalised to what follows `src/`: the crate is not part of the
        // module's identity, and putting it there made the table stale the day
        // of a move.
        let under_src = |p: &Path| -> String {
            let t = p.to_string_lossy().replace('\\', "/");
            match t.split_once("/src/") {
                Some((_, rest)) => rest.to_string(),
                None => t,
            }
        };
        let rel = under_src(f);
        // The module's FAMILY, not the file. `spawn_wrapped` returns its
        // `Child` to its single caller, which registers it in the guarded
        // group — and splitting `wrap.rs` put the two in two files. The file is
        // the wrong unit: what matters is that a child launched here dies with
        // us, and that answer lives at module scale.
        let (fam_root, fam_name) = {
            let dir = f.parent().expect("a directory");
            let as_child = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| {
                    dir.with_extension("rs").is_file()
                        || dir
                            .parent()
                            .is_some_and(|g| g.join(format!("{n}.rs")).is_file())
                });
            match as_child {
                Some(n) => (dir.parent().expect("a grandparent").to_path_buf(), n),
                None => (
                    dir.to_path_buf(),
                    f.file_stem()
                        .expect("a name")
                        .to_string_lossy()
                        .into_owned(),
                ),
            }
        };
        let family = tokenade_core::source_guard::module_family(&fam_root, &fam_name);
        if family.contains("guard_group_against_our_death") {
            guarded += 1;
            continue;
        }
        // The declaration applies to the family: it is accepted on the file or
        // on its module's trunk.
        let trunk = under_src(&fam_root.join(format!("{fam_name}.rs")));
        match EXEMPT
            .iter()
            .find(|(suffix, _)| rel.ends_with(suffix) || trunk.ends_with(suffix))
        {
            Some((suffix, _)) => {
                used.insert(suffix);
            }
            None => unclassified.push(rel),
        }
    }

    assert!(
        unclassified.is_empty(),
        "these files spawn a process but neither call \
         `guard_group_against_our_death` nor appear in EXEMPT. Answer \"who kills \
         this when tokenade is SIGKILLed?\" — then guard it, or add it with the \
         reason:\n  {}",
        unclassified.join("\n  ")
    );
    // Anti-vacuity, both directions. Without the first, deleting the guard
    // everywhere still passes; without the second, EXEMPT rots into a list of
    // files that no longer exist and nobody notices.
    assert!(
        guarded >= 4,
        "only {guarded} launchers hold the guarantee — it was 5 when this was written"
    );
    let stale: Vec<&str> = EXEMPT
        .iter()
        .map(|(s, _)| *s)
        .filter(|s| !used.contains(s))
        .collect();
    assert!(
        stale.is_empty(),
        "EXEMPT lists files that no longer spawn anything — remove them:\n  {}",
        stale.join("\n  ")
    );
}

/// Nothing in the product may ever create a PATH shim again.
///
/// The shim dropped a directory of per-command executables at the FRONT of the
/// user's `PATH`. It was removed in 2026-07 after it hijacked 134 commands on a
/// tester's machine; the environments it must survive vary far too much for
/// that technique to be safe. What is LEFT under that name is only the
/// dismantler — it strips the rc block and deletes `~/.tokenade/shim` on
/// machines that still carry one — plus a module about embedding the binary
/// path, which merely had a misleading name and has been renamed.
///
/// Deliberately BEHAVIOURAL. Two textual versions of this guard were written
/// first and both were wrong: one matched `shim_dir()` while the real function
/// takes an argument, so it passed with a shim-creating line in front of it;
/// the next flagged every legitimate call. What the product must promise is not
/// a shape of source code, it is that a real `install` leaves no shim behind.
#[cfg(unix)]
#[test]
fn install_never_creates_a_path_shim() {
    let h = TempHome::new("no-path-shim");
    // `install` refuses when it detects no agent, so give it one.
    std::fs::create_dir_all(h.path().join(".claude")).unwrap();
    for rc in [".bashrc", ".zshrc", ".profile"] {
        std::fs::write(h.path().join(rc), "# user content\nexport FOO=1\n").unwrap();
    }
    // Plant what an OLD version left behind: a shim dir and an rc PATH block.
    // This is what makes the test non-vacuous — `install` must DISMANTLE it.
    // (Creating a shim at the start of `install` cannot be used as a red check:
    // the dismantler runs inside `install` and deletes it again, which is
    // exactly the property being asserted.)
    let planted = h.path().join(".tokenade").join("shim");
    std::fs::create_dir_all(&planted).unwrap();
    std::fs::write(planted.join("git"), "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::write(
        h.path().join(".bashrc"),
        "# user content\nexport PATH=\"$HOME/.tokenade/shim:$PATH\"\n",
    )
    .unwrap();

    let (out, err, _c) = run(&["install"], h.path());
    assert!(
        !planted.exists(),
        "install left an OLD version's shim in place — this is the only thing \
         that gets a hijacked machine repaired.\nstdout: {out}\nstderr: {err}"
    );
    // Anti-vacuity FIRST: two earlier versions of this guard passed because the
    // install under test never ran at all. Prove it did something before
    // proving what it did not do.
    assert!(
        h.path().join(".tokenade").join("installed-at").exists(),
        "install did not install — this guard would pass by testing nothing.
stdout: {out}
stderr: {err}"
    );

    let shim_dir = h.path().join(".tokenade").join("shim");
    assert!(
        !shim_dir.exists(),
        "install created {} — the PATH shim is removed from the product",
        shim_dir.display()
    );
    for rc in [".bashrc", ".zshrc", ".profile"] {
        let body = std::fs::read_to_string(h.path().join(rc)).unwrap_or_default();
        assert!(
            !body.contains("/.tokenade/shim"),
            "install wrote a shim PATH entry into {rc}:\n{body}"
        );
        assert!(
            body.contains("# user content"),
            "install destroyed pre-existing content in {rc}"
        );
    }

    // Anti-vacuity: the DISMANTLER must still exist. Without it this test would
    // pass by describing a world with no shim code at all, while every machine
    // that still carries one keeps a hijacked PATH forever.
    let dismantler = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("install")
        .join("legacy_shim_removal.rs");
    let src = std::fs::read_to_string(&dismantler)
        .unwrap_or_else(|e| panic!("{}: {e}", dismantler.display()));
    assert!(
        src.contains("fn uninstall_path_shim"),
        "the dismantler is gone — old machines would stay hijacked"
    );
}

/// The openclaw bridge must LOAD in a real openclaw, not merely be written.
///
/// The reason this exists: the bridge had been inert for an unknown
/// length of time — three independent breakages (an SDK subpath that no longer
/// exists, a contract absent from the host, a `--force` flag that was never an
/// option) — and every test passed throughout, because they all checked that
/// the FILE was written and well-formed. `openclaw plugins list` showed 30
/// plugins and no tokenade.
///
/// So this drives the host's own CLI: link the asset into a throwaway
/// `OPENCLAW_STATE_DIR` and require openclaw itself to report it `loaded`.
/// It also pins the identifier agreement that broke the first attempt —
/// openclaw keys the config entry by the PACKAGE name and then looks the plugin
/// up by its `id`, so `package.json`'s name, the manifest `id` and the exported
/// `id` must all be the same string.
#[cfg(unix)]
#[test]
fn e2e_the_openclaw_bridge_is_loaded_by_a_real_openclaw() {
    if !have("openclaw", &["--version"]) {
        eprintln!("SKIP: openclaw not on PATH — the bridge is untested here");
        return;
    }
    let h = TempHome::new("openclaw-load");
    let plugin = h.path().join("plugin");
    std::fs::create_dir_all(&plugin).unwrap();
    // The installer substitutes the absolute binary path; do the same here so
    // the asset under test is the one users get.
    let index = asset("openclaw_plugin/index.js")
        .replace("@@TOKENADE_BIN@@", &binary().display().to_string());
    std::fs::write(plugin.join("index.js"), index).unwrap();
    for f in ["package.json", "openclaw.plugin.json"] {
        std::fs::write(plugin.join(f), asset(&format!("openclaw_plugin/{f}"))).unwrap();
    }

    let state = h.path().join(".openclaw");
    let run = |args: &[&str]| -> (String, String, bool) {
        let o = Command::new("openclaw")
            .args(args)
            .env("OPENCLAW_STATE_DIR", &state)
            .env("HOME", h.path())
            .output()
            .expect("spawn openclaw");
        (
            String::from_utf8_lossy(&o.stdout).to_string(),
            String::from_utf8_lossy(&o.stderr).to_string(),
            o.status.success(),
        )
    };

    let (out, err, ok) = run(&["plugins", "install", plugin.to_str().unwrap(), "--link"]);
    assert!(
        ok,
        "openclaw refused to register the bridge.\nstdout: {out}\nstderr: {err}"
    );

    let (list, lerr, _) = run(&["plugins", "list"]);
    // Match on the STATUS, not on the name. The plugin's own `register()` logs
    // a line containing "tokenade", so the first line bearing the name is our
    // log output, not openclaw's verdict — picking it made this assertion
    // report a load failure while the plugin was in fact loaded. Probe for what
    // success LEAVES (a table row saying `loaded`), never for the name.
    let named: Vec<&str> = list.lines().filter(|l| l.contains("tokenade")).collect();
    assert!(
        !named.is_empty(),
        "openclaw does not list the tokenade plugin at all.\n{list}\n{lerr}"
    );
    // Two spellings of the same verdict, because the host renamed it: openclaw
    // 2026.1.30 printed `loaded` in that column and 2026.6.34 prints `enabled`
    // (measured 2026-08-21 on both). Asserting only the older word failed on a
    // plugin that was in fact active, which is the same false accusation the
    // comment above is about, one release later.
    assert!(
        named
            .iter()
            .any(|l| l.contains("loaded") || l.contains("enabled")),
        "openclaw lists the plugin but did not LOAD it — that is exactly the \
         state the bridge was once found in, and no test could see:\n{}",
        named.join("\n")
    );
}

/// One authority per payload: an agent with a plugin bridge is never ALSO
/// MCP-wrapped.
///
/// The bridges fold every `tool_result` block inside the agent, MCP results
/// included. Wrapping that agent's MCP servers as well puts a second compactor
/// on the same bytes — the one thing "one authority per payload" forbids,
/// and the source of the rollup double-count this project already paid for.
///
/// `mcp_wrap` states the rule in a comment listing OpenCode, KiloCode and
/// Hermes as deliberately absent. Cline was listed anyway: the single agent
/// that had both channels was the one the rule existed for. A comment cannot
/// fail, so the rule lives here now.
#[test]
fn an_agent_with_a_plugin_is_not_also_mcp_wrapped() {
    let both: Vec<&str> = tokenade_core::detect::Agent::all()
        .iter()
        .filter(|a| matches!(a.coverage(), tokenade_core::detect::Coverage::Plugin))
        .map(|a| a.as_str())
        .filter(|name| tokenade_core::mcp_wrap::agent_has_known_mcp_site(name))
        .collect();
    assert!(
        both.is_empty(),
        "these agents run a tokenade PLUGIN and would ALSO have their MCP \
         servers wrapped — two authorities on the same bytes: {both:?}"
    );
    // Anti-vacuity in both directions: there must BE plugin agents, and the
    // MCP site table must not be empty either, or this passes by describing a
    // product with no channels at all.
    let plugins = tokenade_core::detect::Agent::all()
        .iter()
        .filter(|a| matches!(a.coverage(), tokenade_core::detect::Coverage::Plugin))
        .count();
    assert!(
        plugins >= 5,
        "only {plugins} plugin agents — the walk is wrong"
    );
    assert!(
        ["claude", "cursor", "codex", "devin-desktop"]
            .iter()
            .all(|a| tokenade_core::mcp_wrap::agent_has_known_mcp_site(a)),
        "the MCP site table lost agents it is supposed to carry"
    );
}

/// `--cmd` must reach the reader `wrap` would use. A real `dotnet build` with
/// 122 warnings went to content sniffing, which read it as an archive listing:
/// 549 bytes, no warning, no verdict (2026-10-05).
#[cfg(unix)]
#[test]
fn e2e_a_hint_reaches_the_command_reader_wrap_uses() {
    let h = TempHome::new("hint-dotnet");
    let build = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tokenade-core/tests/fixtures/linux/dotnet_build_warnings.txt"
    ))
    .unwrap();
    let (out, _err, code) =
        run_with_stdin(&["read", "-", "--cmd", "dotnet build"], h.path(), &build);
    assert_eq!(code, 0);
    for kept in [
        "Build succeeded.",
        "warning CS0219 ×41",
        "warning CS8603 ×40",
        "122 Warning(s)",
    ] {
        assert!(out.contains(kept), "{kept} missing:\n{out}");
    }
}
