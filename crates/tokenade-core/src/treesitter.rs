//! Tree-sitter based symbol extraction.
//!
//! Replaces the regex extractor in `index.rs` for the 17 languages we
//! ship grammars for. Why bother:
//!
//! * **No false positives in strings / comments** — the regex
//!   extractor matches `// fn foo` and `"fn foo"`; the AST does not.
//! * **Multi-line signatures** — `fn foo(\n    a: T,\n    b: U,\n) {`
//!   is one regex headache; tree-sitter parses it correctly.
//! * **Macro-generated symbols** — `lazy_static! { … }` produces a
//!   `function_item` node we can walk; regex can't see inside macros.
//!
//! The walker is intentionally **kind-driven, not query-based**. Each
//! [`LangSpec`] declares the node kinds that count as functions /
//! types / modules / imports for its language. We walk the tree once,
//! check `node.kind()` against the per-language sets, and pull the
//! identifier child. Adding a new language is one entry in [`LANGS`].
//!
//! ## When tree-sitter fails
//!
//! Grammars are robust to syntactically invalid input — they emit
//! `ERROR` nodes around the bad span and continue. So extraction
//! still succeeds on files that don't compile. If the parser
//! genuinely can't load the grammar (very rare), [`extract`] returns
//! `None` and the caller falls back to the regex extractor.

use std::ops::ControlFlow;
use tree_sitter::{Language, Node, ParseOptions, Parser};

/// Per-language extraction config. We pre-declare which node kinds
/// correspond to which symbol kinds — no queries, no .scm files. The
/// lists are derived from each grammar's official `node-types.json`,
/// trimmed to the kinds an "agent navigating code" actually cares
/// about (functions, types, methods, modules, imports). Skip
/// arrow_function / lambda / call_expression — too noisy.
struct LangSpec {
    name: &'static str,
    language: fn() -> Language,
    /// Node kinds that yield `fn` / `method` symbols.
    functions: &'static [&'static str],
    /// Node kinds that yield `type` / `class` symbols.
    types: &'static [&'static str],
    /// Node kinds that yield `module` / `namespace` symbols.
    modules: &'static [&'static str],
    /// Node kinds that yield top-level *data* symbols: constants,
    /// statics, struct/class fields, enum variants. Harvested ONLY at
    /// module/type scope (never inside a function body) so an agent can
    /// `tokenade query MAX_RETRIES` / `query StatusCode` and find the
    /// declaration, without flooding the index with function-local
    /// `let`/`var` bindings. The label is derived from the node kind via
    /// [`member_label`].
    members: &'static [&'static str],
    /// The `members` kinds that stay indexable INSIDE a function body.
    ///
    /// The scope gate on `members` exists to keep function-local `let`/`var`
    /// bindings out, and it was already letting nested `fn`s through — so the
    /// behaviour was not "locals are excluded", it was "locals are excluded
    /// unless they are functions". A rule that splits a scope by node kind is
    /// not a scope rule, so the exception is named here instead of being an
    /// accident of which branch a kind falls into.
    ///
    /// Rust only, and on the grammar rather than on taste: a `const`/`static`
    /// inside a function is a COMPILE-TIME binding that the grammar marks as
    /// such, where a Python function's `X = 1` is a runtime local rebindable
    /// on every call. Measured on this repo 2026-08-27: 379 function-local
    /// `const`/`static` in production code, 294 distinct names, and 22 of a
    /// 40-name sample had no row in the index at all — `MIN_INPUT`,
    /// `SUSPICIOUS_PCT`, `MAX_RESCUED`, `MAX_NAMES` among them.
    function_scoped_members: &'static [&'static str],
    /// Node kinds of the ASSIGNMENT that a language uses to declare an
    /// instance field, because the language has no field DECLARATION.
    ///
    /// `members` above keys on a declaration node, which is what Rust, Go,
    /// Java and TypeScript give it. Python, Ruby, JavaScript and PHP give it
    /// nothing: an object's fields are the `self.x = …` / `@x = …` /
    /// `this.x = …` / `$this->x = …` assignments in the constructor, and the
    /// grammar calls those plain assignments. So five languages had every
    /// field they own missing from the index — measured on litellm 2026-08-28,
    /// **653 distinct `self.<name>` attributes over 176 files**, and
    /// `query async_handler` (36 assignments) answered `no symbol matching`.
    ///
    /// This is NOT the case the `members` reserve on `py` declines. That one
    /// is module/class-level `X = …`, refused because the value side leaks
    /// identifiers and a runtime local is not a declaration. A receiver
    /// assignment has neither problem: the name is read off the LEFT side
    /// only, and it is unambiguously a field of the enclosing class.
    ///
    /// Harvested inside method bodies on purpose — a constructor IS where the
    /// declaration lives here — so the `members` scope gate does not apply.
    receiver_fields: &'static [&'static str],
    /// Node kinds that yield `import` edges. The name field on each is
    /// extracted via [`first_named_string_child`] (handles both
    /// `import "foo"` and `import foo`).
    imports: &'static [&'static str],
    /// Node kinds that represent a call / method invocation. The
    /// callee identifier is pulled via [`callee_name`]. Empty when the
    /// grammar doesn't model calls distinctly (HTML/CSS).
    calls: &'static [&'static str],
}

