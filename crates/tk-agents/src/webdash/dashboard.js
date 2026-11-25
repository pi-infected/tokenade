"use strict";
// Tokenade offline dashboard.
//
// ONE rule drives the whole file: the period selector is the only truth about
// what is on screen. Every series in the payload — measured days, estimated
// accounting, redactions, per-model usage, per-tool bypass, compounding — is
// filtered through the SAME `inWindow(date)` predicate, and every panel prints
// the window it used. The previous version windowed some series against the
// measured ledger's day list, which meant a day with agent activity but no
// tokenade op simply did not exist for the "outside tokenade" and model
// panels; changing the range moved some numbers and left others frozen, with
// nothing on screen saying which was which. Anything that genuinely cannot be
// windowed (the activity map, the coverage notes) now says "lifetime" on its
// own panel instead of quietly ignoring the control.

const D = JSON.parse(document.getElementById("data").textContent);
const $ = (id) => document.getElementById(id);

// ---- i18n ---------------------------------------------------------------
// Both languages ship inside this static file — it must keep working offline
// with zero external requests, so no locale files are fetched.
const I18N = {
en: {
  range_7:"7d", range_30:"30d", range_90:"90d", range_all:"All",
  gran_day:"Day", gran_week:"Week", gran_month:"Month",
  hook_on:"hook active", hook_off:"hook not installed",
  window_line:"Showing %from → %to (%n days) · %ops ops intercepted · generated %at",
  window_all:"Showing all history: %from → %to (%n days) · %ops ops intercepted · generated %at",
  window_drill:"Drilled into %x — click the bar again, or pick a range, to go back.",

  hero_k:"API-$ equivalent saved", hero_k_life:"API-$ equivalent saved (lifetime)",
  hero_t:"Every token saved, priced at the rate of the model that ran it, by kind — fresh input, cache read, output: folded output, re-reads spared until compaction, turns saved by batching and shorter answers; minus what tokenade itself adds to the conversation, its re-reads, and the output it hands back when a fold is reopened. Measured where the ledger or transcript records it; the rest is estimated on the low side.",
  hero_sub:"<b>%tok</b> tokens saved over <b>%ops</b> ops — input %i · cache %c · output %o",

  gauge_k:"intercepted", gauge_n:"%b → %a", gauge_n2:"tool output tokenade actually saw",
  bill_k:"share of agent bill", bill_n:"of everything your agents consumed",
  bill_na:"no transcript data in this window",

  kpi_saved:"tokens saved", kpi_saved_s:"measured, %n ops",
  kpi_est:"+ estimated", kpi_est_s:"code-intelligence heuristic",
  kpi_weighted:"weighted tokens",
  kpi_weighted_s:"priced across classes",
  kpi_weighted_t:"Weighted at each model's own price ratios (Claude Opus: 1 output token = 5 input = 50 cache-read; every model at tokenade.net/model-weights). Raw counts aren't comparable across kinds; this total is what the saving costs.",
  kpi_saved_t:"Tool output before minus after compaction, summed over every operation tokenade ran in this window (local ledger). Counted with a cl100k-style estimator; priced in each model's own tokens at 1.6 per estimated token for Claude (measured 1.57 on local transcripts, middle half 1.36–1.77).",
  kpi_est_t:"Code-intelligence calls (skeleton, impact, query…) against the file reads they replace — a heuristic, not a measurement.",
  kpi_cache:"future cache saved", kpi_cache_s:"re-reads spared until the next compaction",
  kpi_cache_t:"Each folded token × the API calls that would have re-read it, until the conversation was compacted — or would have been compacted earlier without tokenade, whichever comes first. Claude Code and Codex: counted on the calls and compactions their transcripts record. Inside a subagent (own context, shared session): 9 re-reads, half its measured p25 of calls. Other agents: estimated from measured averages (6.4 calls per message, 1,429 tokens of context growth per call), on the low side. Before tokenade's own added context, whose re-reads are subtracted in the $ total.",
  kpi_redact:"secrets redacted", kpi_redact_s:"masked before reaching the model",
  kpi_lost:"lost to bypasses", kpi_lost_s:"%n raw runs · NOT counted in savings",
  kpi_lost_t:"Tokens your agent paid because the tokenade hook was bypassed (tokenade raw / TOKENADE_HOOK_DISABLED) on output a compactor could have shrunk. Tracked in a separate ledger; never added to any savings figure.",

  chart_h:"With vs without tokenade",
  chart_sub:"Full bar = what your agents would have consumed. The blue part is what they actually consumed after compaction. Click a bar to drill into that period.",
  leg_spent:"spent (with tokenade)", leg_saved:"saved (measured)",
  leg_est:"saved (code-intel estimate)", leg_cum:"cumulative saved",

  break_h:"Breakdown",
  tab_ops:"Functions", tab_layers:"Layers", tab_agents:"Agents",
  tab_projects:"Projects", tab_models:"Models", tab_bypass:"Outside tokenade",
  sub_ops:"Compactors and tools, by tokens saved in this window.",
  sub_layers:"Where the savings come from: shell, file reads, auto-detected content, web, wrapped MCPs.",
  sub_agents:"Which AI agent the intercepted traffic came from.",
  sub_projects:"Per working directory, by tokens saved in this window.",
  sub_models:"Token consumption per model, from local transcripts of every agent that records the model name. Input counts fresh content only; re-sent history (cache reads) is excluded.",
  sub_bypass:"Tool output your agents consumed that never routed through tokenade, minus tokenade's own output for the same days.",

  heat_h:"Activity",
  heat_sub:"Tokens saved per day over the last 26 weeks. Click a day to drill into it.",
  scope_life:"lifetime",
  coverage_h:"Agents we can't (fully) measure",
  coverage_sub:"Everything here comes from transcripts agents leave on this machine. When an agent doesn't write something down it is missing, which means “not measurable”, not “zero usage”.",

  th_name:"name", th_saved:"saved", th_rate:"rate", th_calls:"calls",
  th_model:"model", th_input:"input", th_output:"output", th_msgs:"msgs",
  th_tool:"tool", th_tokens:"tokens",
  col_function:"function", col_layer:"layer", col_agent:"agent", col_project:"project",
  est_abbr:"est.",

  empty_window:"Nothing recorded between %from and %to.",
  empty_hint:"Widen the range, or run an agent and regenerate.",
  empty_all:"show all history",
  empty_panel:"nothing in this window",

  privacy:"100% local — this page is a static file, no network calls, nothing leaves this machine.",
  copy_btn:"copy summary", copied:"copied ✓",
  foot_index:"semantic index: %d docs · %t terms",
  foot_est:"“est.” rows are heuristic estimates for code-intelligence tools whose savings can't be measured directly.",
  foot_regen:"regenerate anytime with <code>tokenade dashboard</code>",
  vs_prev:"vs prev",
  summary:"tokenade — %win: %s tokens saved (%p) ≈ $%usd across %n ops",
},
fr: {
  range_7:"7j", range_30:"30j", range_90:"90j", range_all:"Tout",
  gran_day:"Jour", gran_week:"Semaine", gran_month:"Mois",
  hook_on:"hook actif", hook_off:"hook non installé",
  window_line:"Affichage %from → %to (%n jours) · %ops ops interceptées · généré %at",
  window_all:"Tout l'historique : %from → %to (%n jours) · %ops ops interceptées · généré %at",
  window_drill:"Zoom sur %x — recliquez la barre, ou choisissez une plage, pour revenir.",

  hero_k:"équivalent $ API économisé", hero_k_life:"équivalent $ API économisé (vie entière)",
  hero_t:"Chaque jeton économisé, au tarif du modèle qui l'a traité, par nature — entrée neuve, lecture de cache, sortie : sortie pliée, relectures évitées jusqu'à la compaction, tours évités par le regroupement et réponses plus courtes ; moins ce que tokenade ajoute lui-même à la conversation, ses relectures, et ce qu'il rend quand un pli est rouvert. Mesuré là où le registre ou le transcript l'enregistre ; le reste est estimé en bas de fourchette.",
  hero_sub:"<b>%tok</b> tokens économisés sur <b>%ops</b> ops — entrée %i · cache %c · sortie %o",

  gauge_k:"intercepté", gauge_n:"%b → %a", gauge_n2:"sorties d'outils vues par tokenade",
  bill_k:"part de la facture", bill_n:"de tout ce que vos agents ont consommé",
  bill_na:"aucune donnée de transcript sur cette période",

  kpi_saved:"tokens économisés", kpi_saved_s:"mesuré, %n ops",
  kpi_est:"+ estimé", kpi_est_s:"heuristique code-intelligence",
  kpi_weighted:"tokens pondérés",
  kpi_weighted_s:"valorisé par classe de prix",
  kpi_weighted_t:"Pondéré aux ratios de prix de chaque modèle (Claude Opus : 1 token de sortie = 5 d'entrée = 50 cache-read ; tous les modèles sur tokenade.net/fr/model-weights). Les volumes bruts ne sont pas comparables entre classes ; ce total reflète ce que l'économie vaut.",
  kpi_saved_t:"Sortie d'outil avant moins après compaction, sommée sur chaque opération de tokenade dans la période (registre local). Comptée avec un estimateur de type cl100k ; valorisée en jetons de chaque modèle, 1,6 par jeton estimé pour Claude (mesuré 1,57 sur les transcripts locaux, moitié centrale 1,36–1,77).",
  kpi_est_t:"Appels d'intelligence de code (skeleton, impact, query…) contre les lectures de fichiers qu'ils remplacent — une heuristique, pas une mesure.",
  kpi_cache:"cache futur", kpi_cache_s:"relectures évitées jusqu'à la compaction",
  kpi_cache_t:"Chaque jeton plié × les appels API qui l'auraient relu, jusqu'à la compaction de la conversation — ou jusqu'à celle qui serait arrivée plus tôt sans tokenade, la première des deux. Claude Code et Codex : compté sur les appels et compactions qu'enregistrent leurs transcripts. Dans un sous-agent (contexte propre, session partagée) : 9 relectures, la moitié de son p25 d'appels mesuré. Autres agents : estimé par des moyennes mesurées (6,4 appels par message, 1 429 jetons de croissance du contexte par appel), en bas de fourchette. Avant le coût des relectures de ce que tokenade ajoute, déduit du total en $.",
  kpi_redact:"secrets caviardés", kpi_redact_s:"masqués avant d'atteindre le modèle",
  kpi_lost:"perdu en bypass", kpi_lost_s:"%n exécutions brutes · PAS compté dans les économies",
  kpi_lost_t:"Tokens payés par votre agent parce que le hook tokenade a été contourné (tokenade raw / TOKENADE_HOOK_DISABLED) sur une sortie qu'un compacteur aurait pu réduire. Registre séparé ; jamais ajouté aux chiffres d'économies.",

  chart_h:"Avec vs sans tokenade",
  chart_sub:"La barre entière est ce que vos agents auraient consommé. La partie bleue est ce qu'ils ont réellement consommé après compaction. Cliquez une barre pour zoomer sur la période.",
  leg_spent:"dépensé (avec tokenade)", leg_saved:"économisé (mesuré)",
  leg_est:"économisé (estimation code-intel)", leg_cum:"cumul économisé",

  break_h:"Détail",
  tab_ops:"Fonctions", tab_layers:"Couches", tab_agents:"Agents",
  tab_projects:"Projets", tab_models:"Modèles", tab_bypass:"Hors tokenade",
  sub_ops:"Compacteurs et outils, par tokens économisés sur cette période.",
  sub_layers:"D'où viennent les économies : shell, lectures de fichiers, contenu auto-détecté, web, MCP encapsulés.",
  sub_agents:"De quel agent IA venait le trafic intercepté.",
  sub_projects:"Par répertoire de travail, par tokens économisés sur cette période.",
  sub_models:"Consommation par modèle, d'après les transcripts locaux de chaque agent qui enregistre le nom du modèle. L'entrée ne compte que le contenu nouveau ; l'historique renvoyé (lectures de cache) est exclu.",
  sub_bypass:"Sorties d'outils consommées par vos agents sans passer par tokenade, moins la sortie propre de tokenade sur les mêmes jours.",

  heat_h:"Activité",
  heat_sub:"Tokens économisés par jour sur les 26 dernières semaines. Cliquez un jour pour zoomer.",
  scope_life:"vie entière",
  coverage_h:"Agents impossibles à mesurer (entièrement)",
  coverage_sub:"Tout ce qui précède vient des transcripts que les agents laissent sur cette machine. Quand un agent n'écrit rien, il manque ici : ça veut dire “non mesurable”, pas “zéro usage”.",

  th_name:"nom", th_saved:"économisé", th_rate:"taux", th_calls:"appels",
  th_model:"modèle", th_input:"entrée", th_output:"sortie", th_msgs:"msgs",
  th_tool:"outil", th_tokens:"tokens",
  col_function:"fonction", col_layer:"couche", col_agent:"agent", col_project:"projet",
  est_abbr:"estim.",

  empty_window:"Rien d'enregistré entre le %from et le %to.",
  empty_hint:"Élargissez la plage, ou lancez un agent puis régénérez.",
  empty_all:"voir tout l'historique",
  empty_panel:"rien sur cette période",

  privacy:"100% local — cette page est un fichier statique, aucun appel réseau, rien ne quitte cette machine.",
  copy_btn:"copier le résumé", copied:"copié ✓",
  foot_index:"index sémantique : %d docs · %t termes",
  foot_est:"les lignes “estim.” sont des estimations heuristiques pour les outils de code-intelligence dont les économies ne sont pas mesurables directement.",
  foot_regen:"régénérez à tout moment avec <code>tokenade dashboard</code>",
  vs_prev:"vs préc.",
  summary:"tokenade — %win : %s tokens économisés (%p) ≈ %usd $ sur %n ops",
},
};
// Language follows the browser, and only the browser. There was an EN/FR
// toggle here with a localStorage preference behind it; a page regenerated
// from the command line is not a place anyone goes to change a language
// setting, and a remembered choice made the page disagree with the browser it
// was opened in. Anything that isn't French gets English.
const LANG = ((navigator.language || navigator.userLanguage || "")
  .toLowerCase().startsWith("fr") && I18N.fr) ? "fr" : "en";
