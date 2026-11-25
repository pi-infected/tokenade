//! Screenshot-grade dashboard renderer.
//!
//! The look users see when they run `tokenade dashboard` IS the marketing
//! material for the project — people screenshot CLI dashboards and share
//! them. So this module treats the output as a brand artifact:
//!
//! - **ASCII brand block** at the top so any screenshot is unambiguously
//!   Tokenade (no confused "neat dashboard, but what tool is this?").
//! - **One hero number**: total tokens saved + % + dollar value, set in
//!   the largest visual weight on the screen so the eye lands there.
//! - **Bar charts** instead of bare percentages — humans compare lengths
//!   faster than they compare numbers.
//! - **Color** when the terminal supports it, falling back to mono when
//!   piped, when `NO_COLOR=1`, or when stdout is not a TTY. The shape of
//!   the dashboard does not depend on color.
//!
//! All rendering is pure: callers pass a [`DashboardData`] in and get a
//! `String` back. No I/O, no env reads, no time calls inside the renderer
//! — easier to unit-test, easier to embed in HTML / web exports later.

use std::fmt::Write;

mod elsewhere;
mod hero;
mod sections;
use elsewhere::*;
pub use hero::*;
use sections::*;

/// Aggregated savings + heuristic data the dashboard renders. Built by
/// the CLI from `~/.tokenade/gain.jsonl`; consumed by [`render`].
#[derive(Debug, Clone, Default)]
pub struct DashboardData {
    pub project_path: String,
    pub ledger_path: String,
    pub operations: u64,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub last_7d_saved: u64,
    /// Time window (days) the figures above cover; 0 = lifetime. Surfaced in
    /// the header so the user knows the totals are scoped to `--days N`.
    pub window_days: u32,
    /// Cumulative tokens saved across ALL history (survives windowing) —
    /// rendered as a small "lifetime" line beside the windowed hero.
    pub lifetime_saved: u64,
    /// Tokens saved by code-intelligence tools (semantic_search,
    /// symbol_find, …) — values are HEURISTIC ESTIMATES, not measured.
    /// Surfaced as a separate hero number so the headline figure stays
    /// honest about what was actually measured.
    pub estimated_tokens_saved: u64,
    /// Full-context cache rewrites DETECTED in the window, as
    /// `(trigger, events, tokens)` sorted by tokens desc.
    ///
    /// tokenade has booked these since 2026-07-12 and shown them to nobody.
    /// MEASURED on this install when the section was added: 1 068 events and
    /// **181 million tokens** on a single trigger, `scheduled` firing again
    /// within two minutes. A cache rewrite is money, so a figure that large
    /// belongs where a user goes to read figures — not in an injected nudge,
    /// which measurement the day before put at 4–8 % adherence.
    pub cache_rewrites: Vec<(String, u64, u64)>,
    pub top_compactors: Vec<(String, u64, u64, u64)>, // (op, before, after, count)
    /// Per-LAYER rollup of MEASURED compaction (conflux-inspired, added
    /// 2026-06-09): every measured op is bucketed into a logical layer by
    /// [`op_layer`] and summed, so the user sees "where the savings come
    /// from" at a glance — shell vs file-read vs content-auto vs web vs
    /// wrapped-mcp — instead of scanning a long per-op list. Heuristic
    /// code-intel is deliberately excluded (it has its own section and
    /// would mix estimated with measured tokens). Tuple is
    /// `(layer, before, after, calls)`, sorted by tokens saved desc.
    pub by_layer: Vec<(String, u64, u64, u64)>,
    pub by_project: Vec<(String, u64, u64, u64)>, // (project_path, before, after, count)
    pub code_intel: Vec<(String, u64, u64)>,      // (op, calls, est_tokens_saved)
    pub code_intel_disclaimer: Option<String>,
    /// Secret values masked (`<redacted>`) before the output reached the
    /// model, within the same `window_days` scope as the token figures.
    /// Fed from the redaction ledger (`~/.tokenade/redact.jsonl`).
    pub secrets_redacted: u64,
    /// Which rule masked the most values in this window, and how many. A bare
    /// count cannot tell a busy week from a rule firing on something it should
    /// not — the entropy sweep spent weeks masking Rust test names and this
    /// line said nothing but a number. `None` for rows written before the
    /// ledger carried a breakdown.
    pub secrets_redacted_top_rule: Option<(String, u64)>,
    /// #498: per-wrapped-MCP compaction stats. Same shape as
    /// `top_compactors` (op, before, after, count) but isolated so
    /// the user can see wrapped-MCP traffic separately from
    /// in-tree compactors. Populated from gain.jsonl rows whose
    /// `op` starts with `mcp-proxy:` (written by #493).
    pub wrapped_mcps: Vec<(String, u64, u64, u64)>,
    /// Per-project Claude Code API usage scraped from
    /// `~/.claude/projects/*/*.jsonl`. Surfaces tokens + tool-call
    /// breakdown so the user can see where the model bypassed
    /// Tokenade (no MCP, no hook).
    pub claude_usage: Vec<ClaudeUsageRow>,
    /// #683 — global aggregation of tool_result tokens by tool name
    /// across all sessions in scope, sorted desc by tokens. Each
    /// (tool_name, tokens) pair powers the "AGENT TOKENS (outside
    /// of tokenade) - Functions" section. Built by summing
    /// `ProjectUsage.tool_result_tokens_by_tool` across all rows.
    pub bypass_by_tool: Vec<(String, u64)>,
    /// #684 — per-tool sum of `after` tokens from gain.jsonl entries
    /// attributable to that tool's route. Lets the Functions section
    /// subtract genuine tokenade activity from `tool_result_tokens`
    /// so Bash/Read (which DO go through tokenade hooks) stop being
    /// mis-reported as "outside". Attribution rules:
    ///   source=hook + label "Bash:…"   → "Bash"
    ///   source=hook + label "Read:…"   → "Read"
    ///   source=wrap                    → "Bash" (CLI `tokenade wrap` is Bash-invoked)
    ///   source=cli                     → skip (not agent-driven)
    pub tokenade_after_by_tool: std::collections::HashMap<String, u64>,
    /// Display-only date string (e.g. "2026-05-29") shown in the
    /// CLAUDE USAGE subtitle so the user knows the cutoff. `None`
    /// means no cutoff (count everything ever recorded).
    pub claude_usage_since: Option<String>,
    /// #740 — home directory path (e.g. `/home/username`) for
    /// render-time `~/` rewrite of project paths. by_project keys
    /// are now stored as RAW absolute paths so the AGENT TOKENS
    /// Projects section's lookup against `row.cwd` (also a raw
    /// absolute path) can succeed. The PROJECTS section render
    /// applies this prefix-strip just before display.
    pub home_dir: Option<String>,
    /// Render the few localized strings (coverage notes) in French.
    /// Resolved by the CLI from `LC_ALL`/`LC_MESSAGES`/`LANG` and passed
    /// in, because this renderer is pure (no env reads — see module doc).
    pub lang_fr: bool,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_bytes_served: u64,
    pub semantic_docs: u64,
    pub semantic_terms: u64,
    /// Dominant model the agent runs (most tokens), used to price savings. (#3)
    pub model_used: Option<String>,
    /// Estimated real-dollar value of the savings at that model's live prices.
    pub dollars_saved: f64,
}

