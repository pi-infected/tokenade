//! A layer only names what is BELOW it.
//!
//! `tokenade-core` carries 210 modules declared flat in `lib.rs`, all `pub`,
//! hence all reachable from anywhere. Nothing ever stopped a primitive from
//! calling a product layer, and the result is measurable: the 210 modules form
//! ONE strongly connected component of 136 — `platform`, `debug_log` and
//! `paths` are in it, with `gain`, `license`, `web` and `cmd`. As long as it
//! holds, no crate can be extracted: the compiler refuses a cycle between
//! crates.
//!
//! This test is the boundary the language does not give yet. It reads the
//! `crate::<module>` of every file, projects them onto the table below, and
//! refuses any edge that goes UP — except those already there, written one by
//! one in `DEBT` with their number of sites. The debt therefore cannot grow in
//! silence, nor stay written after being paid.
//!
//! Why a DECLARED debt rather than a test that fails right away: 136 sites are
//! not fixed in one commit, and a red test left red guards nothing any more.
//! Counted, it guards the rest while it is being paid.
//!
//! MEASURED, counting only CODE: 69 edges / 116 sites. Counting comments too,
//! it was 122 / 246 before `archetypes` and the table fold left `cmd/` — a
//! third of that gap was prose.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The layers, from lowest to highest. The ORDER is the rule.
pub(crate) const LAYERS: &[(&str, &[&str])] = &[
    // the business-free primitives: paths, platform, token counting, log
    (
        "base",
        &[
            "ansi",
            "argv",
            "session",
            "signing",
            "redact",
            "platform",
            "paths",
            "tokenizer",
            "delivery",
            "leaders",
            "regexes",
            "env_compat",
            "debug_log",
            "process_util",
            "text_encoding",
            "oem",
            "lru",
            "cache",
            "testtmp",
            "crash_guard",
            "cache_align",
            "cachekey",
            "edit_distance",
            "gitignore",
            "line_diff",
            "line_patch",
            "profile",
            "progress",
            "prompt",
            "runtime",
            "source_guard",
            "sqlite_tuning",
            "sqlite_vec_util",
            "srcsweep",
            "stores",
            "text_distance",
            // `url`: reading an address is STRING parsing. The five functions
            // lived in `net`, the HTTP client, and none opens a connection —
            // that is what forced rendering an RSS feed to name the web layer
            // for a single word, `scheme_host`.
            "url",
        ],
    ),
    // the FOLD itself: gate, budget, elision, truncation, redaction, capability tables
    (
        "fold",
        &[
            "archetypes",
            "elision",
            "fold_gate",
            "coefficients",
            "dedup_ref",
            "template_fold",
            "phrase_fold",
            "compress",
            "fold_count",
            "fold_store",
            "textshape",
            "agent_caps",
            "table_fold",
            "adaptive",
            "caps",
            "fold_marker",
            "glob_filter",
            "image_downscale",
            "line_gutter",
            "prose",
            "read_mode",
            "read_view",
            "richness",
            "store_scrub",
            "superseded",
            "turn_summary",
            // `savings`: the before / after measurement. It lived at the root of
            // `tokenade-core`; `with_agent_delivery` queries `agent_caps`, which is
            // a table of this layer — so it is here, and not lower.
            "savings",
        ],
    ),
    // one module per data format; each knows its shape and nothing else
    (
        "formats",
        &[
            "code_strip",
            "json",
            "csv",
            "yaml",
            "markdown",
            "logs",
            "diff",
            "xml",
            "ini",
            "archive",
            "lockfile",
            "tabular",
            "stacktrace",
            "toml_compact",
            "hcl",
            "dockerfile",
            "makefile",
            "junit",
            "latex",
            "proto",
            "sql",
            "pem",
            "minified",
            "ndjson",
            "openapi",
            "graphql",
            "sourcemap",
            "metrics",
            "accesslog",
            "journal",
            "systemd",
            "strace",
            "tcpdump",
            "vmstat",
            "netstat",
            "disk",
            "filelist",
            "searchout",
            "containerlogs",
            "cargobench",
            "jvmbuild",
            "ghactions",
            "ansible",
            "k8smanifest",
            "kubectl",
            "gitlog",
            "dns",
            "terraform",
            "bazel",
            "cmake",
            "bibtex",
            "pkginstall",
            "notebook",
            "har",
            "email",
            "icalendar",
            "vcard",
            "pdf_text",
            "head_tail",
            "env",
            "sql_source",
            "value_shapes",
            "table_profile",
            "base64_blob",
            "fences",
            "pdf_layout",
            "source_signals",
            "ssh_keys",
            "treesitter",
            "docs",
            // `auto` closes the formats layer instead of opening the commands
            // one. Thirteen format modules asked it to fold what they had not
            // recognised, going UP one level; they no longer do. MEASURED: 13
            // edges / 13 sites paid against 4 created (`auto → cmd` 2,
            // `auto → filter` 2, plus `command_chain` and `tsc` at 1 each),
            // i.e. 42/51 → 33/42. The four created are the real remainder of
            // the subject: the formats router still asks the commands router
            // for two things.
            "auto",
            // `disclose` closes the formats layer too. The fold gate does not
            // merely STORE bytes: it names them (`auto::detect` → an
            // extension), it renders a summary of them by shape
            // (`json::schema_fold`, `markdown::outline`) and it asks `docs`
            // whether a binary is extractable. It is a FORMATS module, and
            // making it live one level lower cost 5 upward edges throughout
            // this work. DECIDED on 2026-09-13: what kept it there was two fold
            // modules calling it — `web_replay`, which only `web_pipeline`
            // calls (it moves up to `web`), and `session_compact`, which nobody
            // in the core calls (it moves up here). The store was not split in
            // two: it NAMES what it stores, so it cannot go down.
            "disclose",
            // Folding a session is format work: the transcript is one. No core
            // module calls it — only the CLI — so its place is constrained only
            // by what it names (`tokenizer`, `superseded`, `disclose`).
            "session_compact",
            // `command_chain`: the output of a CHAIN of commands. Its name says
            // "commands", its job says otherwise — it is detected in TEXT, like
            // every format, and it only names `archetypes` and `auto`. So it
            // closes the formats instead of opening the commands.
            "command_chain",
            // `post_passes`: the passes that apply AFTER routing. Its own header
            // said so — "none chooses a route; all rework the TEXT" — and it
            // only names `ssh_keys`, `pem`, `logs` and `compress`. It lived
            // under `filter/` because `filter` is its only caller, which is not
            // a reason for a layer.
            "post_passes",
            // `speaks_for`: "does this summary still say what the input said?".
            // A fidelity floor, not a route — see the module header.
            "speaks_for",
            // `tsc`: the TypeScript compiler's output. A diagnostic SHAPE,
            // recognised in text like any format — and its only caller is
            // `auto`. It could not go down as long as it named
            // `cmd::linux::git_listing::path_census`; that primitive has lived
            // in `filelist` since this commit, and the path is clear.
            "tsc",
            // `cmd` closes the formats layer, and it is the graph that says
            // so, not a preference. A "per command" compactor is a SHAPE
            // compactor: `git status` has a shape, `cargo build` another, and
            // the command name is merely how that shape got its name. The two
            // gates already say it to each other — `cmd → auto` counts
            // **29 sites** and `auto → cmd` 2. Two modules that call each other
            // in BOTH directions are of the same layer; keeping them apart only
            // decided which of the two calls would be declared as debt.
            //
            // MEASURED: 2 edges paid, 0 created. `cmd` names NO module of the
            // commands layer — not `filter`, not `presets`, not `sandbox`. What
            // remains above is ROUTING alone, what the plan called phase 4:
            // the knot comes undone by declaring, not by moving.
            "cmd",
            // `transcript_view`: rendering a session transcript as a readable
            // view. Taken out of `claudecode_transcript`, which did TWO jobs in
            // 1,450 lines — reading usage (product, 22 sites) and folding
            // (format, 2 sites). See the module header.
            "transcript_view",
            // **HTML is a FORMAT.** The converter and its family close the
            // formats layer: `web` (whose header says "html_to_markdown" and
            // "compact_a11y_tree"), `web_tree`, `html_nesting`, `tree_dedup`,
            // `dom_dedup`, plus `stealth` and `edge_error` — which READ a page
            // to say it is not the page — and `site_readers`, which renders a
            // feed or a thread from its markup.
            //
            // They could not go down as long as three ROUTE primitives were
            // parked with them: `net::scheme_host` and its four neighbours
            // (string parsing, never a request — moved to `tk_base::url`) and
            // `web_fetch::normalized_content_type` (a `Content-Type` reduced
            // to its bare type — moved to `text_encoding`). Once those three
            // were returned to their subject, the whole family had not a single
            // edge left towards the layer above.
            //
            // What remains in `web` is the ROUTE: `net` (the HTTP client),
            // `web_fetch`, `web_pipeline`, `web_replay`, `js_render`, `serp`,
            // `search`, `web_map`, `web_impersonate`, `text_hygiene`.
            "web",
            "web_tree",
            "html_nesting",
            "tree_dedup",
            "dom_dedup",
            "stealth",
            "edge_error",
            "site_readers",
            // `serp`: a RESULTS page is a shape, not a route. It is recognised
            // in markup, like any format, and the module only names `web`,
            // `stealth`, `caps`, `regexes` and `text_encoding` — all here or
            // lower since the HTML converter moved down.
            "serp",
        ],
    ),
    // the per-command compactors and their routing
    // `auto` is NOT here. DECIDED on 2026-09-13, measured: it is the router
    // OF THE FORMATS, and its place is with them.
    (
        "commands",
        &[
            "filter",
            "availability",
            "command_rewrite",
            "corpus_key",
            "mcp_compact",
            "presets",
            "sandbox",
            "user_filters",
        ],
    ),
    // the web route: fetching, tree, pruning, site readers
    (
        "web",
        &[
            "web_fetch",
            "js_render",
            "web_impersonate",
            "web_map",
            "net",
            "search",
            "text_hygiene",
            "web_pipeline",
            // `web_replay`: replaying a web response. A single caller,
            // `web_pipeline`, and it is here. It lived with the fold, which
            // forced `disclose` to stay there with it.
            "web_replay",
        ],
    ),
    // code intelligence: index, symbols, embeddings, search
    (
        "code",
        &[
            "index",
            "index_db",
            "semantic",
            "semantic_db",
            "embedding",
            "static_embed",
            "model_fetch",
            "rag",
            "file_search",
            "centrality",
            "code_intel_heuristic",
            "query_parser",
            "error",
            "framework",
            "ranking",
            "reranker",
            // `skeleton`: rendering a file by its STRUCTURE. It asks
            // `treesitter` for its syntax tree and `index` for its declaration
            // patterns; that is the code layer, not the fold one, and it is in
            // fact what made `compress` go up three levels.
            "skeleton",
        ],
    ),
    // what the user sees: ledger, dashboard, installation
    (
        "product",
        &[
            "gain",
            "web_credit",
            "install_state",
            "dashboard",
            "updater",
            "detect",
            "project_scaffold",
            "mcp_wrap",
            "recalc",
            "pricing",
            "model_costs",
            "competitors",
            "webdash",
            "agent_antigravity",
            "agent_cline",
            "agent_codex",
            "agent_copilot",
            "agent_gemini",
            "agent_grok",
            "agent_hermes",
            "agent_jsonl_usage",
            "agent_kilo",
            "agent_openclaw",
            "agent_opencode",
            "agent_pi",
            "agent_qwen",
            "agent_sqlite_usage",
            "agent_transcripts",
            "autocompact",
            "cache_frame",
            "cache_thrash",
            "claude_md_audit",
            "claudecode_transcript",
            "judge",
            "mcp_stats",
            "media",
            "mistakes",
            "nudge",
            "pack",
            "rate_history",
            "receipt",
            "review_context",
            "style",
            "transcript_cache",
            "usage_dedup",
        ],
    ),
];