const t = (k) => I18N[LANG][k] ?? I18N.en[k] ?? k;
const tf = (k, vars) => Object.entries(vars).reduce(
  (s, [n, v]) => s.replaceAll("%" + n, v), t(k));
function applyI18n(){
  document.documentElement.lang = LANG;
  for (const el of document.querySelectorAll("[data-i18n]")) el.innerHTML = t(el.dataset.i18n);
}

// ---- formatting ---------------------------------------------------------
const AGENT_NAMES = {
  "claude-code":"Claude Code", codex:"Codex", cline:"Cline", copilot:"Copilot",
  "gemini-cli":"Gemini CLI", cursor:"Cursor", grok:"Grok", windsurf:"Windsurf", "devin-desktop":"Devin Desktop",
  opencode:"OpenCode", kilocode:"Kilo Code", aider:"Aider", hermes:"Hermes",
  odysseus:"Odysseus", openclaw:"OpenClaw", antigravity:"Antigravity", unknown:"Unknown",
};
const agentName = (id) => AGENT_NAMES[id] || id;

function fmt(n){
  n = Math.round(n || 0);
  if (n < 0) return "−" + fmt(-n);
  if (n >= 1e9) return (n/1e9).toFixed(2)+"B";
  if (n >= 1e6) return (n/1e6).toFixed(2)+"M";
  if (n >= 1e3) return (n/1e3).toFixed(1)+"k";
  return String(n);
}
const money = (v) => v >= 1000 ? Math.round(v).toLocaleString()
  : v >= 100 ? v.toFixed(0) : v.toFixed(2);
