//! The ONE web-search engine: rotation, circuit breaker, parallel fan-out,
//! validation, merge and ranking.
//!
//! Two things live outside this module on purpose. Session-scoped dedup (what
//! this session has already been shown) belongs to the hook, which has a
//! session. Delivery — a PreToolUse verdict, or stdout — belongs to the channel.
//! Everything else about "what does the web say about X" is here, so that
//! `tokenade search` and the WebSearch hook cannot answer it differently.
//!
//! Before this module the orchestration lived inside the hook, and there was no
//! CLI equivalent at all: an agent without hooks (Codex, Cursor, Grok, Droid,
//! Aider) had NO search.

use crate::serp;

/// Engines fanned out per query. Three is the measured knee: the union of
/// independent crawls is where the coverage win comes from (Ecosia=Google,
/// Bing=Microsoft, Mojeek=its own index), and the fourth engine buys hits the
/// first three already had.
///
/// The fetches are CONCURRENT, so the cost of a fourth is not latency — it is
/// one more outbound request per search, and one more source of per-IP
/// rate-limit pressure. That is the reason for a bound at all.
///
/// This is the only place the number lives. A second copy of it sat in the
/// websearch hook, unused, documenting the same decision it did not control —
/// so anyone who had changed it would have changed nothing.
pub const FANOUT: usize = 3;

/// Below this an engine's answer is treated as a failed parse rather than a
/// short result set.
pub const MIN_HITS: usize = 3;

/// Most results returned to a caller.
pub const MAX_HITS: usize = 15;

/// Consecutive failures before an engine's breaker opens.
pub const CB_MAX_FAILS: u32 = 2;

/// How long an open breaker stays open.
pub const CB_TTL_SECS: u64 = 15 * 60;

/// The FLOOR on engines fanned out per query. Cross-engine agreement — the
/// strongest relevance signal here — needs at least two independent indexes to
/// separate a real hit from an accidental one, so a query served by fewer than
/// this is a degraded query, taken only when the fleet is saturated.
pub const MIN_FANOUT: usize = 2;

/// Per-engine request budget PER WINDOW, shared across every tokenade process.
///
/// This is a per-minute VOLUME ceiling, NOT the anti-burst mechanism — that job
/// belongs to [`min_interval_ms`] (≥1 s between two calls to one engine), which
/// is what actually stops the back-to-back 403/429 (Brave, Mojeek). With calls
/// spaced ≥1 s, one engine can be hit at most ~60×/min anyway, so these budgets
/// are set generously enough that a single agent's research burst (a dozen-plus
/// searches in a minute) is not starved — the earlier low caps returned n=0 for
/// 40 % of a 22-query burst. The breaker still backs off on real refusals.
/// `TOKENADE_ENGINE_BUDGET=<n>` caps them all.
fn engine_budget(e: Engine) -> u32 {
    if let Some(n) = std::env::var("TOKENADE_ENGINE_BUDGET")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        return n;
    }
    match e {
        Engine::Ecosia => 24,
        Engine::Bing => 18,
        Engine::DuckDuckGo => 18,
        Engine::Yahoo => 18,
        Engine::Marginalia => 15,
        Engine::Brave => 12,
        Engine::Mojeek => 12,
    }
}

