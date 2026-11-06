//! The one-line status the agent's UI shows.
//!
//! Different constraints from the dashboard: it is rendered on every prompt,
//! so the dollar figure comes from a cached hot path and the refresh runs
//! detached — never on the line the user is waiting for.

use super::*;

mod totals;
pub(crate) use totals::*;

/// `tokenade statusline` — one-line summary for `~/.claude/settings.json` `statusLine.command`.
///
/// Designed to be cheap: one read of `.tokenade/gain.jsonl`, one read of the cache
/// stats dir (already memory-mapped if present). Output: a single ANSI-free
/// line under ~80 chars, e.g.
///
///   `[tokenade] saved 12.4k tok · 88% · $1.27`
///
/// Flags:
///   `--json`     emit JSON instead (for non-Claude statusline consumers)
///   `--model X`  use a non-default model for the USD figure
/// Session-scoped WEIGHTED gains for the live statusline ticker.
/// Claude Code pipes a JSON context on stdin (`session_id`, `model`,
/// `workspace`); the first tick stamps the session start, later ticks sum
/// the ledgers since that stamp and weight them at price ratios (the same
/// plain-"tokens" unit as every dashboard). Ledger rows carry no session
/// id, so the window is time-scoped — concurrent sessions on one machine
/// share the ticker, which is fine for a glanceable indicator.
/// The session's saving: weighted tokens (the display unit), and its value in
/// dollars — each row priced by `gain::row_usd`, the rule the lifetime figure
/// uses (it once was the weighted total × the input rate, which skipped the
/// conversion to the model's own tokens: ×1.6 on Claude).
pub(super) fn statusline_session_weighted(session_id: &str, model: &str) -> (u64, f64) {
    let skey: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(40)
        .collect();
    let Some(home) = tokenade_core::platform::home_dir() else {
        return (0, 0.0);
    };
    let sessions = home.join(".tokenade").join("sessions");
    let _ = std::fs::create_dir_all(&sessions);
    // Incremental per-session tally (audits A+B):
    //  - OFFSET-based: each tick parses only the bytes APPENDED since the
    //    last tick (the ledgers are append-only between rotations) — the
    //    previous full re-parse burned CPU proportional to a 10 MiB ledger
    //    at ~3 Hz.
    //  - MONOTONE: a rotation (file shrinks / offset beyond EOF) resets the
    //    offset but keeps the accumulated tally — the session figure can
    //    never shrink because ANOTHER session pushed the global ledger over
    //    the rotation threshold (the #1042 class, audit A critical).
    //  - Same inclusion rules as the lifetime figure: code-intel heuristic
    //    rows excluded, 48 h time bound on BOTH ledgers.
    let cache_path = sessions.join(format!("{skey}-sl-tally.json"));
    let st: serde_json::Value = std::fs::read_to_string(&cache_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    // A tally written before `usd` existed is read again from the start, once:
    // its dollars cannot be rebuilt from the token buckets.
    let st = if st.get("usd").is_some() {
        st
    } else {
        serde_json::json!({"start": st["start"]})
    };
    let mut usd = st["usd"].as_f64().unwrap_or(0.0);
    let now = tokenade_core::platform::now_unix_secs();
    let start = st["start"].as_u64().unwrap_or(now);
    let cutoff = start.saturating_sub(48 * 3600);
    let mut measured = st["measured"].as_u64().unwrap_or(0);
    let mut est = tokenade_core::gain::EstimatedSummary {
        output: st["e_out"].as_u64().unwrap_or(0),
        batching: st["e_bat"].as_u64().unwrap_or(0),
        compounding: st["e_comp"].as_u64().unwrap_or(0),
        overhead: st["e_ovh"].as_u64().unwrap_or(0),
        overhead_cache: st["e_ovc"].as_u64().unwrap_or(0),
        recovery: st["e_rec"].as_u64().unwrap_or(0),
        weighted: None,
    };
    // Weighted twins (each row at its model's ratios). A tally written before
    // they existed carries only the raw buckets: seed the twins from those at
    // the default ratios, then every new row adds its own model's weight.
    est.weighted = Some(
        match (
            st["w_out"].as_f64(),
            st["w_cc"].as_f64(),
            st["w_ovc"].as_f64(),
        ) {
            (Some(output), Some(cache_credit), Some(overhead_cache)) => {
                tokenade_core::gain::WeightedBuckets {
                    output,
                    cache_credit,
                    overhead_cache,
                }
            }
            _ => est.weighted_or_default(),
        },
    );
    let mut heads: Vec<(String, String)> = Vec::new();
    let mut scan = |path: std::path::PathBuf, off_key: &str, estimated: bool| {
        let Ok(mut f) = std::fs::File::open(&path) else {
            return 0u64;
        };
        use std::io::{Read as _, Seek as _};
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let off = st[off_key].as_u64().unwrap_or(0);
        // Regrowth race (migration audit — the reporter grew this guard in
        // #960, the tally never did): a rotation followed by enough appends
        // to exceed the old offset passes the `off > len` check while the
        // bytes at `off` belong to a DIFFERENT file generation. Fingerprint
        // the first 64 bytes; mismatch = rotation = same banked-EOF policy.
        let head_key = format!("{off_key}_head");
        let mut head = [0u8; 64];
        let head_n = f.read(&mut head).unwrap_or(0);
        let head_hash = tokenade_core::dedup_ref::hash_content(&head[..head_n]);
        let prev_head = st[head_key.as_str()].as_str().unwrap_or("");
        let rotated = off > len || (!prev_head.is_empty() && prev_head != head_hash);
        heads.push((head_key, head_hash.clone()));
        if rotated {
            // Rotation: the tally is BANKED; surviving rows were already
            // counted — jump to EOF and only count future appends (a reset
            // to 0 double-counted survivors: hermetic lab tick4).
            return len;
        }
        // Fresh cache: seed with ONE full scan — the sess filter already
        // scopes it to THIS session, so skipping history zeroed a
        // mid-flight session at migration/cache-loss (owner report). Paid
        // once (~tens of ms), then every tick is incremental.
        let _ = f.seek(std::io::SeekFrom::Start(off));
        let mut buf = String::new();
        if f.read_to_string(&mut buf).is_err() {
            return len;
        }
        // A torn last line (writer mid-append) re-reads next tick: only
        // advance past the final newline.
        let complete = buf.rfind('\n').map(|i| i + 1).unwrap_or(0);
        for line in buf[..complete].lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if v["sess"].as_str() != Some(skey.as_str()) || v["ts"].as_u64().unwrap_or(0) < cutoff {
                continue;
            }
            // The estimated ledger is bounded by the SAME rule the full
            // dashboard applies (`gain::estimated_saved_of_row`) — a
            // statusline that disagrees with `tokenade gain` about the same
            // rows is how the two surfaces drifted 51M tokens apart before.
            let saved = if estimated {
                tokenade_core::gain::estimated_saved_of_row(&v)
            } else {
                v["saved"].as_u64().unwrap_or_else(|| {
                    v["before"]
                        .as_u64()
                        .unwrap_or(0)
                        .saturating_sub(v["after"].as_u64().unwrap_or(0))
                })
            };
            let row_model = v["model"]
                .as_str()
                .filter(|m| !m.is_empty())
                .unwrap_or(model);
            if estimated {
                est.add_row(&v, saved);
                usd += tokenade_core::gain::row_usd(&v, row_model);
            } else if !tokenade_core::code_intel_heuristic::is_code_intel(
                v["op"].as_str().unwrap_or(""),
            ) {
                measured += saved;
                usd += tokenade_core::gain::row_usd(&v, row_model);
            }
        }
        off + complete as u64
    };
    let g_off = scan(global_gain_path(), "g_off", false);
    let e_off = scan(
        home.join(".tokenade").join("gain_estimated.jsonl"),
        "e_off",
        true,
    );
    let mut cache = serde_json::json!({
        "start": start, "g_off": g_off, "e_off": e_off,
        "measured": measured, "e_out": est.output, "e_bat": est.batching,
        "e_comp": est.compounding, "e_ovh": est.overhead, "e_ovc": est.overhead_cache, "e_rec": est.recovery,
        "usd": usd,
    });
    let w = est.weighted_or_default();
    cache["w_out"] = serde_json::json!(w.output);
    cache["w_cc"] = serde_json::json!(w.cache_credit);
    cache["w_ovc"] = serde_json::json!(w.overhead_cache);
    for (k, h) in heads {
        cache[k] = serde_json::Value::from(h);
    }
    let _ = std::fs::write(&cache_path, cache.to_string());
    (
        tokenade_core::gain::weighted_from_parts(measured, 0, &est),
        usd.max(0.0),
    )
}

