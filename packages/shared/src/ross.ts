// The Ross Panel (owner, 2026-10-19/20; design canvas "Ross Panel"): the client side of the
// server's `ross.*` channel (protocol.ts, crates/server/src/context.rs) and ONE row view model,
// so the web side panel and the phone card format a member identically.

import type { RossMessage, RossNearRowWire, RossNewsEvidenceWire, RossOfferingWire, RossPanelWire, RossReadinessWire, RossRowWire, RossSetupState } from "./protocol.ts";

export interface RossState {
  /** null until the first snapshot; false when the server does not run stock context. */
  enabled: boolean | null;
  panel: RossPanelWire | null;
  /**
   * Why the panel looks the way it does, when the server sent it. An empty panel with a
   * readiness summary is a different statement from an empty panel without one.
   */
  readiness: RossReadinessWire | null;
  /**
   * A frame has arrived on the connection that is open now.
   *
   * While false the panel is the **last one received**, not the current market: the channel used
   * to keep showing it with nothing to say the feed had dropped, so stale rows read as live. The
   * rows are kept on purpose — the last known state is useful — but every client must label them.
   *
   * A reopened socket does not set it. The rows on screen are still the ones from before the
   * drop until a frame arrives on the new connection, and the server sends a fresh
   * `ross.snapshot` for every `ross.subscribe` (which `createWsClient` re-sends on `welcome`),
   * so that is the moment they are current again — not the moment the socket came back.
   */
  connected: boolean;
}

export const EMPTY_ROSS: RossState = { enabled: null, panel: null, readiness: null, connected: false };

export interface RossChannel {
  get(): RossState;
  /** `fn` runs after every frame. Returns the unsubscribe. */
  subscribe(fn: () => void): () => void;
}

/** The freshness cue a client shows above the rows. */
export interface RossFreshness {
  /** The rows on screen are not being updated: say so, do not let them read as live. */
  stale: boolean;
  label: string;
  tone: RossTone;
}

/**
 * What to say about the panel's freshness.
 *
 * A connection that has not delivered a frame is the only staleness a client can judge for
 * itself: a replay's cutoff is its own simulated clock, and comparing a live cutoff with a
 * browser's clock would measure the two clocks' disagreement as well as any delay.
 *
 * The three states are distinct on purpose. Never had a frame: connecting. Had one and the feed
 * is delivering: live. Had one and the feed is not — dropped, or reopened but not yet delivered —
 * the rows are the last known state and say so.
 */
export function rossFreshness(state: RossState, formatTime?: (epochNs: number) => string): RossFreshness {
  if (state.connected) return { stale: false, label: "", tone: "dim" };
  if (state.enabled === null && !state.panel) return { stale: true, label: "Connecting…", tone: "dim" };
  const at = state.panel?.cutoff;
  const when = at !== undefined && formatTime ? ` · last ${formatTime(at)}` : "";
  return { stale: true, label: `Reconnecting${when}`, tone: "warning" };
}

/** A blocking reason as a short phrase; unknown reasons fall back to the raw string. */
const REASON_LABEL: Record<string, string> = {
  no_previous_close: "no previous close",
  previous_close_date_mismatch: "previous close is for another date",
  adjustment_coverage_unknown: "share basis unchecked",
  adjustment_not_current: "share basis not current",
  actions_coverage_expired: "share-basis check out of date",
  no_daily_baseline: "no volume history",
  insufficient_baseline: "fewer than 30 comparable sessions",
  numerator_coverage_unverified: "today's volume unverified",
  zero_denominator: "no comparable volume",
  no_float_evidence: "no float figure",
  float_stale: "float figure out of date",
  nonpositive_float: "float figure unusable",
  news_coverage_unknown: "news not searched yet",
  news_revision_order_unknown: "news version order unclear",
  price_unknown: "no price yet",
  price_stale: "price out of date",
};

export interface RossReadinessView {
  /** "1,204 symbols · none at 5/5". */
  headline: string;
  /** "812 need volume history", biggest first. */
  reasons: string[];
  /** "taken 09:31:15", when a formatter is given. */
  takenAt: string;
}

/** The readiness summary as lines a panel can show instead of a bare "nothing yet". */
export function rossReadinessView(r: RossReadinessWire, formatTime?: (epochNs: number) => string): RossReadinessView {
  const n = (x: number) => x.toLocaleString("en-US");
  const headline = `${n(r.symbols)} symbol${r.symbols === 1 ? "" : "s"} · ${r.members > 0 ? `${n(r.members)} on the panel` : r.qualified > 0 ? `${n(r.qualified)} at 5/5` : "none at 5/5"}`;
  const label = (reason: string) => REASON_LABEL[reason] ?? reason.replace(/_/g, " ");
  return {
    headline,
    reasons: r.blocking.map((b) => `${n(b.symbols)}: ${label(b.reason)}`),
    takenAt: formatTime ? `taken ${formatTime(r.observed_at)}` : "",
  };
}

