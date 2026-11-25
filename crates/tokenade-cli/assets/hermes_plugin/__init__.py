"""tokenade <-> Hermes bridge (auto-installed by `tokenade install`).

Pipes large tool results through the tokenade binary for compaction, returning
the smaller version that Hermes then puts in front of the model. Terminal
output is intentionally skipped: tokenade's universal shell proxy already
compacts shell commands before Hermes sees them.

Hook API (verified against NousResearch/hermes-agent, hermes_cli/plugins.py
VALID_HOOKS + model_tools.py invoke): Hermes exposes `transform_tool_result`,
invoked with ALL-KEYWORD args (tool_name=, args=, result=, session_id=, …);
the first callback returning a str replaces the result. Registering an unknown
hook name only WARNS (and pollutes the user's Hermes log every startup), so we
register exactly the one valid name. A per-registration and per-fire marker is
written under the plugin dir so `tokenade doctor` can tell a Hermes user
whether the hook ever actually fired. Nothing here ever raises into the host.

Stdlib-only (subprocess + os + shutil): all real compaction logic lives in the
Rust binary. This file is a thin bridge so the multi-OS surface stays in Rust —
it runs under Hermes' own Python interpreter, which is guaranteed present.
"""
import functools
import os
import shutil
import subprocess

# Only truly-interactive PTY tools are skipped. Bash/shell command RESULTS are
# folded HERE (in-plugin) rather than delegated to the PATH shell-proxy: the
# proxy only intercepts shells it can shim (PowerShell / Git-Bash), so on
# Windows `cmd.exe` command output reaches Hermes uncompacted. Folding the
# RESULT string is shell-agnostic — closes the cmd.exe gap. When the shim
# already folded it (Linux / PowerShell), re-running `read` is a harmless no-op
# (we only replace when it got strictly shorter → no double-fold, no double-
# count). Matched case-insensitively, by substring.

_SHELL_TOOLS = ("bash", "shell", "run", "execute", "command", "terminal")


def _command_hint(tool_name, args):
    """The shell command behind a tool result, or None.

    Only shell-ish tools are considered: a `read`/`grep` tool's "command"
    would send the dispatcher to the wrong compactor. Never raises — a bad
    shape just means no hint, i.e. exactly the previous behaviour.
    """
    try:
        if not tool_name or not any(s in str(tool_name).lower() for s in _SHELL_TOOLS):
            return None
        if isinstance(args, str):
            c = args
        elif isinstance(args, dict):
            c = args.get("command") or args.get("cmd") or ""
        else:
            return None
        c = str(c).strip()
        return c[:400] if c else None
    except Exception:
        return None

# A file the HOST could not read properly.
#
# This bridge folds a tool RESULT, which works for text and cannot work for a
# document: by the time a `.docx`, `.xlsx` or `.pdf` reaches this hook, Hermes
# has already decoded its bytes lossily to text, and no amount of compaction
# puts the words back. `tokenade read <path>` opens the file itself and runs
# the extractor — the same one the Read HOOK gives the Claude family.
#
# Substring matching, like `_command_hint` above and for the same reason. A
# false positive is bounded twice: the tool must also carry a path in its
# arguments, and a RANGE read is left alone, because the range targets the
# original file and our extraction is not it.
_READ_TOOLS = ("read", "view")


def _read_target_path(tool_name, args):
    """The file a read-ish tool named, or None. Never raises."""
    try:
        if not tool_name or not any(s in str(tool_name).lower() for s in _READ_TOOLS):
            return None
        if not isinstance(args, dict):
            return None
        for k in ("offset", "limit", "start_line"):
            if args.get(k) is not None:
                return None
        for k in ("filePath", "file_path", "path", "absolute_path", "target_file"):
            v = args.get(k)
            if isinstance(v, str) and v.strip():
                return v
        return None
    except Exception:
        return None

# tokenade prints this on stderr when its output has SECRETS REMOVED. That
# output is often LONGER than what we sent (`API_KEY=k0` becomes
# `API_KEY=<redacted>`), so the "strictly shorter" rule below would throw the
# scrubbed version away and hand the model the raw key. Measured 2026-08-08:
# three ~2 kB payloads well above _MIN_CHARS each grew about a tenth once
# scrubbed, and all of them were discarded. Length answers "did it fold"; it
# cannot answer "was a secret removed", and only tokenade knows that.
_REDACTED_SIGNAL = "[tokenade] redacted:"


