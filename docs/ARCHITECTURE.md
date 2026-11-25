# Tokenade — Architecture

A bird's-eye view for anyone landing in the repo for the first time.
Read this before grep-diving — it explains *why* the layout is the
way it is.

## TL;DR

Tokenade sits between an AI coding agent (Claude Code, Cursor, …) and
the noisy outputs of the tools that agent uses (bash commands, code
searches, web fetches). It transforms 5 000-line `cargo build` logs
into 200-line summaries before they hit the agent's context window.

Two surfaces, one core:

```
   ┌──────────────────────────────────────────────────────┐
   │           Agent (Claude Code, Cursor, …)             │
   └────────────────┬───────────────────────┬─────────────┘
                    │ shell hooks           │ its own MCP servers
                    ▼                       ▼
       ┌────────────────────┐   ┌──────────────────────┐
       │  tokenade-cli      │   │  tokenade mcp-proxy  │
       │  (hooks, wrap,     │   │  (wraps SOMEONE      │
       │  install, query,   │   │  ELSE's MCP server;  │
       │  dashboard)        │   │  folds its results)  │
       └────────┬───────────┘   └──────────┬───────────┘
                │                          │
                └──────────┬───────────────┘
                           ▼
                  ┌────────────────────┐
                  │  tokenade-core     │
                  │  ─ all the         │
                  │    intelligence    │
                  └────────────────────┘
```

## Crate map

The library is no longer one piece. The lower layers were split out into
crates on **2026-09-13**, in the order the dependency graph imposes: a layer
can only become a crate once NONE of its edges points upwards, because a
dependency between crates cannot be a cycle.

| Crate | Files | Lines | What it is |
|---|---|---|---|
| `tk-base` | 62 | 19 610 | The BASE layer, 32 modules: `platform`, `paths`, `tokenizer`, `regexes`, `argv`, `debug_log`, `process_util`, `redact`, `srcsweep`, `stores`, `testtmp`… No outgoing edge. |
| `tk-fold` | 51 | 17 054 | The FOLD layer, 26 modules + `Savings`: `archetypes`, `fold_gate`, `coefficients`, `elision`, `template_fold`, `phrase_fold`, `compress`, `agent_caps`, `textshape`… No outgoing edge. External dependencies: `serde_json`, `regex`, `rusqlite`, `image` — no tree-sitter, no scraper. |
| `tk-formats` | 53 | 21 253 | The FORMATS layer, 41 modules: `json`, `csv`, `yaml`, `markdown`, `logs`, `lockfile`, `tabular`, `junit`, `ndjson`, `metrics`, `systemd`, `terraform`, `pem`, `ssh_keys`, `post_passes`… No outgoing edge. External dependencies: `serde`, `serde_json`, `regex` — and NOTHING else. That is the cut criterion: the seven format modules carrying a heavy dependency (`docs` and `pdf_layout` via `lopdf`, `treesitter` and `dom_dedup` via tree-sitter, `web_tree` and `tree_dedup` via `scraper`, `cmd` via `zip`) stay in `tokenade-core` until `tk-web` and `tk-code` exist. |
| `tk-agents` | 53 | 21 708 | The AGENTS layer, 29 modules: one per recognised harness (`agent_codex`, `agent_gemini`, `agent_cline`, `agent_kilo`, `agent_opencode`, `agent_pi`… fourteen), transcript reading (`claudecode_transcript`, `agent_transcripts`, `transcript_cache`, `usage_dedup`), detection (`detect`, `competitors`) and per-agent accounting (`agent_jsonl_usage`, `agent_sqlite_usage`, `cache_frame`, `cache_thrash`, `mcp_stats`). No outgoing edge, TESTS INCLUDED. It needs nothing from the formats: a transcript reader reads JSONL and SQLite. External dependencies: `serde_json`, `rusqlite`, `toml`, `flate2`, `postcard`, `rayon`, `fs2`. |
| `tokenade-core` | 1 281 | 190 100 | Everything else, plus the FACADE: it re-exports the 128 modules of the four crates under their historical paths, so no downstream import has to change. |
| `tokenade-cli` | 257 | 107 361 | The binary, the hooks, the installer, the dashboard. |
| `tokenade-bench` | 82 | 21 806 | The measurement harness. It is what puts a number on every decision in this document. |

