import { normalizeRows, pointOfControl, profileTotal, valueArea } from "./profile.ts";
import { sessionRefFromId, type SessionSchedule } from "./time.ts";
import {
  TIMEFRAME_MS,
  type Candle,
  type ChartIndicatorIncrement,
  type ChartIndicatorsSnapshot,
  type ChartLevels,
  type ChartZones,
  type ChartHalt,
  type ChartIncrement,
  type ChartLuld,
  type ChartSnapshot,
  type Clock,
  type DataQuality,
  type Gap,
  type Halt,
  type LuldPoint,
  type ProfileFigures,
  type ProfileRow,
  type ProfileWire,
  type SessionRef,
  type Timeframe,
} from "./types.ts";

/**
 * Client-side state for one (symbol, timeframe): exactly what the server
 * sent, kept sorted. It never re-aggregates trades. Every change produces a
 * NEW Series object (the signal renderers key on); the candle and gap arrays
 * are reused and mutated in place for O(1) appends, so their identity is not
 * a change signal. `version` counts changes.
 */
export interface Series {
  symbol: string;
  timeframe: Timeframe;
  session: SessionRef | null;
  /** Sorted by t, unique t. */
  candles: Candle[];
  /** Sorted by t, non-overlapping. */
  gaps: Gap[];
  /**
   * Current session's profile. total/poc/valueArea are the server's (snapshot, and every
   * row-changing increment from a server that sends them); recomputed here only for an older
   * server's increments.
   */
  profile: ProfileWire | null;
  /** From the last snapshot (increments do not carry counters). */
  quality: DataQuality | null;
  /** Halts, ascending by start (shared by every timeframe of the symbol). */
  halts: Halt[];
  /** LULD band changes, ascending by t (shared by every timeframe of the symbol). */
  luld: LuldPoint[];
  /**
   * The previous trading day's official close, from the last snapshot (static
   * for the day, so increments do not carry it); null when the server has none.
   */
  prevClose: number | null;
  /**
   * The server's session schedule from the last snapshot (`sessions.v1`; time.ts
   * `SessionSchedule`): what the chart filters regular hours and shades sessions by. Absent or
   * empty from an older server (the fixed ET schedule then).
   */
  sessions?: SessionSchedule;
  version: number;
}

export interface StoreOptions {
  /** Ring-buffer bound per series; the oldest candles and gaps drop first. */
  maxCandles?: number;
  /**
   * Coalesces change notifications: the first change after a notification calls
   * `schedule(flush)`, and every listener hears once when `flush` runs, however many frames were
   * applied meanwhile. Returns a cancel function. Absent = listeners hear every change at once
   * (tests, and anything that reads the store synchronously after applying).
   *
   * Why: a candle and its indicator values (and its levels and zones) are separate socket
   * messages, each its own browser task, so with one notification per apply React rendered the
   * chart once per message, and between the `chart.update` and its `chart.indicator` it drew the
   * new candle under the previous values. The hosts pass `frameScheduler`.
   */
  schedule?: (flush: () => void) => () => void;
  /**
   * The profile's total, POC and value area come only from the server (every increment with
   * profile rows carries them since the profile-figures change). The web sets it: it always
   * ships with its server, so the recompute in `withDerived` only ever ran against servers older
   * than it. An increment without them keeps the figures held. The phone leaves it off while
   * installed builds may meet an older server.
   */
  serverProfileFigures?: boolean;
}

/**
 * One flush per display frame (requestAnimationFrame), for `StoreOptions.schedule` on the web and
 * the phone. A hidden page gets no animation frames at all, so there it falls back to a timer
 * (which the browser throttles to about once a second): the store still settles, and whatever
 * reads it (the clock's trading day, for one) still moves, at no drawing cost.
 *
 * In a browser the frame also races a timer (FRAME_BACKSTOP_MS), because the page can be hidden
 * AFTER the frame was asked for: a tab switched away during a live session almost always leaves
 * one pending, the browser then never runs it until the tab comes back, and while one is pending
 * the store schedules nothing else, so it would stop notifying entirely (the visibility check
 * below only sees the state at scheduling time). Whichever fires first flushes and cancels the
 * other. Not on the phone (no `document`): there a backgrounded app draws nothing and reads
 * nothing, and a timer per frame would cost the JS thread for no one.
 */