function esc(s){
  return String(s).replace(/[&<>"]/g, (c)=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;"}[c]));
}
const toDate = (d) => new Date(d + "T00:00:00Z");
const dkey = (dt) => dt.toISOString().slice(0,10);
const shiftDay = (d, n) => { const x = toDate(d); x.setUTCDate(x.getUTCDate()+n); return dkey(x); };

function bucketKey(d, g){
  if (g === "day") return d;
  if (g === "month") return d.slice(0,7);
  const dt = toDate(d);
  dt.setUTCDate(dt.getUTCDate() - ((dt.getUTCDay() + 6) % 7));
  return dkey(dt);
}
function bucketLabel(k, g){
  if (g === "month") return k;
  if (g === "week") return k.slice(5);
  return k.slice(5);
}

// ---- state + the single window ------------------------------------------
let gran = "day";
let rangeDays = 30;
let selected = null;   // bucket key, or null = the whole range

const TODAY = dkey(new Date(D.generated_ts * 1000));
// Every dated series in the payload, so "all time" starts at the FIRST day
// anything was recorded and not at the first day the measured ledger has.
const FIRST_DAY = (() => {
  let first = TODAY;
  const eat = (d) => { if (d && d < first) first = d; };
  for (const x of D.days) eat(x.d);
  for (const src of [D.est_by_day, D.redact_by_day, D.models_by_day,
                     D.bypass_by_day, D.compounding_by_day, D.cache_creation_by_day])
    for (const d in (src || {})) eat(d);
  return first;
})();

// {from, to} — inclusive ISO days. THE window; nothing else computes one.
function windowRange(){
  if (!rangeDays) return {from: FIRST_DAY, to: TODAY};
  return {from: shiftDay(TODAY, -(rangeDays - 1)), to: TODAY};
}
let W = windowRange();
// A date is on screen when it is inside the range AND, if the user drilled
// into a bar, inside that bucket. Undated rows (legacy rollups carry no day)
// only count when nothing is being windowed at all.
function inWindow(d){
  if (!d) return isLifetime();
  if (d < W.from || d > W.to) return false;
  return !selected || bucketKey(d, gran) === selected;
}
const isLifetime = () => !rangeDays && !selected;
const windowDayCount = () =>
  Math.round((toDate(W.to) - toDate(W.from)) / 86400000) + 1;

const rangeLabel = () => selected ? bucketLabel(selected, gran)
  : (rangeDays ? `${W.from} → ${W.to}` : `${FIRST_DAY} → ${TODAY}`);