def _scrubbed(stderr):
    return isinstance(stderr, str) and _REDACTED_SIGNAL in stderr


_SKIP_SUBSTR = ("terminal",)
_MIN_CHARS = 1000   # below this there's nothing worth folding; skip the round-trip
# A payload too small to be worth FOLDING is not too small to hold a SECRET.
# Measured 2026-08-08 over 8 940 real tool results: 78 % sit under _MIN_CHARS,
# so that floor meant more than three quarters of everything an agent read was
# never scanned for credentials. An `AKIA...` key is 20 characters.
#
# Folding still needs _MIN_CHARS; the two floors are separate concerns now.
_REDACT_MIN_CHARS = 8
_MAX_CHARS = 4_000_000  # above this, don't load+pipe it: a 200MB result would
                        # stall the turn and thrash memory for no realistic win
_TIMEOUT_S = 10     # synchronous per tool_result — keep the worst-case turn
                    # stall bounded; a slower fold isn't worth the freeze

# The one valid Hermes hook for replacing a tool result before the model sees
# it (VALID_HOOKS in hermes_cli/plugins.py). Terminal output has its own hook
# (transform_terminal_output) which we deliberately DON'T take — the shell
# proxy already compacts commands before Hermes runs them.
_TRANSFORM_HOOKS = ("transform_tool_result",)

_MARKER_DIR = os.path.dirname(os.path.abspath(__file__))


def _mark(name):
    # Best-effort breadcrumb for `tokenade doctor`; never raises.
    try:
        with open(os.path.join(_MARKER_DIR, name), "w") as f:
            f.write("1")
    except Exception:
        pass


def _looks_binary(s):
    # A base64 image blob or binary payload won't fold and could be mangled;
    # a data: URI or a NUL byte is a clear tell. Cheap prefix check only.
    head = s[:256]
    return "\x00" in head or head.lstrip().startswith("data:")


def _skipped(tool_name):
    low = (tool_name or "").lower()
    return any(s in low for s in _SKIP_SUBSTR)


# `@@TOKENADE_BIN@@` is replaced with the absolute binary path by the installer
# ── Session identity, so a re-delivery can be folded ─────────────────────
#
# `tokenade read` folds a repeated delivery to a §ref pointer only when it is
# told which session provably received those bytes; with no session it never
# folds, by design — it cannot claim "you already have this" about a context it
# knows nothing about. Nothing on the bridge path ever supplied one, so every
# bridge agent went without read-dedup. Measured on the compat bench
# 2026-08-18, all three OS: the same bytes through `tokenade read -` twice came
# back the same size both times.
#
# Hermes is the one host that hands the id over itself (`session_id=` in the
# hook kwargs), so this bridge claims exactly what it can prove. The process is
# the fallback, and it is a session's lifetime here: Hermes runs one agent per
# process.
def _session_key(session_id):
    if isinstance(session_id, str) and session_id.strip():
        return session_id
    return "p%d-hermes" % os.getpid()


# (cross-OS — no `~/.local/bin` guess). The env var still wins; if injection was
# skipped we fall back to the bare name resolved on PATH.
_INJECTED_BIN = "@@TOKENADE_BIN@@"


# An injected ABSOLUTE path goes stale the moment tokenade is moved, upgraded
# into a different prefix, or uninstalled and reinstalled. Every run then raises
# FileNotFoundError, the blanket `except` swallows it, and this bridge folds
# nothing for the rest of the install's life — with no signal to the user. "The
# binary existed at install time" is not the same question as "it exists now".
# Fall back to the bare name on PATH once, and remember what worked.
_RESOLVED_BIN = None


def _run_tokenade(bin_path, argv_tail, **kw):
    global _RESOLVED_BIN
    first = _RESOLVED_BIN or bin_path
    try:
        proc = subprocess.run([first] + argv_tail, **kw)
        _RESOLVED_BIN = first
        return proc
    except FileNotFoundError:
        if first == "tokenade":
            raise
        proc = subprocess.run(["tokenade"] + argv_tail, **kw)
        _RESOLVED_BIN = "tokenade"
        return proc