/**
 * Applies a frame's day-line slices (`RossRowWire.day_line`) to the lines already held, so every
 * row of the returned panel holds its WHOLE line with `day_line_from: 0`.
 *
 * A slice from index `from` keeps the first `from` points held for the same symbol and line
 * start and replaces the rest (normally an append; less when a correction revised a close). A
 * slice this client cannot place — fewer points held than `from`, another start — drops that
 * row's line and sets `resync`: the caller asks for a fresh snapshot. A row without a line has
 * none.
 */
export function mergeRossDayLines(prev: RossPanelWire | null, next: RossPanelWire): { panel: RossPanelWire; resync: boolean } {
  // Nothing to merge (a snapshot, an older server): the frame's own panel, unchanged.
  if (!next.rows.some((r) => r.day_line && (r.day_line_from ?? 0) > 0)) return { panel: next, resync: false };
  let resync = false;
  const held = new Map(prev?.rows.map((r) => [r.symbol, r]) ?? []);
  const rows = next.rows.map((r) => {
    const from = r.day_line_from ?? 0;
    if (!r.day_line || from === 0) return r;
    const old = held.get(r.symbol);
    if (old?.day_line && old.day_line_start === r.day_line_start && old.day_line.length >= from) {
      return { ...r, day_line: old.day_line.slice(0, from).concat(r.day_line), day_line_from: 0 };
    }
    resync = true;
    const { day_line: _line, day_line_from: _from, day_line_start: _start, ...rest } = r;
    return rest;
  });
  return { panel: { ...next, rows }, resync };
}

/** Subscribed on demand like the pressure channel: the first listener opts in, the last opts out. */
export function createRossChannel(io: { subscribe(): void; unsubscribe(): void }): RossChannel & { apply(m: RossMessage): void; setConnected(open: boolean): void } {
  let state: RossState = EMPTY_ROSS;
  const listeners = new Set<() => void>();
  return {
    get: () => state,
    subscribe(fn) {
      listeners.add(fn);
      if (listeners.size === 1) io.subscribe();
      return () => {
        if (!listeners.delete(fn)) return;
        if (listeners.size > 0) return;
        io.unsubscribe();
        state = EMPTY_ROSS;
      };
    },
    apply(m) {
      const merged = m.panel ? mergeRossDayLines(state.panel, m.panel) : { panel: m.panel, resync: false };
      state = { enabled: m.enabled, panel: merged.panel, readiness: m.readiness ?? null, connected: true };
      // A slice this client cannot place (a frame it never saw): the server answers every
      // `ross.subscribe` with a whole snapshot, so ask again rather than draw a broken line.
      if (merged.resync && listeners.size > 0) io.subscribe();
      for (const l of listeners) l();
    },
    /**
     * The socket's state. A drop keeps the last panel and marks it not live.
     *
     * Opening is deliberately **not** enough to mark it live again: `apply` does that, when a
     * frame from the new connection arrives.
     */
    setConnected(open: boolean) {
      if (open || !state.connected) return;
      state = { ...state, connected: false };
      for (const l of listeners) l();
    },
  };
}

/** Semantic colours; each app maps them to its tokens (good/critical/warning/accent/text/muted/dim). */
export type RossTone = "good" | "critical" | "warning" | "accent" | "text" | "muted" | "dim";

export interface RossRowView {
  symbol: string;
  price: string;
  gain: string;
  /** ▲ / ▼ when the rank moved since the last panel, else "". */
  move: "▲" | "▼" | "";
  moveTone: RossTone;
  badge: { label: "NEW" | "HALTED"; tone: RossTone } | null;
  /** The last minute's move ("+3.0%", "—" when unranked). */
  m1: string;
  m1Tone: RossTone;
  /** Distance below the day's high ("−0.4%"), bright within 1 %. */
  fromHigh: string;
  fromHighTone: RossTone;
  /** Daily-denominator relative volume ("7.4×"), "—" when unknown. */
  rvol: string;
  /** The missing pillar ("Float 30M ✗", "News ?"), else the float when known ("Float 5.0M"). */
  tag: string;
  tagTone: RossTone;
  setup: { state: RossSetupState; label: string; note: string; tone: RossTone; pulse: boolean; attention: boolean; meter: number | null };
  /**
   * Where the news pillar's PASS came from ("8-K 09:12 (SEC)", "Benzinga 09:12"), with a link to
   * the filing or story when the server sent a usable https URL; null when news is not passing.
   */
  news: RossNewsSourceView | null;
  /** Warnings on the row: "No news confirmed" (news-optional panel), "Offering filed". Warning tone, never the accent. */
  flags: RossRowFlag[];
  /** Dimmed row (an invalidated setup). */
  dimmed: boolean;
  spark: number[];
  /** The row chart: the day line when the server sends one, else the 15 s spark; null with < 2 points. */
  line: RossLineView | null;
  /** Spoken summary for the row's accessible name. */
  aria: string;
}

