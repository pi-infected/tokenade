<p align="center">
  <a href="https://tokenade.net">
    <img src="docs/assets/hero.svg" alt="Tokenade — a marble hand squeezes a lemon and every drop is a coding agent (Claude Code, Codex, Cursor, Copilot, OpenCode…). Squeeze your agent's token bill: open source, 100% local. #1 on THOL — 39% cheaper on long sessions." width="900" />
  </a>
</p>

<h1 align="center">Tokenade</h1>

<p align="center">
  <b>Cut your AI coding agent's token bill — locally, automatically, on every prompt.</b>
</p>

<p align="center">
  <img src="docs/assets/demo.gif" alt="Demo: npm install -g @tokenade/cli, tokenade install, then a Claude Code session runs pytest -v and Tokenade folds the 342 passing-test lines; the dashboard shows the tokens saved." width="900" />
</p>

<p align="center">
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="license" /></a>
  <a href="https://www.npmjs.com/package/@tokenade/cli"><img src="https://img.shields.io/npm/v/@tokenade/cli?label=%40tokenade%2Fcli&logo=npm" alt="npm" /></a>
  <a href="https://tokenade.net/en/thol"><img src="https://img.shields.io/badge/THOL-%231-ffd103" alt="#1 on THOL" /></a>
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#what-it-does">What it does</a> ·
  <a href="https://tokenade.net/en/thol">Benchmark</a> ·
  <a href="https://tokenade.net">Website</a>
</p>

Tokenade is a single Rust binary that sits between your coding agent (Claude
Code, Codex, Cursor, Copilot CLI, OpenCode, Kilo Code, Qwen Code…) and the
model. It strips what the model never needed to see — 2,000-line build logs,
whole files when one function mattered, verbose MCP payloads, re-reads of
content already in context — before it is billed, and keeps every byte it
folds recoverable on demand.

## #1 on THOL

Tokenade ranks **#1 on THOL**, the Token-Harness Optimizer Leaderboard —
real end-to-end agent runs, every optimizer measured the same way:
**39% cheaper on long sessions**. Method and full results:
<https://tokenade.net/en/thol>.

## What it does

- **Command compaction** — test runs, builds, logs, `git`, `kubectl`,
  `docker`, package managers… noisy output is folded to what matters
  (failures, numbers, decisive lines) before the agent reads it.
- **Read deduplication** — content the agent already has comes back as a
  short reference, or as a diff when it changed.
- **Code intelligence** — `map`, `query`, `skeleton`, `impact` and hybrid
  `semantic` search, so the agent stops reading whole files to find one thing.
- **MCP optimization** — third-party MCP servers are wrapped and their large
  results compacted in transit.
- **Web** — `tokenade web` / `tokenade search` return compact Markdown and
  result lists instead of raw HTML; on agents with hooks, the agent's own web
  fetch and search go through them, and fall back to the native tool when
  Tokenade's search engines can't answer.
- **Documents and media** — `tokenade read` turns PDF, Office, EPUB, audio,
  video and image files into text the agent can use (a transcript when a
  local `whisper` or `whisper-cli` is installed, OCR when `tesseract` is).
- **Context and output** — prompt-cache-aware compaction of stale context,
  and a lean-output style note that drops filler without dropping facts.
- **Secret redaction** — credentials are scrubbed from what Tokenade logs,
  caches and folds.
- **Nothing is lost** — every fold carries a hash; `tokenade expand-ref`
  gives the exact bytes back.

## Install

### npm (prebuilt binaries)

```sh
npm install -g @tokenade/cli
tokenade install      # detects your coding agents and wires Tokenade in
tokenade healthcheck  # optional: verify the wiring
```

The binary for your platform ships as an optional per-platform npm package;
if that is skipped, `postinstall` downloads the archive from this repository's
GitHub Release and checks it against the release's Ed25519-signed manifest
(SHA-256). Linux (x64 glibc/musl, arm64), macOS (x64, arm64), Windows (x64,
arm64). Node ≥ 18.