/// The budget window in seconds; every engine's counter resets once this
/// elapses since the window opened. Env-tunable.
fn window_secs() -> u64 {
    std::env::var("TOKENADE_ENGINE_WINDOW_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60)
}

/// Minimum gap in MILLIS between two calls to the SAME engine, fleet-wide. The
/// budget caps volume per window; this caps CADENCE, because engines that 403/
/// 429 do so on rapid back-to-back queries, not on their per-minute total.
/// Default 1000 ms; env-tunable.
fn min_interval_ms() -> u64 {
    std::env::var("TOKENADE_ENGINE_MIN_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000)
}

/// Unix time in milliseconds — the resolution the min-interval needs (the
/// breaker/window logic stays in whole seconds).
pub use crate::platform::now_unix_millis as now_millis;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Engine {
    Ecosia,
    Bing,
    Mojeek,
    Yahoo,
    Brave,
    Marginalia,
    /// DuckDuckGo's HTML endpoint. Reachable today with our full browser header
    /// set (measured 200 + 11 results); an independent-enough index to widen the
    /// union. D8's first added engine.
    DuckDuckGo,
}

/// Rotation priority (also the tiebreaker when load is even).
///
/// Ecosia first (Google-backed index, reliable, key-free), Bing second
/// (independent index), Brave third (its own crawl, server-rendered),
/// Mojeek fourth (independent crawl), Marginalia fifth (a JSON API, and the
/// only one here that indexes the small non-commercial web — it finds things
/// the mainstream engines structurally cannot), Yahoo last (Bing-backed, so it
/// adds least to a union that already has Bing).
///
/// Brave had a working extractor in `serp.rs` wired to NOTHING but an example
/// binary — an engine written and never reachable. It rate-limits hard (429 on
/// back-to-back queries, MEASURED), which is precisely what the breaker and the
/// rotation exist for: it contributes when it is available and steps aside when
/// it is not.
///
/// Marginalia is last and it earns its slot differently from the others: it is
/// the only index here that crawls the small non-commercial web, so it surfaces
/// pages the mainstream engines structurally cannot. It is reached over HTML —
/// its public JSON API (`api.marginalia.nu/public/search`) answers **404** now,
/// which is why `marginalia_extract_json` had never been wired to anything.
/// MEASURED at `old-search.marginalia.nu`: HTTP 200, 25 results, clean markup.
pub const ENGINES: [Engine; 7] = [
    Engine::Ecosia,
    Engine::Bing,
    Engine::Brave,
    Engine::DuckDuckGo,
    Engine::Mojeek,
    Engine::Yahoo,
    Engine::Marginalia,
];

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::Ecosia => "ecosia",
            Engine::Bing => "bing",
            Engine::Mojeek => "mojeek",
            Engine::Yahoo => "yahoo",
            Engine::Brave => "brave",
            Engine::Marginalia => "marginalia",
            Engine::DuckDuckGo => "duckduckgo",
        }
    }

    pub fn url(self, query: &str) -> String {
        let q = urlencode_query(query);
        match self {
            Engine::Ecosia => format!("https://www.ecosia.org/search?q={q}"),
            // No `mkt=`/`setlang=`: a fixed market wrecks non-English queries
            // (measured — pinning en-US took "Klimagerät …" from 5/6 on-topic to
            // 0/6, returning Scratch/Baidu junk). Bare `q=` lets Bing infer the
            // locale from the query itself. Bing still intermittently serves an
            // entity/disambiguation page for ambiguous head-noun queries from a
            // datacenter IP; that is caught downstream by `plausible_for_query`
            // (→ OffTopic → engine rotated out), not by any URL parameter.
            Engine::Bing => format!("https://www.bing.com/search?q={q}"),
            Engine::Mojeek => format!("https://www.mojeek.com/search?q={q}"),
            // `iscqry=` (empty) is, per SearXNG, "necessary for results to
            // appear properly on the first page"; `pz=7,bct=0,xargs=0` are the
            // companion params it sends. Likely closes our 7-vs-10 gap.
            Engine::Yahoo => {
                format!("https://search.yahoo.com/search?p={q}&iscqry=&pz=7&bct=0&xargs=0")
            }
            Engine::Brave => format!("https://search.brave.com/search?q={q}&source=web"),
            // `old-search`, not `search`: the latter 302s and the API 404s.
            Engine::Marginalia => {
                format!("https://old-search.marginalia.nu/search?query={q}")
            }
            Engine::DuckDuckGo => format!("https://html.duckduckgo.com/html/?q={q}"),
        }
    }

    /// Per-engine `Cookie`. Yahoo gates the full server-rendered SERP behind a
    /// preference cookie; without it it serves a thin or JS-only page. `None` =
    /// none needed.
    ///
    /// Bing sends NO cookie: the old `SRCHHPGUSR=SRCHLANG=en` was a thin, static
    /// preference cookie that a real browser never sends alone, and it measurably
    /// pushed Bing toward the entity-collapse page ("how to make sourdough bread"
    /// → the *Make* build tool). Bare requests render the full SERP fine, so the
    /// cookie only hurt.
    pub fn cookie(self) -> Option<&'static str> {
        match self {
            Engine::Yahoo => Some("sB=v=1&vm=p&fl=1&vl=lang_en&pn=10&rw=new&userset=1"),
            // DDG opens on the header set, not a cookie: a cookie jar without
            // browser headers gets a 202 + challenge, so send none.
            _ => None,
        }
    }

    pub fn parse(self, body: &str) -> Vec<serp::Result> {
        match self {
            Engine::Ecosia => serp::ecosia_extract(body),
            Engine::Bing => serp::bing_extract(body),
            Engine::Mojeek => serp::mojeek_extract(body),
            Engine::Yahoo => serp::yahoo_extract(body),
            Engine::Brave => serp::brave_extract(body),
            Engine::Marginalia => serp::marginalia_extract(body),
            Engine::DuckDuckGo => serp::ddg_extract(body),
        }
    }

    /// Test-only: production selects an engine by rotation, never by name.
    #[cfg(test)]
    fn from_name(n: &str) -> Option<Self> {
        ENGINES.into_iter().find(|e| e.name() == n)
    }
}