/// Look up a [`LangSpec`] by file extension. Returns `None` for
/// extensions we don't have a grammar for (caller falls back to regex).
fn lang_for_ext(ext: &str) -> Option<&'static LangSpec> {
    LANGS.iter().find(|l| {
        l.name == ext
            || matches!(
                (l.name, ext),
                // Synonyms / variants — keep narrow to avoid mapping
                // the wrong grammar to a tricky file.
                ("js", "jsx" | "mjs" | "cjs")
                    | ("ts", "tsx")
                    | ("py", "pyw")
                    | ("rb", "rbw")
                    | ("c", "h")
                    | ("cpp", "cc" | "cxx" | "hpp" | "hxx")
            )
    })
}

/// Per-file parse budget. Defense-in-depth against pathological
/// inputs: deeply-nested expressions, malformed Unicode that throws
/// the grammar into exponential backtracking, etc. `MAX_INDEX_FILE_BYTES`
/// in `index.rs` already caps inputs at 1 MiB, so this timeout is
/// belt-and-suspenders — most parses finish under a millisecond.
///
/// Scales linearly: 1 second per 100 KiB of input, with a 1-second
/// floor and a 30-second ceiling. The ceiling matters because a
/// runaway parse blocks the rayon worker thread; 30 s × N cores caps
/// the total damage to a startup index build.
fn parse_timeout_micros(source_len: usize) -> u64 {
    let base: u64 = 1_000_000; // 1 s minimum
    let scaled: u64 = (source_len as u64) * 10; // ~10 µs / byte = 1 s / 100 KiB
    base.saturating_add(scaled).min(30_000_000)
}

/// Extract symbols + imports + call edges from `source` using the
/// tree-sitter grammar for `ext`. Returns `None` only when no grammar
/// is registered for the extension or the parse hits the timeout;
/// never panics on bad input.
///
/// The call list is the syntactic edges only (caller name → callee
/// name + line). Resolution (which `auth()` overload was hit?) lives
/// in `Index::callers` and is best-effort.
///
/// ## Note on worker recycling
///
/// CodeGraph (the TypeScript reference impl) recycles parser workers
/// every 250 files because `web-tree-sitter` runs in WebAssembly and
/// WASM linear memory never shrinks — fragmentation otherwise grows
/// unbounded over a 10k-file project. We use the native `tree-sitter`
/// crate which goes through standard `malloc`/`free`, so a fresh
/// `Parser` per file (instantiated below) is dropped at end-of-scope
/// and the OS reclaims its memory normally. No worker pool needed.
/// Formats whose CODE lives inside a `<script>` block. The file's own
/// grammar yields no declarations — `html` and `css` are registered in
/// [`LANGS`] with empty node lists on purpose, and `vue`/`svelte`/`astro`
/// have no grammar at all — so every symbol they contain is invisible to
/// `query`, `skeleton` and `semantic` until the block is handed to the JS/TS
/// parser. Measured on the local corpus: 3 053 files, **zero** symbols
/// extracted, against 18 810 declarations sitting inside their script blocks.
const SCRIPT_HOSTS: &[&str] = &["html", "htm", "vue", "svelte", "astro"];

/// Whether a file of this extension carries its declarations inside a
/// `<script>` block. The index walker asks BEFORE collecting the path: these
/// extensions are absent from `index::lang_for`'s regex table, so without
/// this the file is never read and the extractor below is unreachable — the
/// gate before the gate, which is how this was first measured wrong.
pub fn hosts_embedded_script(ext: &str) -> bool {
    SCRIPT_HOSTS.contains(&ext)
}

