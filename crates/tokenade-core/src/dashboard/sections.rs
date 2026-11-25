//! **The sections: by compactor, by layer, by project, by agent.**
//!
//! Each answers a different question about the same ledger, and that is
//! why they do not add up to one another. A section that
//! finds nothing SAYS so rather than disappearing — a silent absence
//! reads as a measured zero.

use super::*;

pub(super) fn render_section_heading(s: &mut String, label: &str, p: &Pal<'_>) {
    // Multi-byte glyph (`─` = 3 bytes); slice by char count, never bytes,
    // or the slice cuts inside a UTF-8 sequence and we panic.
    let rule_chars = 60usize.saturating_sub(label.chars().count() + 4);
    let rule: String = "─".repeat(rule_chars);
    let _ = writeln!(
        s,
        "  {}── {}{}{}{} {}{}",
        p.gray(),
        p.bold(),
        label,
        p.rst(),
        p.gray(),
        rule,
        p.rst()
    );
}

pub(super) fn render_top_compactors(
    s: &mut String,
    d: &DashboardData,
    _theme: &Theme,
    p: &Pal<'_>,
) {
    if d.top_compactors.is_empty() {
        return;
    }
    render_section_heading(s, "TOP COMPACTORS", p);
    let max_saved = d
        .top_compactors
        .iter()
        .map(|(_, b, a, _)| b.saturating_sub(*a))
        .max()
        .unwrap_or(1)
        .max(1);
    for (op, b, a, n) in d.top_compactors.iter().take(8) {
        let saved = b.saturating_sub(*a);
        let pct = if *b > 0 {
            100.0 * saved as f64 / *b as f64
        } else {
            0.0
        };
        let bar_w = ((30.0 * saved as f64) / max_saved as f64).round() as usize;
        let bar_w = bar_w.clamp(1, 30);
        let bar = format!(
            "{}{}{}{}{}",
            p.green(),
            "█".repeat(bar_w),
            p.fade(),
            "░".repeat(30 - bar_w),
            p.rst()
        );
        let _ = writeln!(
            s,
            "   {}{:<22}{}  {}  {}{:>7}{}  saved {}{:>7}{}  {}{:>5.1}%{}",
            p.cyan(),
            op.chars().take(22).collect::<String>(),
            p.rst(),
            bar,
            p.dim(),
            format!("{n}×"),
            p.rst(),
            p.green(),
            human_tokens(saved),
            p.rst(),
            p.yellow(),
            pct,
            p.rst()
        );
    }
    let _ = writeln!(s);
}

/// Conflux-inspired BY LAYER rollup (added 2026-06-09): the same measured
/// savings as TOP COMPACTORS, but bucketed by [`op_layer`] so the user reads
/// "where do my savings come from" in one glance. Skipped when empty or when
/// every op collapses to a single layer (no added signal over the per-op list).
pub(super) fn render_by_layer(s: &mut String, d: &DashboardData, _theme: &Theme, p: &Pal<'_>) {
    if d.by_layer.len() < 2 {
        return;
    }
    render_section_heading(s, "BY LAYER", p);
    let max_saved = d
        .by_layer
        .iter()
        .map(|(_, b, a, _)| b.saturating_sub(*a))
        .max()
        .unwrap_or(1)
        .max(1);
    for (layer, b, a, n) in d.by_layer.iter() {
        let saved = b.saturating_sub(*a);
        let pct = if *b > 0 {
            100.0 * saved as f64 / *b as f64
        } else {
            0.0
        };
        let bar_w = ((30.0 * saved as f64) / max_saved as f64).round() as usize;
        let bar_w = bar_w.clamp(1, 30);
        let bar = format!(
            "{}{}{}{}{}",
            p.green(),
            "█".repeat(bar_w),
            p.fade(),
            "░".repeat(30 - bar_w),
            p.rst()
        );
        let _ = writeln!(
            s,
            "   {}{:<14}{}  {}  {}{:>7}{}  saved {}{:>7}{}  {}{:>5.1}%{}",
            p.cyan(),
            layer.chars().take(14).collect::<String>(),
            p.rst(),
            bar,
            p.dim(),
            format!("{n}×"),
            p.rst(),
            p.green(),
            human_tokens(saved),
            p.rst(),
            p.yellow(),
            pct,
            p.rst()
        );
    }
    let _ = writeln!(s);
}