/// The edges that still go up, with the measured number of call sites.
/// An edge absent from here that goes up fails the test; an edge from here
/// that has MORE sites than announced too; an edge from here that no longer
/// exists must be removed. It is a ratchet, not a list of exceptions.
/// **What remains, and why — 15 edges / 20 sites against 122 / 246 at the
/// start.** These fifteen are no longer misplaced primitives: those have all
/// been returned to their layer. They are FEATURE couplings, and each carries
/// here the reason it did not give way, so the next reader does not replay
/// the analysis.
///
/// Three families:
///
/// 1. **`auto → …` (11 sites).** The formats router still asks the commands
///    router for two things (`cmd`, `filter`) and the web route for four
///    (`serp`, `stealth`, `web`, plus `claudecode_transcript`). These edges
///    were drowned among 24 before `auto` moved down to the formats; isolated,
///    they finally read as the short question they are.
/// 2. **`disclose → …` (6 sites).** A stash's summary must CHOOSE a shape —
///    JSON schema fold, markdown outline, head+tail — and the fold gate is
///    the single choke point by construction (class C10, see `gate_closed`).
///    Spreading that choice across the eight callers is precisely the shape
///    this project has already paid for. `record_recovery` is the other
///    direction: the gate TELLS the ledger that a pointer was offered, at the
///    only place that knows it.
/// 3. **Two documents that arrive through the wrong door.**
///    `xml → site_readers` (an RSS feed read as XML) and
///    `mcp_compact → web` (an MCP tool result that is HTML). Moving `web`
///    under `commands` to pay the second was MEASURED: it pays 1 site and
///    creates 3 (`web_pipeline → filter`). Layer order is counted, not
///    preferred.
const DEBT: &[(&str, &str, usize)] = &[
    // 1 — the formats router towards layers it should not name
    // 2 — the fold gate counts what it has just not delivered
    //
    // The other four edges of `disclose` (`markdown`, `json`, `docs`,
    // `auto`) are paid since the gate is declared for what it is: a
    // FORMATS module. This one remains, which is of another nature —
    // `record_recovery` writes to the ledger, and the comment at the site
    // says why it is there and not in the eight callers: it is the only
    // choke point they all go through.
    ("disclose", "gain", 1),
    // 3 — the fold gate wanted SEMANTIC recovery — PAID on 2026-09-13, the
    // very day it became visible.
    //
    // It was not paid like the eighteen before: `rag` is not a primitive
    // parked with its first requester, it asks for `embedding` and it CALLS
    // BACK `disclose`. What went up was therefore not a module, it was a
    // DECISION — "which engine answers this question?" — and it belongs to
    // whoever knows both engines. `dispatch_scoped_one`,
    // `prompt_scoped_extract_dispatch` and `answer_over_blob` moved up to
    // `rag`; `disclose` keeps the flat extractor, the BM25 primitives and the
    // splitting of a prompt into questions, none of which has an opinion on
    // the engine.
];
/// The modules no layer claims yet. A new module lands here and fails the
/// test: the question "which layer?" is asked at writing time, not three
/// months later. The list must SHRINK.
///
/// **EMPTY since 2026-09-13.** It carried 85 modules — the whole `agent_*`
/// family, `claudecode_transcript`, `command_rewrite`, `caps`, `framework`,
/// `adaptive`, `search`, `net`… Each was placed at the layer its REAL edges
/// allow, computed on the production half, not guessed from the name: 82 of
/// the 85 went in without creating a single upward edge. The three that cost
/// something are written in `DEBT` above.
///
/// A new module must now be placed. That is the point: this list was the
/// way out, and it is closed.
const UNASSIGNED: &[&str] = &[];