/// Which engine produced this saved results page?
///
/// A page read from disk deserves the same extractor the live path would use.
/// Without this, `tokenade search <file>` fell back to the ENGINE-AGNOSTIC
/// anchor walk, which is exactly the weak parse that returns navigation:
/// measured on our own frozen fixtures, marginalia.html came back as
/// "Marginalia / About" and mojeek.html as a user survey, while the per-engine
/// extractors return 25 and 10 real results from the same bytes.
///
/// Keyed on STRUCTURAL markers — quoted attribute values that only appear in
/// that engine's markup — never on a URL or a brand name in prose, which is
/// the mistake `serp::detect` documents at length.
pub fn engine_of_page(html: &str) -> Option<Engine> {
    // Cheap pre-scan: these markers all appear well inside the document.
    const SCAN: usize = 400_000;
    let head: &str = html.get(..html.len().min(SCAN)).unwrap_or(html);
    for (marker, engine) in [
        ("data-ms-rank=", Engine::Marginalia),
        ("data-test-id=\"organic-result\"", Engine::Ecosia),
        ("class=\"b_algo\"", Engine::Bing),
        ("data-type=\"web\"", Engine::Brave),
        ("class=\"results-standard\"", Engine::Mojeek),
        ("id=\"web_results\"", Engine::Mojeek),
        ("class=\"algo algo-sr", Engine::Yahoo),
        ("id=\"web\" class=\"", Engine::Yahoo),
        ("class=\"result__a", Engine::DuckDuckGo),
    ] {
        if head.contains(marker) {
            return Some(engine);
        }
    }
    None
}

/// Extract a SAVED results page with the best parser available for it.
///
/// Falls back to the engine-agnostic walk when the page is from an engine we do
/// not know, so an arbitrary SERP still yields something.
pub fn extract_saved_page(html: &str) -> (Vec<serp::Result>, Option<Engine>) {
    if let Some(e) = engine_of_page(html) {
        let hits = e.parse(html);
        if hits.len() >= MIN_HITS {
            return (hits, Some(e));
        }
    }
    (serp::extract(html), None)
}

/// One engine's health, persisted because each hook invocation is a fresh
/// process.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct EngineHealth {
    pub fails: u32,
    /// Unix secs the breaker stays open until (0 = closed).
    pub open_until: u64,
    /// Unix secs of the last attempt — spreads load across healthy engines.
    pub last_used: u64,
    /// Unix secs the current rate-limit window opened. `#[serde(default)]` so an
    /// engine file written before budgets existed loads as a fresh (elapsed)
    /// window and resets on first use.
    #[serde(default)]
    pub window_start: u64,
    /// Requests made against this engine in the current window (across ALL
    /// processes — the file is shared). Capped by [`engine_budget`].
    #[serde(default)]
    pub window_count: u32,
    /// Unix MILLIS of the last selection — the budget caps the count per window
    /// but does not SPACE the calls, and engines like Mojeek/Brave 403/429 on
    /// back-to-back queries specifically. This enforces a minimum gap
    /// ([`min_interval_ms`]) between two calls to the same engine, fleet-wide.
    #[serde(default)]
    pub last_used_ms: u64,
}