/** The news item behind a member's news pillar, as a row shows it ([`rossNewsSource`]). */
export interface RossNewsSourceView {
  /** "8-K 09:12 (SEC)", "Benzinga 09:12". */
  label: string;
  /** The filing index (SEC) or the story; null when absent or not an https URL. */
  href: string | null;
  /** An SEC filing: the issuer's own report, not a news provider's story. */
  sec: boolean;
}

/** A warning on a Ross row. Status colour (warning), never the accent: these ask for a check. */
export interface RossRowFlag {
  kind: "no_news" | "offering";
  label: string;
  /** The longer explanation (tooltip / accessible description). */
  title: string;
  href: string | null;
}

/** Only https links reach an anchor: the URL is provider data, not ours. */
function safeHref(url: string | null | undefined): string | null {
  return typeof url === "string" && /^https:\/\/[^\s"'<>]+$/i.test(url) ? url : null;
}

/**
 * The news source text (pillars-v6: Benzinga or SEC EDGAR). An SEC filing is named by its form
 * and ACCEPTANCE time ("8-K 09:12 (SEC)"), a Benzinga story by its creation time, another
 * provider by its raw name. `formatTime` takes epoch ns.
 */
export function rossNewsSource(ev: RossNewsEvidenceWire | null | undefined, formatTime: (epochNs: number) => string): RossNewsSourceView | null {
  if (!ev || typeof ev.source !== "string" || !ev.source) return null;
  const at = typeof ev.published_at === "number" && Number.isFinite(ev.published_at) ? ` ${formatTime(ev.published_at)}` : "";
  const href = safeHref(ev.url);
  if (ev.source === "sec_edgar") return { label: `${ev.form || "Filing"}${at} (SEC)`, href, sec: true };
  if (ev.source === "benzinga") return { label: `Benzinga${at}`, href, sec: false };
  return { label: `${ev.source}${at}`, href, sec: false };
}

/** "S-3 filed 08:05 (SEC) · dilution risk": an SEC offering filing on the row, never a catalyst. */
export function rossOfferingFlag(o: RossOfferingWire | null | undefined, formatTime: (epochNs: number) => string): RossRowFlag | null {
  if (!o || typeof o.form !== "string" || !o.form) return null;
  const at = typeof o.published_at === "number" && Number.isFinite(o.published_at) ? ` ${formatTime(o.published_at)}` : "";
  return { kind: "offering", label: "Offering filed", title: `${o.form} filed${at} (SEC) · dilution risk`, href: safeHref(o.url) };
}

/** A member admitted on a news-optional panel without a passing news pillar. */
export const ROSS_NO_NEWS_FLAG: RossRowFlag = {
  kind: "no_news",
  label: "No news confirmed",
  title: "Admitted on the other four pillars (news optional): no fresh Benzinga story or SEC 8-K/6-K was found. Check the catalyst yourself.",
  href: null,
};

/** The panel header's mode note: "news optional" when the owner switched the news pillar off. */
export interface RossPanelModeView {
  newsOptional: boolean;
  /** "news optional", or "" under the five-pillar rule. */
  label: string;
  title: string;
}

export function rossPanelMode(panel: RossPanelWire | null | undefined): RossPanelModeView {
  if (panel?.news_optional === true) {
    return { newsOptional: true, label: "news optional", title: "Require a news catalyst is off: members need the other four pillars (4 to enter, 3 to stay)." };
  }
  return { newsOptional: false, label: "", title: "" };
}

/**
 * A Ross row's chart (owner, 2026-10-21: "get a sense of how the actual price action ... is
 * developing before I actually click on the ticker").
 */
export interface RossLineView {
  /**
   * `day`: the day's 30 s closes from 04:00 ET, then the row's live price as the end point.
   * `spark`: an older server's last ≤20 15 s closes (five minutes), drawn the same way.
   */
  kind: "day" | "spark";
  /** Micros, oldest first, every point (the drawing may thin them; this never does). */
  values: number[];
  /** The previous regular close (micros): a dotted baseline when it falls inside the drawing. */
  prevClose: number | null;
  /**
   * Good when the line ends at or above its FIRST point (the day's first price), else critical.
   * Deliberately not against the previous close: every member gained ≥ 10 % on it (the gain
   * pillar), so that colour would be green on every row and say nothing about the day.
   */
  tone: "good" | "critical";
}

/** The row chart's line from a wire row ([`RossLineView`]). */
export function rossLine(r: RossRowWire): RossLineView | null {
  const day = r.day_line;
  if (day && day.length > 0) {
    const values = r.price !== null && r.price !== undefined ? [...day, r.price] : day.slice();
    if (values.length < 2) return null;
    return { kind: "day", values, prevClose: r.prev_close ?? null, tone: values[values.length - 1]! >= values[0]! ? "good" : "critical" };
  }
  const spark = r.spark ?? [];
  if (spark.length < 2) return null;
  return { kind: "spark", values: spark, prevClose: null, tone: spark[spark.length - 1]! >= spark[0]! ? "good" : "critical" };
}

/** SVG paths for a row chart in a `w`×`h` box (y down), shared by web and phone. */
export interface RossLinePaths {
  line: string;
  /** The line closed down to the bottom edge, for the faint fill under it. */
  area: string;
  /** The previous close's y, or null when it lies outside the line's range (then not drawn). */
  baseY: number | null;
}

/**
 * Draws every point of a line into `w`×`h`. Beyond two points per horizontal unit the points are
 * thinned for DRAWING only: each column keeps its lowest and highest close in time order, so no
 * spike disappears; the first and last points are always exact. A previous close inside the
 * line's range (plus 10 % of it) joins the vertical scale; one far below — a member up 40 % on
 * the day — would flatten the shape the chart exists to show, so it is left out.
 */
/**
 * The vertical scale a row chart draws with: the line's range, widened to the previous close when
 * it lies within 10 % of that range (`base`, else null). Exposed so a mark drawn over the line (the
 * trigger, the last-minute tail: rowMarks.ts) sits on the same scale.
 */
export function rossLineScale(line: RossLineView, h: number, pad = 2): { y: (p: number) => number; lo: number; hi: number; base: number | null; slack: number } {
  let lo = Infinity;
  let hi = -Infinity;
  for (const x of line.values) {
    if (x < lo) lo = x;
    if (x > hi) hi = x;
  }
  const slack = (hi - lo) * 0.1;
  const pc = line.prevClose;
  const base = pc !== null && pc >= lo - slack && pc <= hi + slack ? pc : null;
  if (base !== null) {
    lo = Math.min(lo, base);
    hi = Math.max(hi, base);
  }
  const span = hi - lo || 1;
  return { y: (p: number) => pad + (1 - (p - lo) / span) * (h - 2 * pad), lo, hi, base, slack };
}

export function rossLinePaths(line: RossLineView, w: number, h: number, pad = 2): RossLinePaths {
  const v = line.values;
  const { y, base } = rossLineScale(line, h, pad);
  const n = v.length;
  const xAt = (i: number) => (i / (n - 1)) * w;
  const pts: [number, number][] = [];
  const cols = Math.max(1, Math.floor(w));
  if (n <= 2 * cols) {
    for (let i = 0; i < n; i++) pts.push([xAt(i), y(v[i]!)]);
  } else {
    pts.push([0, y(v[0]!)]);
    let i = 1;
    for (let c = 0; c < cols && i < n - 1; c++) {
      const end = Math.min(n - 1, Math.floor(((c + 1) * (n - 1)) / cols));
      if (i >= end) continue;
      let iMin = i;
      let iMax = i;
      for (let j = i; j < end; j++) {
        if (v[j]! < v[iMin]!) iMin = j;
        if (v[j]! > v[iMax]!) iMax = j;
      }
      for (const k of iMin === iMax ? [iMin] : iMin < iMax ? [iMin, iMax] : [iMax, iMin]) pts.push([xAt(k), y(v[k]!)]);
      i = end;
    }
    pts.push([w, y(v[n - 1]!)]);
  }
  const f = (x: number) => (Math.round(x * 10) / 10).toString();
  const d = `M${pts.map(([x, yy]) => `${f(x)} ${f(yy)}`).join(" L")}`;
  return { line: d, area: `${d} L${f(w)} ${f(h)} L0 ${f(h)} Z`, baseY: base === null ? null : Math.round(y(base) * 10) / 10 };
}

/** Micro-dollars as the app prints prices: four decimals below $1, two from $1. */
export function rossPrice(micros: number): string {
  const p = micros / 1_000_000;
  return p < 1 ? p.toFixed(4) : p.toFixed(2);
}

function pct(bp: number, digits = 1): string {
  const v = bp / 100;
  const s = Math.abs(v).toFixed(digits);
  return v > 0 ? `+${s}%` : v < 0 ? `−${s}%` : `${(0).toFixed(digits)}%`;
}

function shares(n: number): string {
  return n >= 1_000_000 ? `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M` : n >= 1_000 ? `${Math.round(n / 1_000)}K` : String(n);
}

const DAY_NS = 86_400 * 1_000_000_000;

/** `Float 3.1M`, plus its age once the figure is a day old or more (pillars-v4 keeps it 4 days). */
function floatTag(r: RossRowWire, cutoff: number): string {
  const days = r.float_as_of ? Math.floor((cutoff - r.float_as_of) / DAY_NS) : 0;
  const age = days >= 1 ? ` · ${days}d old` : "";
  // pillars-v5: no float figure, but the shares outstanding bound it from above.
  if (!r.float_shares && r.outstanding_shares) return `Float ≤ ${shares(r.outstanding_shares)} (shares out)${age}`;
  return `Float ${shares(r.float_shares ?? 0)}${age}`;
}

const PILLAR_LABEL: Record<string, string> = { price: "Price", gain: "Gain", rvol: "Volume", float: "Float", news: "News" };

/**
 * The cue labels (owner, 2026-10-21). A cue names the STAGE a member's pullback is at, from the
 * same micro-pullback episodes the Detection Panel shows; it is not an entry signal. On the
 * holdout a "broke trigger" episode failed about three times in four, so it gets the normal text
 * colour, never a success green, and is not counted as needing attention. Accent is spent only on
 * a crossing in progress; amber on a price at the trigger.
 */
const SETUP: Record<RossSetupState, { label: string; tone: RossTone; pulse: boolean; attention: boolean }> = {
  crossing: { label: "Crossing", tone: "accent", pulse: true, attention: true },
  near: { label: "At trigger", tone: "warning", pulse: false, attention: true },
  triggered: { label: "Broke trigger", tone: "text", pulse: false, attention: false },
  forming: { label: "Pullback", tone: "muted", pulse: false, attention: false },
  invalidated: { label: "Failed", tone: "dim", pulse: false, attention: false },
  halted: { label: "Halted", tone: "dim", pulse: false, attention: false },
  watching: { label: "Watching", tone: "dim", pulse: false, attention: false },
};

/** A member admitted within this long before the panel's cutoff is NEW. */
export const ROSS_NEW_NS = 5 * 60 * 1_000_000_000;

/** The row's display, from the engine's row and the panel's cutoff (ns). */
export function rossRowView(r: RossRowWire, cutoff: number, formatTime: (epochNs: number) => string): RossRowView {
  const setupWire = r.setup ?? null;
  // A token this client does not know (a newer server) reads as watching rather than breaking.
  const state: RossSetupState = setupWire?.state && setupWire.state in SETUP ? setupWire.state : "watching";
  const look = SETUP[state];
  const m = r.momentum;
  const m1 = r.move_1m_bp ?? (m.status === "ready" ? m.r_primary_bp : null);
  const fromHighBp = r.from_high_bp ?? m.below_high_bp;
  const first = r.missing?.[0];
  const missing = first ? ([first.pillar, first.state] as const) : r.membership.nonpassing[0];
  let tag = "";
  let tagTone: RossTone = "muted";
  if (missing) {
    const [pillar, st] = missing;
    const name = PILLAR_LABEL[pillar] ?? pillar;
    if (st === "FAIL") {
      tag = pillar === "float" && r.float_shares ? `${floatTag(r, cutoff)} ✗` : `${name} ✗`;
      tagTone = "critical";
    } else {
      tag = `${name} ?`;
      tagTone = "warning";
    }
  } else if (r.float_shares || r.outstanding_shares) {
    tag = floatTag(r, cutoff);
  }
  const trigger = setupWire?.trigger?.price ?? null;
  const inval = setupWire?.invalidation?.price ?? null;
  // How far the price sits above the pullback low (the failure level): a distance, not a stop.
  const overLow = setupWire?.to_invalidation_bp ?? setupWire?.risk_to_stop_bp ?? null;
  const lowText = overLow !== null ? ` · ${(overLow / 100).toFixed(1)}% over low` : "";
  let note = "";
  if (state === "crossing" && trigger !== null) note = `${rossPrice(trigger)} · close pending${lowText}`;
  else if (state === "near" && trigger !== null) note = `${rossPrice(trigger)} · ${((setupWire?.distance_to_trigger_bp ?? 0) / 100).toFixed(1)}% away${lowText}`;
  else if (state === "forming" && trigger !== null) note = `trigger ${rossPrice(trigger)}`;
  else if (state === "triggered" && trigger !== null) note = `${rossPrice(trigger)}${setupWire?.state_since ? ` · ${formatTime(setupWire.state_since)}` : ""}`;
  else if (state === "invalidated" && inval !== null) note = `below ${rossPrice(inval)}`;
  const admitted = r.membership.episodes.at(-1)?.admitted_at;
  const badge = state === "halted" ? { label: "HALTED" as const, tone: "warning" as RossTone } : admitted !== undefined && cutoff - admitted <= ROSS_NEW_NS ? { label: "NEW" as const, tone: "accent" as RossTone } : null;
  const move = r.rank !== null && r.prev_rank !== null && r.rank !== r.prev_rank ? (r.rank < r.prev_rank ? "▲" : "▼") : "";
  // The news source only while the pillar passes (the server sends it only then; an older or
  // inconsistent frame must never show a source next to "News ?").
  const newsMissing = (r.missing ?? r.membership.nonpassing.map(([pillar]) => ({ pillar }))).some((x) => x.pillar === "news");
  const news = newsMissing ? null : rossNewsSource(r.news_evidence, formatTime);
  const flags: RossRowFlag[] = [];
  if (r.news_unconfirmed === true) flags.push(ROSS_NO_NEWS_FLAG);
  const offering = rossOfferingFlag(r.offering, formatTime);
  if (offering) flags.push(offering);
  const view: RossRowView = {
    symbol: r.symbol,
    price: r.price !== null ? rossPrice(r.price) : "—",
    gain: r.gain_bp !== null ? pct(r.gain_bp) : "—",
    move,
    moveTone: move === "▲" ? "good" : move === "▼" ? "critical" : "dim",
    badge,
    m1: m1 !== null ? pct(m1) : "—",
    m1Tone: m1 === null ? "dim" : m1 > 0 ? "good" : m1 < 0 ? "critical" : "muted",
    fromHigh: fromHighBp !== null && fromHighBp !== undefined ? pct(-fromHighBp) : "—",
    fromHighTone: fromHighBp !== null && fromHighBp !== undefined && fromHighBp <= 100 ? "text" : "muted",
    rvol: r.rvol_milli ? `${(r.rvol_milli / 1000).toFixed(1)}×` : "—",
    tag,
    tagTone,
    setup: { state, label: look.label, note, tone: look.tone, pulse: look.pulse, attention: look.attention, meter: state === "watching" || state === "halted" || state === "invalidated" || setupWire?.meter_milli == null ? null : setupWire.meter_milli / 1000 },
    news,
    flags,
    dimmed: state === "invalidated",
    spark: r.spark ?? [],
    line: rossLine(r),
    aria: "",
  };
  view.aria = `${view.symbol} ${view.price} ${view.gain}, last minute ${view.m1}, ${look.label}${note ? ` ${note}` : ""}${tag ? `, ${tag}` : ""}${news ? `, news ${news.label}` : ""}${flags.map((f) => `, ${f.kind === "offering" ? f.title : f.label}`).join("")}`;
  return view;
}

/** One near-qualifying row as both apps show it. */
export interface RossNearRowView {
  symbol: string;
  price: string;
  gain: string;
  rvol: string;
  /** "Float ?" / "News ?", each with the reason as a phrase (the badge's title). Neutral/warning, never the accent. */
  badges: { label: string; reason: string }[];
  aria: string;
}

export interface RossNearView {
  rows: RossNearRowView[];
  /** All near-qualifying symbols the server counted (more than `rows` when it bounded them). */
  total: number;
}

/** Rows a near section shows before it is opened (it starts collapsed beyond this). */
export const ROSS_NEAR_PREVIEW = 3;

/** The near-qualifying section's view model (readiness `near`, owner 2026-11-22). */
export function rossNearView(r: RossReadinessWire | null | undefined): RossNearView {
  const near: RossNearRowWire[] = r?.near ?? [];
  const label = (reason: string) => REASON_LABEL[reason] ?? reason.replace(/_/g, " ");
  const rows = near.map((n) => {
    const badges = n.unknown.map((u) => ({ label: `${PILLAR_LABEL[u.pillar] ?? u.pillar} ?`, reason: label(u.reason) }));
    const row: RossNearRowView = {
      symbol: n.symbol,
      price: n.price !== null ? rossPrice(n.price) : "—",
      gain: n.gain_bp !== null ? pct(n.gain_bp) : "—",
      rvol: n.rvol_milli ? `${(n.rvol_milli / 1000).toFixed(1)}×` : "—",
      badges,
      aria: "",
    };
    row.aria = `${row.symbol} ${row.price} ${row.gain}, volume ${row.rvol}, not on the panel: ${badges.map((b) => `${b.label.replace(" ?", "")} unknown, ${b.reason}`).join("; ")}`;
    return row;
  });
  return { rows, total: Math.max(r?.near_total ?? 0, rows.length) };
}

/** Rows needing attention (at trigger, crossing): the panel's count, or derived from the rows. */
export function rossAttention(panel: RossPanelWire): number {
  return panel.attention ?? panel.rows.filter((r) => r.setup && SETUP[r.setup.state]?.attention).length;
}

/**
 * A synthetic readiness for the labelled demo mode (dev builds only, `?rossDemo`) and tests,
 * carrying near-qualifying rows so the section can be seen: never shown as live data.
 */
export function rossDemoReadiness(cutoff: number): RossReadinessWire {
  const f = (reason: string) => ({ pillar: "float", reason });
  const n = (reason: string) => ({ pillar: "news", reason });
  const near = [
    { symbol: "GGGG", price: 3_420_000, gain_bp: 8800, rvol_milli: 31_000, unknown: [f("no_float_evidence")], float_shares: null },
    { symbol: "HHHH", price: 1_180_000, gain_bp: 4600, rvol_milli: 12_400, unknown: [n("news_coverage_unknown")], float_shares: 6_200_000 },
    { symbol: "IIII", price: 742_000, gain_bp: 2900, rvol_milli: 8_100, unknown: [f("float_stale"), n("news_coverage_unknown")], float_shares: 4_400_000 },
    { symbol: "JJJJ", price: 12_350_000, gain_bp: 1700, rvol_milli: 6_300, unknown: [f("nonpositive_float")], float_shares: 0 },
    { symbol: "KKKK", price: 205_000, gain_bp: 1240, rvol_milli: 5_100, unknown: [f("no_float_evidence")], float_shares: null },
  ].map((r) => ({ ...r, cutoff }));
  return { symbols: 1_204, qualified: 3, members: 5, pillars: [], blocking: [], evidence_records: 0, observed_at: cutoff, near, near_total: near.length };
}

/**
 * A fixed synthetic panel for the labelled demo mode (dev builds only, `?rossDemo`) and tests:
 * never shown as live data. The members mirror the design canvas.
 */
export function rossDemoPanel(cutoff: number, opts: { newsOptional?: boolean } = {}): RossPanelWire {
  const newsOptional = opts.newsOptional === true;
  const MIN = 60 * 1_000_000_000;
  const ep = (agoMin: number) => [{ id: "demo", admitted_at: cutoff - agoMin * MIN, retained_since: null, exited_at: null }];
  const mom = (m1: number | null, high: number) => ({ status: m1 === null ? ("unavailable" as const) : ("ready" as const), reason: m1 === null ? "halted" : "ready", r_short_bp: null, r_primary_bp: m1, r_context_bp: null, accel_bp: null, below_high_bp: high });
  const spark = (from: number, to: number, wobble = 0) => Array.from({ length: 20 }, (_, i) => Math.round(from + ((to - from) * i) / 19 + (i % 3 === 1 ? wobble : 0)));
  // A synthetic day: 04:00 to the cutoff at 30 s, from the previous close's level through a gap
  // up and the spark's last stretch, with a deterministic wobble. Never market data.
  const STEP = 30 * 1_000_000_000;
  const dayLine = (price: number, gainBp: number, seed: number) => {
    const n = 360;
    const pc = Math.round(price / (1 + gainBp / 10_000));
    const open = pc + (price - pc) * 0.55;
    return {
      day_line_start: cutoff - n * STEP,
      day_line_step_s: 30,
      day_line_from: 0,
      prev_close: pc,
      day_line: Array.from({ length: n }, (_, i) => Math.round(open + (price - open) * (i / (n - 1)) + Math.sin(i / (9 + seed)) * (price - pc) * 0.12)),
    };
  };
  const row = (o: Partial<RossRowWire> & Pick<RossRowWire, "symbol" | "price" | "gain_bp" | "momentum">): RossRowWire => ({
    membership: { status: "qualified", episodes: ep(20), nonpassing: [] },
    counts: { pass: 5, fail: 0, unknown: 0 },
    rank: null,
    prev_rank: null,
    ...(o.price !== null && o.gain_bp !== null ? dayLine(o.price, o.gain_bp, o.symbol.charCodeAt(0) % 7) : {}),
    ...o,
  });
  return {
    revision: 1,
    bar_end: cutoff - 2_000_000_000,
    cutoff,
    attention: 2,
    panel_policy: newsOptional ? "ross-panel-v2@demo" : "ross-panel-v1@demo",
    news_optional: newsOptional,
    rows: [
      row({ symbol: "BBBB", price: 6_080_000, gain_bp: 2150, momentum: mom(300, 40), rank: 1, prev_rank: 3, rvol_milli: 7400, float_shares: 30_100_000, membership: { status: "retained", episodes: ep(11), nonpassing: [["float", "FAIL", "at_or_above_max"]] }, counts: { pass: 4, fail: 1, unknown: 0 }, setup: { state: "crossing", trigger: { price: 6_050_000, version: "demo" }, invalidation: { price: 5_860_000, version: "demo" }, meter_milli: 1000, to_invalidation_bp: 362 }, spark: spark(5_700_000, 6_080_000, 20_000) }),
      row({ symbol: "EEEE", price: 2_470_000, gain_bp: 3100, momentum: mom(140, 80), rank: 2, prev_rank: 2, rvol_milli: 11_200, float_shares: 3_100_000, news_evidence: { source: "sec_edgar", form: "8-K", published_at: cutoff - 38 * MIN, url: "https://www.sec.gov/Archives/edgar/data/0000000000/000000000026000000/0000000000-26-000000-index.htm" }, membership: { status: "qualified", episodes: ep(2), nonpassing: [] }, setup: { state: "near", trigger: { price: 2_490_000, version: "demo" }, invalidation: { price: 2_360_000, version: "demo" }, distance_to_trigger_bp: 80, meter_milli: 820, to_invalidation_bp: 530 }, spark: spark(2_380_000, 2_470_000, 8_000) }),
      row({ symbol: "FFFF", price: 421_000, gain_bp: 6400, momentum: mom(90, 120), rank: 3, prev_rank: 3, rvol_milli: 22_000, float_shares: 1_800_000, news_evidence: { source: "benzinga", form: null, published_at: cutoff - 95 * MIN, url: null }, setup: { state: "forming", trigger: { price: 429_000, version: "demo" }, invalidation: { price: 398_000, version: "demo" }, meter_milli: 550 }, spark: spark(405_000, 421_000, 2_000) }),
      row({ symbol: "AAAA", price: 5_680_000, gain_bp: 4200, momentum: mom(-200, 630), rank: 4, prev_rank: 2, rvol_milli: 9100, float_shares: 5_000_000, news_evidence: { source: "sec_edgar", form: "6-K", published_at: cutoff - 150 * MIN, url: "https://www.sec.gov/Archives/edgar/data/0000000000/000000000026000000/0000000000-26-000001-index.htm" }, offering: { form: "S-3", published_at: cutoff - 70 * MIN, url: "https://www.sec.gov/Archives/edgar/data/0000000000/000000000026000000/0000000000-26-000002-index.htm" }, setup: { state: "forming", trigger: { price: 5_900_000, version: "demo" }, invalidation: { price: 5_500_000, version: "demo" }, meter_milli: 350 }, spark: spark(5_900_000, 5_680_000, 30_000) }),
      row({ symbol: "CCCC", price: 3_410_000, gain_bp: 1280, momentum: mom(-260, 980), rank: 5, prev_rank: 4, rvol_milli: 5600, membership: { status: newsOptional ? "qualified" : "retained", episodes: ep(30), nonpassing: [["news", "UNKNOWN", "news_coverage_unknown"]] }, counts: { pass: 4, fail: 0, unknown: 1 }, news_unconfirmed: newsOptional, setup: { state: "invalidated", invalidation: { price: 3_440_000, version: "demo" } }, spark: spark(3_550_000, 3_410_000) }),
      row({ symbol: "DDDD", price: 8_120_000, gain_bp: 1520, momentum: mom(null, 210), rvol_milli: 6000, float_shares: 14_000_000, setup: { state: "halted" }, spark: spark(8_050_000, 8_120_000) }),
    ],
  };
}