/// The source roots this guard walks.
///
/// There have been TWO since 2026-09-13: the base layer moved out into the
/// `tk-base` crate, and its modules are still modules there — they name each
/// other through `crate::`, and `tokenade-core` re-exports them under the same
/// path. For this test, the crate boundary changes nothing: what counts is
/// the LAYER, not the Cargo file. Reading only one of the roots would make
/// dozens of modules vanish from the map without a single line turning red —
/// and that is precisely what happened on the first try.
///
/// The LIST of roots is not written here: `source_guard::crate_srcs()`
/// derives it from the `crates/` directory. Two crates were extracted the same
/// day; a hand-written list would have missed the second, silently.
fn roots() -> Vec<PathBuf> {
    tokenade_core::source_guard::crate_srcs()
}

/// The root that CONTAINS this path, to deduce the module name from it.
fn root_of(p: &Path) -> PathBuf {
    roots()
        .into_iter()
        .find(|r| p.starts_with(r))
        .expect("a file outside the roots")
}

fn layer_of() -> BTreeMap<&'static str, usize> {
    let mut m = BTreeMap::new();
    for (i, (_, mods)) in LAYERS.iter().enumerate() {
        for x in *mods {
            assert!(m.insert(*x, i).is_none(), "`{x}` is in two layers");
        }
    }
    m
}