pub type Health = std::collections::BTreeMap<String, EngineHealth>;

/// Why an engine contributed nothing. Kept because "the search found nothing"
/// and "every engine refused us" are different facts, and a caller that cannot
/// tell them apart will report the wrong one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Transport failure, or a 4xx/5xx (`--fail` makes those failures).
    Unreachable,
    /// The engine answered with a bot challenge instead of a SERP.
    Challenge,
    /// Parsed, but produced fewer than [`MIN_HITS`] usable results.
    Thin,
    /// Parsed plenty, none of it related to the query — a navigation bar.
    OffTopic,
    /// SELECTION-level: no engine had a closed breaker AND remaining budget AND
    /// was free (not held by another agent). Nothing was even tried — the fleet
    /// is saturated / rate-limited, and the right move is the agent's NATIVE
    /// search, not reporting the web as empty.
    RateLimited,
}

impl Refusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::Unreachable => "unreachable",
            Refusal::Challenge => "bot-challenge",
            Refusal::Thin => "thin",
            Refusal::OffTopic => "off-topic",
            Refusal::RateLimited => "rate-limited",
        }
    }
}

/// What a search actually produced.
pub struct Outcome {
    /// Merged, deduped, ranked. Empty when every engine refused.
    pub results: Vec<serp::Result>,
    /// Engines that contributed.
    pub used: Vec<&'static str>,
    /// Engines that did not, and why. Surfaced so a caller can say WHICH wall
    /// it hit instead of "no results".
    pub refused: Vec<(&'static str, Refusal)>,
}

impl Outcome {
    /// Did we actually search? Distinguishes "the web has nothing" from "we
    /// were unable to look" — the difference between a useful answer and a
    /// misleading one.
    pub fn is_usable(&self) -> bool {
        self.results.len() >= MIN_HITS
    }
}

/// Run a search across the healthiest engines.
///
/// `health` is read AND updated: engines that answer well reset their breaker,
/// engines that refuse trip toward open. The caller persists it (the CLI and
/// the hook share the file, so a wall one channel hits is a wall the other
/// already knows about).
pub fn run(query: &str, health: &mut Health, now: u64) -> Outcome {
    // The pure path (tests, in-process callers): rotation by breaker + LRU, no
    // cross-process budget/lock. `run_search` is the production entry point.
    let batch = rotation_order(health, now);
    fetch_and_update(query, &batch, health, now)
}

/// How long an open breaker stays open, per CAUSE (D10, SearXNG durations).
/// Retrying a bot challenge worsens IP reputation, so a wall backs off far
/// longer than a transient thin/unreachable blip. The flat 15-minute TTL this
/// replaces treated a CAPTCHA and a one-off timeout identically.
fn cb_duration(why: Refusal) -> u64 {
    match why {
        // A bot challenge is the expensive one to retry — an hour, per SearXNG.
        Refusal::Challenge => 3600,
        // Access-denied / rate-limit: back off minutes, recover soon.
        Refusal::Unreachable => 180,
        // Parsed but thin — often transient.
        Refusal::Thin => 180,
        // The engine works but returned junk for this query; moderate backoff.
        Refusal::OffTopic => 900,
        // Selection-level only — never a fetch outcome, so it never trips a
        // breaker; give it the mild default for exhaustiveness.
        Refusal::RateLimited => 180,
    }
}

/// A per-engine advisory permit. Within THIS process, requests to ONE engine
/// serialise (D11's hard constraint: never hit the same engine concurrently, or
/// a batch of queries fires simultaneous requests at Bing and trips its rate
/// limit); requests to DIFFERENT engines still run in parallel. Combined with
/// the cross-process breaker this bounds pressure on any one engine both within
/// and across processes.
fn engine_permit(e: Engine) -> &'static std::sync::Mutex<()> {
    use std::sync::{Mutex, OnceLock};
    static PERMITS: OnceLock<std::collections::HashMap<&'static str, Mutex<()>>> = OnceLock::new();
    static FALLBACK: Mutex<()> = Mutex::new(());
    let m = PERMITS.get_or_init(|| ENGINES.iter().map(|e| (e.name(), Mutex::new(()))).collect());
    // Every engine in ENGINES is registered; a miss cannot happen, but leaking a
    // shared fallback permit is safer than a panic on the search path.
    m.get(e.name()).unwrap_or(&FALLBACK)
}

