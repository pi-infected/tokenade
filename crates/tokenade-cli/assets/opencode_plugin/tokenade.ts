// tokenade <-> opencode bridge (auto-installed by `tokenade install`).
//
// opencode auto-discovers `{plugin,plugins}/*.ts` under its config dir, so this
// file loads with no config edit. It registers the `tool.execute.after` hook
// and compacts large text tool results via the tokenade binary, mutating
// `output.output` in place — which is exactly what opencode returns to the
// model.
//
// Bash/shell results are compacted HERE too, not delegated to the PATH
// shell-proxy: the proxy only intercepts shells it can shim (PowerShell /
// Git-Bash), so on Windows `cmd.exe` command output reaches opencode
// uncompacted. Operating on the RESULT string is shell-agnostic — it closes
// the cmd.exe gap without needing cmd.exe interception at all. When the shim
// DID already fold the output (Linux / PowerShell), re-running `read` is a
// harmless no-op: fail-open only replaces when the result actually shrank,
// and already-folded text no longer shrinks. Only genuinely interactive
// `terminal` sessions are skipped (live PTY, not a finished command result).
//
// The ENTRY SHAPE differs per host, and one file serves both:
//   * opencode 1.x / Kilo Code: a v1 plugin default-exports a FACTORY that
//     RETURNS the hooks object;
//   * opencode 2.x: the default export must be a DEFINITION object with a
//     stable `id` and a `setup(ctx)` (or `effect`) function. A function there
//     fails as `PluginModule.LoadError: Plugin must export a default definition
//     with an id and an effect or setup function (cause: SchemaError(Expected
//     object at ["default"]))` and the bridge folds nothing, silently.
// So the default export is an object carrying BOTH: `setup` for v2 and
// `server()` for v1 (the v1 loader has honoured an object entrypoint with a
// `server` method since 1.18.29). No SDK import needed (the plugin types are
// erased); node:child_process is provided by the Bun runtime opencode runs
// under, so all real work stays in the Rust bin.
import { spawn, spawnSync } from "node:child_process";

const SKIP = new Set(["terminal"]);
const MIN_CHARS = 1000;
// A payload too small to be worth FOLDING is not too small to hold a SECRET.
// Measured 2026-08-08 over 8 940 real tool results: 78 % sit under MIN_CHARS,
// so that floor meant more than three quarters of everything an agent read was
// never scanned for credentials. An `AKIA…` key is 20 characters.
//
// Folding still needs MIN_CHARS — below it there is nothing to compact — so the
// two floors are now separate concerns rather than one number doing two jobs.
// Between the two, the binary redacts and does not fold, and the acceptance
// test below keeps the raw text unless something was actually scrubbed: a clean
// small payload therefore comes back byte-identical.
const REDACT_MIN_CHARS = 8;
const MAX_CHARS = 4000000; // above this, don't load+pipe it (a 200MB result stalls the turn)
const TIMEOUT_MS = 10000; // bounded synchronous stall per tool result

function tokenadeBin() {
  // `@@TOKENADE_BIN@@` is replaced with the absolute binary path by the
  // installer (cross-OS). The env var still wins; if injection was skipped we
  // fall back to the bare name resolved on PATH.
  const injected = "@@TOKENADE_BIN@@";
  return process.env.TOKENADE_BIN || (injected.startsWith("@@") ? "tokenade" : injected);
}