/// A `type=` that means "this is not code". Measured over 37 623 real script
/// tags: `text/javascript` (17 677), absent (16 246), `application/ld+json`
/// (1 096), `application/json` (414), `module` (301), `text/plain` (239),
/// `text/template` (213). Lazy-loader types (`rocketlazyloadscript`, and
/// Cloudflare's `<hash>-text/javascript`) DO carry javascript, so the rule
/// names what to REJECT rather than what to accept.
fn script_type_is_data(ty: &str) -> bool {
    let ty = ty.to_ascii_lowercase();
    ty.contains("json") || ty.contains("template") || ty.contains("html") || ty.contains("plain")
}

/// Every `<script>` body in `source`, as (0-based line the body starts on,
/// body). Hand-rolled rather than a regex because the tag name must end at
/// the `>` or at whitespace: `<script>` and `<script defer>` are script tags
/// and `<scriptish>` is not — a failure that once cost a day, where
/// `<p[^>]*>` happily matched `<pre>`.
fn script_blocks(source: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    // ONE lowercase copy, scanned for both halves of the tag, with every
    // slice taken from the ORIGINAL. `to_ascii_lowercase` leaves non-ASCII
    // bytes alone, so byte offsets are identical in both strings.
    //
    // Two defects in one change. The close tag was already matched
    // case-insensitively while the open tag was not, so `<SCRIPT>` opened
    // nothing while `</SCRIPT>` would have closed something — html tag names
    // are case-insensitive and the two halves disagreed. Rare (3 tags in
    // 37 295 on the local corpus) but free to get right.
    //
    // And the close search used to lowercase the REMAINDER of the document
    // for every block, which is quadratic: 20 000 small blocks in 500 KB took
    // **7.9 s**, and the worst real page (513 KB, 412 tags) 270 ms. One copy
    // up front makes both linear.
    let lower = source.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut i = 0usize;
    let mut line = 0usize;
    let mut scanned = 0usize;
    while let Some(rel) = lower[i..].find("<script") {
        let open = i + rel;
        let after = open + "<script".len();
        let next = b.get(after).copied();
        // The tag name ends here, or it was never `script`: `<script>` and
        // `<script defer>` are script tags, `<scriptish>` is not.
        let ends_name = matches!(next, Some(b'>') | Some(b'/'))
            || next.is_some_and(|c| c.is_ascii_whitespace());
        if !ends_name {
            i = after;
            continue;
        }
        let Some(gt) = lower[after..].find('>') else {
            break;
        };
        let attrs = &lower[after..after + gt];
        let body_start = after + gt + 1;
        let Some(close) = lower[body_start..].find("</script") else {
            break;
        };
        let body = &source[body_start..body_start + close];
        // Accumulated from the previous block rather than recounted from
        // the start of the document each time. Both compute the same line —
        // no test can tell them apart — but the corpus holds a 513 KB page
        // with 412 script tags, where recounting is 211 M byte-scans against
        // 513 K. The incremental form is a measured cost, not a style.
        line += source[scanned..body_start].matches('\n').count();
        scanned = body_start;
        // `attrs` is already lowercase, so one `type=` test covers `TYPE=`.
        let ty = attrs
            .split_ascii_whitespace()
            .find_map(|a| a.strip_prefix("type="))
            .map(|v| v.trim_matches(['"', '\'']))
            .unwrap_or("");
        // A `src=`-only tag has an empty body and nothing to parse. Not
        // cosmetic: 18 094 of the 37 071 script bodies in the local corpus
        // are empty (48 %), so this skips half the parser spin-ups.
        if !body.trim().is_empty() && !script_type_is_data(ty) {
            out.push((line, body));
        }
        i = body_start + close;
    }
    out
}