/// Fetch one engine and decide whether its answer is a result set at all.
fn fetch_and_validate(eng: Engine, query: &str) -> Result<Vec<serp::Result>, Refusal> {
    // Hold the per-engine permit across the request so a concurrent batch never
    // hits this engine twice at once. Released when this frame returns.
    let _permit = engine_permit(eng).lock().unwrap_or_else(|p| p.into_inner());
    let body = crate::web_fetch::fetch_search(&eng.url(query), eng.cookie())
        .ok_or(Refusal::Unreachable)?;
    // A challenge page answers HTTP 200 with the query echoed in the title, so
    // no upstream check can see it. Parsing it for whatever links are lying
    // around is how a CAPTCHA became ten search results.
    if serp::looks_like_challenge(&body) {
        return Err(Refusal::Challenge);
    }
    let hits = eng.parse(&body);
    if hits.len() < MIN_HITS {
        // ONE challenge marker is not enough to condemn a page by itself — a
        // real results page for "captcha bypass" carries one. Combined with
        // ZERO parsed results it is conclusive, and no genuine SERP can produce
        // that pair. This is what actually catches Bing's challenge, which
        // carries the single marker `captcha` six times.
        if serp::challenge_markers(&body) >= 1 {
            return Err(Refusal::Challenge);
        }
        return Err(Refusal::Thin);
    }
    // Counting hits used to be the ONLY test, and a portal's navigation bar is
    // reliably more numerous than the floor.
    if !serp::plausible_for_query(&hits, query) {
        return Err(Refusal::OffTopic);
    }
    Ok(hits)
}

/// Tracking parameters that change a URL without changing the page. Two links
/// to the same article counted as two results AND as cross-engine agreement
/// before these came off — the merge was rewarding us for a duplicate.
///
/// `srsltid` is Google's, and it shows up in our OWN shipped output via Ecosia.
const TRACKING_PREFIXES: &[&str] = &["utm_", "ref_"];
const TRACKING_KEYS: &[&str] = &[
    "srsltid", "gclid", "fbclid", "msclkid", "mc_cid", "mc_eid", "igshid", "ref", "spm", "_ga",
    "yclid", "twclid",
];

/// A leading path segment that is a language/locale tag: `en`, `de`, `en-US`,
/// `zh-CN`, `pt-BR`. The bare two-letter form is restricted to a curated set of
/// common web languages so a genuine short segment (`/io/`, `/ai/`) is not
/// mistaken for a locale and two distinct pages wrongly merged; the hyphenated
/// `xx-YY` form is unambiguous enough to accept generically.
fn is_locale_segment(seg: &str) -> bool {
    const COMMON: &[&str] = &[
        "en", "de", "fr", "es", "it", "pt", "nl", "ru", "ja", "zh", "ko", "pl", "tr", "ar", "sv",
        "cs", "da", "fi", "el", "he", "hi", "id", "no", "ro", "uk", "vi", "th",
    ];
    match seg.split_once('-') {
        Some((lang, region)) => {
            lang.len() == 2
                && lang.bytes().all(|c| c.is_ascii_lowercase())
                && (2..=3).contains(&region.len())
                && region.bytes().all(|c| c.is_ascii_alphabetic())
        }
        None => COMMON.contains(&seg.to_ascii_lowercase().as_str()),
    }
}