// ---- slicing ------------------------------------------------------------
function selectionDays(){ return D.days.filter((x) => inWindow(x.d)); }
function sumByDay(map){
  let n = 0;
  for (const [d, v] of Object.entries(map || {})) if (inWindow(d)) n += v;
  return n;
}
function mergeMaps(days, field){
  const out = {};
  for (const day of days)
    for (const [k, v] of Object.entries(day[field] || {})){
      const e = out[k] || (out[k] = [0,0,0,v[3]||0]);
      e[0]+=v[0]; e[1]+=v[1]; e[2]+=v[2];
    }
  return out;
}
function buckets(){
  const m = new Map();
  // Seed every bucket the window covers, including the empty ones. A ledger
  // is sparse — this install has 29 days of rows spread over three months —
  // and plotting only the days that exist drew the same handful of bars
  // whatever range was picked, which is most of why the selector looked
  // broken. An empty day is a fact about the week; it gets a slot.
  if (!selected){
    let d = W.from;
    let guard = 0;
    while (d <= W.to && guard++ < 4000){
      const k = bucketKey(d, gran);
      if (!m.has(k)) m.set(k, {k, b:0, a:0, est:0, n:0});
      d = shiftDay(d, 1);
    }
  }
  for (const day of selectionDays()){
    const k = bucketKey(day.d, gran);
    const b = m.get(k) || {k, b:0, a:0, est:0, n:0};
    b.b += day.b; b.a += day.a; b.est += day.est; b.n += day.n;
    m.set(k, b);
  }
  return [...m.values()].sort((x,y)=> x.k < y.k ? -1 : 1);
}
function periodTotals(days){
  let b=0,a=0,est=0,n=0,lost=0,lost_n=0;
  for (const d of days){
    b+=d.b; a+=d.a; est+=d.est; n+=d.n; lost+=d.lost||0; lost_n+=d.lost_n||0;
  }
  return {b, a, est, n, lost, lost_n, saved: Math.max(0, b-a)};
}

// Total tokens the agents actually consumed in this window — every message
// from the local transcripts, not only the ops tokenade intercepted. Cache
// WRITES are subtracted: re-caching the conversation is context tokenade
// structurally can't compact, so leaving it in makes the bill share
// meaningless. 0 = no transcript data for these days.
function consumedTotal(){
  if (!D.models_by_day) return 0;
  const cc = D.cache_creation_by_day || {};
  let tot = 0;
  for (const [d, models] of Object.entries(D.models_by_day)){
    if (!inWindow(d)) continue;
    for (const v of Object.values(models)) tot += v[0] + v[1];
    tot -= (cc[d] || 0);
  }
  return Math.max(0, tot);
}

// The windowed 3-way accounting — SAME buckets, formula and rates as the
// terminal "Savings accounting" block: input = measured + batching + recovery
// − overhead, cache = compounding − overhead re-bills, output = output-lean
// (+ the cross-agent estimate, which only exists lifetime).
// `w` = the same buckets already weighted at each row's model ratios
// (est_by_day [5..7], `EstimatedSummary::add_row`); absent on an older
// payload, where the default ratios apply.
const weightedTok = (inp, cache, out, w) => Math.round(
  w ? w.input + w.cache + w.out : inp + cache / 10 + out * 5);
function accounting(saved){
  if (!D.io_split || !D.est_by_day) return null;
  let eo = 0, eb = 0, ec = 0, ov = 0, wo = 0, wc = 0, weighted = true;
  for (const [date, v] of Object.entries(D.est_by_day)){
    if (!inWindow(date)) continue;
    eo += v[0]; eb += v[1]; ec += v[2]; ov += v[3]; ec -= v[4] || 0;
    if (v.length < 8) weighted = false;
    else { wo += v[5]; wc += v[6] - v[7]; }
  }
  // Batching avoids re-reading a CACHED prefix: a cache saving, like the
  // re-reads; the bucket stays signed so a re-bill larger than both is a cost
  // in the dollar figure, not a silent zero (`EstimatedSummary::by_io`).
  ec += eb;
  const cross = isLifetime() ? (D.est_cross || 0) : 0;
  const p = D.io_split.prices;
  const input = Math.max(0, saved - ov);
  const output = eo + cross;
  // Weighted twins: batching rides the cache-read bucket there (as in
  // `gain::weighted_from_parts`), so it leaves `input`; the cross-agent
  // estimate has no model and keeps the default output ratio.
  const w = weighted ? {input: Math.max(0, saved - ov), out: wo + cross * 5, cache: wc} : null;
  return {input, cache: ec, output, p, w,
          usd: input/1e6*p.input + ec/1e6*p.cache + output/1e6*p.output};
}

// Days of the comparable previous period: the bucket before the drilled one,
// or the same-length window immediately before this one. null = no meaningful
// previous (all-time, or the first bucket).
function prevPeriodTotals(){
  if (selected){
    const bs = buckets();
    const i = bs.findIndex((x) => x.k === selected);
    if (i <= 0) return null;
    return {saved: Math.max(0, bs[i-1].b - bs[i-1].a), est: bs[i-1].est};
  }
  if (!rangeDays) return null;
  const hi = shiftDay(W.from, -1), lo = shiftDay(W.from, -rangeDays);
  const d = D.days.filter((x) => x.d >= lo && x.d <= hi);
  if (!d.length) return null;
  const p = periodTotals(d);
  return {saved: p.saved, est: p.est};
}
function deltaHtml(cur, prev){
  if (!prev || prev <= 0) return "";
  const pc = 100 * (cur - prev) / prev;
  const cls = Math.abs(pc) < 1 ? "flat" : (pc > 0 ? "up" : "down");
  const arrow = cls === "flat" ? "=" : (pc > 0 ? "▲" : "▼");
  return `<span class="delta ${cls}">${arrow} ${Math.abs(pc).toFixed(0)}% ${t("vs_prev")}</span>`;
}