/// Declarations, imports and calls from the `<script>` blocks of a host
/// document, with every line rebased onto the HOST file. A symbol whose line
/// points inside the extracted fragment sends the reader to the wrong place,
/// which is the same honesty owed for paths.
fn extract_from_script_hosts(source: &str) -> Option<(Vec<Symbol>, Vec<String>, Vec<Call>)> {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut calls = Vec::new();
    for (line0, body) in script_blocks(source) {
        // A bundled third-party blob inlined in the page. Its declarations
        // are `r`, `i`, `o`, `e` — nothing a reader will ever search for, and
        // they outrank the real API by sheer count. Reusing the minified-code detector
        // rather than writing a second opinion: measured on 1 046 scraped
        // pages it removes 9 209 of 13 883 symbols, and on hand-written
        // templates it removes NONE (99 in, 99 out).
        if crate::minified::detect(body) {
            continue;
        }
        let Some((mut s, i, c)) = extract_full(body, "ts") else {
            continue;
        };
        // What the detector's 4 KB floor cannot reach: the small analytics
        // snippet, minified all the same. Measured on what survives it —
        // name length 1 appears 1 830 times against 92 at length 2, spread
        // over 367 files so no size rule catches them, while the same
        // measurement over hand-written templates finds NOTHING shorter than
        // four characters. Cutting at ONE and not at two: `id`, `db` and `ok`
        // are names somebody could have chosen, `r` is not.
        s.retain(|sym| sym.name.chars().count() > 1);
        for sym in s.iter_mut() {
            sym.line += line0;
        }
        symbols.append(&mut s);
        imports.extend(i);
        calls.extend(c);
    }
    Some((symbols, imports, calls))
}

pub fn extract_full(source: &str, ext: &str) -> Option<(Vec<Symbol>, Vec<String>, Vec<Call>)> {
    if SCRIPT_HOSTS.contains(&ext) {
        return extract_from_script_hosts(source);
    }
    let spec = lang_for_ext(ext)?;
    let lang = (spec.language)();
    let mut parser = Parser::new();
    parser.set_language(&lang).ok()?;
    // tree-sitter 0.25+ retired `set_timeout_micros`; the new
    // cancellation API is a progress callback the runtime calls
    // periodically. We close over a deadline and bail out when the
    // wall clock crosses it. A `None` return from `parse_with_options`
    // means we tripped the timeout; the caller falls back to the
    // regex extractor (which has its own backtracking bounds).
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_micros(parse_timeout_micros(source.len()));
    let mut on_progress = |_state: &tree_sitter::ParseState| {
        if std::time::Instant::now() > deadline {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = ParseOptions::new().progress_callback(&mut on_progress);
    let src_bytes = source.as_bytes();
    let mut feed = |offset: usize, _pt: tree_sitter::Point| -> &[u8] {
        src_bytes.get(offset..).unwrap_or(&[])
    };
    let tree = parser.parse_with_options(&mut feed, None, Some(options))?;
    let mut symbols: Vec<Symbol> = Vec::new();
    let mut imports: Vec<String> = Vec::new();
    let mut calls: Vec<Call> = Vec::new();
    walk_with_calls(
        tree.root_node(),
        src_bytes,
        spec,
        "<module>",
        &mut symbols,
        &mut imports,
        &mut calls,
        0,
    );
    dedup_fields(&mut symbols);
    Some((symbols, imports, calls))
}

/// One field, one row — keyed on (owner, name) within the file.
///
/// A declared field cannot legally repeat inside its own type, so this is a
/// no-op for every language that has field DECLARATIONS. It exists for the
/// receiver assignments: `self.api_base = …` is written once per code path,
/// and litellm has classes that set the same attribute on two consecutive
/// lines. Measured on its `api_base` answer before this: 30 rows for 25
/// distinct fields, 17 % of the page repeating a line the reader already had.
/// The FIRST occurrence wins — earliest line, which is the constructor.
///
/// Same defect as the repeated `const (5): MAX_RESCUED, MAX_RESCUED` once seen
/// in the skeleton, one extractor upstream of where that one was fixed.
fn dedup_fields(symbols: &mut Vec<Symbol>) {
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    symbols.retain(|s| s.kind != "field" || seen.insert((s.owner.clone(), s.name.clone())));
}

/// Back-compat alias kept for callers that don't need calls. Drops
/// the call list. Use [`extract_full`] when you need it.
pub fn extract(source: &str, ext: &str) -> Option<(Vec<Symbol>, Vec<String>)> {
    extract_full(source, ext).map(|(s, i, _)| (s, i))
}

// Six subjects. Extraction stays here; the language table, the skeleton, the
// call graph, naming, fingerprinting and the records move out.
mod calls;
mod hash;
mod langs;
mod names;
mod records;
mod skeleton;
use calls::*;
use hash::*;
use langs::*;
use names::*;
pub use records::*;
pub(crate) use skeleton::*;

#[cfg(test)]
include!("treesitter_tests.rs");