/// A URL reduced to what actually identifies the page: no fragment, no tracking
/// parameters, no trailing slash, no `amp`/`index` leaf, no locale prefix, no
/// `www.`, params sorted.
pub fn canonical_url(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s, r),
        None => ("https", url),
    };
    let (host_path, query) = match rest.split_once('?') {
        Some((hp, q)) => (hp, Some(q)),
        None => (rest, None),
    };
    // The fragment never reaches the server.
    let host_path = host_path.split('#').next().unwrap_or(host_path);
    let (host, path) = match host_path.split_once('/') {
        Some((h, p)) => (h, p),
        None => (host_path, ""),
    };
    let host = host.trim_start_matches("www.").to_ascii_lowercase();
    let mut path = path.trim_end_matches('/').to_string();
    // An AMP leaf is the same article, and engines disagree about which to
    // return.
    for leaf in ["/amp", "/amp.html"] {
        if let Some(stripped) = path.strip_suffix(leaf) {
            path = stripped.to_string();
        }
    }
    // A default-document leaf identifies the same page as its directory: engines
    // disagree about whether to return `/docs/` or `/docs/index.html`.
    for leaf in ["/index.html", "/index.htm", "/index.php"] {
        if let Some(stripped) = path.strip_suffix(leaf) {
            path = stripped.to_string();
        }
    }
    // A leading locale segment (`/en/`, `/en-US/`) names the same page in a
    // language variant; two engines returning `/en/x` and `/x` are one result.
    // Only stripped when a REAL path segment follows: a bare `/de` or `/fr` is
    // a language HOMEPAGE, and clearing it would merge every language homepage
    // into the root — collapsing `/de` and `/fr` (genuinely different pages)
    // into one and inflating cross-engine agreement on whichever won.
    if let Some((first, rest)) = path.split_once('/') {
        if is_locale_segment(first) {
            path = rest.to_string();
        }
    }
    let mut kept: Vec<&str> = query
        .map(|q| {
            q.split('&')
                .filter(|kv| {
                    let k = kv.split('=').next().unwrap_or("").to_ascii_lowercase();
                    !k.is_empty()
                        && !TRACKING_KEYS.contains(&k.as_str())
                        && !TRACKING_PREFIXES.iter().any(|p| k.starts_with(p))
                })
                .collect()
        })
        .unwrap_or_default();
    // Sort the surviving params so `?a=1&b=2` and `?b=2&a=1` — the same page,
    // ordered differently by two engines — canonicalise identically.
    kept.sort_unstable();
    let mut out = if path.is_empty() {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}/{path}")
    };
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    out
}

/// How much of the query this result visibly answers, in `[0.0, 1.0]`.
///
/// Cross-engine agreement separates the strong from the accidental, but it
/// cannot separate two results both found by two engines. Term coverage can:
/// a page whose title and snippet carry every word of the query is a better
/// answer than one carrying half. Prefix-matched for the same reason
/// `plausible_for_query` is — inflections and compounds must count.
fn term_coverage(r: &serp::Result, needles: &[String]) -> f64 {
    if needles.is_empty() {
        return 0.0;
    }
    let hay = format!("{} {} {}", r.title, r.snippet, r.url).to_lowercase();
    let hit = needles.iter().filter(|n| hay.contains(n.as_str())).count();
    hit as f64 / needles.len() as f64
}

/// Query tokens reduced to comparable prefixes.
fn needles_for(query: &str) -> Vec<String> {
    const MIN_TOKEN: usize = 4;
    const PREFIX: usize = 6;
    query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= MIN_TOKEN)
        .map(|t| t.chars().take(PREFIX).collect::<String>())
        .collect()
}