// The command that produced a shell result, when it can be found.
//
// `tokenade read -` detects by CONTENT, and content does not say which program
// wrote it: measured 2026-08-07, `git log --oneline` piped through it came back
// unchanged while the same bytes with `--cmd` folded to a quarter. The command
// is already in the tool input; not passing it threw away the one thing the
// detector cannot infer.
//
// Deliberately defensive about WHERE it lives: opencode's hook payload shape is
// not part of a stable contract we control, so every plausible field is tried
// and a miss simply means today's behaviour. Only shell-ish tools are
// considered — a `read`/`grep` tool's "command" would mislead the dispatcher.
// tokenade prints this on stderr when its output has SECRETS REMOVED. That
// output is often LONGER than what we sent (`API_KEY=k0` becomes
// `API_KEY=<redacted>`), so the "strictly shorter" rule below would throw the
// scrubbed version away and hand the model the raw key. Measured 2026-08-08:
// three ~2 kB payloads well above MIN_CHARS each grew about a tenth once
// scrubbed, and all of them were discarded. Length answers "did it fold"; it
// cannot answer "was a secret removed", and only tokenade knows that.
// An injected ABSOLUTE path goes stale the moment tokenade is moved, upgraded
// into a different prefix, or uninstalled and reinstalled. Every spawn then
// fails with ENOENT, the catch below swallows it, and this bridge folds
// nothing for the rest of the install's life — with no signal to the user.
// "The binary exists at install time" is not the same question as "it exists
// now". Fall back to the bare name on PATH once, and remember what worked.
let resolvedBin = null;
// One tool call = at least one API call that re-reads everything tokenade put
// in this conversation. The host runs no hook process of ours per call, so the
// bridge tells the binary (`session-tick`), detached: the tool never waits.
// `reset`: the host compacted the conversation — what tokenade folded out of
// it or put into it is no longer re-read, on either side of the ledger.
function tickSession(session, agent, reset = false) {
  if (!session) return;
  try {
    const argv = ["session-tick", "--session", session, "--agent", agent];
    if (reset) argv.push("--reset");
    const c = spawn(resolvedBin || tokenadeBin(), argv, {
      stdio: "ignore",
      detached: true,
      // Windows: a detached child gets a console window of its own.
      windowsHide: true,
    });
    c.on("error", () => {});
    c.unref();
  } catch {
    /* fail-open */
  }
}

function runTokenade(argv, opts) {
  const first = resolvedBin || tokenadeBin();
  let r = spawnSync(first, argv, opts);
  if (r.error && r.error.code === "ENOENT" && first !== "tokenade") {
    r = spawnSync("tokenade", argv, opts);
    if (!r.error) resolvedBin = "tokenade";
  } else if (!r.error) {
    resolvedBin = first;
  }
  return r;
}

// This one asset serves TWO agents: opencode and Kilo Code, whose plugin
// system it mirrors. The label was hardcoded `"opencode"`, so every saving a
// Kilo user made was credited to opencode in the ledger — and any check asking
// "has the kilo bridge ever fired?" answered no, for ever. The installer
// substitutes the real one, exactly like `@@TOKENADE_BIN@@`.
const AGENT_INJECTED = "@@TOKENADE_AGENT@@";
const AGENT = AGENT_INJECTED.startsWith("@@") ? "opencode" : AGENT_INJECTED;

const REDACTED_SIGNAL = "[tokenade] redacted:";
function scrubbed(r) {
  return typeof r.stderr === "string" && r.stderr.includes(REDACTED_SIGNAL);
}

// Substring, not exact match, and the difference is total rather than partial.
// Measured 2026-08-08 with the real binary behind the bridge: a tool named
// `execute_command` (which is what Cline calls its shell tool — this file's
// twin had already been patched for it, alone) produced NO hint, and without a
// hint `tokenade read -` leaves `git log` output untouched. So the result did
// not shrink, the fail-open rule declined to replace it, and the model got all
// 29 115 bytes. With the hint: 4 569. Not a degraded fold — no fold at all.
//
// Hermes' bridge has always matched by substring; the four JS/TS twins used a
// closed set of five names, and one of them had been quietly extended. Aligning
// on the broader test removes the drift and covers `run_command`,
// `shell_exec`, and whatever the next host calls it. A false positive is
// bounded: a non-shell tool has no `command` string in its input, so the hint
// comes back null anyway.
//
// The list stops at five ON PURPOSE. Hermes has a sixth, `terminal`, and that
// is not drift: on these four hosts `terminal` is a LIVE PTY session, skipped
// outright (`SKIP`), while hermes only ever sees finished results. Making the
// five lists identical would add a word whose only known bearer these bridges
// must not touch. Asserted, with the reason, in
// `the_bridge_assets_agree_on_the_constants_they_share`.
// A file the HOST could not read properly.
//
// The bridges fold a tool RESULT, which works for text and cannot work for a
// document: by the time a `.docx`, `.xlsx` or `.pdf` reaches this hook the host
// has already decoded its bytes lossily to UTF-8, and no amount of compaction
// puts the words back. `tokenade read <path>` opens the file itself and runs
// the extractor — the same one the Read HOOK gives the Claude family, which is
// why `docs` was green there and amber on every bridge agent.
//
// Substring matching, like the shell test above and for the same reason: hosts
// name it `read`, `read_file`, `readFile`, `view`, `Read`. A false positive is
// bounded twice — the tool must also carry a path in its arguments, and a
// RANGE read (offset/limit) is left alone, because the range targets the
// original file and our extraction is not it.
const READ_WORDS = ["read", "view"];
function isReadTool(name) {
  const n = String(name || "").toLowerCase();
  return READ_WORDS.some((w) => n.includes(w));
}
function readTargetPath(args) {
  if (!args || typeof args !== "object") return null;
  if (args.offset != null || args.limit != null || args.start_line != null) return null;
  const p = args.filePath ?? args.file_path ?? args.path ?? args.absolute_path ?? args.target_file;
  return typeof p === "string" && p.trim() ? p : null;
}
const SHELL_WORDS = ["bash", "shell", "run", "execute", "command"];
function isShellTool(name) {
  const n = String(name || "").toLowerCase();
  return SHELL_WORDS.some((w) => n.includes(w));
}
function commandHint(input, output) {
  if (!isShellTool(input?.tool)) return null;
  const c =
    input?.args?.command ??
    input?.args?.cmd ??
    output?.metadata?.command ??
    output?.title;
  if (typeof c !== "string") return null;
  const t = c.trim();
  // Cap it: this becomes an argv entry, and only the program name is read.
  return t ? t.slice(0, 400) : null;
}