export function frameScheduler(flush: () => void): () => void {
  // chart-core has no DOM types (it runs on the phone too): read the host's globals untyped.
  const g = globalThis as { requestAnimationFrame?: (cb: () => void) => number; cancelAnimationFrame?: (id: number) => void; document?: { visibilityState?: string } };
  const raf = g.requestAnimationFrame;
  if (raf && g.document?.visibilityState !== "hidden") {
    if (!g.document) {
      const id = raf(() => flush());
      return () => g.cancelAnimationFrame?.(id);
    }
    let done = false;
    let frame = 0;
    let backstop: ReturnType<typeof setTimeout> | undefined;
    const cancel = () => {
      done = true;
      g.cancelAnimationFrame?.(frame);
      clearTimeout(backstop);
    };
    const run = () => {
      if (done) return;
      cancel();
      flush();
    };
    frame = raf(run);
    backstop = setTimeout(run, FRAME_BACKSTOP_MS);
    return cancel;
  }
  const id = setTimeout(flush, 16);
  return () => clearTimeout(id);
}

/**
 * How long a browser flush waits for its animation frame before a timer runs it instead
 * (`frameScheduler`): well past a visible page's frame (16 ms at 60 Hz, 33 at 30), so it does not
 * fire while frames come, yet a page hidden mid-wait keeps settling (throttled to ~1 Hz there).
 */
const FRAME_BACKSTOP_MS = 250;

const DEFAULT_MAX = 5000;

export function seriesKey(symbol: string, timeframe: Timeframe): string {
  return `${symbol}|${timeframe}`;
}