/// Merge the engines' hits, dedup, and rank.
///
/// Score, highest first:
///   * cross-engine AGREEMENT — independent indexes converging on a URL is the
///     strongest relevance signal available here, and it costs nothing;
///   * query TERM COVERAGE — separates two results that agreement ties;
///   * best NATIVE RANK — a rank of 1 on some index outweighs a rank of 9, and
///     Marginalia even publishes its own score.
/// Ties break on first-seen, so the order is total and stable for identical
/// inputs.
///
/// Then DOMAIN DIVERSITY: no host may take more than [`MAX_PER_HOST`] of the
/// list. One site with a good SEO team could otherwise own the answer.
fn merge_and_rank(per_engine: &[Vec<serp::Result>], query: &str) -> Vec<serp::Result> {
    let needles = needles_for(query);
    // key -> (agreement, best_rank, first_seen, result)
    let mut acc: std::collections::HashMap<String, (usize, usize, usize, serp::Result)> =
        std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for hits in per_engine {
        // Agreement must count DISTINCT engines, not repeats. One engine can
        // return two URLs that canonicalise equal (a result plus its AMP/utm/
        // locale variant), and counting both as agreement lets a single-engine
        // result impersonate a cross-engine-agreed one and jump to the top. Dedup
        // within this engine's list so each engine contributes at most +1.
        let mut seen_this_engine: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for (rank, hit) in hits.iter().enumerate() {
            let key = canonical_url(&hit.url);
            let first_from_engine = seen_this_engine.insert(key.clone());
            match acc.get_mut(&key) {
                Some((agree, best, _, existing)) => {
                    if first_from_engine {
                        *agree += 1;
                    }
                    *best = (*best).min(rank);
                    // Keep the richest description we have seen for this URL.
                    if existing.snippet.len() < hit.snippet.len() {
                        existing.snippet = hit.snippet.clone();
                    }
                    if existing.title.len() < hit.title.len() {
                        existing.title = hit.title.clone();
                    }
                }
                None => {
                    let seq = order.len();
                    order.push(key.clone());
                    acc.insert(key, (1, rank, seq, hit.clone()));
                }
            }
        }
    }
    let mut scored: Vec<(usize, u32, usize, usize, serp::Result)> = order
        .iter()
        .filter_map(|k| acc.remove(k))
        .map(|(agree, best, seq, r)| {
            // Coverage as a coarse integer so the sort stays exact — floats
            // would make the order depend on rounding.
            let cov = (term_coverage(&r, &needles) * 100.0) as u32;
            (agree, cov, best, seq, r)
        })
        .collect();
    scored.sort_by(|x, y| {
        y.0.cmp(&x.0)
            .then(y.1.cmp(&x.1))
            .then(x.2.cmp(&y.2))
            .then(x.3.cmp(&y.3))
    });

    // Domain diversity, applied AFTER scoring so a site's best page is the one
    // that survives rather than whichever came first.
    let mut per_host: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut out = Vec::new();
    let mut overflow = Vec::new();
    // The SAME page under a second address — `doc.rust-lang.org/std/path/`
    // and `…/nightly/std/path/`, same title, same snippet — took one of the
    // ten places (2026-10-03, "rust pathlib equivalent std::path join"). The
    // address cannot say so; the text the engine showed can. The better-ranked
    // copy stays and the next result takes the place.
    //
    // The snippet's OPENING, not all of it: each engine cuts its snippet at
    // its own length, and the same page came back twice with one sentence
    // more on one copy (`/stable/std/path/` and `/std/path/index.html`).
    const SAME_OPENING: usize = 80;
    let squash = |s: &str, n: usize| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
            .chars()
            .take(n)
            .collect::<String>()
    };
    let mut said: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    // …and the same PATH with a query string an engine appended
    // (`?fileGuid=xxQTRXtVcqtHK6j8` on docs.npmjs.com, 2026-10-03) under a
    // title it cut short (`… packages ...`). Both are needed: a query can
    // name a different page, the title says whether it does.
    let mut at_path: Vec<(String, String)> = Vec::new();
    let scored = scored.into_iter().filter(|x| {
        let r = &x.4;
        let path = r
            .url
            .split(['?', '#'])
            .next()
            .unwrap_or(&r.url)
            .trim_end_matches('/')
            .to_lowercase();
        let title = squash(&r.title, usize::MAX);
        let title = title.trim_end_matches(['.', '…', ' ']).to_string();
        let same_page = !title.is_empty()
            && at_path.iter().any(|(p, t)| {
                *p == path
                    && !t.is_empty()
                    && (t.starts_with(&title) || title.starts_with(t.as_str()))
            });
        if same_page {
            return false;
        }
        at_path.push((path, title));
        r.snippet.trim().chars().count() < SAME_OPENING
            || said.insert((
                squash(&r.title, usize::MAX),
                squash(&r.snippet, SAME_OPENING),
            ))
    });
    for (_, _, _, _, r) in scored {
        let host = host_of(&r.url);
        let n = per_host.entry(host).or_insert(0);
        if *n < MAX_PER_HOST {
            *n += 1;
            out.push(r);
        } else {
            overflow.push(r);
        }
        if out.len() >= MAX_HITS {
            break;
        }
    }
    // A thin result set beats a diverse one: if the cap left us short, put the
    // held-back hits back rather than return fewer answers than we found.
    for r in overflow {
        if out.len() >= MIN_HITS.max(out.len()) && out.len() >= MAX_HITS {
            break;
        }
        if out.len() < MIN_HITS {
            out.push(r);
        }
    }
    out.truncate(MAX_HITS);
    out
}