def _tokenade_bin():
    env_bin = os.environ.get("TOKENADE_BIN")
    if env_bin:
        return env_bin
    if not _INJECTED_BIN.startswith("@@"):
        return _INJECTED_BIN
    return shutil.which("tokenade") or "tokenade"


def _transform_tool_result(bin_path, *args, tool_name=None, result=None, **_kw):  # noqa: C901
    # Positional- AND keyword-tolerant: Hermes may call hook(name,
    # result) or hook(result=…, tool_name=…). Recover the two values
    # from whatever shape arrives; never raise into the host turn.
    try:
        if result is None and args:
            # Heuristic: the longest string arg is the tool result;
            # a shorter string arg is the tool name.
            strs = [a for a in args if isinstance(a, str)]
            if strs:
                result = max(strs, key=len)
                if tool_name is None and len(strs) > 1:
                    tool_name = min(strs, key=len)
        if not isinstance(result, str):
            return None
        if len(result) > _MAX_CHARS:
            return None
        # The floor below is about FOLDING and about scanning for
        # secrets, and a document read is neither: the words are in the
        # FILE, not in what the host handed back. A host that decoded
        # nothing must not close the door on the extractor.
        _doc_path = _read_target_path(tool_name, _kw.get("args"))
        if len(result) < _REDACT_MIN_CHARS and not _doc_path:
            return None
        if _skipped(tool_name) or _looks_binary(result):
            return None
        env = dict(os.environ)
        # Attribute the saving to the Hermes agent in tokenade's ledger.
        # Highest-precedence override so a parent agent's env can't
        # mis-attribute it.
        env["TOKENADE_AGENT"] = "hermes"
        # See `_session_key`: without this the repeat-delivery fold is
        # unreachable, because the binary refuses to claim a context it
        # was told nothing about.
        env["TOKENADE_SESSION"] = _session_key(_kw.get("session_id"))
        # The command behind a shell result. `read -` detects by
        # CONTENT and content cannot say which program produced it —
        # measured 2026-08-07, git output came back uncompacted without
        # this and folded to a quarter with it. Hermes passes the tool
        # input as `args=`; we were dropping it into `**_kw`.
        # Documents first: extract from the FILE, not from what the
        # host made of it. Falls through to the generic fold when
        # there is nothing to gain.
        rp = _doc_path
        if rp:
            dp = _run_tokenade(
                bin_path,
                ["read", rp],
                capture_output=True,
                encoding="utf-8",
                errors="replace",
                timeout=_TIMEOUT_S,
                env=env,
            )
            dout = dp.stdout
            # Not a fold, so not the fold's rule: with an empty host
            # result "is it smaller?" is false for every extraction,
            # and the words are dropped for the empty string.
            if dp.returncode == 0 and dout and (
                len(result) == 0
                or len(dout) < len(result)
                or _scrubbed(dp.stderr)
            ):
                _mark("hook-fired")
                return dout
        hint = _command_hint(tool_name, _kw.get("args"))
        argv = ["read", "-"]
        if hint:
            argv += ["--cmd", hint]
        proc = _run_tokenade(
            bin_path,
            argv,
            input=result,
            capture_output=True,
            # Pin UTF-8 (not the locale default) so non-ASCII tool
            # output — CJK, accents, emoji — round-trips without
            # mojibake.
            encoding="utf-8",
            errors="replace",
            timeout=_TIMEOUT_S,
            env=env,
        )
        out = proc.stdout
        # Only replace when the result got strictly SHORTER. `auto -`
        # may echo an unchanged input back verbatim (+ a trailing
        # newline) when nothing folds — returning that would hand the
        # model a LONGER string. "Never make it worse" beats reclaiming
        # the rare token-shorter-but-char-longer reflow.
        if proc.returncode == 0 and out and (len(out) < len(result) or _scrubbed(proc.stderr)):
            _mark("hook-fired")
            return out
    except Exception:
        # Any failure -> return None so Hermes keeps the original.
        pass
    return None