### `tokenade-core` — 1 281 files, 190 100 lines

The library. Everything except the binary and the hook shells lives here, so
that another surface (web UI, HTTP API, Python binding) never has to touch the
logic.

Modules are declared FLAT in `lib.rs`, but they carry an order, and that order
is enforced by a test —
`crates/tokenade-core/tests/guards/layers_only_point_downwards.rs`. Seven
layers, lowest to highest; a layer only names what is below it, and every edge
that still points upwards is listed one by one with its number of call sites:

| Layer | What it holds | Heavy dependencies |
|---|---|---|
| **base** (33, crate `tk-base`) | `platform`, `paths`, `tokenizer`, `regexes`, `argv`, `ansi`, `url`, `debug_log`, `process_util`, `redact`, `env_compat`, `lru`, `cache`, `text_encoding`, `srcsweep`, `source_guard`, `runtime`, `stores`, `session`, `signing`, `testtmp`, `edit_distance`, `gitignore`, `prompt`, `progress`… | none |
| **fold** (27, crate `tk-fold`) | `archetypes`, `fold_gate`, `fold_count`, `fold_store`, `elision`, `coefficients`, `dedup_ref`, `template_fold`, `phrase_fold`, `compress`, `table_fold`, `agent_caps`, `caps`, `adaptive`, `read_mode`, `read_view`, `richness`, `fold_marker`, `prose`, `textshape`, `superseded`, `image_downscale`, `glob_filter`, `savings` | `image`, `rusqlite` |
| **formats** (82, 41 of them in crate `tk-formats`) | one module per format: `json`, `csv`, `yaml`, `markdown`, `logs`, `diff`, `xml`, `ini`, `archive`, `lockfile`, `tabular`, `stacktrace`, `code_strip`… **+ `auto`, their dispatcher; `cmd/**` (987 files), the per-command compactors; `disclose`, the fold gate; `docs`, the document formats; `treesitter`, the parser** + the whole HTML family — `web` (`html_to_markdown`, `compact_a11y_tree`), `web_tree`, `html_nesting`, `tree_dedup`, `dom_dedup`, `stealth`, `edge_error`, `site_readers`, `serp` — plus `post_passes`, `speaks_for`, `transcript_view`, `session_compact`, `tsc`, `command_chain`, `filelist` | `zip`, `pdf`, `scraper` |
| **commands** (8) | DISPATCH only: `filter`, `command_rewrite`, `user_filters`, `availability`, `corpus_key`, `presets`, `sandbox`, `mcp_compact`. The compactors themselves sit one floor lower, with the formats — `cmd → auto` has 29 call sites against 2 the other way, and two modules that call each other both ways belong on the same floor. | none |
| **web** (9) | The ROUTE and nothing else: `net` (the HTTP client), `web_fetch`, `web_pipeline`, `web_replay`, `js_render`, `search`, `web_map`, `web_impersonate`, `text_hygiene`. **HTML is a format** — the converter and its family sit one floor lower, since the three route primitives once parked there (`scheme_host` and its four neighbours, `normalized_content_type`) were moved back to where they belong. | `rquickjs` |
| **code** (17) | `index`, `index_db`, `semantic`, `semantic_db`, `skeleton`, `embedding`, `static_embed`, `model_fetch`, `rag`, `file_search`, `ranking`, `reranker`, `framework`, `centrality` | 16 tree-sitter grammars, `rusqlite`, `model2vec` |
| **product** (29 of them in crate `tk-agents`) | `gain`, `dashboard`, `updater`, `detect`, `project_scaffold`, `mcp_wrap`, `pricing`, `model_costs`, `agent_*` (14), `agent_transcripts`, `claudecode_transcript`, `transcript_cache`, `usage_dedup`, `receipt`, `pack`, `judge`, `media` | — |

