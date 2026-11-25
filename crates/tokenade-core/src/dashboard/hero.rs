//! **The top banner: the number read first.**
//!
//! It is the only part many will look at, so it is where an implausible
//! number does the most damage — one day, « 3.1B tokens saved
//! (=46 688 $) » came from 136 lines for an 800 MB video. The banner also says
//! what the measurement does NOT cover, without which a partial total reads
//! as a total.

use super::*;

pub fn render(d: &DashboardData, theme: &Theme) -> String {
    let p = Pal { t: theme };
    let mut s = String::with_capacity(4096);

    render_header(&mut s, theme, &p);
    render_hero(&mut s, d, theme, &p);
    render_top_compactors(&mut s, d, theme, &p);
    render_by_layer(&mut s, d, theme, &p);
    render_projects(&mut s, d, theme, &p);
    render_wrapped_mcps(&mut s, d, theme, &p);
    // #687 — user request: CODE INTELLIGENCE belongs ABOVE the AGENT
    // TOKENS sections. The mental model is "what tokenade DID help
    // with (compactors → code-intel) first, then what tokenade DID
    // NOT touch (bypass) afterwards" — putting bypass last makes the
    // savings story end on the missed-opportunity surface.
    render_code_intel(&mut s, d, theme, &p);
    render_claude_usage(&mut s, d, theme, &p);
    render_outside_tokenade_functions(&mut s, d, theme, &p);
    render_cache_and_index(&mut s, d, theme, &p);
    render_cache_rewrites(&mut s, d, &p);
    render_footer(&mut s, d, &p);

    s
}

pub(super) fn render_header(s: &mut String, theme: &Theme, p: &Pal<'_>) {
    let logo_w = LOGO[0].chars().count();
    let _ = writeln!(s);
    if theme.width >= logo_w + 2 {
        for line in LOGO {
            let _ = writeln!(s, "  {}{}{}", p.orange(), line, p.rst());
        }
    } else {
        let _ = writeln!(s, "  {}{}{}", p.orange(), LOGO_COMPACT, p.rst());
    }
    let _ = writeln!(
        s,
        "  {}{:^width$}{}",
        p.cyan(),
        "tokenade.net  ·  universal token optimizer for AI coding agents",
        p.rst(),
        width = if theme.width >= logo_w {
            logo_w
        } else {
            LOGO_COMPACT.chars().count()
        },
    );
    let _ = writeln!(s);
}