// ---- header -------------------------------------------------------------
function renderHookBadge(){
  const el = $("hookbadge");
  if (D.hook_installed === true){ el.className = "pill ok"; el.innerHTML = `<i class="dot"></i>${t("hook_on")}`; }
  else if (D.hook_installed === false){ el.className = "pill off"; el.innerHTML = `<i class="dot"></i>${t("hook_off")}`; }
  else { el.className = "pill"; el.textContent = ""; }
}
function renderWindowLine(){
  const tot = periodTotals(selectionDays());
  const base = tf(rangeDays ? "window_line" : "window_all", {
    from: W.from, to: W.to, n: windowDayCount(), ops: tot.n, at: D.generated_at,
  });
  $("windowline").innerHTML = esc(base)
    + (selected ? ` <span class="pos">${esc(tf("window_drill", {x: bucketLabel(selected, gran)}))}</span>` : "");
}

// ---- hero ---------------------------------------------------------------
function renderHero(){
  const tot = periodTotals(selectionDays());
  const acc = accounting(tot.saved);
  const prev = prevPeriodTotals();

  $("hero-label").textContent = t(isLifetime() ? "hero_k_life" : "hero_k");
  $("hero-label").title = t("hero_t");
  const usd = acc ? acc.usd : 0;
  $("hero-value").innerHTML = `<span class="cur">$</span>${money(usd)}`;
  $("hero-sub").innerHTML = tf("hero_sub", {
    tok: fmt(tot.saved + tot.est), ops: tot.n,
    i: acc ? fmt(acc.input) : "—",
    c: acc ? fmt(acc.cache) : "—",
    o: acc ? fmt(acc.output) : "—",
  }) + deltaHtml(tot.saved + tot.est, prev ? prev.saved + prev.est : 0);

  // Sparkline of cumulative savings across the window's buckets.
  const bs = buckets();
  const svg = $("hero-spark");
  if (bs.length < 2){ svg.innerHTML = ""; }
  else {
    const H = 74, Wd = 400;
    let acc2 = 0;
    const pts = bs.map((b) => (acc2 += Math.max(0, b.b - b.a) + b.est));
    const max = pts[pts.length-1] || 1;
    const X = (i) => i * Wd / (pts.length - 1);
    const Y = (v) => H - 4 - (H - 10) * v / max;
    const line = pts.map((v,i) => `${X(i).toFixed(1)},${Y(v).toFixed(1)}`).join(" ");
    svg.setAttribute("viewBox", `0 0 ${Wd} ${H}`);
    svg.innerHTML =
      `<polygon points="0,${H} ${line} ${Wd},${H}" fill="var(--save)" opacity="0.16"/>` +
      `<polyline points="${line}" fill="none" stroke="var(--save)" stroke-width="2"/>`;
  }

  // Gauge = compaction rate on what tokenade actually intercepted.
  const rate = tot.b > 0 ? tot.saved / tot.b : 0;
  const R = 44, C = 2 * Math.PI * R;
  $("gauge").innerHTML =
    `<circle cx="60" cy="60" r="${R}" fill="none" stroke="var(--surface-3)" stroke-width="11"/>` +
    `<circle cx="60" cy="60" r="${R}" fill="none" stroke="var(--save)" stroke-width="11"
       stroke-linecap="round" stroke-dasharray="${(C*rate).toFixed(1)} ${C.toFixed(1)}"
       transform="rotate(-90 60 60)"/>` +
    `<text x="60" y="67" text-anchor="middle" font-size="24" font-weight="700"
       fill="var(--fg)">${tot.b > 0 ? (100*rate).toFixed(0)+"%" : "—"}</text>`;

  const consumed = consumedTotal();
  const wouldHave = consumed > 0 ? consumed + tot.saved : 0;
  const share = wouldHave > 0 ? (100 * tot.saved / wouldHave) : null;
  $("gauge-facts").innerHTML = `
    <div class="fact"><span class="k">${t("gauge_k")}</span>
      <span class="v">${tf("gauge_n", {b: fmt(tot.b), a: fmt(tot.a)})}</span>
      <span class="n">${t("gauge_n2")}</span></div>
    <div class="fact"><span class="k">${t("bill_k")}</span>
      <span class="v">${share === null ? "—" : share.toFixed(1)+"%"}</span>
      <span class="n">${share === null ? t("bill_na") : t("bill_n")}</span></div>`;
}

// ---- KPI strip ----------------------------------------------------------
function renderKpis(){
  const tot = periodTotals(selectionDays());
  const acc = accounting(tot.saved);
  const prev = prevPeriodTotals();
  const compounding = sumByDay(D.compounding_by_day);
  const redacted = (() => {
    let n = 0;
    for (const [d, v] of Object.entries(D.redact_by_day || {})) if (inWindow(d)) n += v;
    return n;
  })();

  const tile = (cls, k, v, s, title) =>
    `<div class="kpi ${cls}"${title ? ` title="${esc(title)}"` : ""}>
       <div class="k">${k}</div><div class="v">${v}</div><div class="s">${s}</div></div>`;

  let html = "";
  html += tile("good", t("kpi_saved"), fmt(tot.saved),
    tf("kpi_saved_s", {n: tot.n}) + deltaHtml(tot.saved, prev ? prev.saved : 0), t("kpi_saved_t"));
  html += tile("", t("kpi_est"), fmt(tot.est), t("kpi_est_s"), t("kpi_est_t"));
  if (acc)
    html += tile("", t("kpi_weighted"),
      fmt(weightedTok(acc.input, acc.cache, acc.output, acc.w)), t("kpi_weighted_s"),
      t("kpi_weighted_t"));
  if (compounding > 0)
    html += tile("good", t("kpi_cache"), fmt(compounding), t("kpi_cache_s"), t("kpi_cache_t"));
  if (D.redact_by_day)
    html += tile("", t("kpi_redact"), fmt(redacted), t("kpi_redact_s"));
  if (tot.lost > 0)
    html += tile("bad", t("kpi_lost"), fmt(tot.lost),
      tf("kpi_lost_s", {n: tot.lost_n}), t("kpi_lost_t"));
  $("kpis").innerHTML = html;
}