The `UNASSIGNED` list of the same test — the escape hatch for a module whose
place was unclear — has been **empty since 2026-09-13**. It used to hold 85
modules: the whole `agent_*` family, `claudecode_transcript`,
`command_rewrite`, `caps`, `framework`, `adaptive`, `search`, `net`. Each was
placed on the layer its REAL edges allow, computed on the production half and
not guessed from the name; **82 of the 85 went in without creating a single
upward edge**, which says the mess was paperwork, not design.

So a new module now has to be placed for the guard suite to compile. That was
the point.

**What remains, as of 2026-09-13: ONE edge, ONE site.** This work found
**122, over 246 sites**.

| Edge | Sites | Why it still stands |
|---|---|---|
| `disclose → gain` | 1 | `record_recovery` writes to the ledger at the exact moment tokens stop being delivered, and the comment at the site says why it lives there and not in the eight callers: it is the only choke point they all go through. Paying it off would take THREE splits in the product layer — the ledger writer names `detect`, which names `mcp_wrap` — for one accounting call: the "pay 1 site, create 3" shape this document refuses elsewhere. Inverting it through an installed sink was weighed too, and refused: the default would be silent, and `crates/tokenade-core/tests/fold/recovery_counters.rs` requires the row from the LIBRARY, not from the binary. |

Why this order is written down rather than implied: the 212 modules used to
form ONE strongly connected component of 136 — `platform`, `debug_log` and
`paths` inside it, together with `gain`, `web` and `cmd`. As long as it held,
no crate could be extracted: the compiler refuses a cycle between crates.
**It no longer holds**: `tk-base`, `tk-fold`, `tk-formats` and `tk-agents` came
out on 2026-09-13, and the guard is what made it possible.

**Declared reserves** — written, tested, called by nobody, and kept on
purpose: `cachekey` (prompt-cache hints), `query_parser` (qualified filters for
`semantic_search`), `nudge` (nudge centralisation, whose 25 real sites still
live in `tokenade-cli`), `error` (the unified `TokenadeError` from the backlog
below). Each says so in its own header. A module that looks wired costs real
work to whoever believes it — see the note on `cache_align` and `path_safety`
below.

### There is no `tokenade-mcp` crate

This section used to describe one: a JSON-RPC 2.0 MCP **server** over stdio with
62 tools. It is gone. tokenade reaches agents through **hooks and the CLI**, and
its only MCP surface is `tokenade mcp-proxy`, which wraps *someone else's* MCP
server and folds the results on the way back.

Kept as a heading rather than deleted silently, because this doc's claim outlived
the code and cost real work: two modules — `cache_align` and `path_safety` —
were written to protect "tokenade's own MCP instructions string" and "user-input
paths reaching MCP handlers", surfaces that no longer existed. Both sat wired to
nothing until 2026-08-04, each reading like a protection that was not there. The
map showed a building that had been demolished, so people kept maintaining doors
for it.

### `tokenade-cli` — 257 files, 107 361 lines

The binary. The split announced here as "pending" for months never happened:
the figure grew eighteen-fold in the meantime. `main.rs` is 3 223 lines and
declares 31 modules; `src/tests/` holds 15 378 lines of tests compiled into
the binary.

The `foo.rs` + `foo/` layout (`code_intel`, `compactors`, `install`, `wrap`,
`healthcheck`, `dashboard_cli`) is NOT a duplicate: it is the idiomatic form
since the 2018 edition, and this workspace is on 2021 —
`crates/tokenade-cli/src/code_intel.rs` declares `mod args;`, which resolves to
`crates/tokenade-cli/src/code_intel/args.rs`.

Both conventions do coexist, the other way round: **35 directories keep the
2015-edition `mod.rs`** (27 in `tokenade-core`, 7 in the CLI, 1 in the bench)
against 20 in 2018 style. Unifying is not free: `cmd/linux/` has its `pub mod`
lines generated by `build.rs` from the directory contents, and renaming
`crates/tokenade-core/src/cmd/linux/cargo/mod.rs` to `cargo.rs` adds a file
that generator counts as one more module. Measured: the 35 renames break the
build.