# `pre_tool_call` — the only moment a delegation's arguments are
# still modifiable. Hermes passes the SAME `args` dict the executor
# will use next (`model_tools.py` calls
# `resolve_pre_tool_block(function_name, function_args, …)` and
# `_get_pre_tool_call_directive_details` forwards it without a copy),
# so an in-place mutation lands in the real call.
#
# The tool is `delegate_task`, whose schema carries `goal` (the task)
# and `context` (what the child must know), in single form or batched
# under `tasks`. We prefix `context`, never `goal`: the goal stays
# exactly what the parent wrote.
#
# The brief text is NOT copied here — `tokenade subagent-brief`
# builds it and counts the overhead once, as for the five other
# bridges.
def _brief_subagent(bin_path, *_args, tool_name=None, args=None, **_kw):
    try:
        if not isinstance(args, dict):
            return None
        name = str(tool_name or "").lower()
        if "delegate" not in name and "task" not in name and "agent" not in name:
            return None
        targets = []
        if isinstance(args.get("tasks"), list):
            targets = [t for t in args["tasks"] if isinstance(t, dict)]
        else:
            targets = [args]
        for t in targets:
            original = t.get("context")
            if not isinstance(original, str):
                original = ""
            proc = subprocess.run(
                [bin_path, "subagent-brief"],
                input=original,
                capture_output=True,
                text=True,
                timeout=_TIMEOUT_S,
                env=dict(os.environ, TOKENADE_AGENT="hermes"),
            )
            out = proc.stdout or ""
            if proc.returncode == 0 and len(out) > len(original):
                t["context"] = out
                _mark("subagent-briefed")
    except Exception:
        # fail-open: never block a delegation for a prefix
        pass
    return None


def _tick(key, *flags):
    try:
        subprocess.Popen(
            [_RESOLVED_BIN or _tokenade_bin(), "session-tick", "--session", key, "--agent", "hermes", *flags],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            start_new_session=True,
            # Windows: no console window for a detached child.
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
    except Exception:
        pass


# Messages each session sent on its last API request, and whether the
# per-request hook fires at all on this Hermes.
_HISTORY = {}
_PER_REQUEST = {"seen": False}


def _tick_request(*_args, session_id=None, message_count=None, approx_input_tokens=None, **_kw):
    # `pre_api_request` (hermes `agent/conversation_loop.py`): once per
    # API request — the exact call clock. A compression in place
    # (`compression.in_place`, the default of new configs) keeps the
    # session id and fires no plugin hook. Within one conversation a
    # request only grows, so a smaller one was compacted. The request
    # SIZE is the signal: MEASURED 2026-10-15 on a real hermes 0.18.2
    # session, a compaction took the request from 55 541 to 39 680
    # tokens, while in another run eleven compactions left the message
    # COUNT unchanged. One detached call either way: two could run out
    # of order.
    _PER_REQUEST["seen"] = True
    key = _session_key(session_id)
    if not key:
        return None
    prev_n, prev_t = _HISTORY.get(key, (0, 0))
    n = message_count if isinstance(message_count, int) else prev_n
    t = approx_input_tokens if isinstance(approx_input_tokens, int) else prev_t
    shrunk = n < prev_n or t * 10 < prev_t * 9
    _HISTORY[key] = (n, t)
    _tick(key, "--per-call", *(["--reset"] if shrunk else []))
    return None

def _tick_session(*_args, session_id=None, **_kw):
    # Older Hermes without `pre_api_request`: one tool call stands for
    # its API calls. Skipped once the per-request clock runs.
    if _PER_REQUEST["seen"]:
        return None
    key = _session_key(session_id)
    if key:
        _tick(key)
    return None


def register(ctx):
    # The whole entry point is defensive: a plugin that raises here could stop
    # Hermes from starting. Never let anything escape.
    try:
        bin_path = _tokenade_bin()

        transform_tool_result = functools.partial(_transform_tool_result, bin_path)

        try:
            ctx.register_hook("pre_tool_call", functools.partial(_brief_subagent, bin_path))
        except Exception:
            pass

        for name, fn in (("pre_api_request", _tick_request), ("pre_tool_call", _tick_session)):
            try:
                ctx.register_hook(name, fn)
            except Exception:
                pass

        # Register under every candidate name; whichever Hermes supports wins,
        # unknown names are caught and ignored. Record that at least one
        # registration succeeded so doctor can distinguish "never installed"
        # from "installed but the hook never fired" (= Hermes build has no
        # tool-result transform point).
        registered = False
        for name in _TRANSFORM_HOOKS:
            try:
                ctx.register_hook(name, transform_tool_result)
                registered = True
            except Exception:
                continue
        if registered:
            _mark("hook-registered")
    except Exception:
        # Absolute backstop: register() must never raise into Hermes.
        pass