/// The modules `lib.rs` may declare: a `.rs` or a directory under `src/`. The
/// FILE SYSTEM is read rather than `lib.rs`, so the test sees an added module
/// even if its declaration is generated.
fn declared_modules() -> BTreeSet<String> {
    // A file SPLICED by `include!` is not a module: its tokens live in the
    // file that includes it. The tests taken out of the god files
    // (`redact_tests.rs`, `docs_tests.rs`, …) are mounted that way, on
    // purpose — a `mod` would have added a level and changed the meaning of
    // every `super::super::`.
    let mut included: BTreeSet<String> = BTreeSet::new();
    let mut all = Vec::new();
    for r in roots() {
        collect(&r, &mut all);
    }
    for f in &all {
        let Ok(t) = std::fs::read_to_string(f) else {
            continue;
        };
        let mut rest = t.as_str();
        while let Some(at) = rest.find("include!(\"") {
            rest = &rest[at + "include!(\"".len()..];
            if let Some(end) = rest.find('"') {
                if let Some(stem) = rest[..end].strip_suffix(".rs") {
                    included.insert(stem.to_string());
                }
            }
        }
    }
    let mut v = BTreeSet::new();
    for e in roots()
        .into_iter()
        .flat_map(|r| std::fs::read_dir(r).expect("src/ readable"))
        .flatten()
    {
        let p = e.path();
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            // A directory is a module only if it contains RUST. `src/js/`
            // carries three `.js` files mounted by `include_str!` from
            // `js_render.rs`: it was counted as a module with no referent,
            // hence as dead code, and it is alive.
            let has_rust = std::fs::read_dir(&p)
                .map(|it| {
                    it.flatten()
                        .any(|e| e.path().extension().is_some_and(|x| x == "rs"))
                })
                .unwrap_or(false);
            if has_rust {
                v.insert(name);
            }
        } else if let Some(stem) = p.file_stem() {
            let stem = stem.to_string_lossy().to_string();
            if p.extension().is_some_and(|x| x == "rs")
                && stem != "lib"
                && !included.contains(&stem)
            {
                v.insert(stem);
            }
        }
    }
    v
}