// ---- main chart ---------------------------------------------------------
const tip = () => $("tip");
function showTip(e, html){
  const el = tip();
  el.style.display = "block";
  el.innerHTML = html;
  const w = el.offsetWidth || 220;
  el.style.left = Math.max(8, Math.min(e.clientX + 14, innerWidth - w - 10)) + "px";
  el.style.top = Math.min(e.clientY + 16, innerHeight - el.offsetHeight - 10) + "px";
}
const hideTip = () => { tip().style.display = "none"; };

function renderChart(){
  const svg = $("chart");
  $("chart-scope").textContent = rangeLabel();
  const bs = buckets();
  const Wd = svg.clientWidth || 1040, H = 280, padB = 22, padT = 8;
  svg.setAttribute("viewBox", `0 0 ${Wd} ${H}`);
  if (!bs.length){ svg.innerHTML = ""; return; }

  const max = Math.max(...bs.map((x)=> x.b + x.est), 1);
  const slot = (Wd - 8) / bs.length;
  const bw = Math.min(56, slot * 0.72);
  const sy = (v) => (H - padB - padT) * v / max;
  const cx = (i) => 4 + slot * i + slot / 2;

  let out = "";
  bs.forEach((bk, i) => {
    const x = cx(i) - bw/2;
    const hSpent = sy(bk.a), hSaved = sy(Math.max(0, bk.b - bk.a)), hEst = sy(bk.est);
    let y = H - padB;
    out += `<g class="bucket${selected === bk.k ? " sel" : ""}" data-k="${bk.k}" data-i="${i}">`;
    out += `<rect class="hit" x="${4 + slot*i}" y="${padT-4}" width="${slot}" height="${H-padB-padT+4}" rx="6"/>`;
    y -= hSpent;
    out += `<rect x="${x}" y="${y}" width="${bw}" height="${hSpent}" fill="var(--spend)" rx="2"/>`;
    y -= hSaved;
    out += `<rect x="${x}" y="${y}" width="${bw}" height="${hSaved}" fill="var(--save)" rx="2"/>`;
    if (hEst > 0.6){
      y -= hEst;
      out += `<rect x="${x}" y="${y}" width="${bw}" height="${hEst}" fill="var(--save)" opacity="0.42" rx="2"/>`;
    }
    if (bs.length <= 46)
      out += `<text x="${cx(i)}" y="${H-6}" fill="var(--fg-3)" font-size="10"
        text-anchor="middle">${esc(bucketLabel(bk.k, gran))}</text>`;
    out += "</g>";
  });

  // Cumulative saved, drawn over the bars on its own scale.
  if (bs.length > 1){
    let a = 0;
    const pts = bs.map((b) => (a += Math.max(0, b.b - b.a) + b.est));
    const cmax = pts[pts.length-1] || 1;
    const line = pts.map((v,i) =>
      `${cx(i).toFixed(1)},${(H - padB - (H-padB-padT) * v / cmax * 0.92).toFixed(1)}`).join(" ");
    out += `<polyline id="cumline" points="${line}"/>`;
  }
  svg.innerHTML = out;

  svg.querySelectorAll("g.bucket").forEach((g) => {
    const bk = bs[+g.dataset.i];
    g.addEventListener("mousemove", (e) => {
      const saved = Math.max(0, bk.b - bk.a);
      showTip(e, `<div class="t">${esc(bucketLabel(bk.k, gran))} · ${bk.n} ops</div>
        <div class="row"><span>${t("leg_spent")}</span><b>${fmt(bk.a)}</b></div>
        <div class="row"><span>${t("leg_saved")}</span><b class="pos">${fmt(saved)}</b></div>
        ${bk.est ? `<div class="row"><span>${t("leg_est")}</span><b>${fmt(bk.est)}</b></div>` : ""}`);
    });
    g.addEventListener("mouseleave", hideTip);
    g.addEventListener("click", () => {
      selected = selected === g.dataset.k ? null : g.dataset.k;
      renderAll();
    });
  });
}

// ---- heatmap (lifetime, says so on its panel) ---------------------------
function renderHeat(){
  const svg = $("heat");
  const byDay = {};
  for (const d of D.days) byDay[d.d] = Math.max(0, d.b - d.a) + d.est;
  const end = new Date(D.generated_ts * 1000);
  const start = new Date(end);
  start.setUTCDate(start.getUTCDate() - 7*26 + 1);
  start.setUTCDate(start.getUTCDate() - (start.getUTCDay() + 6) % 7);
  const cell = 12, step = 15, padT = 16;
  const days = [];
  for (const c = new Date(start); c <= end; c.setUTCDate(c.getUTCDate() + 1)) days.push(dkey(c));
  const Wd = Math.ceil(days.length / 7) * step, H = padT + 7 * step;
  svg.setAttribute("viewBox", `0 0 ${Wd} ${H}`);
  const max = Math.max(1, ...days.map((d) => byDay[d] || 0));
  const dark = matchMedia("(prefers-color-scheme: dark)").matches;
  const colors = dark
    ? ["#1b2230", "#10402f", "#177f56", "#22b37a", "#34d399"]
    : ["#eceff3", "#cbead9", "#84cfae", "#3aab7d", "#0f9d58"];
  let out = "", lastMonth = "";
  days.forEach((d, i) => {
    const wk = Math.floor(i / 7), row = i % 7;
    const v = byDay[d] || 0;
    const lvl = v === 0 ? 0 : 1 + Math.min(3, Math.floor(3 * Math.sqrt(v / max)));
    if (row === 0 && d.slice(0, 7) !== lastMonth){
      lastMonth = d.slice(0, 7);
      out += `<text x="${wk*step}" y="10" fill="var(--fg-3)" font-size="9">${d.slice(5, 7)}</text>`;
    }
    out += `<rect data-d="${d}" data-v="${v}" x="${wk*step}" y="${padT + row*step}"
      width="${cell}" height="${cell}" rx="2.5" fill="${colors[lvl]}"/>`;
  });
  svg.innerHTML = out;
  svg.querySelectorAll("rect").forEach((r) => {
    r.addEventListener("mousemove", (e) => showTip(e,
      `<div class="t">${r.dataset.d}</div>
       <div class="row"><span>${t("leg_saved")}</span><b class="pos">${fmt(+r.dataset.v)}</b></div>`));
    r.addEventListener("mouseleave", hideTip);
    r.addEventListener("click", () => {
      gran = "day"; rangeDays = 0; selected = r.dataset.d;
      syncSegs();
      renderAll();
    });
  });
}

