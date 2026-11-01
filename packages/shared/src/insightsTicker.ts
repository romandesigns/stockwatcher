// The Insights ticker page (owner, 2026-10-31, approved design "Insights ticker page"): one symbol
// on one trading day. Rust builds the whole package (crates/server/src/insights: stats, outcomes,
// scorecard, halts and LULD, missed setups and their reasons) and pushes it; this module only
// parses it, follows the subscription, and words it for the screens. No outcome math, counting
// or detector logic lives here: every number shown is one the server sent. Shared by the web and
// the phone so both say the same thing about the same day.

/** The server builds and pushes the page (`insights.ticker.subscribe`); old servers lack it. */
export const INSIGHTS_TICKER_FEATURE = "insights.ticker.v1";

export interface TickerOutcomeWire {
  ref_price: number;
  ref_at: number;
  best_price: number | null;
  best_at: number | null;
  best_pct: number | null;
  plus5_price: number | null;
  plus5_at: number | null;
  stop: number | null;
  stop_hit_at: number | null;
  crossing: { price: number; at: number; best_price: number | null; best_at: number | null; best_pct: number | null; plus5_at: number | null } | null;
  crossed_after_stop_at: number | null;
  through: number | null;
}

/** A notification-history row: the ledger's notice plus what the server measured after it. */
export interface TickerAlertWire {
  event_id: string;
  n: number;
  at?: number;
  price?: number;
  notice?: string;
  title?: string;
  body?: string;
  label?: string;
  /** Sent rows' chart number, oldest first. */
  marker?: number;
  status?: "sent" | "held";
  reason?: string | null;
  reason_text?: string | null;
  outcome: TickerOutcomeWire | null;
}

export interface TickerMissedWire {
  episode_id: string;
  armed_at: number;
  trigger: number | null;
  stop: number | null;
  /** `warmup` | `one_per_day` | `window_closed` | `no_context` | `not_recorded` (or a newer code). */
  reason: string;
  reason_text: string;
  restart_at: number | null;
  flagged_at: number | null;
  outcome: TickerOutcomeWire;
}

export interface TickerMarkerWire {
  kind: "alert" | "missed";
  at: number;
  price: number;
  n?: number;
  event_id?: string;
  episode_id?: string;
}

export interface TickerSessionWire {
  open: number | null;
  high: number | null;
  high_at: number | null;
  low: number | null;
  low_at: number | null;
  close: number | null;
  volume: number;
  dollar_volume: number;
}

export interface TickerStatsWire {
  prev_close: { day: string; price: number } | null;
  close: number | null;
  change: { abs: number; pct: number | null } | null;
  last: number | null;
  sessions: { pre: TickerSessionWire | null; regular: TickerSessionWire | null; after: TickerSessionWire | null };
  volume: number;
  dollar_volume: number;
  high: { price: number; at: number; session: string } | null;
  float: { available: boolean; shares?: number; as_of?: number; provider?: string; reason?: string };
  short_interest: { available: boolean; reason?: string };
}

export interface TickerScorecardWire {
  alerts: number;
  sent: number;
  held: number;
  reached_5pct: number;
  best_entry: { event_id?: string; notice?: string; marker?: number | null; price?: number | null; best_pct: number } | null;
  setups_armed: number;
  missed: number;
  by_kind: { notice: string; label: string; alerts: number; sent: number; reached_5pct: number; best_pct: number | null }[];
}

export interface TickerHaltsWire {
  halts: { start: number; end: number | null; reason: string; code: string }[];
  move_session: string | null;
  outside_regular: boolean;
  luld_applies: boolean;
  closest: { up: TickerBandWire | null; down: TickerBandWire | null; regular_prints: number } | null;
}

export interface TickerBandWire {
  bp: number;
  at: number;
  price: number;
  band: number;
}

export interface TickerCandleWire {
  t: number;
  o: number;
  h: number;
  l: number;
  c: number;
  v: number;
  n: number;
  final: boolean;
}