/// One row of the AGENT USAGE section. `display_cwd` is what the
/// dashboard prints (already `~`-shortened); `agent_label` is the
/// short tag ("Claude" / "Codex" / "Cline") that identifies the
/// upstream agent; the tool counters drive the per-row breakdown.
#[derive(Debug, Clone, Default)]
pub struct ClaudeUsageRow {
    /// Short agent identifier shown in front of the path
    /// ("Claude", "Codex", "Cline"). Empty for back-compat with
    /// older callers that built rows without setting it.
    pub agent_label: String,
    pub display_cwd: String,
    /// #675 — raw absolute cwd, used to look up matching gain
    /// aggregation in `DashboardData.by_project` so the coverage
    /// line below the row can compare apples-to-apples. Kept
    /// separately from `display_cwd` which gets a `~/` prefix
    /// rewrite for narrow-terminal display.
    pub cwd: String,
    pub sessions: u32,
    pub total_tokens: u64,
    /// #673 — sum of tool_result content tokens scraped from the
    /// session transcripts. THIS is what tokenade COULD have
    /// compacted (vs `total_tokens` which is `input + output +
    /// cache_creation` — the latter accumulates ALL fresh content
    /// added across the session including Claude's responses, fresh
    /// user messages, and the system prompt being cache-created on
    /// first sight; not comparable to a per-call savings number).
    /// #678 corrected: cache_read (the re-sent history) is excluded
    /// — the inflation comes from cumulative fresh content, not
    /// from re-sent history.
    pub tool_result_tokens: u64,
    /// #683 — per-tool breakdown of `tool_result_tokens` for this
    /// session group. Keyed by tool name as emitted by the agent
    /// (e.g. `Bash`, `Read`, `mcp__tokenade__symbol_find`). Summed
    /// across all rows by the renderer to power the "AGENT TOKENS
    /// (outside of tokenade) - Functions" section.
    pub tool_result_tokens_by_tool: std::collections::HashMap<String, u64>,
    pub last_ts: String,
    pub tokenade_calls: u64,
    pub read_calls: u64,
    pub grep_calls: u64,
    pub glob_calls: u64,
    pub bash_calls: u64,
    pub web_calls: u64,
}