// ── Session identity, so a re-delivery can be folded ──────────────────
//
// `tokenade read` folds a repeated delivery to a §ref pointer only when it is
// told which session provably received those bytes; with no session it never
// folds, by design — it cannot claim "you already have this" about a context
// it knows nothing about. Nothing on the bridge path ever supplied one, so
// every bridge agent went without read-dedup. Measured on the compat bench
// 2026-08-18, all three OS: the same bytes through `tokenade read -` twice
// came back the same size both times.
//
// The host's own conversation id is preferred, because that is what the claim
// is actually about. The process is the fallback and is a session's lifetime
// on every host that spawns one agent per process.
//
// The pid alone is NOT the fallback, and the bridge e2e proved why: it drives
// all four JS bridges in ONE node process, they shared a key, and the second
// bridge to see a payload folded it to a pointer into the FIRST one's context.
// Production spawns one agent per process, so this was a harness artefact —
// but a key that can name two contexts is wrong wherever it happens.
// DELEGATION tools: the ones that launch a subagent. The list is broad
// because the name varies (`task` on opencode, `new_task` on hosts that follow
// Cline) and a false positive only costs a prefix on a `prompt` argument that
// does not exist.
const DELEGATION_WORDS = ["task", "agent", "delegate", "subagent", "spawn"];
function isDelegationTool(name) {
  const n = String(name || "").toLowerCase();
  return DELEGATION_WORDS.some((w) => n.includes(w));
}

// The key that carries the subagent's instructions, as it comes in.
// We rewrite THE ONE we read, never another.
const PROMPT_KEYS = ["prompt", "message", "instructions", "task"];
function promptKey(args) {
  if (!args || typeof args !== "object") return null;
  for (const k of PROMPT_KEYS) {
    if (typeof args[k] === "string" && args[k].trim()) return k;
  }
  return null;
}

const PROCESS_SESSION = `p${process.pid}-${AGENT}`;
function sessionKey(...candidates) {
  for (const c of candidates) {
    if (typeof c === "string" && c.trim()) return c;
  }
  return PROCESS_SESSION;
}

// No savings counter here, and that is a measured decision, not an oversight.
//
// opencode has no status bar at all: no `statusLine` key in the shipped Go
// binary, nothing equivalent in the plugin SDK. The only surface is
// `client.tui.showToast({title, message, variant, duration})`, and it was
// tried then REMOVED on 2026-08-25 after reading how the binary renders it:
//
//   * the banner sits top right (`position:absolute, top:2, right:2`), clears
//     itself, does not take focus and does not block input — so it is not a
//     popup, and that was not the problem;
//   * its width is `min(60, terminal_width - 6)` with `wrapMode:"word"`.
//     The ticker is ~220 characters: four to five lines in a corner of the
//     screen, for a text written for a one-line bar;
//   * and above all, the provider keeps ONE SINGLE slot — `T("currentToast",
//     X)` overwrites whatever was there, with `clearTimeout` on its timer.
//     It is the same channel opencode uses for its own errors
//     (`session.error` goes through it with `duration: 5000`). Our toast could
//     therefore wipe an error message the user needed to read, and the HTTP
//     API does not expose `currentToast`: no way to look before writing.
//
// The third point cannot be fixed on our side. A promotional surface that can
// swallow an error is not an acceptable trade-off, so the "visible savings
// counter" column is `Ko bin` for opencode and Kilo Code, and that is the
// truth: this host has nowhere to put a permanent counter.
//
// Should it ever come back, it would be ONE short line ("⚡ 17.5k tokens
// saved"), no title, at the host's default duration — and the single-slot
// problem would remain whole.