/// #498 — render a dedicated section for wrapped MCPs (third-party
/// servers like github, sqlite, … that the agent invokes through
/// tokenade's mcp-proxy). Same shape as TOP COMPACTORS but isolated
/// so the user sees per-MCP savings as a first-class category rather
/// than mixed in with in-tree compactors. Empty section = no wraps
/// active, so just skip render.
pub(super) fn render_wrapped_mcps(s: &mut String, d: &DashboardData, _theme: &Theme, p: &Pal<'_>) {
    if d.wrapped_mcps.is_empty() {
        return;
    }
    render_section_heading(s, "WRAPPED MCPs (compacted by tokenade)", p);
    let _ = writeln!(
        s,
        "   {}Third-party MCP servers tokenade wraps via `tokenade install`. \
         Each row is the basename derived from the wrap argv.{}",
        p.dim(),
        p.rst()
    );
    let max_saved = d
        .wrapped_mcps
        .iter()
        .map(|(_, b, a, _)| b.saturating_sub(*a))
        .max()
        .unwrap_or(1)
        .max(1);
    for (op, before, after, count) in d.wrapped_mcps.iter().take(15) {
        let saved = before.saturating_sub(*after);
        let pct = if *before > 0 {
            100.0 * (*before as f64 - *after as f64) / *before as f64
        } else {
            0.0
        };
        let bar_width = ((saved as f64 / max_saved as f64) * 30.0).round() as usize;
        let bar_full = "█".repeat(bar_width.max(1));
        let bar_empty = "░".repeat(30usize.saturating_sub(bar_width.max(1)));
        let _ = writeln!(
            s,
            "   {}{:<24}{} {}{}{}{}  {:>5}×  saved {}{:>6}{}   {}{:>5.1}%{}",
            p.cyan(),
            op.chars().take(24).collect::<String>(),
            p.rst(),
            p.green(),
            bar_full,
            p.dim(),
            bar_empty,
            count,
            p.green(),
            human_tokens(saved),
            p.rst(),
            p.yellow(),
            pct,
            p.rst()
        );
    }
    let _ = writeln!(s);
}

pub(super) fn render_projects(s: &mut String, d: &DashboardData, _theme: &Theme, p: &Pal<'_>) {
    if d.by_project.is_empty() {
        return;
    }
    render_section_heading(s, "PROJECTS", p);
    let max_saved = d
        .by_project
        .iter()
        .map(|(_, b, a, _)| b.saturating_sub(*a))
        .max()
        .unwrap_or(1)
        .max(1);
    for (project, b, a, n) in d.by_project.iter().take(6) {
        let saved = b.saturating_sub(*a);
        let bar_w = ((24.0 * saved as f64) / max_saved as f64).round() as usize;
        let bar_w = bar_w.clamp(1, 24);
        let bar = format!(
            "{}{}{}{}{}",
            p.green(),
            "█".repeat(bar_w),
            p.fade(),
            "░".repeat(24 - bar_w),
            p.rst()
        );
        let pct = if *b > 0 {
            100.0 * saved as f64 / *b as f64
        } else {
            0.0
        };
        // #740 — apply ~/ rewrite at render time. by_project key is
        // now the RAW absolute path (was pre-rewritten before #740,
        // which broke the AGENT TOKENS bypass lookup). Display
        // convenience is now the renderer's job.
        let rewritten = match &d.home_dir {
            Some(h) => project
                .strip_prefix(h.as_str())
                .map(|tail| format!("~{tail}"))
                .unwrap_or_else(|| project.clone()),
            None => project.clone(),
        };
        let display = truncate_tail(&rewritten, 36);
        let _ = writeln!(
            s,
            "   {}{:<36}{}  {}  {}{:>4}×{}  {}{:>7}{}  {}{:>5.1}%{}",
            p.cyan(),
            display,
            p.rst(),
            bar,
            p.dim(),
            n,
            p.rst(),
            p.green(),
            human_tokens(saved),
            p.rst(),
            p.yellow(),
            pct,
            p.rst()
        );
    }
    let _ = writeln!(s);
}