/// `(calling module, called module) -> number of sites`.
///
/// The calling module is the FIRST segment of the path under `src/`: all of
/// `cmd/linux/ip.rs` counts for `cmd`, since it is `cmd` that will become a
/// crate. Both import forms are read — `crate::x::y` and
/// `crate::{x, y::z}` — because the second escapes the first and a
/// measurement that misses it under-counts by half.
fn edges() -> BTreeMap<(String, String), usize> {
    let known = declared_modules();
    let mut out: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut files = Vec::new();
    for r in roots() {
        collect(&r, &mut files);
    }
    for f in files {
        let rel = f.strip_prefix(root_of(&f)).unwrap_or(&f).to_path_buf();
        let first = rel
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().to_string());
        let owner = match first {
            Some(s) if rel.components().count() > 1 => s,
            Some(s) => s.trim_end_matches(".rs").to_string(),
            None => continue,
        };
        let text = match std::fs::read_to_string(&f) {
            Ok(t) => t,
            Err(_) => continue,
        };
        // TEST code does not count. The six `pem -> filter` sites were all in
        // a `#[cfg(test)] mod wiring` checking that every command route folds
        // a PEM block — an end-to-end test is allowed to reach the router, and
        // after a split it would live with it. Counting them declared a
        // violation where there is only an assertion.
        //
        // Removing test blocks comes from `srcsweep`, the declarer of "the
        // half of a file that ships": it follows braces and raw strings
        // instead of believing tests are at the end — `auto.rs` carries ten
        // blocks, the first at line 1041 of 3397. Writing a second version of
        // it here would be the duplicate this work exists to remove.
        let production = tokenade_core::srcsweep::production_source(&text);
        let code = code_only(&production);
        // `super::` at depth zero of a FIRST-level module designates the
        // crate root: `super::docs::rows_to_markdown`, in `table_profile.rs`,
        // was exactly the `table_profile -> docs` edge — and this test only
        // read `crate::`, so it did not see it. It only appeared when the
        // compiler refused the crate boundary. A measurement that knows only
        // one spelling of the same path is a wrong measurement.
        //
        // Only at depth ZERO, and only in a file placed directly under
        // `src/`: lower, or in a `mod tests`, `super` designates the module
        // itself and the vast majority of the repo's `super::x` are that.
        let first_level = rel.components().count() == 1;
        for target in crate_refs(&code).into_iter().chain(if first_level {
            root_supers(&code)
        } else {
            Default::default()
        }) {
            if target != owner && known.contains(&target) {
                *out.entry((owner.clone(), target)).or_default() += 1;
            }
        }
    }
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// CODE only comes from `srcsweep`, the declarer.
///
/// It lived here, and its local version did not model the CHARACTER literal:
/// `'"'` — a line of `disclose/scoped.rs`, a tokenizer removing quotes —
/// opened a string that only closed at the next quote in the file. Everything
/// in between was dropped, including `crate::rag::prompt_scoped_extract_rag`
/// line 221: an edge from the formats layer to the code layer, invisible the
/// day `DEBT` declared only ONE. A scrubber that loses code fails silently
/// and in the reassuring direction.
///
/// With no scrubbing at all, the opposite happens: this test declared
/// `table_fold` as violating towards `cmd` for the sole reason that its
/// header TELLS of having lived under `cmd/linux/`.
use tokenade_core::srcsweep::code_only;