// ---- lazy scrolling tables ----------------------------------------------
// The full data set lives in a scrollable box: the first CHUNK rows render
// immediately, the rest stream in on scroll. Everything is already in this
// file — batching only caps DOM size.
const CHUNK = 50;
function lazyTable(host, headHtml, rows, rowHtml){
  host.classList.add("scrolly");
  host.innerHTML = `<table><thead>${headHtml}</thead><tbody></tbody></table>`;
  const tb = host.querySelector("tbody");
  let i = 0;
  const feed = () => {
    const end = Math.min(rows.length, i + CHUNK);
    let html = "";
    for (; i < end; i++) html += rowHtml(rows[i]);
    tb.insertAdjacentHTML("beforeend", html);
  };
  feed();
  host.onscroll = i >= rows.length ? null : () => {
    if (i < rows.length && host.scrollTop + host.clientHeight >= host.scrollHeight - 140) feed();
  };
}
function emptyPanel(host){
  host.classList.remove("scrolly");
  host.onscroll = null;
  host.innerHTML = `<div class="empty">${t("empty_panel")}</div>`;
}
const track = (pct, cls) =>
  `<div class="track"><i class="${cls||""}" style="width:${Math.max(2, pct).toFixed(1)}%"></i></div>`;

// ---- breakdown tabs -----------------------------------------------------
// Six panels became one, because they answer the same question sliced six
// ways and stacking them was most of the noise.
const TABS = [
  {id:"ops",      label:"tab_ops",      sub:"sub_ops"},
  {id:"layers",   label:"tab_layers",   sub:"sub_layers"},
  {id:"agents",   label:"tab_agents",   sub:"sub_agents"},
  {id:"projects", label:"tab_projects", sub:"sub_projects"},
  {id:"models",   label:"tab_models",   sub:"sub_models"},
  {id:"bypass",   label:"tab_bypass",   sub:"sub_bypass"},
];
let tab = localStorage.getItem("tokenade-tab") || "ops";
if (!TABS.some((x) => x.id === tab)) tab = "ops";

// Only a path INSIDE the home folder shortens: "/home/al2/x" is not under
// "/home/al", so the prefix must end on a separator (either one, Windows too).
function shortProject(p){
  const h = D.home;
  if (!h || !p.startsWith(h)) return p;
  const rest = p.slice(h.length);
  return rest === "" || rest[0] === "/" || rest[0] === "\\" ? "~" + rest : p;
}
function savingsRows(map){
  return Object.entries(map)
    .map(([k, v]) => ({k, b:v[0], a:v[1], n:v[2], ci:!!v[3], saved:Math.max(0, v[0]-v[1])}))
    .filter((r) => r.saved > 0)
    .sort((x,y) => y.saved - x.saved);
}
function renderSavingsTable(host, rows, nameFn, col){
  if (!rows.length) return emptyPanel(host);
  const max = rows[0].saved || 1;
  const head = `<tr><th>${col}</th><th></th><th class="num">${t("th_saved")}</th>
    <th class="num">${t("th_rate")}</th><th class="num">${t("th_calls")}</th></tr>`;
  lazyTable(host, head, rows, (r) => `<tr>
      <td class="name" title="${esc(nameFn(r.k))}">${esc(nameFn(r.k))}${r.ci ? `<span class="tag">${t("est_abbr")}</span>` : ""}</td>
      <td class="bar">${track(100*r.saved/max, r.ci ? "est" : "")}</td>
      <td class="num pos">${fmt(r.saved)}</td>
      <td class="num">${r.b > 0 ? (100*r.saved/r.b).toFixed(0)+"%" : "—"}</td>
      <td class="num">${r.n}</td></tr>`);
}
function modelRows(){
  const agg = {};
  for (const [d, models] of Object.entries(D.models_by_day || {})){
    if (!inWindow(d)) continue;
    for (const [m, v] of Object.entries(models)){
      const e = agg[m] || (agg[m] = [0,0,0]);
      e[0]+=v[0]; e[1]+=v[1]; e[2]+=v[2];
    }
  }
  return Object.entries(agg).map(([k, v]) => ({k, inp:v[0], out:v[1], n:v[2]}))
    .sort((x, y) => (y.inp + y.out) - (x.inp + x.out));
}
function bypassRows(){
  const byDay = D.bypass_by_day || {}, afterByDay = D.tokenade_after_by_day || {};
  const outside = {};
  for (const d of Object.keys(byDay)){
    if (!inWindow(d)) continue;
    const bt = byDay[d] || {}, it = afterByDay[d] || {};
    for (const tool in bt){
      // Per-day max(0,…): a day of heavy tokenade activity must not lend
      // negative slack to another day.
      const net = Math.max(0, bt[tool] - (it[tool] || 0));
      if (net > 0) outside[tool] = (outside[tool] || 0) + net;
    }
  }
  return Object.entries(outside).map(([k, n]) => ({k, n})).sort((a, b) => b.n - a.n);
}