/** Index of the first element with t >= target (binary search). */
export function lowerBound<T extends { t: number }>(list: readonly T[], target: number): number {
  let lo = 0;
  let hi = list.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (list[mid]!.t < target) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** Insert or replace by t, keeping order. */
function upsert<T extends { t: number }>(list: T[], item: T): void {
  const last = list[list.length - 1];
  // Fast paths: the forming candle (same t as last) or a strictly newer one.
  if (!last || item.t > last.t) {
    list.push(item);
    return;
  }
  if (item.t === last.t) {
    list[list.length - 1] = item;
    return;
  }
  // Older interval (late trade, correction, cancel): replace it in place. It
  // never touches the forming candle; the server decides the interval.
  const i = lowerBound(list, item.t);
  if (list[i]?.t === item.t) list[i] = item;
  else list.splice(i, 0, item);
}

function removeAt<T extends { t: number }>(list: T[], t: number): void {
  const i = lowerBound(list, t);
  if (list[i]?.t === t) list.splice(i, 1);
}

/**
 * A candle at t supersedes any gap covering t. A multi-interval gap is split
 * around the candle's interval rather than dropped, so the intervals that are
 * still empty stay marked. Split pieces keep the reason; a volume_only_trades
 * gap's volume cannot be apportioned, so it stays on the left piece (or the
 * right one if there is no left piece).
 */
function supersedeGaps(gaps: Gap[], t: number, tfMs: number): void {
  const i = lowerBound(gaps, t + 1) - 1;
  if (i < 0) return;
  const g = gaps[i]!;
  if (!(g.t <= t && t < g.end)) return;
  const pieces: Gap[] = [];
  if (g.t < t) pieces.push({ ...g, end: t, intervals: Math.round((t - g.t) / tfMs) });
  if (t + tfMs < g.end) pieces.push({ ...g, t: t + tfMs, intervals: Math.round((g.end - t - tfMs) / tfMs), v: pieces.length ? 0 : g.v });
  gaps.splice(i, 1, ...pieces);
}

/**
 * The profile after a change. `figures` are the server's (an increment with `total`): taken as
 * sent, so the client only merges rows. Without them, `held` (the profile's figures so far) when
 * the host takes figures from the server only (StoreOptions.serverProfileFigures, the web), else
 * (a phone that may meet an older server) total, POC and value area are recomputed here, the
 * same rules as crates/chart-engine/src/profile.rs.
 */
function withDerived(p: Omit<ProfileWire, "total" | "poc" | "valueArea">, figures?: ProfileFigures, held?: Pick<ProfileWire, "total" | "poc" | "valueArea"> | null): ProfileWire {
  const rows = normalizeRows(p.rows, p.bin);
  if (figures && typeof figures.total === "number") return { bin: p.bin, anchor: p.anchor, rows, total: figures.total, poc: figures.poc ?? null, valueArea: figures.valueArea ?? null };
  if (held !== undefined) return { bin: p.bin, anchor: p.anchor, rows, total: held?.total ?? 0, poc: held?.poc ?? null, valueArea: held?.valueArea ?? null };
  return { bin: p.bin, anchor: p.anchor, rows, total: profileTotal(rows), poc: pointOfControl(rows), valueArea: valueArea(rows, p.bin) };
}

const sameBin = (a: number, b: number) => Math.abs(a - b) < 1e-9;

export function applySnapshot(prev: Series | undefined, snap: ChartSnapshot, opts: StoreOptions = {}): Series {
  const series: Series = {
    symbol: snap.symbol,
    timeframe: snap.timeframe,
    session: snap.session,
    candles: [...snap.candles].sort((a, b) => a.t - b.t),
    gaps: [...snap.gaps].sort((a, b) => a.t - b.t),
    // A snapshot is authoritative: its poc/valueArea are taken as sent.
    profile: snap.profile ? { ...snap.profile, rows: [...snap.profile.rows] } : null,
    quality: snap.quality,
    halts: [...(snap.halts ?? [])].sort((a, b) => a.start - b.start),
    luld: [...(snap.luld ?? [])].sort((a, b) => a.t - b.t),
    prevClose: snap.prevClose ?? null,
    ...(snap.sessions?.length ? { sessions: [...snap.sessions].sort((a, b) => a.start - b.start) } : {}),
    version: (prev?.version ?? 0) + 1,
  };
  trim(series, opts.maxCandles ?? DEFAULT_MAX);
  return series;
}

/**
 * Merges an increment's touched bins (absolute totals) into the profile of
 * `session`. Rows for an older session than the current one are ignored
 * (e.g. a late print from premarket arriving just after the open); rows for a
 * newer session start that session's profile.
 */
function mergeProfile(s: Series, session: string, bin: number, rows: readonly ProfileRow[], figures?: ProfileFigures, serverOnly = false): void {
  const current = s.profile;
  if (current && !sameBin(current.bin, bin)) return; // a stale subscription's bin
  if (!current || current.anchor !== session) {
    const next = sessionRefFromId(session, s.sessions);
    const curStart = s.session?.start ?? -Infinity;
    if (current && (!next || next.start <= curStart)) return;
    if (rows.length === 0 && current) return;
    s.profile = withDerived({ bin, anchor: session, rows: [] }, undefined, serverOnly ? null : undefined);
    if (next) s.session = next;
  }
  if (rows.length === 0) return;
  const byKey = new Map<number, ProfileRow>();
  for (const r of s.profile!.rows) byKey.set(Math.round(r.p / bin), r);
  for (const r of rows) byKey.set(Math.round(r.p / bin), { p: r.p, v: r.v });
  s.profile = withDerived({ bin, anchor: session, rows: [...byKey.values()] }, figures, serverOnly ? s.profile : undefined);
}

/** Applies one increment, returning a new Series object. */
export function applyIncrement(prev: Series, inc: ChartIncrement, opts: StoreOptions = {}): Series {
  const next: Series = { ...prev, version: prev.version + 1 };
  const tfMs = TIMEFRAME_MS[prev.timeframe];
  if (inc.type === "chart.update") {
    upsert(next.candles, inc.candle);
    supersedeGaps(next.gaps, inc.candle.t, tfMs);
  } else if (inc.type === "chart.remove") {
    removeAt(next.candles, inc.t);
    removeAt(next.gaps, inc.t);
  } else {
    upsert(next.gaps, inc.gap);
  }
  mergeProfile(next, inc.session, inc.profileBin, inc.profile, inc, opts.serverProfileFigures === true);
  trim(next, opts.maxCandles ?? DEFAULT_MAX);
  return next;
}

/** Upserts a halt (by start) into a series, returning a new Series. */
export function applyHalt(prev: Series, halt: Halt): Series {
  const halts = [...prev.halts];
  const i = halts.findIndex((h) => h.start === halt.start);
  if (i >= 0) halts[i] = halt;
  else {
    halts.push(halt);
    halts.sort((a, b) => a.start - b.start);
  }
  return { ...prev, halts, version: prev.version + 1 };
}

/** Upserts a LULD point (by t) into a series, returning a new Series. */
export function applyLuld(prev: Series, point: LuldPoint): Series {
  const luld = [...prev.luld];
  const i = lowerBound(luld, point.t);
  if (luld[i]?.t === point.t) luld[i] = point;
  else luld.splice(i, 0, point);
  return { ...prev, luld, version: prev.version + 1 };
}

function trim(s: Series, max: number): void {
  if (s.candles.length > max) s.candles.splice(0, s.candles.length - max);
  const oldest = s.candles[0]?.t;
  if (oldest !== undefined && s.gaps.length > 0 && s.gaps[0]!.t < oldest) {
    s.gaps.splice(0, lowerBound(s.gaps, oldest));
  }
}

/**
 * The server's indicator values for one chart key (symbol, timeframe, rth), `indicators.v1`:
 * exactly what `chart.indicators` and `chart.indicator` said, kept sorted by bar time. Like
 * Series, every change produces a NEW object (what renderers key on) while the arrays inside
 * are reused and mutated in place, so a forming-bar update is O(lines), not a copy.
 */
export interface IndicatorValues {
  symbol: string;
  timeframe: Timeframe;
  rth: boolean;
  /** Bar times, ascending, unique. */
  t: number[];
  /** By canonical spec key (indicators.ts `specKey`); each line's values parallel to `t`, NaN = none. */
  studies: Map<string, { lines: { name: string; style: "line" | "histogram"; values: number[] }[] }>;
  /** The last snapshot left older bars out (the host asked for a `limit`): see `applyIndicatorIncrement`. */
  truncated: boolean;
  version: number;
}

export function indicatorsKey(symbol: string, timeframe: Timeframe, rth: boolean): string {
  return `${symbol}|${timeframe}|${rth ? "rth" : "eth"}`;
}

/**
 * A `chart.indicators` replaces everything held for its chart key, not only the keys it names:
 * the server always sends a socket the union of its slots' studies on that chart key
 * (crates/server/src/indicators.rs `Studies::union`), so a key it leaves out is one no slot
 * asks for any more, and the `t` it carries is the whole series (a seek or day switch must not
 * leave the old day's bars behind).
 */
export function applyIndicatorsSnapshot(prev: IndicatorValues | undefined, snap: ChartIndicatorsSnapshot, max = DEFAULT_MAX): IndicatorValues {
  const n = snap.t.length;
  const studies: IndicatorValues["studies"] = new Map();
  for (const st of snap.studies) {
    studies.set(st.key, {
      lines: st.lines.map((l) => {
        const values = Array.from({ length: n }, () => Number.NaN);
        l.values.forEach((v, i) => {
          if (v !== null) values[l.from + i] = v;
        });
        return { name: l.name, style: l.style, values };
      }),
    });
  }
  const next: IndicatorValues = { symbol: snap.symbol, timeframe: snap.timeframe, rth: snap.rth, t: [...snap.t], studies, truncated: snap.truncated === true, version: (prev?.version ?? 0) + 1 };
  trimIndicators(next, max);
  return next;
}

/**
 * Indicators the server computed over candles that are not a live chart's: the Insights ticker
 * page's `insights.ticker.indicators` (crates/server/src/indicators.rs `day_snapshot`: the
 * `chart.indicators` shape over the page's 1-minute bars, plus its `day`), as the values
 * CandleChart's `serverIndicators` draws. Null (none yet) stays null.
 */
export function tickerIndicatorValues(frame: Pick<ChartIndicatorsSnapshot, "symbol" | "rth" | "t" | "studies"> | null): IndicatorValues | null {
  return frame ? applyIndicatorsSnapshot(undefined, { type: "chart.indicators", symbol: frame.symbol, timeframe: "1m", rth: frame.rth, t: frame.t, studies: frame.studies }) : null;
}

/**
 * Applies one `chart.indicator`: the bars in `removed` go first, then each point upserts its
 * keys' values (a new bar gets NaN for the keys the point does not name: they have no value
 * there yet). A key no snapshot introduced is ignored, as the server's increments carry every
 * computed key of the chart, including ones only another socket asked for.
 */
export function applyIndicatorIncrement(prev: IndicatorValues, inc: ChartIndicatorIncrement, max = DEFAULT_MAX): IndicatorValues {
  const next: IndicatorValues = { ...prev, version: prev.version + 1 };
  const all = [...next.studies.values()].flatMap((s) => s.lines);
  for (const t of inc.removed ?? []) {
    const i = lowerBoundT(next.t, t);
    if (next.t[i] !== t) continue;
    next.t.splice(i, 1);
    for (const l of all) l.values.splice(i, 1);
  }
  for (const p of inc.points) {
    // A revision older than this ring's first bar (the phone keeps fewer bars than the server):
    // nothing shows it, and inserting it would only push out a newer bar. Likewise below a
    // limited snapshot's first bar: a bar this client was never sent, which inserted would sit
    // alone before the gap with only the moved key's values.
    if ((next.t.length >= max || next.truncated) && p.t < next.t[0]!) continue;
    const last = next.t.length - 1;
    // Fast paths: the forming bar (same t as the last) or a new one after it.
    let i = last >= 0 && next.t[last] === p.t ? last : last < 0 || p.t > next.t[last]! ? -1 : lowerBoundT(next.t, p.t);
    if (i === -1) {
      next.t.push(p.t);
      for (const l of all) l.values.push(Number.NaN);
      i = next.t.length - 1;
    } else if (next.t[i] !== p.t) {
      next.t.splice(i, 0, p.t);
      for (const l of all) l.values.splice(i, 0, Number.NaN);
    }
    for (const key in p.v) {
      const study = next.studies.get(key);
      if (!study) continue;
      p.v[key]!.forEach((v, k) => {
        const line = study.lines[k];
        if (line) line.values[i] = v === null ? Number.NaN : v;
      });
    }
  }
  trimIndicators(next, max);
  return next;
}

function lowerBoundT(list: readonly number[], target: number): number {
  let lo = 0;
  let hi = list.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (list[mid]! < target) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** The same ring bound as the candles: a live day appends bars the server pops from its front without telling. */
function trimIndicators(v: IndicatorValues, max: number): void {
  const extra = v.t.length - max;
  if (extra <= 0) return;
  v.t.splice(0, extra);
  for (const s of v.studies.values()) for (const l of s.lines) l.values.splice(0, extra);
}

type Listener = () => void;

/**
 * Holds one Series per (symbol, timeframe) plus the feed clock. Switching
 * timeframe never touches another timeframe's series: each is its own
 * server subscription. Framework-free; React binds via subscribe +
 * getSeries/getClock (useSyncExternalStore).
 */
export class ChartStore {
  private series = new Map<string, Series>();
  private indicators = new Map<string, IndicatorValues>();
  /** `chart.levels` by seriesKey, `chart.zones` by indicatorsKey (levels.v1, zones.v1): as sent. */
  private levels = new Map<string, ChartLevels>();
  private zones = new Map<string, ChartZones>();
  private listeners = new Set<Listener>();
  private clock: Clock | null = null;
  private readonly opts: StoreOptions;
  /** The scheduled flush's cancel, while one is pending (`StoreOptions.schedule`). */
  private pending: (() => void) | null = null;
  constructor(opts: StoreOptions = {}) {
    this.opts = opts;
  }

  getSeries(symbol: string, timeframe: Timeframe): Series | undefined {
    return this.series.get(seriesKey(symbol, timeframe));
  }

  /** The server's indicator values for (symbol, timeframe, rth), `indicators.v1`; undefined until its first `chart.indicators`. */
  getIndicators(symbol: string, timeframe: Timeframe, rth: boolean): IndicatorValues | undefined {
    return this.indicators.get(indicatorsKey(symbol, timeframe, rth));
  }

  applyIndicators(snap: ChartIndicatorsSnapshot): void {
    const key = indicatorsKey(snap.symbol, snap.timeframe, snap.rth);
    this.indicators.set(key, applyIndicatorsSnapshot(this.indicators.get(key), snap, this.opts.maxCandles));
    this.emit();
  }

  /** Increments before the chart key's first snapshot are dropped (the snapshot always comes first, then carries them). */
  applyIndicatorIncrement(inc: ChartIndicatorIncrement): boolean {
    const key = indicatorsKey(inc.symbol, inc.timeframe, inc.rth);
    const prev = this.indicators.get(key);
    if (!prev) return false;
    this.indicators.set(key, applyIndicatorIncrement(prev, inc, this.opts.maxCandles));
    this.emit();
    return true;
  }

  /**
   * Forgets (symbol, timeframe, rth)'s values until its next `chart.indicators`. For a chart that
   * just asked for that rth again: the set held for it stopped updating when the chart last left
   * it (the server sends a key only while a slot asks for it), so drawn meanwhile it would show
   * lines frozen at that moment and blank after, for as long as the answer takes (seconds for a
   * 4000-bar snapshot on cellular). Dropped, the chart draws nothing until then.
   */
  dropIndicators(symbol: string, timeframe: Timeframe, rth: boolean): void {
    if (this.indicators.delete(indicatorsKey(symbol, timeframe, rth))) this.emit();
  }

  /** The server's levels for (symbol, timeframe), `levels.v1`; undefined until its first `chart.levels`. */
  getLevels(symbol: string, timeframe: Timeframe): ChartLevels | undefined {
    return this.levels.get(seriesKey(symbol, timeframe));
  }

  /** A `chart.levels` is a whole state: it replaces what was held. */
  applyLevels(m: ChartLevels): void {
    this.levels.set(seriesKey(m.symbol, m.timeframe), m);
    this.emit();
  }

  /** The server's zones for (symbol, timeframe, rth), `zones.v1`; undefined until its first `chart.zones`. */
  getZones(symbol: string, timeframe: Timeframe, rth: boolean): ChartZones | undefined {
    return this.zones.get(indicatorsKey(symbol, timeframe, rth));
  }

  applyZones(m: ChartZones): void {
    this.zones.set(indicatorsKey(m.symbol, m.timeframe, m.rth), m);
    this.emit();
  }

  /**
   * Forgets (symbol, timeframe, rth)'s zones until the next `chart.zones`: for a chart that just
   * asked for them again (layer back on, extended hours flipped), whose held set stopped updating
   * when it last stopped asking. As `dropIndicators`.
   */
  dropZones(symbol: string, timeframe: Timeframe, rth: boolean): void {
    if (this.zones.delete(indicatorsKey(symbol, timeframe, rth))) this.emit();
  }

  getClock(): Clock | null {
    return this.clock;
  }

  applySnapshot(snap: ChartSnapshot): void {
    const key = seriesKey(snap.symbol, snap.timeframe);
    this.series.set(key, applySnapshot(this.series.get(key), snap, this.opts));
    if (snap.clock) this.clock = snap.clock;
    this.emit();
  }

  /** Increments for a series without a snapshot yet are dropped (a snapshot always comes first). */
  applyIncrement(inc: ChartIncrement): boolean {
    const key = seriesKey(inc.symbol, inc.timeframe);
    const prev = this.series.get(key);
    if (!prev) return false;
    this.series.set(key, applyIncrement(prev, inc, this.opts));
    this.emit();
    return true;
  }

  /** Halts and LULD points belong to the symbol: every timeframe's series of it takes them. */
  applySymbolEvent(m: ChartHalt | ChartLuld): void {
    let changed = false;
    for (const [key, s] of this.series) {
      if (s.symbol !== m.symbol) continue;
      this.series.set(key, m.type === "chart.halt" ? applyHalt(s, m.halt) : applyLuld(s, m.point));
      changed = true;
    }
    if (changed) this.emit();
  }

  applyClock(clock: Clock): void {
    this.clock = clock;
    this.emit();
  }

  clear(symbol?: string, timeframe?: Timeframe): void {
    if (symbol && timeframe) {
      this.series.delete(seriesKey(symbol, timeframe));
      this.levels.delete(seriesKey(symbol, timeframe));
      for (const rth of [true, false]) {
        this.indicators.delete(indicatorsKey(symbol, timeframe, rth));
        this.zones.delete(indicatorsKey(symbol, timeframe, rth));
      }
    } else {
      this.series.clear();
      this.indicators.clear();
      this.levels.clear();
      this.zones.clear();
    }
    this.emit();
  }

  subscribe(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  /** Notifies now whatever a scheduled flush would have (a host tearing down, or a test). */
  flush(): void {
    if (!this.pending) return;
    this.pending();
    this.pending = null;
    this.notify();
  }

  /** Cancels a scheduled flush (the host is going away; nobody is left to draw it). */
  dispose(): void {
    this.pending?.();
    this.pending = null;
  }

  private emit(): void {
    const schedule = this.opts.schedule;
    if (!schedule) return this.notify();
    if (this.pending) return;
    // A scheduler may run the flush before returning (a test's): then nothing is pending.
    let ran = false;
    const cancel = schedule(() => {
      ran = true;
      this.pending = null;
      this.notify();
    });
    if (!ran) this.pending = cancel;
  }

  private notify(): void {
    for (const fn of this.listeners) fn();
  }
}