/// All `crate::<ident>` of the text, including those of a group
/// `crate::{a, b::c}`. A macro's `$crate::` counts the same: it is the same
/// path.
fn crate_refs(text: &str) -> Vec<String> {
    let mut v = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(at) = text[i..].find("crate::") {
        let start = i + at + "crate::".len();
        i = start;
        if bytes.get(start) == Some(&b'{') {
            // group: each member starts a path
            let end = text[start..]
                .find('}')
                .map(|e| start + e)
                .unwrap_or(text.len());
            for part in text[start + 1..end].split(',') {
                if let Some(id) = ident_at(part.trim_start()) {
                    v.push(id);
                }
            }
        } else if let Some(id) = ident_at(&text[start..]) {
            v.push(id);
        }
    }
    v
}

/// The `super::<ident>` written at depth ZERO — hence, in a first-level
/// module, the crate root.
fn root_supers(code: &str) -> Vec<String> {
    let b = code.as_bytes();
    let (mut i, mut depth) = (0usize, 0i32);
    let mut v = Vec::new();
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        if depth == 0 && code[i..].starts_with("super::") {
            if let Some(id) = ident_at(&code[i + "super::".len()..]) {
                v.push(id);
            }
        }
        i += 1;
    }
    v
}

fn ident_at(s: &str) -> Option<String> {
    let id: String = s
        .chars()
        .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
        .collect();
    (!id.is_empty()).then_some(id)
}

#[test]
fn every_module_is_either_placed_in_a_layer_or_declared_unplaced() {
    let placed = layer_of();
    let declared = declared_modules();
    let unassigned: BTreeSet<&str> = UNASSIGNED.iter().copied().collect();

    let homeless: Vec<&String> = declared
        .iter()
        .filter(|m| !placed.contains_key(m.as_str()) && !unassigned.contains(m.as_str()))
        .collect();
    assert!(
        homeless.is_empty(),
        "these modules exist under `src/` and no layer claims them. \
         Put them in `LAYERS`, or — if the place is not clear yet — \
         list them in `UNASSIGNED` knowing that the list must \
         shrink:\n  {:?}",
        homeless
    );

    let ghosts: Vec<&str> = LAYERS
        .iter()
        .flat_map(|(_, m)| m.iter().copied())
        .chain(UNASSIGNED.iter().copied())
        .filter(|m| !declared.contains(*m))
        .collect();
    assert!(
        ghosts.is_empty(),
        "these names are placed in a layer but no longer exist under `src/`: \
         a map describing a demolished building costs real work (see \
         `cache_align` and `path_safety` in `docs/ARCHITECTURE.md`).\n  {:?}",
        ghosts
    );
}