impl ClaudeUsageRow {
    /// Tool calls that COULD have been routed through Tokenade —
    /// everything except Edit/Write (mutations) and `other`.
    pub fn bypass_eligible(&self) -> u64 {
        self.read_calls + self.grep_calls + self.glob_calls + self.bash_calls + self.web_calls
    }
}

/// Visual surface options. Set [`Theme::color`] to `false` to emit a
/// mono dashboard (for files, pipes, CI logs).
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub color: bool,
    pub width: usize,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            color: true,
            width: 78,
        }
    }
}

/// 256-colour ANSI helpers. Resolves to empty strings when [`Theme::color`]
/// is false, so the same render path works for both surfaces.
struct Pal<'a> {
    t: &'a Theme,
}

impl<'a> Pal<'a> {
    fn esc(&self, n: u8) -> String {
        if self.t.color {
            format!("\x1b[38;5;{n}m")
        } else {
            String::new()
        }
    }
    // #557 — `fn bg(n: u8)` removed; the dashboard layout never paints
    // background colors, and `fg(n)` is the only ANSI helper actually
    // used. Re-add 6 lines if a future block-style highlight wants it.
    fn bold(&self) -> &'static str {
        if self.t.color {
            "\x1b[1m"
        } else {
            ""
        }
    }
    fn dim(&self) -> &'static str {
        if self.t.color {
            "\x1b[2m"
        } else {
            ""
        }
    }
    fn rst(&self) -> &'static str {
        if self.t.color {
            "\x1b[0m"
        } else {
            ""
        }
    }
    // Brand palette — orange/cyan duo, easy to recognise across screenshots.
    fn orange(&self) -> String {
        self.esc(208)
    } // brand primary
    fn cyan(&self) -> String {
        self.esc(81)
    } // brand secondary
    fn green(&self) -> String {
        self.esc(120)
    } // success / savings
    fn yellow(&self) -> String {
        self.esc(220)
    } // metric highlight
    fn gray(&self) -> String {
        self.esc(244)
    } // dim text
    fn label(&self) -> String {
        self.esc(110)
    } // section labels
    fn fade(&self) -> String {
        self.esc(238)
    } // bar empty
}

/// Block-letters logo, rendered with Unicode box characters. Six rows tall,
/// fits in 72 columns. Embedded as raw lines so we can colourise on the fly.
const LOGO: [&str; 6] = [
    "████████╗ ██████╗ ██╗  ██╗███████╗███╗   ██╗ █████╗ ██████╗ ███████╗",
    "╚══██╔══╝██╔═══██╗██║ ██╔╝██╔════╝████╗  ██║██╔══██╗██╔══██╗██╔════╝",
    "   ██║   ██║   ██║█████╔╝ █████╗  ██╔██╗ ██║███████║██║  ██║█████╗  ",
    "   ██║   ██║   ██║██╔═██╗ ██╔══╝  ██║╚██╗██║██╔══██║██║  ██║██╔══╝  ",
    "   ██║   ╚██████╔╝██║  ██╗███████╗██║ ╚████║██║  ██║██████╔╝███████╗",
    "   ╚═╝    ╚═════╝ ╚═╝  ╚═╝╚══════╝╚═╝  ╚═══╝╚═╝  ╚═╝╚═════╝ ╚══════╝",
];

/// Compact form when stdout < ~70 columns. Falls back to a horizontal
/// banner that still carries the brand without breaking wrapping.
const LOGO_COMPACT: &str = "──── T O K E N A D E ────";

/// Truncate a path-like string to `max` chars, keeping the tail
/// (the distinguishing leaf) when truncation happens. Returns
/// `"…<last max-1 chars>"` so the user sees the part that varies
/// between sibling paths instead of a stack of identical-looking
/// prefixes. ASCII ellipsis (`…`) is one glyph but multi-byte —
/// hence the char-count math.
pub fn truncate_tail(s: &str, max: usize) -> String {
    let len = s.chars().count();
    if len <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let skip = len - keep;
    let tail: String = s.chars().skip(skip).collect();
    format!("…{tail}")
}

/// Format an integer count of tokens as a short human string (1.2k, 3.4M…).
pub fn human_tokens(n: u64) -> String {
    if n < 1_000 {
        format!("{n}")
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else if n < 1_000_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    }
}