Prebuilt archives are also attached to every
[GitHub Release](https://github.com/pi-infected/tokenade/releases).

### Build from source

Requires a recent stable Rust toolchain ([rustup](https://rustup.rs)) and a
C/C++ compiler (tree-sitter grammars and SQLite are compiled from source).

```sh
git clone https://github.com/pi-infected/tokenade
cd tokenade
cargo install --locked --path crates/tokenade-cli   # or use a script below
tokenade install
```

Per-OS scripts in `scripts/build/` build the same targets as the release CI
and print where the binary is:

- **Linux** (x86_64 / aarch64): `scripts/build/build-linux.sh` for a native
  glibc build; `--musl` for a static binary (uses
  [`cargo zigbuild`](https://github.com/rust-cross/cargo-zigbuild) if
  installed); `--target <triple>` for anything else. `lld` is used as linker
  on x86_64 when present.
- **macOS** (Xcode Command Line Tools): `scripts/build/build-macos.sh` for
  this Mac's architecture; `--target aarch64-apple-darwin` (Apple Silicon) or
  `--target x86_64-apple-darwin` (Intel); `--universal` for both, merged with
  `lipo`.
- **Windows** (PowerShell): `.\scripts\build\build-windows.ps1` builds
  `x86_64-pc-windows-gnu` with a static CRT; it needs MinGW-w64 (`gcc`, `g++`)
  on `PATH`, e.g. from MSYS2. `-Target aarch64-pc-windows-gnullvm` builds
  ARM64 with [llvm-mingw](https://github.com/mstorsjo/llvm-mingw).

The semantic-search embedding model is downloaded on first use (Hugging Face,
with a signed GitHub Release asset as fallback) and cached; set
`TOKENADE_MODEL_DIR` to a directory that already holds it for offline machines.

## Supported agents

`tokenade install` detects the agents on your machine and wires each through
the strongest channel it offers:

- **Hooks:** Claude Code (CLI and Claude Desktop's Code tab), Codex CLI,
  Copilot CLI, Cursor, Qwen Code, Droid, Antigravity CLI, T3 Code (through
  Claude Code's hooks), and Grok (hooks plus the local LLM proxy, since its
  hooks can't rewrite output).
- **Plugin or extension:** OpenCode, Kilo Code, Cline, Hermes, OpenClaw, Pi.
- **Local LLM proxy:** Aider (with a startup command that loads
  `tokenade map`) and the Codex app, plus Codex CLI until its hooks are
  approved.
- **Rules file and MCP wrapping only:** Devin Desktop (formerly Windsurf),
  Copilot in VS Code, and Claude Desktop's Chat (MCP only).

Hook and plugin agents get nearly every feature above (context optimisation
is partial on some); the others get part of them, more with the LLM proxy.
The per-agent, per-feature matrix is at
<https://tokenade.net/en/docs/compatibility>.

`tokenade install --only <agent>` limits the install to one agent;
`tokenade uninstall` removes everything it added.

## Network access

Your savings
ledger and dashboard (`tokenade gain`, `tokenade dashboard`) stay on your
machine. The only network calls are:

- web fetches and searches: the ones you or your agent ask for
  (`tokenade web`, `tokenade search`, and the agent's own web tools on hook
  agents), sent straight to the site or to public search engines;
- the one-time embedding-model download (see above);
- a daily **auto-update** check that reads the signed `manifest.json` of the
  latest GitHub Release and only installs a binary whose signature and
  SHA-256 verify.

Turn auto-update off with `TOKENADE_NO_AUTO_UPDATE=1` (or
`tokenade upgrade --off`); `tokenade upgrade` updates on demand.
`TOKENADE_NO_WEBSEARCH=1` / `TOKENADE_NO_WEBFETCH=1` keep your agent's own
web tools.

`tokenade install` ends with a one-time line inviting you to star the
repository; `TOKENADE_NO_STAR_INVITE=1` turns it off.

## Upgrading from 1.x

1.x installs update themselves to 2.0 (or run `npm i -g @tokenade/cli@latest`).
On its first run, 2.0 deletes the leftover 1.x licence, quota and
usage-report files under `~/.tokenade` once and keeps your savings history.
`tokenade login`, `activate` and `report` no longer exist. Run
`tokenade install` again if your agent's hooks look stale.

## Everyday commands

```sh
tokenade wrap 'cargo test'          # run a noisy command, get the compacted output
some-cmd | tokenade read -          # compact any blob; format auto-detected
tokenade read report.pdf            # a document or media file as text
tokenade search "tokio cancel"      # web search: titles, URLs, snippets
tokenade web https://example.com    # a page as compact Markdown
tokenade map                        # codebase structure
tokenade query parse_config         # locate a symbol
tokenade skeleton src/lib.rs        # signatures only
tokenade semantic "where is auth"   # search by meaning
tokenade expand-ref <hash>          # get folded bytes back
tokenade dashboard                  # what you saved: graphical local report
tokenade gain                       # same totals, in the terminal
tokenade help                       # everything else
```

## Contributing

Bug reports, compactor fixtures and pull requests are welcome — see
[CONTRIBUTING.md](CONTRIBUTING.md) and the
[Code of Conduct](CODE_OF_CONDUCT.md). Security issues: see
[SECURITY.md](SECURITY.md).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option (`MIT OR Apache-2.0`).

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this project by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