export interface TickerPackageWire {
  symbol: string;
  day: string;
  today: boolean;
  name: string | null;
  prev_day: { day: string; archived: boolean } | null;
  next_day: { day: string; archived: boolean } | null;
  available: boolean;
  unavailable_reason: string | null;
  stats: TickerStatsWire | null;
  bars: TickerCandleWire[];
  /** The window the chart opens on (epoch ms; the server's `focus_window`): the move, not the whole day. Null = open as usual. */
  focus: { from: number; to: number } | null;
  alerts: TickerAlertWire[];
  markers: TickerMarkerWire[];
  scorecard: TickerScorecardWire | null;
  halts: TickerHaltsWire | null;
  missed: TickerMissedWire[];
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const arr = <T>(v: unknown, keep: (x: unknown) => boolean): T[] => (Array.isArray(v) ? (v.filter(keep) as T[]) : []);
const neighbour = (v: unknown) => (isObj(v) && typeof v.day === "string" ? { day: v.day, archived: v.archived === true } : null);
const finite = (v: unknown) => typeof v === "number" && Number.isFinite(v);

/**
 * A `GET /api/insights/ticker` body or an `insights.ticker.snapshot` / `.update` frame, or null.
 * The server is the only producer: this checks the shape the screens index into and drops rows
 * that lack it, rather than re-validating every number.
 */
export function parseTickerPackage(raw: unknown): TickerPackageWire | null {
  if (!isObj(raw) || typeof raw.symbol !== "string" || typeof raw.day !== "string") return null;
  const available = raw.available === true;
  return {
    symbol: raw.symbol,
    day: raw.day,
    today: raw.today === true,
    name: typeof raw.name === "string" && raw.name ? raw.name : null,
    prev_day: neighbour(raw.prev_day),
    next_day: neighbour(raw.next_day),
    available,
    unavailable_reason: typeof raw.unavailable_reason === "string" ? raw.unavailable_reason : null,
    stats: available && isObj(raw.stats) ? (raw.stats as unknown as TickerStatsWire) : null,
    bars: arr<TickerCandleWire>(raw.bars, (b) => isObj(b) && finite(b.t) && finite(b.o) && finite(b.h) && finite(b.l) && finite(b.c)),
    focus: isObj(raw.focus) && finite(raw.focus.from) && finite(raw.focus.to) && (raw.focus.to as number) >= (raw.focus.from as number) ? { from: raw.focus.from as number, to: raw.focus.to as number } : null,
    alerts: arr<TickerAlertWire>(raw.alerts, (a) => isObj(a) && typeof a.event_id === "string"),
    markers: arr<TickerMarkerWire>(raw.markers, (m) => isObj(m) && (m.kind === "alert" || m.kind === "missed") && finite(m.at) && finite(m.price)),
    scorecard: available && isObj(raw.scorecard) ? { ...(raw.scorecard as unknown as TickerScorecardWire), by_kind: arr(raw.scorecard.by_kind, isObj) } : null,
    halts: available && isObj(raw.halts) ? { ...(raw.halts as unknown as TickerHaltsWire), halts: arr(raw.halts.halts, (h) => isObj(h) && finite(h.start)) } : null,
    missed: arr<TickerMissedWire>(raw.missed, (m) => isObj(m) && typeof m.episode_id === "string" && finite(m.armed_at) && isObj(m.outcome)),
  };
}

// --- the subscription ----------------------------------------------------------------------------

export interface TickerSnapshotMessage {
  type: "insights.ticker.snapshot" | "insights.ticker.update";
  symbol: string;
  day: string;
  [k: string]: unknown;
}
export type TickerClientMessage = { type: "insights.ticker.subscribe"; symbol: string; day?: string } | { type: "insights.ticker.unsubscribe" };

export interface TickerRequest {
  symbol: string;
  day: string;
}

export interface TickerState {
  /** Whether the connected server builds the page: null until the first welcome. */
  available: boolean | null;
  /** What is open, null when nothing is. */
  want: TickerRequest | null;
  /** The package for `want`, null until it arrives (a build takes about a second). */
  pkg: TickerPackageWire | null;
}

export const EMPTY_TICKER: TickerState = { available: null, want: null, pkg: null };

/**
 * The state of a feed with no ticker channel at all (a local or demo feed): it will never build a
 * page, so the screens say so instead of showing "Building…" forever (EMPTY_TICKER's `null`
 * means "no welcome yet", which a channel-less feed never gets).
 */
export const NO_TICKER: TickerState = { available: false, want: null, pkg: null };

export interface TickerChannel {
  get(): TickerState;
  subscribe(fn: () => void): () => void;
  /** Opens `want` (replacing what was open); the returned function closes it if it is still the one open. */
  open(want: TickerRequest): () => void;
}

const same = (a: TickerRequest | null, b: TickerRequest | null) => !!a && !!b && a.symbol === b.symbol && a.day === b.day;

/**
 * The page's package, following one symbol-day at a time. Only a frame for exactly the open
 * symbol and day is kept: the server drops a replaced subscription's late build, and this
 * check also covers a frame already in flight when the person switched days.
 */
export function createTickerChannel(io: { subscribe(want: TickerRequest): void; unsubscribe(): void }): TickerChannel & { apply(m: TickerSnapshotMessage): void; setAvailable(on: boolean): void } {
  let state: TickerState = EMPTY_TICKER;
  const listeners = new Set<() => void>();
  const set = (next: TickerState) => {
    state = next;
    for (const l of listeners) l();
  };
  return {
    get: () => state,
    subscribe(fn) {
      listeners.add(fn);
      return () => {
        listeners.delete(fn);
      };
    },
    open(want) {
      const w = { symbol: want.symbol.toUpperCase(), day: want.day };
      if (!same(state.want, w)) set({ ...state, want: w, pkg: null });
      io.subscribe(w);
      return () => {
        if (!same(state.want, w)) return;
        io.unsubscribe();
        set({ ...state, want: null, pkg: null });
      };
    },
    apply(m) {
      const want = state.want;
      if (!want) return;
      // A failed build answers without its symbol and day (crate::insights `package`): it can
      // only be the open request's, since the server drops a replaced one's late frame.
      const failed = typeof m.symbol !== "string" && m.available === false;
      if (!failed && (m.symbol !== want.symbol || m.day !== want.day)) return;
      const pkg = parseTickerPackage(failed ? { ...m, ...want } : m);
      if (pkg) set({ ...state, pkg });
    },
    setAvailable(on) {
      if (state.available !== on) set({ ...state, available: on });
    },
  };
}

// --- the route -----------------------------------------------------------------------------------

const DAY_RE = /^\d{4}-\d{2}-\d{2}$/;
const SYMBOL_RE = /^[A-Z][A-Z0-9.-]{0,9}$/;

/** A typed ticker as the page accepts it ("ino " -> "INO"), or null. */
export function tickerSymbol(raw: string): string | null {
  const s = raw.trim().toUpperCase();
  return SYMBOL_RE.test(s) ? s : null;
}

/** The ticker page's hash, `#/insights/ticker/INO/2026-10-30` (shareable; back and forward work). */
export function tickerPageHash(symbol: string, day: string): string {
  return `#/insights/ticker/${encodeURIComponent(symbol.toUpperCase())}/${day}`;
}

/** The symbol-day a hash opens, or null when it is not the ticker page. */
export function tickerPageOf(hash: string): TickerRequest | null {
  const m = /^#\/?insights\/ticker\/([^/?#]+)\/([^/?#]+)/.exec(hash);
  if (!m) return null;
  const symbol = tickerSymbol(decodeURIComponent(m[1]!));
  return symbol && DAY_RE.test(m[2]!) ? { symbol, day: m[2]! } : null;
}

// --- wording -------------------------------------------------------------------------------------

export type Tone = "neutral" | "up" | "down" | "dim";

/** Dollars as the alerts print them: four decimals under $1 ($0.7264), two above ($2.01). */
export function money(p: number | null | undefined): string {
  if (p === null || p === undefined || !Number.isFinite(p)) return "—";
  return `$${p.toFixed(Math.abs(p) < 1 ? 4 : 2)}`;
}

/** A server percentage ("+273.1%"); it already comes rounded to one decimal. */
export function signedPct(p: number | null | undefined): string {
  if (p === null || p === undefined || !Number.isFinite(p)) return "—";
  return `${p > 0 ? "+" : ""}${p.toFixed(1)}%`;
}

/** 80_501_707 -> "80.5M", 4_995 -> "5.0K". */
export function compact(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return "—";
  const a = Math.abs(n);
  if (a >= 1e9) return `${(n / 1e9).toFixed(2)}B`;
  if (a >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (a >= 1e3) return `${(n / 1e3).toFixed(1)}K`;
  return String(Math.round(n));
}

const DAY_FMT = new Intl.DateTimeFormat("en-US", { weekday: "short", month: "short", day: "numeric", year: "numeric", timeZone: "UTC" });
const SHORT_FMT = new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric", timeZone: "UTC" });
const dayDate = (day: string) => new Date(`${day}T12:00:00Z`);
// An instant's ET calendar date ("Oct 30"): the server's float `as_of` is when it was retrieved.
// A UTC date would say "Oct 31" for an evening retrieval on Oct 30, which reads as look-ahead.
const ET_SHORT_FMT = new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric", timeZone: "America/New_York" });

/** "Oct 30" for an epoch-ms instant, in ET. */
export function etDayShort(ms: number): string {
  return ET_SHORT_FMT.format(new Date(ms));
}

/** "Fri, Oct 30, 2026" for "2026-10-30". */
export function dayLabel(day: string): string {
  return DAY_RE.test(day) ? DAY_FMT.format(dayDate(day)) : day;
}

/** "Oct 30" for "2026-10-30". */
export function dayShort(day: string): string {
  return DAY_RE.test(day) ? SHORT_FMT.format(dayDate(day)) : day;
}

const capital = (s: string) => (s ? s[0]!.toUpperCase() + s.slice(1) : s);

/** The notice's own name from its title ("INO · Run +20% (after …) · $0.7800" -> "Run +20% (after …)"), else the server's label. */
export function alertKind(a: Pick<TickerAlertWire, "title" | "label" | "notice">, symbol: string): string {
  const parts = (a.title ?? "").split(" · ");
  if (parts.length >= 3 && parts[0] === symbol && /^\$\d/.test(parts.at(-1)!)) return parts.slice(1, -1).join(" · ");
  return a.label ?? a.notice ?? "Notice";
}

const UNAVAILABLE: Record<string, string> = {
  not_archived: "No recorded trades for this ticker on that day.",
  retention_expired: "That day is older than the archive keeps.",
  not_a_trading_day: "The market was closed that day.",
  in_the_future: "That day has not happened yet.",
  build_failed: "The server could not build this day. Try again in a moment.",
};

export interface TickerStatView {
  id: string;
  label: string;
  value: string;
  note: string;
  tone: Tone;
}

export interface TickerAlertView {
  key: string;
  marker: number | null;
  sent: boolean;
  time: string;
  kind: string;
  detail: string;
  price: string;
  best: string;
  bestPct: string;
  bestTone: Tone;
  plus5: string;
  plus5Tone: Tone;
  stop: string;
  stopTone: Tone;
  /** Held rows: why, in plain words (the server's). */
  held: string | null;
}

export interface TickerMissedView {
  key: string;
  time: string;
  kind: string;
  levels: string;
  outcome: string;
  outcomeTone: Tone;
  why: string;
}

export interface TickerPageView {
  symbol: string;
  name: string | null;
  day: string;
  dayLabel: string;
  dayShort: string;
  prevDay: string | null;
  nextDay: string | null;
  today: boolean;
  /** Set when the day has no package: the reason in plain words. */
  unavailable: string | null;
  headline: { price: string; change: string; tone: Tone; note: string };
  stats: TickerStatView[];
  alerts: TickerAlertView[];
  alertSummary: string;
  score: { label: string; value: string; note: string }[];
  byKind: { key: string; label: string; value: string }[];
  halts: { lines: string[]; luld: string | null; closestTitle: string; closest: string[] };
  /** The first few missed setups, shown; the rest behind "N later setups". */
  missed: TickerMissedView[];
  missedLater: TickerMissedView[];
  markers: TickerMarkerWire[];
}

/** Missed setups shown before the "N later setups" toggle (the design shows four). */
export const MISSED_SHOWN = 4;

const range = (s: TickerSessionWire | null) => (s && s.low !== null && s.high !== null ? `${money(s.low)} – ${money(s.high)}` : "—");

/**
 * The page in words, from the server's package. `time(ms, seconds)` formats an epoch-ms instant
 * in ET (the platform's own formatter: chart-core formatEtTime12). Only wording and layout
 * choices are made here; every count, percentage and verdict is the server's.
 */
export function tickerPageView(p: TickerPackageWire, time: (ms: number, seconds?: boolean) => string): TickerPageView {
  const s = p.stats;
  const so = p.today ? "so far" : null;
  const reg = s?.sessions.regular ?? null;
  const pre = s?.sessions.pre ?? null;
  const after = s?.sessions.after ?? null;
  const changeTone: Tone = s?.change?.pct ? (s.change.pct > 0 ? "up" : s.change.pct < 0 ? "down" : "neutral") : "dim";
  const stats: TickerStatView[] = s
    ? [
        { id: "close", label: "Close", value: money(s.close), note: s.last !== null && s.last !== s.close ? `last ${money(s.last)}` : "4:00 PM", tone: s.close === null ? "dim" : "neutral" },
        { id: "change", label: "Change", value: s.change ? signedPct(s.change.pct) : "—", note: s.prev_close ? `vs prev close ${money(s.prev_close.price)}` : "no previous close on file", tone: changeTone },
        { id: "pre", label: "Premarket", value: range(pre), note: pre ? "4:00–9:30 AM" : "no trades", tone: pre ? "neutral" : "dim" },
        { id: "regular", label: "Regular session", value: range(reg), note: reg ? `open ${money(reg.open)} · close ${money(reg.close)}` : "no trades", tone: reg ? "neutral" : "dim" },
        {
          id: "after",
          label: "After hours",
          value: range(after),
          note: after && after.low_at !== null && after.high_at !== null ? `low ${time(after.low_at)} · high ${time(after.high_at)}` : after ? "4:00–8:00 PM" : "no trades",
          tone: after ? "neutral" : "dim",
        },
        { id: "volume", label: "Volume", value: compact(s.volume), note: `$${compact(s.dollar_volume)}${so ? ` ${so}` : ""}`, tone: "neutral" },
        {
          id: "float",
          label: "Float (est.)",
          value: s.float.available && s.float.shares !== undefined ? compact(s.float.shares) : "—",
          note: s.float.available && s.float.as_of !== undefined ? `as of ${etDayShort(s.float.as_of)}` : "no float record by that day",
          tone: s.float.available ? "neutral" : "dim",
        },
        { id: "short", label: "Short interest", value: "—", note: s.short_interest.available ? "" : "not collected", tone: "dim" },
      ]
    : [];

  const alerts: TickerAlertView[] = p.alerts.map((a) => {
    const o = a.outcome;
    const sent = a.status !== "held";
    const pending = p.today ? "not yet" : null;
    return {
      key: a.event_id,
      marker: a.marker ?? null,
      sent,
      time: a.at ? time(a.at, true) : "—",
      kind: alertKind(a, p.symbol),
      detail: (a.body ?? "").split(/\r?\n/)[0] ?? "",
      price: money(o?.ref_price ?? a.price),
      best: o?.best_price !== null && o?.best_price !== undefined ? money(o.best_price) : "—",
      bestPct: signedPct(o?.best_pct),
      bestTone: o?.best_pct ? (o.best_pct > 0 ? "up" : "down") : "dim",
      plus5: o?.plus5_at ? time(o.plus5_at, true) : (pending ?? "no"),
      plus5Tone: o?.plus5_at ? "up" : "dim",
      stop: !o || o.stop === null ? "—" : o.stop_hit_at ? `hit ${time(o.stop_hit_at, true)}` : (pending ?? "not hit"),
      stopTone: o?.stop_hit_at ? "down" : "dim",
      held: sent ? null : capital(a.reason_text ?? "held"),
    };
  });

  const sc = p.scorecard;
  const bestLabel = sc?.best_entry ? (sc.by_kind.find((k) => k.notice === sc.best_entry?.notice)?.label ?? sc.best_entry.notice ?? "") : "";
  const score = sc
    ? [
        { label: "Alerts", value: `${sc.sent} sent`, note: `${sc.held} held` },
        { label: "+5% reached", value: `${sc.reached_5pct} of ${sc.alerts}`, note: so ?? "from each alert's price" },
        { label: "Best entry", value: sc.best_entry ? signedPct(sc.best_entry.best_pct) : "—", note: sc.best_entry ? `${bestLabel}${sc.best_entry.marker ? ` · #${sc.best_entry.marker}` : ""}` : "no alerts" },
        { label: "Missed setups", value: String(sc.missed), note: `of ${sc.setups_armed} armed` },
      ]
    : [];
  const byKind = (sc?.by_kind ?? []).map((k) => ({ key: k.notice, label: k.label, value: `${k.alerts} · +5% ${k.reached_5pct} of ${k.alerts}${k.best_pct !== null ? ` · best ${signedPct(k.best_pct)}` : ""}` }));

  const h = p.halts;
  const sessionWord = (x: string | null | undefined) => (x === "after" ? "after hours" : x === "pre" ? "in the premarket" : "outside regular hours");
  const band = (b: TickerBandWire | null, side: "upper" | "lower") => (b ? `${(b.bp / 100).toFixed(1)}% ${side === "upper" ? "below the upper" : "above the lower"} band at ${time(b.at)} (${money(b.price)} vs ${money(b.band)})` : null);
  const halts = {
    lines: h?.halts.length ? h.halts.map((x) => `${time(x.start, true)} · ${x.reason || x.code || "halt"} · ${x.end ? `resumed ${time(x.end, true)}` : "not resumed"}`) : ["No halts recorded that day."],
    luld: h?.outside_regular ? `The day's high came ${sessionWord(h.move_session)}${s?.high ? ` (${money(s.high.price)} at ${time(s.high.at)})` : ""}. LULD bands only apply 9:30 AM–4:00 PM, so none were active during the move.` : null,
    closestTitle: "Regular session · closest to a modelled band",
    closest: [band(h?.closest?.up ?? null, "upper"), band(h?.closest?.down ?? null, "lower")].filter((x): x is string => !!x),
  };
  if (h && halts.closest.length === 0) halts.closest.push("No band during the regular session.");

  const missedAll: TickerMissedView[] = p.missed.map((m) => {
    const o = m.outcome;
    const stopped = o.stop_hit_at;
    const outcome = stopped ? (o.plus5_at ? `+5%, then stopped ${time(stopped)}` : `stopped ${time(stopped)}`) : o.best_pct !== null ? `best ${signedPct(o.best_pct)}${so ? ` ${so}` : ""}` : "no prints after";
    const extra = m.restart_at ? ` (restart at ${time(m.restart_at, true)})` : m.flagged_at ? ` (flagged ${time(m.flagged_at)})` : "";
    return {
      key: m.episode_id,
      time: time(m.armed_at, true),
      kind: "Pullback",
      levels: [m.trigger !== null ? `trigger ${money(m.trigger)}` : null, m.stop !== null ? `stop ${money(m.stop)}` : null, `armed at ${money(o.ref_price)}`].filter(Boolean).join(" · "),
      outcome,
      outcomeTone: stopped ? (o.plus5_at ? "dim" : "down") : o.plus5_at ? "up" : "dim",
      why: `${capital(m.reason_text)}${extra}`,
    };
  });

  return {
    symbol: p.symbol,
    name: p.name,
    day: p.day,
    dayLabel: dayLabel(p.day),
    dayShort: dayShort(p.day),
    prevDay: p.prev_day?.day ?? null,
    nextDay: p.next_day?.day ?? null,
    today: p.today,
    unavailable: p.available ? null : (UNAVAILABLE[p.unavailable_reason ?? ""] ?? `Nothing to show for this day${p.unavailable_reason ? ` (${p.unavailable_reason.replace(/_/g, " ")})` : ""}.`),
    headline: { price: money(s?.close ?? s?.last), change: s?.change ? signedPct(s.change.pct) : "", tone: changeTone, note: s?.last !== null && s?.last !== undefined && s.last !== s.close ? `last ${money(s.last)} after hours` : "" },
    stats,
    alerts,
    alertSummary: sc ? `${sc.sent} sent · ${sc.held} held` : "",
    score,
    byKind,
    halts,
    missed: missedAll.slice(0, MISSED_SHOWN),
    missedLater: missedAll.slice(MISSED_SHOWN),
    markers: p.markers,
  };
}