/// Render a horizontal bar of `width` columns filled to `pct/100` with the
/// foreground colour. Uses sub-cell block glyphs (`▏▎▍▌▋▊▉█`) so the bar
/// reflects fractional progress smoothly.
fn render_bar(width: usize, pct: f64, p: &Pal<'_>) -> String {
    let pct = pct.clamp(0.0, 100.0);
    let cells = width as f64 * pct / 100.0;
    let full = cells.floor() as usize;
    let frac = cells - cells.floor();
    let partial_glyph = match (frac * 8.0).round() as u32 {
        0 => "",
        1 => "▏",
        2 => "▎",
        3 => "▍",
        4 => "▌",
        5 => "▋",
        6 => "▊",
        7 => "▉",
        _ => "█",
    };
    let empty = width.saturating_sub(full + usize::from(!partial_glyph.is_empty()));
    format!(
        "{}{}{}{}{}{}{}{}",
        p.green(),
        "█".repeat(full),
        partial_glyph,
        p.fade(),
        "░".repeat(empty),
        p.rst(),
        "",
        ""
    )
}

/// Render the full dashboard. Pure function: no I/O, no env reads.
/// Classify a gain-ledger `op` name into a logical compaction LAYER
/// (conflux-inspired per-layer rollup, added 2026-06-09). Heuristic but
/// stable; defers to [`crate::code_intel_heuristic::is_code_intel`] for the
/// code-intel bucket so there is a single source of truth for that family.
///
/// Layers:
///   * `wrapped-mcp`   — third-party servers fronted by `tokenade mcp-proxy`.
///   * `code-intel`    — structure/symbol/semantic tools (heuristic savings).
///   * `file-read`     — `read_*` / `cat` file-read compactors.
///   * `web`           — serp / webfetch / html-to-markdown.
///   * `content-auto`  — the content-routed `auto:*` family (json, yaml, logs,
///     stacktrace, archive, …): tokenade picked the compactor.
///   * `shell`         — explicit command wrappers (exec/script/wrap/cargo/git/
///     ls/grep/…), the proven shell-boundary lever.
///   * `other`         — anything unclassified.
pub fn op_layer(op: &str) -> &'static str {
    if op.starts_with("mcp-proxy:") {
        return "wrapped-mcp";
    }
    if crate::code_intel_heuristic::is_code_intel(op) {
        return "code-intel";
    }
    if op.starts_with("read_") || op == "cat" || op == "read" {
        return "file-read";
    }
    if op.contains("serp") || op.contains("web") || op.contains("html") {
        return "web";
    }
    if op.starts_with("auto:") {
        return "content-auto";
    }
    // `code_strip` strips comments/blank lines from a source buffer — it is a
    // file-content lever, not a shell one. Group it with the file readers.
    if op == "code_strip" || op.starts_with("code_strip") {
        return "file-read";
    }
    // Split on every separator the op-namespace uses (`:`, `-`, `_`) so the
    // `*_compact` / `*-compact` suffix families collapse to their tool root
    // (`grep_compact` → `grep`, `glob_compact` → `glob`, …) instead of
    // falling through to `other`.
    let base = op.split(&[':', '-', '_'][..]).next().unwrap_or(op);
    if matches!(
        base,
        "exec"
            | "script"
            | "wrap"
            | "cargo"
            | "git"
            | "ls"
            | "grep"
            | "cat"
            | "find"
            | "cd"
            | "glob"
            | "diff"
            | "pkgmgr"
            | "network"
            | "sysinfo"
    ) || matches!(
        op,
        "generic" | "text-util" | "lsp_enforce" | "lsp_enforce_bash"
    ) {
        return "shell";
    }
    "other"
}

/// Roll up a set of MEASURED `(op, before, after, calls)` rows into per-layer
/// totals via [`op_layer`], sorted by tokens saved (desc). Used to populate
/// [`DashboardData::by_layer`]. Pure + deterministic so it unit-tests cheaply.
pub fn per_layer_rollup(ops: &[(String, u64, u64, u64)]) -> Vec<(String, u64, u64, u64)> {
    use std::collections::BTreeMap;
    let mut acc: BTreeMap<&'static str, (u64, u64, u64)> = BTreeMap::new();
    for (op, b, a, n) in ops {
        let e = acc.entry(op_layer(op)).or_insert((0, 0, 0));
        e.0 += *b;
        e.1 += *a;
        e.2 += *n;
    }
    let mut out: Vec<(String, u64, u64, u64)> = acc
        .into_iter()
        .map(|(layer, (b, a, n))| (layer.to_string(), b, a, n))
        .collect();
    out.sort_by_key(|(_, b, a, _)| std::cmp::Reverse(b.saturating_sub(*a)));
    out
}

#[cfg(test)]
include!("dashboard_tests.rs");