function tabCount(id){
  const days = selectionDays();
  if (id === "models") return modelRows().length;
  if (id === "bypass") return bypassRows().length;
  return savingsRows(mergeMaps(days, id)).length;
}
function renderTabs(){
  $("tabs").innerHTML = TABS.map((x) =>
    `<button data-t="${x.id}" class="${x.id === tab ? "on" : ""}">${t(x.label)}<span class="count">${tabCount(x.id)}</span></button>`
  ).join("");
  $("tabs").querySelectorAll("button").forEach((b) =>
    b.addEventListener("click", () => {
      tab = b.dataset.t;
      localStorage.setItem("tokenade-tab", tab);
      renderTabs(); renderTabBody();
    }));
}
function renderTabBody(){
  const host = $("tabtable"), days = selectionDays();
  $("tab-scope").textContent = rangeLabel();
  $("tab-sub").textContent = t(TABS.find((x) => x.id === tab).sub);
  if (tab === "models"){
    const rows = modelRows();
    if (!rows.length) return emptyPanel(host);
    const max = rows[0].inp + rows[0].out || 1;
    const head = `<tr><th>${t("th_model")}</th><th></th><th class="num">${t("th_input")}</th>
      <th class="num">${t("th_output")}</th><th class="num">${t("th_msgs")}</th></tr>`;
    return lazyTable(host, head, rows, (r) => `<tr>
      <td class="name" title="${esc(r.k)}">${esc(r.k)}</td>
      <td class="bar">${track(100*(r.inp + r.out)/max, "spend")}</td>
      <td class="num">${fmt(r.inp)}</td><td class="num">${fmt(r.out)}</td>
      <td class="num">${r.n}</td></tr>`);
  }
  if (tab === "bypass"){
    const rows = bypassRows();
    if (!rows.length) return emptyPanel(host);
    const max = rows[0].n || 1;
    const head = `<tr><th>${t("th_tool")}</th><th></th><th class="num">${t("th_tokens")}</th></tr>`;
    return lazyTable(host, head, rows, (r) => `<tr>
      <td class="name" title="${esc(r.k)}">${esc(r.k)}</td>
      <td class="bar">${track(100*r.n/max, "warn")}</td>
      <td class="num">${fmt(r.n)}</td></tr>`);
  }
  const nameFn = tab === "agents" ? agentName : tab === "projects" ? shortProject : (k) => k;
  const col = t(tab === "ops" ? "col_function" : tab === "layers" ? "col_layer"
    : tab === "agents" ? "col_agent" : "col_project");
  renderSavingsTable(host, savingsRows(mergeMaps(days, tab)), nameFn, col);
}

// ---- coverage + footer --------------------------------------------------
function renderCoverage(){
  if (!D.coverage_notes || !D.coverage_notes.length){
    $("coverage-fold").style.display = "none";
    return;
  }
  $("coverage").innerHTML = D.coverage_notes
    .map(([a, en, fr]) => `<li><b>${esc(a)}</b> — ${esc(LANG === "fr" && fr ? fr : en)}.</li>`)
    .join("");
}
function renderFoot(){
  $("genat").textContent = D.generated_at;
  $("ledger").textContent = D.ledger;
  $("foot-index").textContent =
    tf("foot_index", {d: fmt(D.semantic.docs), t: fmt(D.semantic.terms)});
}

// ---- empty window -------------------------------------------------------
// A range with nothing in it used to render as a page of zeroes, which reads
// like a broken dashboard rather than a quiet week.
function renderEmptyState(){
  const anything = selectionDays().some((d) => d.b || d.est || d.n)
    || Object.keys(D.models_by_day || {}).some(inWindow);
  const host = $("chart").parentElement;
  const old = host.querySelector(".empty");
  if (old) old.remove();
  $("chart").style.display = anything ? "" : "none";
  if (anything) return;
  const div = document.createElement("div");
  div.className = "empty";
  div.innerHTML = `<b>${esc(tf("empty_window", {from: W.from, to: W.to}))}</b><br>${t("empty_hint")}`
    + (rangeDays ? `<br><button class="btn" id="go-all">${t("empty_all")}</button>` : "");
  host.insertBefore(div, $("chart"));
  const go = $("go-all");
  if (go) go.addEventListener("click", () => { setRange(0); });
}

// ---- exports ------------------------------------------------------------
function download(name, mime, text){
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([text], {type: mime}));
  a.download = name;
  document.body.appendChild(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}
const selSlug = () => rangeLabel().replace(/[^\w.-]+/g, "-");
$("btn-csv").addEventListener("click", () => {
  let csv = "date,before,after,saved,estimated,ops\n";
  for (const d of selectionDays())
    csv += `${d.d},${d.b},${d.a},${Math.max(0, d.b - d.a)},${d.est},${d.n}\n`;
  download(`tokenade-${selSlug()}.csv`, "text/csv", csv);
});
$("btn-json").addEventListener("click", () => {
  download(`tokenade-${selSlug()}.json`, "application/json",
    JSON.stringify({generated_at: D.generated_at, window: {from: W.from, to: W.to},
      selection: selected, days: selectionDays()}, null, 1));
});
$("btn-copy").addEventListener("click", async () => {
  const tot = periodTotals(selectionDays());
  const acc = accounting(tot.saved);
  const text = tf("summary", {
    win: rangeLabel(), s: fmt(tot.saved),
    p: tot.b > 0 ? (100*tot.saved/tot.b).toFixed(1)+"%" : "—",
    usd: money(acc ? acc.usd : 0), n: tot.n,
  });
  try { await navigator.clipboard.writeText(text); }
  catch (e) {
    const ta = document.createElement("textarea");
    ta.value = text; document.body.appendChild(ta);
    ta.select(); document.execCommand("copy"); ta.remove();
  }
  $("btn-copy").textContent = t("copied");
  setTimeout(() => { $("btn-copy").textContent = t("copy_btn"); }, 1200);
});

// ---- wiring -------------------------------------------------------------
function syncSegs(){
  document.querySelectorAll("#gran button")
    .forEach((b) => b.classList.toggle("on", b.dataset.g === gran));
  document.querySelectorAll("#range button")
    .forEach((b) => b.classList.toggle("on", +b.dataset.r === rangeDays));
}
function setRange(n){
  rangeDays = n; selected = null; W = windowRange();
  syncSegs(); renderAll();
}
for (const btn of document.querySelectorAll("#range button"))
  btn.addEventListener("click", () => setRange(+btn.dataset.r));
for (const btn of document.querySelectorAll("#gran button"))
  btn.addEventListener("click", () => {
    gran = btn.dataset.g; selected = null;
    syncSegs(); renderAll();
  });
addEventListener("resize", () => { renderChart(); renderHero(); });
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", renderHeat);

function renderAll(){
  W = windowRange();
  renderWindowLine();
  renderHero();
  renderKpis();
  renderChart();
  renderTabs();
  renderTabBody();
  renderEmptyState();
}
function renderStatic(){
  applyI18n();
  renderHookBadge();
  renderFoot();
  renderCoverage();
  renderHeat();
  syncSegs();
  renderAll();
}
renderStatic();