/// Most results one host may contribute.
pub const MAX_PER_HOST: usize = 3;

// ── D4: the ranking bake-off ─────────────────────────────────────────────────
//
// Three ALTERNATIVE formulas, kept as pure functions so Battery 5 can measure
// them against ours (`merge_and_rank`) on the frozen per-engine corpus — engine
// results fixed, only merge/rank varies. "ours" ships unless a challenger holds
// precision@5 AND MRR. The losers' numbers are recorded, not deleted.

mod engines;
mod rank;
mod render;
pub use engines::*;
use rank::*;
pub use render::*;

/// Run a search AND persist the breaker safely across processes.
///
/// The rotation reads a lock-free snapshot (stale health only costs a slightly
/// worse engine pick), the network fan-out runs OUTSIDE any lock, and only the
/// tiny read-merge-write of the touched engines is serialised. This is the entry
/// point both the CLI and the WebSearch hook call, so a wall one agent hits is a
/// wall every other agent then reads.
pub fn run_search(query: &str, now: u64) -> Outcome {
    let mut health = load_health();
    // Round-robin selection with per-engine budget + a cross-process lock held
    // for the whole fetch, so N agents spread across engines and never hit one
    // engine at once or blow its per-window rate limit.
    let locks = select_and_lock(&mut health, now);
    if locks.is_empty() {
        // Nothing eligible — every engine is breaker-open, out of budget this
        // window, or held by another agent. Persist the (possibly reset) windows
        // and signal RATE-LIMITED so the caller falls back to the agent's native
        // search instead of reporting the web as empty.
        let all: Vec<String> = ENGINES.iter().map(|e| e.name().to_string()).collect();
        persist_health_merge(&health, &all);
        return Outcome {
            results: Vec::new(),
            used: Vec::new(),
            refused: vec![("all-engines", Refusal::RateLimited)],
        };
    }
    let engines: Vec<Engine> = locks.iter().map(|g| g.engine()).collect();
    let touched: Vec<String> = engines.iter().map(|e| e.name().to_string()).collect();
    let outcome = fetch_and_update(query, &engines, &mut health, now);
    // Persist the incremented budget + breaker BEFORE releasing the engine
    // locks, so the next selector (this or another process) sees the usage.
    persist_health_merge(&health, &touched);
    drop(locks);
    if !outcome.results.is_empty() {
        return outcome;
    }
    // **Every engine it was given failed: ask the others once.** In a batch of
    // three, the siblings held bing, duckduckgo and yahoo; this query got
    // ecosia and brave, both unreachable, and answered `no usable results`
    // while three engines that WORKED sat one call away (2026-10-03, "how to
    // rebase interactive squash last commits git" — alone, it had results).
    // The locks are released by now, so the siblings' engines can be taken.
    let mut health = load_health();
    let retry: Vec<EngineLock> = select_and_lock(&mut health, now)
        .into_iter()
        .filter(|g| !engines.contains(&g.engine()))
        .collect();
    if retry.is_empty() {
        return outcome;
    }
    let again: Vec<Engine> = retry.iter().map(|g| g.engine()).collect();
    let touched: Vec<String> = again.iter().map(|e| e.name().to_string()).collect();
    let mut second = fetch_and_update(query, &again, &mut health, now);
    persist_health_merge(&health, &touched);
    drop(retry);
    let mut refused = outcome.refused;
    refused.append(&mut second.refused);
    second.refused = refused;
    second
}

#[cfg(test)]
include!("search_tests.rs");