pub(super) fn render_hero(s: &mut String, d: &DashboardData, _theme: &Theme, p: &Pal<'_>) {
    let saved = d.tokens_before.saturating_sub(d.tokens_after);
    let pct = if d.tokens_before > 0 {
        100.0 * saved as f64 / d.tokens_before as f64
    } else {
        0.0
    };
    let rule = "━".repeat(68);
    let _ = writeln!(s, "  {}{}{}", p.gray(), rule, p.rst());
    let _ = writeln!(
        s,
        "   {}{}{}{}  measured   {}·{}   {}{:.1}%{}   {}·{}   {} ops",
        p.bold(),
        p.green(),
        human_tokens(saved),
        p.rst(),
        p.gray(),
        p.rst(),
        p.yellow(),
        pct,
        p.rst(),
        p.gray(),
        p.rst(),
        d.operations
    );
    // #window — state the scope explicitly so the hero number isn't read as
    // all-time, and keep the cumulative lifetime total visible beside it.
    let _ = writeln!(
        s,
        "   {}{}{}",
        p.dim(),
        if d.window_days > 0 {
            format!(
                "last {} days · lifetime saved: {}",
                d.window_days,
                human_tokens(d.lifetime_saved)
            )
        } else {
            "lifetime (all history) · scope with `--days N`".to_string()
        },
        p.rst(),
    );
    if d.estimated_tokens_saved > 0 {
        let _ = writeln!(
            s,
            "   {}+ {}{}{} estimated (code-intelligence heuristic){}",
            p.dim(),
            p.bold(),
            human_tokens(d.estimated_tokens_saved),
            p.rst(),
            p.rst(),
        );
    }
    if d.dollars_saved > 0.0 {
        let _ = writeln!(
            s,
            // LIFETIME, not the window above (`gain::real_savings_compute_with`
            // reads the whole ledger): said, or it reads as the window's.
            "   {}≈ {}${:.2}{} saved since install, each token priced at the real rate of the model that ran it{}",
            p.dim(),
            p.green(),
            d.dollars_saved,
            p.rst(),
            p.rst(),
        );
    }
    // Security counter — always rendered (a zero is reassurance, not noise):
    // secret values masked before ANY byte reached a model, same window as
    // the token figures above.
    let _ = writeln!(
        s,
        "   {}🔒 {}{}{} secret{} redacted before reaching the model{}",
        p.dim(),
        p.bold(),
        d.secrets_redacted,
        p.rst(),
        if d.secrets_redacted == 1 { "" } else { "s" },
        p.rst(),
    );
    // Name the rule that did most of it. Two secrets of the same shape is a
    // busy week; two hundred from one rule is that rule going wrong, and the
    // count alone reads identically either way.
    if let Some((rule, n)) = &d.secrets_redacted_top_rule {
        let _ = writeln!(s, "   {}   most by `{rule}` ({n}){}", p.gray(), p.rst(),);
    }
    let _ = writeln!(s, "  {}{}{}", p.gray(), rule, p.rst());
    let _ = writeln!(s);

    // First bar — "measured": % of what tokenade ACTUALLY saw, that it
    // removed. Denominator = tokens_before (only the traffic tokenade
    // intercepted). Yellow because this is the achievable-reduction
    // headline.
    let _ = writeln!(
        s,
        "   {}   {}{:>5.1}%{}  {}of tokenade-intercepted traffic{}",
        render_bar(60, pct, p),
        p.yellow(),
        pct,
        p.rst(),
        p.dim(),
        p.rst(),
    );

    // #730 — second bar: % of ALL agent tool output (intercepted +
    // bypassed) that tokenade removed. Smaller than the first bar
    // by definition, but more honest about end-to-end coverage:
    // tells the user "of every tool-result byte your agent saw, this
    // is the fraction tokenade actually shaved off". Useful for
    // diagnosing low-coverage workflows where MOST traffic bypasses.
    //
    // Bypass: per-cwd `tool_result_tokens - tokenade_after_for_cwd`,
    // summed across cwds. When there's no claude_usage data (fresh
    // install, no transcripts), the second bar would equal the first
    // — we skip it to avoid duplicate noise.
    //
    // #739 — must aggregate by cwd BEFORE subtracting. claude_usage
    // is keyed by (agent_label, cwd) — Claude/Codex/Cline can each
    // produce a row for the same cwd. `by_project[cwd].after` is a
    // single shared total. The pre-#739 implementation subtracted
    // tokenade_after once per ROW, so on a 2-agent cwd it cancelled
    // tokenade_after twice → under-counted bypass by (N-1)×tokenade_after
    // per multi-agent cwd. Single-agent workflows weren't affected
    // (the only kind the user runs today), but the math was wrong.
    use std::collections::BTreeMap;
    let mut by_cwd: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for row in &d.claude_usage {
        let entry = by_cwd.entry(row.cwd.as_str()).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(row.tool_result_tokens);
        if entry.1 == 0 {
            // Look up tokenade_after_for_cwd once — independent of
            // which agent ran. by_project is the source of truth.
            entry.1 = d
                .by_project
                .iter()
                .find(|(path, _, _, _)| *path == row.cwd)
                .map(|(_, _, after, _)| *after)
                .unwrap_or(0);
        }
    }
    let total_bypass: u64 = by_cwd
        .values()
        .map(|(tool_result, after)| tool_result.saturating_sub(*after))
        .sum();
    if total_bypass > 0 {
        let total_potential = d.tokens_before + total_bypass;
        let pct_total = if total_potential > 0 {
            100.0 * saved as f64 / total_potential as f64
        } else {
            0.0
        };
        let _ = writeln!(
            s,
            "   {}   {}{:>5.1}%{}  {}of all messages tokens (incl. {} bypassed){}",
            render_bar(60, pct_total, p),
            p.green(),
            pct_total,
            p.rst(),
            p.dim(),
            human_tokens(total_bypass),
            p.rst(),
        );
    }

    // #730 — clarifying note: conversation history isn't included in
    // either denominator. Cache-read tokens (the previous turn's text
    // re-sent on each call) are billed at 10% but they're identical
    // bytes — tokenade has no way to attribute or compact them
    // without breaking the cache. Both bars measure NEW tool output
    // per turn, not cumulative context size.
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "   {}both bars exclude conversation history{}",
        p.dim(),
        p.rst(),
    );
    let _ = writeln!(s);

    let _ = writeln!(
        s,
        "   {}last 7 days: {}{}{} tokens saved{}",
        p.dim(),
        p.bold(),
        human_tokens(d.last_7d_saved),
        p.rst(),
        p.rst()
    );
    let _ = writeln!(s);
}

/// Cache rewrites: what tokenade could NOT save, because the context was
/// re-sent before its cache entry could be reused.
///
/// Deliberately short and last: it is a cost the user controls (how often a
/// scheduled task or a tool loop fires), not something tokenade can fold away.
/// Silent until there is something to say.
pub(super) fn render_cache_rewrites(s: &mut String, d: &DashboardData, p: &Pal<'_>) {
    if d.cache_rewrites.is_empty() {
        return;
    }
    let events: u64 = d.cache_rewrites.iter().map(|(_, n, _)| n).sum();
    let tokens: u64 = d.cache_rewrites.iter().map(|(_, _, t)| t).sum();
    if events == 0 || tokens == 0 {
        return;
    }
    render_section_heading(s, "CACHE REWRITES", p);
    // Name the window. Every other figure on this page is scoped to it, and a
    // count this large reads as "ever" unless it says otherwise.
    let scope = if d.window_days > 0 {
        format!(" in the last {} day(s)", d.window_days)
    } else {
        String::new()
    };
    let _ = writeln!(
        s,
        "   {} rewrite(s){scope}, {} re-sent — context re-billed before its cache \
         entry was reused",
        events,
        human_tokens(tokens)
    );
    for (trigger, n, t) in d.cache_rewrites.iter().take(3) {
        let (what, gap) = trigger.split_once(':').unwrap_or((trigger.as_str(), ""));
        let gap = match gap {
            "lt2m" => "under 2 min apart",
            "lt5m" => "under 5 min apart",
            "lt15m" => "under 15 min apart",
            "lt1h" => "under 1 h apart",
            other => other,
        };
        let _ = writeln!(
            s,
            "     {:<10} {:>5} × {:>9}   {}",
            what,
            n,
            human_tokens(*t),
            gap
        );
    }
    // One actionable sentence, only for the trigger the user can actually move.
    if let Some((trigger, _, t)) = d.cache_rewrites.first() {
        if trigger.starts_with("scheduled") && *t * 2 > tokens {
            let _ = writeln!(
                s,
                "   Most of it is a scheduled task firing again before the previous \
                 context went cold. Spacing it out is the whole fix."
            );
        }
    }
}
