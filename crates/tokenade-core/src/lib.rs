//! tokenade-core — the token-optimization mechanisms behind Tokenade.
//!
//! Each module is one orthogonal layer of the stack identified in the
//! competitor analysis:
//!
//! - [`tokenizer`] — fast, dependency-free token estimation (the measuring stick).
//! - [`filter`]    — output filtering/compression engine (rtk-style), the proven
//!   60–90% lever at the shell/tool boundary.
//! - [`index`]     — lightweight multi-language symbol & import-graph index so an
//!   agent queries structure instead of reading whole files.
//! - [`compress`]  — context compression: structure maps, dedup, smart truncation.
//! - [`memory`]    — persistent, ROI-ranked cross-session memory.
//! - [`cache`]     — content-addressed on-disk cache.
//! - [`detect`]    — auto-detect which AI coding agent is running.
//!
//! Everything is pure Rust over `std` + `serde` + `regex`. No async runtime,
//! no vector DB, no heavy ML — by design (see decision #1 in the comparison).

pub mod accesslog;

// ─────────────────────────────────────────────────────────────────────
// The BASE layer lives in its own crate
// ─────────────────────────────────────────────────────────────────────
//
// `tk-base` carries the 32 modules NONE of which names anything outside
// them — the rule is checked by `layers_only_point_downwards`, test code
// included. They are re-exported here under their original path: for the
// 149 files of `tokenade-cli`, the 8 test targets and the bench,
// `tokenade_core::platform::…` answers exactly as before.
//
// A re-export, not a wrapper: zero forwarding functions, zero runtime hop,
// a single name for each thing.
pub use tk_base::{
    ansi, argv, cache, cache_align, cachekey, crash_guard, debug_log, edit_distance, env_compat,
    gitignore, leaders, line_diff, lru, paths, platform, process_util, profile, progress, prompt,
    redact, regexes, runtime, session, signing, source_guard, sqlite_tuning, sqlite_vec_util,
    srcsweep, stores, testtmp, text_distance, text_encoding, tokenizer, url,
};

// The fold primitives — capping, elision, head/tail, decorative padding,
// savings accounting. They lived under `cmd/`, where 12 FORMAT modules had
// to climb up into the commands floor to reach `capped_section` or
// `savings_of`: 32 call sites that made a command helper everybody's
// dependency. The job was never the commands'.
pub mod archive;
pub mod auto;
pub mod autocompact;
pub mod availability;
pub mod bazel;
pub mod claude_md_audit;
pub mod cmake;
pub mod cmd;
pub mod code_intel_heuristic;
pub mod code_strip;
pub mod command_chain;
pub mod command_rewrite;
pub mod dashboard;
pub mod diff;
pub mod disclose;
pub mod disk;
pub mod dns;
pub mod dockerfile;
pub mod docs;
pub mod dom_dedup;
pub mod embedding;
pub mod error;
pub mod media;
pub mod pdf_layout;
pub mod site_readers;
pub mod static_embed;
pub mod text_hygiene;
pub use error::{io_err, TokenadeError, TokenadeResult};
pub mod centrality;
pub mod file_search;
pub mod filelist;
pub mod filter;
pub mod framework;
pub mod gain;
pub mod gitlog;
pub mod hcl;
pub mod index;
pub mod index_db;
pub mod ini;
pub mod install_state;
pub mod journal;
pub mod js_render;
pub mod judge;
pub mod line_patch;
pub mod mcp_compact;
pub mod mistakes;
pub mod model_costs;
pub mod model_fetch;
pub mod net;
pub mod netstat;
pub mod pkginstall;
pub mod presets;
pub mod pricing;
pub mod project_scaffold;
pub mod proto;
pub mod query_parser;
pub mod rag;
pub mod ranking;
pub mod rate_history;
pub mod recalc;
pub mod receipt;
pub mod reranker;
pub mod sandbox;
pub mod search;
pub mod searchout;
pub mod semantic;
pub mod semantic_db;
pub mod serp;
pub mod session_compact;
pub mod skeleton;
pub mod sql_source;
// No longer behind a flag: `source_guard` — the seam EVERY source guard goes
// through — delegates "which half of this file ships?" to it. A second
// copy of that question cost 1 521 151 bytes of invisible production code.
pub mod corpus_key;
pub mod stacktrace;
pub mod stealth;
pub mod strace;
pub mod tcpdump;
pub mod toml_compact;
pub mod tree_dedup;
pub mod treesitter;
pub mod tsc;
pub mod updater;
pub mod user_filters;
pub mod vmstat;
pub mod web;
pub mod web_credit;
pub mod web_fetch;
pub mod web_impersonate;
pub mod web_map;
pub mod web_pipeline;
pub mod web_replay;
pub mod web_tree;
pub mod xml;

/// Library version, surfaced by `tokenade --version` and the MCP `serverInfo`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// The FOLD layer has been a crate since 2026-09-13: `tk-fold`. Its 26 modules
// keep their name and path here, through re-export — the 149 files of
// `tokenade-cli`, the 8 test targets and `tokenade-bench` do not change a
// single import. `Savings` moves down with them: `with_agent_delivery` queries
// `agent_caps`, which is a table of that layer.
pub use tk_fold::{
    adaptive, agent_caps, archetypes, caps, coefficients, compress, dedup_ref, elision, fold_count,
    fold_gate, fold_marker, fold_store, glob_filter, image_downscale, line_gutter, phrase_fold,
    prose, read_mode, read_view, richness, store_scrub, superseded, table_fold, template_fold,
    textshape, turn_summary,
};

pub use tk_fold::Savings;

// The FORMATS layer has been a crate since 2026-09-13: `tk-formats`. Its
// 41 modules keep their name and path here, through re-export — nothing that
// names them changes an import. The criterion of the cut is not the subject
// but the DEPENDENCY: seven format modules carry a heavy one (`lopdf`,
// `tree-sitter`, `scraper`, `reqwest`, `zip`) and stay here until `tk-web`
// and `tk-code` exist.
pub use tk_formats::{
    ansible, base64_blob, bibtex, cargobench, containerlogs, csv, edge_error, email, env, fences,
    ghactions, graphql, har, html_nesting, icalendar, json, junit, jvmbuild, k8smanifest, kubectl,
    latex, lockfile, logs, makefile, markdown, metrics, minified, ndjson, notebook, openapi, pem,
    post_passes, source_signals, sourcemap, speaks_for, sql, ssh_keys, systemd, table_profile,
    tabular, terraform, transcript_view, value_shapes, vcard, yaml,
};
// The AGENTS layer has been a crate since 2026-09-13: `tk-agents`. It asks
// nothing of the formats — a transcript reader reads JSONL and SQLite — so
// it relies on `tk-base` and `tk-fold` alone.
pub use tk_agents::{
    agent_antigravity, agent_cline, agent_codex, agent_copilot, agent_gemini, agent_grok,
    agent_hermes, agent_jsonl_usage, agent_kilo, agent_openclaw, agent_opencode, agent_pi,
    agent_qwen, agent_sqlite_usage, agent_transcripts, cache_frame, cache_thrash,
    claudecode_transcript, competitors, detect, mcp_stats, mcp_wrap, nudge, pack, review_context,
    style, transcript_cache, usage_dedup, webdash,
};