The user-facing binary. Subcommands include `proxy`, `filter`, `index`,
`query`, `impact`, `map`, `skeleton`, `web`, `snapshot`, `auto`, `csv`,
`json`, `notebook`, `stealth`, `stack`, `yaml`, `curl`, `logs`, `diff`,
`archive`, `terraform`, `table`, `env`, `md`, `lock`, `sql`, `metrics`,
`access`, `journal`, `sourcemap`, `gain`,
`dashboard`, `statusline`, `measure`, `execute`, `detect`,
`detect-competitors`, `discover`, `healthcheck`, `install`,
`hookwrap`, `hookwrap-pretool`, `mcp`.

The `install` subcommand alone handles 8+ targets (Claude Code,
Cursor, Codex, Hermes, …) and 10+ flags (`--with-hook`,
`--with-claude-md`, `--with-brevity-skill`, `--with-lsp-enforcement-hook`,
`--profile`, `--only`, `--per-project`, `--no-cleanup`, `--no-debug-log`,
`--dry-run`, `-y`).

`hookwrap` is the Claude Code Bash-PostToolUse entry: it reads the
hook input on stdin, identifies the command, applies the matching
filter via `tokenade-core::filter`, prints the compacted output to
stdout, and writes a `gain.jsonl` row.

### `tokenade-bench` — 82 fichiers, 21 806 lignes

Benchmark harness — clones pinned competitor repos, runs scenarios,
emits a savings report. Not shipped in the install path.

## Persistence layout

```
~/.tokenade/
├── gain.jsonl         Append-only savings log (rotated at 10 MiB)
├── debug.log          JSONL debug log (rotated)
└── settings.json      User config

<project>/.tokenade/
├── index.json         Symbol + import + call graph (codebase)
├── semantic.bin       Persisted BM25 index + per-doc embeddings
└── cache/             Per-process LRU cache + compactor output cache
```

## Data flow — a bash command in Claude Code

1. Agent runs `cargo build` via the Bash tool.
2. Bash tool returns 5 000 lines of `Compiling foo`, errors, warnings.
3. Claude Code's PostToolUse hook fires `tokenade hookwrap`.
4. `cmd_hookwrap` reads the JSON event from stdin, parses out the
   command + stdout/stderr.
5. `filter::filter_command(["cargo", "build"], output)` dispatches to
   the `cargo_build` handler in `filter.rs`.
6. `cargo_build` keeps every error/warning *in full*, samples 10
   `Compiling` lines, drops the rest, prints `Finished`.
7. Compacted output goes to stdout (Claude Code shows it to the
   agent); a banner `[tokenade:cargo-build −93% tokens]` goes to
   stderr (visible to the user).
8. `gain_log::record()` appends `{ts, source: "hook", op: "cargo-build",
   before: 12340, after: 870, project: <cwd>}` to `~/.tokenade/gain.jsonl`.
9. `tokenade dashboard` later aggregates this into per-project +
   per-tool charts.

## Data flow — `tokenade query handleAuth`

1. The agent runs it as a shell command (the rules we scaffold teach `query` /
   `skeleton` / `impact` / `semantic` for exactly this).
2. `tokenade-cli` dispatches to the `query` arm.
3. `Index::ensure_fresh(root)` either loads the cached index from `index.db` or
   rebuilds incrementally via mtime diff.
4. `idx.find(query, limit=20)` returns matching `Symbol`s ranked by exact name +
   prefix + kind weight.
5. The result is text-formatted for the terminal; `gain::record` appends the
   before/after estimate to the ledger.

Note what is NOT in this list: no JSON-RPC frame, no `_meta` block, no MCP
server. This section described one until 2026-08-04, three months after that
surface was removed.

## Detector pipeline (`auto::detect`)

When `filter_command` doesn't have a hand-tuned handler for the
command (most cases), it falls through to `auto::compact`, which:

1. **Detects the content kind** via `auto::detect()` — content-sniff
   ordering matters: HTML before JSON, openapi before generic JSON,
   ndjson before json, lockfile before yaml, etc.