// The hook bodies are shared by both entry shapes below. They are written
// against the v1 payload (`input` = {tool, args, sessionID}, `output` = the
// tool result) because that is the common denominator; the v2 adapters map
// `event` onto it.
function onEvent({ event }) {
  // `session.compacted` (SDK `EventSessionCompacted`, `properties.sessionID`).
  if (event?.type === "session.compacted") {
    tickSession(sessionKey(event?.properties?.sessionID), AGENT, true);
  }
}

// The subagent inherits the FOLDING simply by running on the same host;
// it does not inherit the INSTRUCTIONS, which live in the delegation
// prompt. `tool.execute.before` exposes `output.args` as writable, so we
// prefix there what Claude Code's PreToolUse hook prefixes elsewhere.
//
// The text is NOT copied here: `tokenade subagent-brief` builds it,
// decides the gate and counts the overhead once. Two copies of the brief
// would be the "two lists describing a single fact" class, the one that
// has already cost four fixes on this product.
async function onToolBefore(input, output) {
  tickSession(sessionKey(input?.sessionID, input?.sessionId), AGENT);
  try {
    if (!isDelegationTool(input?.tool)) return;
    const args = output?.args;
    const key = promptKey(args);
    if (!key) return;
    const original = args[key];
    const r = runTokenade(
      ["subagent-brief", "--type", String(args?.subagent_type ?? args?.agent ?? "")],
      {
        input: original,
        encoding: "utf8",
        timeout: TIMEOUT_MS,
        maxBuffer: 4 * 1024 * 1024,
        env: {
          ...process.env,
          TOKENADE_AGENT: AGENT,
          TOKENADE_SESSION: sessionKey(input?.sessionID, input?.sessionId),
        },
      },
    );
    if (r.status === 0 && typeof r.stdout === "string" && r.stdout.length > original.length) {
      args[key] = r.stdout;
    }
  } catch {
    // fail-open: never break a delegation over a prefix
  }
}

async function onToolAfter(input, output) {
  if (SKIP.has(String(input?.tool || "").toLowerCase())) return;
  const text = output?.output;
  if (typeof text !== "string" || text.length > MAX_CHARS) return;
  // The floor below is about FOLDING and about scanning for secrets, and a
  // document read is neither: the words are in the FILE, not in what the
  // host handed back, so a host that decoded nothing must not close the
  // door on the extractor. Everything else still stops here.
  const docPath = isReadTool(input?.tool) ? readTargetPath(input?.args) : null;
  if (text.length < REDACT_MIN_CHARS && !docPath) return;
  // A base64/data-URI blob won't fold and could be mangled by text compaction.
  if (text.startsWith("data:") || text.charCodeAt(0) === 0) return;
  try {
    const session = sessionKey(input?.sessionID, input?.sessionId);
    // Documents first: extract from the FILE, not from what the host made
    // of it. Falls through to the generic fold when there is nothing to
    // gain, so a plain text read keeps the cheaper stdin path.
    const rp = docPath;
    if (rp) {
      const d = runTokenade(["read", rp], {
        encoding: "utf8",
        timeout: TIMEOUT_MS,
        maxBuffer: 64 * 1024 * 1024,
        env: { ...process.env, TOKENADE_AGENT: AGENT, TOKENADE_SESSION: session },
      });
      // Not a fold, so not the fold's rule: with an empty host result "is
      // it smaller?" is false for every extraction, and the words are lost.
      if (d.status === 0 && d.stdout && (text.length === 0 || d.stdout.length < text.length || scrubbed(d))) {
        output.output = d.stdout;
        return;
      }
    }
    const hint = commandHint(input, output);
    const argv = hint ? ["read", "-", "--cmd", hint] : ["read", "-"];
    const r = runTokenade(argv, {
      input: text,
      encoding: "utf8",
      timeout: TIMEOUT_MS,
      maxBuffer: 64 * 1024 * 1024,
      env: {
        ...process.env,
        TOKENADE_AGENT: AGENT,
        TOKENADE_SESSION: session,
      },
    });
    // Fail-open: only replace when tokenade actually shrank it.
    if (r.status === 0 && r.stdout && (r.stdout.length < text.length || scrubbed(r))) {
      output.output = r.stdout;
    }
  } catch {
    /* leave output.output untouched */
  }
}