#[test]
fn no_edge_points_upwards_except_the_declared_debt() {
    let placed = layer_of();
    let edges = edges();
    let mut allowed: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for (a, b, n) in DEBT {
        assert!(
            allowed.insert((*a, *b), *n).is_none(),
            "`{a} -> {b}` is listed twice in DEBT"
        );
    }

    let mut fresh = Vec::new();
    let mut grown = Vec::new();
    let mut shrunk = Vec::new();
    let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    for ((from, to), sites) in &edges {
        let (Some(lf), Some(lt)) = (placed.get(from.as_str()), placed.get(to.as_str())) else {
            continue;
        };
        if lt <= lf {
            continue;
        }
        match allowed.get(&(from.as_str(), to.as_str())) {
            None => fresh.push(format!(
                "{from} ({}) -> {to} ({}): {sites} site(s)",
                LAYERS[*lf].0, LAYERS[*lt].0
            )),
            Some(max) => {
                seen.insert((from.as_str(), to.as_str()));
                if sites > max {
                    grown.push(format!(
                        "{from} -> {to}: {max} site(s) declared, {sites} found"
                    ));
                }
                // And the reverse direction. An edge that SHRINKS without the
                // number moving leaves slack: the ratchet then allows the
                // return of the sites just paid, silently. Measured on
                // 2026-09-13: `disclose -> gain` was paid from 4 to 1 by
                // moving `session_key`, and nothing turned red.
                if sites < max {
                    shrunk.push(format!(
                        "{from} -> {to}: {max} site(s) declared, {sites} found"
                    ));
                }
            }
        }
    }

    let paid: Vec<String> = allowed
        .keys()
        .filter(|k| !seen.contains(*k))
        .map(|(a, b)| format!("{a} -> {b}"))
        .collect();

    assert!(
        fresh.is_empty(),
        "a low layer has just named a high layer. That is what holds the \
         136-module cycle and what will make the split impossible. Move \
         down what the low layer needs, or PASS it to it as an \
         argument:\n  {}",
        fresh.join("\n  ")
    );
    assert!(
        grown.is_empty(),
        "a debt already listed has grown — the ratchet is there for that:\n  {}",
        grown.join("\n  ")
    );
    assert!(
        shrunk.is_empty(),
        "a debt already listed has SHRUNK without its number moving. The slack \
         left allows the silent return of what was just paid — \
         write the measured number:\n  {}",
        shrunk.join("\n  ")
    );
    assert!(
        paid.is_empty(),
        "these debts are paid: remove them from `DEBT`, otherwise the list \
         becomes decoration again:\n  {}",
        paid.join("\n  ")
    );
}

#[test]
fn the_debt_is_counted_and_shrinking() {
    // The TOTAL, so that a commit moving debt from one edge to another
    // without reducing it shows. 122 edges / 246 sites before `archetypes`.
    //
    // The ceiling has followed the debt every time it fell; it has been at
    // **1 / 1** since 2026-09-13. That day saw it go down to 1 in the
    // morning, back up to 2 in the afternoon when the test's scrubber stopped
    // being blind to a character literal and made `disclose -> rag` visible,
    // then back down to 1 in the evening when that edge was paid. A ceiling
    // that goes up must say WHAT came in, which means no new edge can be
    // added to this list any more without a human raising this number and
    // writing why. A ceiling left above the debt is not a ratchet, it is a
    // reserve of room.
    let edges: usize = DEBT.len();
    let sites: usize = DEBT.iter().map(|(_, _, n)| n).sum();
    assert!(
        edges <= 1 && sites <= 1,
        "the layer debt has grown: {edges} edges / {sites} sites, \
         ceiling 1 / 1"
    );
}