2. **Dispatches** to the matching compactor via `auto::compact()`.
3. **Falls back** to a `generic` ANSI-strip + dedup pass when no kind
   is detected or the kind-specific compactor doesn't shrink the
   input.

The detector ordering is **load-bearing** — there are explicit
comments documenting why each carve-out is where it is. Treat
re-ordering as a breaking change.

## Memory hybrid RRF recall

`recall_hybrid_rrf(query, n)` combines two ranked lists via
Reciprocal Rank Fusion (k=60):
- Lexical: top-n facts by ROI (`hits / age_days`)
- Semantic: top-n facts by cosine on sqlite-vec embeddings

Each candidate gets `score = sum(1 / (k + rank))` across both lists.
Top-n by score wins. When the embedder model isn't cached, falls
back gracefully to the lexical list.

## Tree-sitter dispatch

`treesitter::extract_full(source, ext)` is the AST entry point:

1. Look up `LangSpec` for `ext` in the static `LANGS` table (16
   languages registered).
2. Spawn a fresh `Parser` (native — no WASM worker pool needed; see
   the module docs for the rationale).
3. Parse with a 1–30 second timeout scaled by file size, via the
   modern progress-callback API.
4. DFS walks the AST emitting `(Symbol, Call)` per matching node
   kind. Symbols carry `decorators` + `bases` populated from the
   surrounding AST context.
5. `extract_file` in `index.rs` calls `extract_full` first; on
   unsupported extensions or parse timeout, falls back to the regex
   extractor (~22 additional languages).

## Adding a new format compactor

Today:
1. Create `crates/tokenade-core/src/yourformat.rs` with
   `pub fn detect(text: &str) -> bool` + `pub fn compact(text: &str)
   -> String`.
2. Register the module in `lib.rs` (`pub mod yourformat;`).
3. Add a variant to `auto::Kind` and dispatch in `auto::detect` +
   `auto::compact`.
4. Add a CLI subcommand to `tokenade-cli/src/main.rs` (one dispatch
   line + one `cmd_yourformat()` function).
5. Add tests: unit tests beside the module, and a fixture under
   `crates/tokenade-core/tests/fixtures/` so the corpus properties
   (idempotence, determinism, error-line survival) cover it too.

Step 4 used to be two steps — one for an MCP tool, one for the CLI — and the
"triplicate cost (auto / mcp / cli)" was on the backlog. Removing the MCP server
removed that third copy.

## Where to look for X

| Need | File |
|---|---|
| New format compactor pattern | `markdown.rs`, `json.rs`, `terraform.rs` |
| How dispatch picks a compactor | `auto.rs::detect()` |
| Walker / file traversal | `index.rs::Index::build()` |
| MCP proxy (wrapping someone else's server) | `tokenade-cli/src/mcp_proxy.rs` |
| Bash hook entry | `tokenade-cli/src/main.rs::cmd_hookwrap()` |
| Install logic | `tokenade-cli/src/main.rs::cmd_install()` |
| Dashboard rendering | `tokenade-core/src/dashboard.rs::render()` |
| Tree-sitter language registry | `treesitter.rs::LANGS` |

## Conventions

- **Errors**: today we mostly return `Option`/`Result<_, io::Error>`
  and swallow at boundaries. A unified `TokenadeError` is on the
  refactor backlog — and it is already WRITTEN, in
  `crates/tokenade-core/src/error.rs`, with zero adopters.
  It is not work to redo but a migration to make; the module says so
  in its own header.
- **Tests**: unit tests in `#[cfg(test)] mod tests` inside each
  module. Snapshot tests via `insta` under `tests/`. Property tests
  via `proptest` under `tests/`. Two end-to-end integration tests
  (more on the backlog).
- **Logging**: `debug_log::log("component", "event", json!({...}))`
  gates on `TOKENADE_DEBUG_LOG=1`. Always silent in default builds.
- **Savings ledger**: `gain_log::record(source, op, label, savings)` is
  always on and stays on the machine (`tokenade gain` reads it).
- **Documentation**: doc-comments explain *why*, not what. The "what"
  should be obvious from the name; the "why" needs preservation.