// v1 entrypoint (opencode 1.x, and Kilo Code which mirrors that API). Kept
// callable so a host whose loader predates object entrypoints still works.
async function server() {
  return {
    event: onEvent,
    "tool.execute.before": onToolBefore,
    "tool.execute.after": onToolAfter,
  };
}

// v2 entrypoint (opencode >= 2). Hooks are registered on their domain, and
// the callback receives ONE mutable event instead of (input, output); the
// model-visible text is located by `modelResult`, in the host's own order.
//
// The bus is an ASYNC ITERABLE of `{ type, data }` — `subscribe()` returns the
// iterable itself, it does not take a callback — and every `session.*` event's
// `data` carries `sessionID`. The 1.x name for the compaction event
// (`session.compacted`, `properties.sessionID`) does not exist in 2.x: the
// 2.2.1 binary contains no such string at all, only
// `session.compaction.started|delta|ended|failed`. Listening for the old name
// here made the reset dead code, silently, for every 2.x install.
async function setup(ctx) {
  const controller = new AbortController();
  void (async () => {
    try {
      for await (const event of ctx.event.subscribe({ signal: controller.signal })) {
        if (event?.type !== "session.compaction.ended") continue;
        tickSession(sessionKey(event?.data?.sessionID), AGENT, true);
      }
    } catch {
      /* the subscription ends with the plugin */
    }
  })();

  await ctx.tool.hook("execute.before", async (event) => {
    await onToolBefore(
      { tool: event?.tool, args: event?.input, sessionID: event?.sessionID },
      { args: event?.input },
    );
  });

  await ctx.tool.hook("execute.after", async (event) => {
    if (event?.status !== "completed") return;
    const result = event?.result;
    if (!result) return;
    // Read what the MODEL reads, in the host's own order (core
    // `normalizeContent`): `content` when it is a string or a non-empty
    // array, `output` only when `content` is empty. `output` is often
    // STRUCTURED (the shell tool's is `{ output, exit, truncated, … }`), so
    // it is never replaced wholesale — only its text field is written, and
    // only when it was itself the text that got folded.
    //
    // In an array only the first text part — the payload — is folded: the
    // shell tool appends its exit/timeout notice as a second part, which
    // must survive. Replacing the array (rather than mutating the part) is
    // deliberate: the host reads `result.content` back after the hook.
    const carried = modelResult(result);
    if (!carried) return;
    const out = { output: carried.text, args: event?.input, metadata: result.metadata };
    await onToolAfter({ tool: event?.tool, args: event?.input, sessionID: event?.sessionID }, out);
    if (out.output !== carried.text) carried.write(out.output);
  });

  // A returned truthy NON-function is treated as a cleanup function and
  // called, so only this controller is returned. Hook registrations are
  // scoped to the plugin and need no disposal.
  return () => controller.abort();
}

// The text the model will read, plus how to put a replacement back where it
// came from. Mirrors the host's fallback chain; returns null when the result
// carries no text at all.
function modelResult(result) {
  const content = result?.content;
  if (typeof content === "string") {
    return { text: content, write: (text) => { result.content = text; } };
  }
  if (Array.isArray(content) && content.length > 0) {
    const i = content.findIndex((p) => p && p.type === "text" && typeof p.text === "string");
    if (i < 0) return null;
    return {
      text: content[i].text,
      write: (text) => { result.content = content.map((p, j) => (j === i ? { ...p, text } : p)); },
    };
  }
  if (typeof result?.output === "string") {
    return { text: result.output, write: (text) => { result.output = text; } };
  }
  if (result?.output && typeof result.output.output === "string") {
    return { text: result.output.output, write: (text) => { result.output.output = text; } };
  }
  return null;
}

export default { id: "tokenade", setup, server };