pub(crate) fn flatten_osc8(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("\u{1b}]8;;") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "\u{1b}]8;;".len()..];
        // URL runs to the string terminator (ESC \ or BEL).
        // WHICHEVER COMES FIRST, not ST-then-BEL: a BEL-terminated link ahead of
        // an ST-terminated one would otherwise have its url run all the way to
        // the later ST, swallowing the label and the second link whole.
        let url_end = {
            let st = after.find("\u{1b}\\");
            let bel = after.find('\u{7}');
            match (st, bel) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
        };
        let Some(url_end) = url_end else {
            // Malformed (a torn escape): keep it verbatim rather than eat it.
            // `rest[..start]` is already out, so only the remainder goes back.
            out.push_str(&rest[start..]);
            return out;
        };
        let url = &after[..url_end];
        let term_len = if after[url_end..].starts_with('\u{7}') {
            1
        } else {
            2
        };
        let body = &after[url_end + term_len..];
        // The closing sequence is an OSC 8 with an EMPTY url.
        let (label, tail) = match body.find("\u{1b}]8;;") {
            Some(i) => {
                let close = &body[i..];
                // First terminator wins, same reason as the opener above: an
                // ST hunted before a BEL runs the closing sequence past a
                // BEL-ended link and eats whatever follows it.
                let st = close.find("\u{1b}\\").map(|j| (j, j + 2));
                let bel = close.find('\u{7}').map(|j| (j, j + 1));
                let close_len = match (st, bel) {
                    (Some(a), Some(b)) => {
                        if a.0 <= b.0 {
                            a.1
                        } else {
                            b.1
                        }
                    }
                    (Some(a), None) => a.1,
                    (None, Some(b)) => b.1,
                    (None, None) => close.len(),
                };
                (&body[..i], &body[i + close_len..])
            }
            None => (body, ""),
        };
        // A label that is already the url in short form (`github.com/pi-infected/tokenade`
        // for `https://github.com/pi-infected/tokenade`) must not be printed twice.
        if url.is_empty() || (!label.is_empty() && url.contains(label)) {
            out.push_str(label);
        } else {
            out.push_str(label);
            out.push_str(" (");
            out.push_str(url);
            out.push(')');
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

pub(crate) fn cmd_statusline(rest: &[String]) -> i32 {
    // Deliberately NOT flag-validated, unlike its siblings. The caller here is
    // Claude Code's status-line hook, not a person: it passes whatever flags
    // its own version knows, and refusing one we have not heard of would blank
    // the user's status line on an agent upgrade. An ignored flag costs
    // nothing here — the command renders either way.
    use tokenade_core::pricing;
    // Config subcommands. These never collide with Claude Code's render
    // call, which passes stdin JSON + flags (`--json`, `--wrap-b64 …`),
    // never a bare `off`/`on`/`status` positional.
    match rest.first().map(|s| s.as_str()) {
        Some("off") | Some("disable") | Some("on") | Some("enable") => {
            let enable = matches!(rest[0].as_str(), "on" | "enable");
            let home = tokenade_core::platform::home_dir();
            return match crate::install::set_statusline_enabled(home.as_deref(), enable) {
                Ok(msg) => {
                    println!("{msg}");
                    0
                }
                Err(e) => {
                    eprintln!("statusline: {e}");
                    1
                }
            };
        }
        Some("status") => {
            let home = tokenade_core::platform::home_dir();
            let off = crate::install::statusline_opted_out(home.as_deref());
            println!(
                "tokenade statusline ticker: {}",
                if off { "OFF (opted out)" } else { "ON" }
            );
            return 0;
        }
        _ => {}
    }
    let json = rest.iter().any(|a| a == "--json");
    // `--plain`: no OSC 8 escapes, links flattened to `label (url)`. For a host
    // that shows our text somewhere other than a terminal line — an opencode
    // TUI toast, a log — where an escape sequence is either printed literally
    // or eats the URL along with itself.
    let plain = rest.iter().any(|a| a == "--plain");
    // Claude Code statusline protocol: a JSON context arrives on stdin.
    // `--wrap-b64 <cmd>` = the user's ORIGINAL statusline command (base64,
    // quoting-proof): run it with the same stdin, keep ALL its lines, and
    // put our session ticker on a line of its own below them.
    let wrap_b64 = rest
        .iter()
        .position(|a| a == "--wrap-b64")
        .and_then(|i| rest.get(i + 1))
        .cloned();
    let mut stdin_buf = String::new();
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        use std::io::Read as _;
        let _ = std::io::stdin().read_to_string(&mut stdin_buf);
    }
    let ctx: serde_json::Value = serde_json::from_str(&stdin_buf).unwrap_or_default();
    let session_id = ctx["session_id"].as_str().unwrap_or("");
    // The statusline is the ONLY surface that is told which model is running:
    // the host hands it `model.id` every turn. Hook payloads do not carry one
    // — measured 2026-08-25, zero model ids across 25 939 lines of
    // `debug.log` — so the compounding horizon fell back to a conservative
    // 200 000-token window on every session, including 1M ones. Harmless while
    // `delivered` read 0 and the horizon was barely consulted; now that
    // the counterfactual is real, being wrong by 5× on the window is wrong by
    // 5× on when a session is judged to have compacted.
    //
    // Recorded HERE, before any rendering branch. It first sat next to the
    // pricing lookup, which is below the "zero saved this session prints
    // nothing" early return — so the one moment it mattered most, a session
    // young enough to have saved nothing yet, was exactly the one it never
    // reached.
    crate::hooks::session_lifecycle::note_session_model(
        session_id,
        ctx["model"]["id"]
            .as_str()
            .or_else(|| ctx["model"]["display_name"].as_str())
            .unwrap_or(""),
    );
    if !session_id.is_empty() && !json {
        let (weighted, session_usd) = statusline_session_weighted(
            session_id,
            ctx["model"]["id"]
                .as_str()
                .or_else(|| ctx["model"]["display_name"].as_str())
                .unwrap_or(""),
        );
        // Zero saved this session (2026-07-11): a "0 tokens saved" ticker
        // is noise — render only the user's original statusline if we wrapped
        // one, otherwise print nothing at all.
        if weighted == 0 {
            if let Some(b64) = wrap_b64 {
                let left = run_original_statusline(&b64, &stdin_buf);
                if !left.trim().is_empty() {
                    println!("{left}");
                }
            }
            return 0;
        }
        // Lifetime measured via the ROLLUP-AWARE accumulator (audit A
        // critical: raw-file totals shrank when rotation moved old rows
        // into gain.rollup.jsonl — the #1042 class on a permanent surface).
        let est_all = tokenade_core::gain::estimated_summary();
        let lt_measured = tokenade_core::gain::lifetime_saved().unwrap_or_else(|| {
            let (b, a) = statusline_totals(&global_gain_path());
            b.saturating_sub(a)
        });
        let lt_weighted = tokenade_core::gain::weighted_from_parts(lt_measured, 0, &est_all);
        // $ = the REAL per-model value; token counts stay weighted (the one
        // honest display unit). SESSION: each row priced as the lifetime
        // figure prices it (`gain::row_usd`, in the model's own tokens).
        // LIFETIME: spans every agent/model ever used, so it comes from the
        // shared per-(model,io) valuation — the SAME figure the terminal +
        // HTML dashboards and the server report — read from a short-TTL cache
        // to stay off the hot path.
        let fmt_usd = |v: f64| {
            if v >= 100.0 {
                format!("{v:.0}")
            } else {
                format!("{v:.2}")
            }
        };
        let usd_session = fmt_usd(session_usd);
        let usd_lifetime = fmt_usd(real_savings_usd_hotpath());
        // System-language ticker. `{s}`/`{S}` session tok/$, `{t}`/`{T}`
        // lifetime tok/$. Currency sign position follows each locale's
        // convention (suffix for most European locales, prefix for en/zh/ja/ko).
        let lang = tokenade_core::platform::system_lang();
        let tpl = match lang.as_str() {
            "fr" => "⚡ {s} tokens sauvés sur cette session (={S}$) | {t} tokens sauvés au total (={T}$)",
            "de" => "⚡ {s} Tokens in dieser Sitzung gespart (={S}$) | {t} Tokens insgesamt gespart (={T}$)",
            "es" => "⚡ {s} tokens ahorrados en esta sesión (={S}$) | {t} tokens ahorrados en total (={T}$)",
            "it" => "⚡ {s} token risparmiati in questa sessione (={S}$) | {t} token risparmiati in totale (={T}$)",
            "pt" => "⚡ {s} tokens poupados nesta sessão (={S}$) | {t} tokens poupados no total (={T}$)",
            "nl" => "⚡ {s} tokens bespaard deze sessie (={S}$) | {t} tokens totaal bespaard (={T}$)",
            "pl" => "⚡ {s} tokenów zaoszczędzonych w tej sesji (={S}$) | {t} tokenów łącznie (={T}$)",
            "ru" => "⚡ {s} токенов сэкономлено за сессию (={S}$) | {t} токенов всего (={T}$)",
            "ja" => "⚡ このセッションで {s} トークン節約 (=${S}) | 合計 {t} トークン (=${T})",
            "zh" => "⚡ 本次会话节省 {s} 个 token (=${S}) | 累计节省 {t} 个 (=${T})",
            "ko" => "⚡ 이번 세션에서 {s} 토큰 절약 (=${S}) | 총 {t} 토큰 (=${T})",
            _ => "⚡ {s} tokens saved this session (=${S}) | {t} saved all-time (=${T})",
        };
        // Lifetime with 2 decimals above 1M: 0.1M steps made the total
        // look FROZEN next to a k-scale session figure (owner report).
        // Above 1000M switch to `B`: "3155.77M" is arithmetic the reader has to
        // do, and a four-digit M reads as a glitch rather than a big number.
        let lt_disp = if lt_weighted >= 1_000_000_000 {
            format!("{:.2}B", lt_weighted as f64 / 1e9)
        } else if lt_weighted >= 1_000_000 {
            format!("{:.2}M", lt_weighted as f64 / 1e6)
        } else {
            human_tokens(lt_weighted)
        };
        let ticker = tpl
            .replace("{s}", &human_tokens(weighted))
            .replace("{S}", &usd_session)
            .replace("{t}", &lt_disp)
            .replace("{T}", &usd_lifetime);
        let mut wrapped_someone_else = false;
        let left = if let Some(b64) = wrap_b64 {
            let theirs = run_original_statusline(&b64, &stdin_buf);
            wrapped_someone_else = !theirs.trim().is_empty();
            theirs
        } else {
            // No prior statusline: mimic Claude Code's default-ish left part.
            let model = ctx["model"]["display_name"].as_str().unwrap_or("");
            let dir = ctx["workspace"]["current_dir"]
                .as_str()
                .map(|d| {
                    std::path::Path::new(d)
                        .file_name()
                        .map(|b| b.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            match (model.is_empty(), dir.is_empty()) {
                (false, false) => format!("{model} · {dir}"),
                (false, true) => model.to_string(),
                (true, false) => dir,
                (true, true) => String::new(),
            }
        };
        // No width fitting, and that is a measurement rather than an omission.
        // The hosts WRAP: Cursor renders the line into an Ink `<Box>` with no
        // width constraint (Ink's default is `wrap`), and Copilot's runner
        // splits the command's output on newlines and pads each one — it
        // deliberately supports a MULTI-LINE status line. A version of this
        // that dropped the lifetime total on a narrow terminal was written and
        // thrown away: it removed a figure the host would simply have wrapped.
        // Our ticker goes on its OWN line when it is sharing the status line
        // with a command the user already had. Reported by a user running
        // another tool's KPI line: the two collapse into one run-on line, and
        // the hosts do support several — Copilot's runner splits the output on
        // newlines and pads each, Cursor renders into an unconstrained Ink box,
        // Claude Code prints what it is given. When the left part is OUR OWN
        // synthesised `model · dir` (no wrapped command), one line is still
        // right: nothing is competing for it.
        let mut line = match (left.is_empty(), wrapped_someone_else) {
            (true, _) => ticker,
            (false, true) => format!("{left}\n{ticker}"),
            (false, false) => format!("{left}  {ticker}"),
        };
        if plain {
            line = flatten_osc8(&line);
        }
        println!("{line}");
        return 0;
    }
    let model_arg = rest
        .iter()
        .position(|a| a == "--model")
        .and_then(|i| rest.get(i + 1));
    let model = model_arg
        .and_then(|m| pricing::find(m))
        .unwrap_or_else(pricing::default_model);

    let path = global_gain_path();
    let (total_before, total_after) = statusline_totals(&path);
    let saved = total_before.saturating_sub(total_after);
    let pct = if total_before > 0 {
        100.0 * saved as f64 / total_before as f64
    } else {
        0.0
    };
    // Real per-model lifetime value — the same figure the ticker path and
    // the dashboards show, not a flat rate on this window's raw `saved`.
    let usd = real_savings_usd_hotpath();

    if json {
        let out = serde_json::json!({
            "saved_tokens": saved,
            "tokens_before": total_before,
            "tokens_after": total_after,
            "percent": pct,
            "usd": usd,
            "model": model.id,
        });
        println!("{out}");
    } else {
        // Compact human form. Use SI suffix for token count so the line stays
        // short across long-lived sessions.
        // #523 — was `[STS]` from the pre-rename era; user-visible
        // statusline now reads `[tokenade]` to match the binary name.
        let mut line = format!(
            "[tokenade] saved {} tok · {:.0}% · ${:.2}",
            human_tokens(saved),
            pct,
            usd
        );
        if plain {
            line = flatten_osc8(&line);
        }
        println!("{line}");
    }
    0
}

/// Run the user's original statusline command (base64-decoded) with the
/// same stdin JSON Claude Code gave us; all its output lines = their text.
/// Any failure degrades to empty (our ticker still renders) — never let
/// a broken user command kill the whole statusline.
pub(super) fn run_original_statusline(b64: &str, stdin_json: &str) -> String {
    use base64::Engine as _;
    let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(b64) else {
        return String::new();
    };
    let Ok(cmd) = String::from_utf8(raw) else {
        return String::new();
    };
    // The host's shell, not `cmd /C` (#10): Claude Code runs this line through
    // Git Bash on Windows, and `cmd` given a Rust-quoted line with its own
    // quotes ran nothing — the user's status line vanished without a word.
    let mut sh_cmd = tokenade_core::process_util::user_shell(&cmd);
    sh_cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Every stream is piped or nulled, so no console is ever needed — and this
    // is the hottest spawn the product has: the host re-runs the status line on
    // each render (Cursor throttles to ~3/s while streaming). Without this, a
    // Windows user with no console of their own gets a `cmd.exe` window opening
    // and closing several times a second.
    tokenade_core::process_util::configure_no_window(&mut sh_cmd);
    let mut child = match sh_cmd.spawn() {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    if let Some(mut si) = child.stdin.take() {
        use std::io::Write as _;
        let _ = si.write_all(stdin_json.as_bytes());
    }
    // Audit B: no timeout froze the tick behind a slow/blocking user
    // command, leaking an orphan per tick. Poll up to 1.5 s, then kill —
    // the ticker renders without the user part rather than never.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return String::new();
            }
        }
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(_) => return String::new(),
    };
    let text = original_statusline_text(&String::from_utf8_lossy(&out.stdout));
    if text.trim().is_empty() && !out.status.success() {
        return failed_statusline_hint(&cmd, &String::from_utf8_lossy(&out.stderr));
    }
    text
}

/// A wrapped status line that FAILED says so, in its own place, instead of
/// disappearing: silence read as "tokenade ate my status line" (#10).
pub(super) fn failed_statusline_hint(cmd: &str, stderr: &str) -> String {
    let why = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no output");
    let clip = |s: &str, n: usize| -> String {
        if s.chars().count() > n {
            format!("{}…", s.chars().take(n).collect::<String>())
        } else {
            s.to_string()
        }
    };
    format!(
        "⚠ status line `{}` failed: {}",
        clip(cmd, 40),
        clip(why, 80)
    )
}

/// EVERY line of the user's statusline, not the first. Keeping only line 1
/// put our ticker right under it and dropped the rest: a user with a
/// multi-line custom status line saw the ticker land in the middle of it and
/// its lower lines vanish. Trailing blank lines are dropped so the ticker sits
/// directly under their last line.
pub(super) fn original_statusline_text(stdout: &str) -> String {
    let lines: Vec<&str> = stdout.lines().map(str::trim_end).collect();
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(0, |i| i + 1);
    lines[..end].join("\n")
}

#[cfg(test)]
include!("statusline_tests.rs");
