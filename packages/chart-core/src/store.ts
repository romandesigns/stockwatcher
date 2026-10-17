import { normalizeRows, pointOfControl, profileTotal, valueArea } from "./profile.ts";
import { sessionRefFromId } from "./time.ts";
import {
  TIMEFRAME_MS,
  type Candle,
  type ChartIncrement,
  type ChartSnapshot,
  type Clock,
  type DataQuality,
  type Gap,
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
  /** Current session's profile; total/poc/valueArea recomputed after every increment. */
  profile: ProfileWire | null;
  /** From the last snapshot (increments do not carry counters). */
  quality: DataQuality | null;
  version: number;
}

export interface StoreOptions {
  /** Ring-buffer bound per series; the oldest candles and gaps drop first. */
  maxCandles?: number;
}

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

function withDerived(p: Omit<ProfileWire, "total" | "poc" | "valueArea">): ProfileWire {
  const rows = normalizeRows(p.rows, p.bin);
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
function mergeProfile(s: Series, session: string, bin: number, rows: readonly ProfileRow[]): void {
  const current = s.profile;
  if (current && !sameBin(current.bin, bin)) return; // a stale subscription's bin
  if (!current || current.anchor !== session) {
    const next = sessionRefFromId(session);
    const curStart = s.session?.start ?? -Infinity;
    if (current && (!next || next.start <= curStart)) return;
    if (rows.length === 0 && current) return;
    s.profile = withDerived({ bin, anchor: session, rows: [] });
    if (next) s.session = next;
  }
  if (rows.length === 0) return;
  const byKey = new Map<number, ProfileRow>();
  for (const r of s.profile!.rows) byKey.set(Math.round(r.p / bin), r);
  for (const r of rows) byKey.set(Math.round(r.p / bin), { p: r.p, v: r.v });
  s.profile = withDerived({ bin, anchor: session, rows: [...byKey.values()] });
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
  mergeProfile(next, inc.session, inc.profileBin, inc.profile);
  trim(next, opts.maxCandles ?? DEFAULT_MAX);
  return next;
}

function trim(s: Series, max: number): void {
  if (s.candles.length > max) s.candles.splice(0, s.candles.length - max);
  const oldest = s.candles[0]?.t;
  if (oldest !== undefined && s.gaps.length > 0 && s.gaps[0]!.t < oldest) {
    s.gaps.splice(0, lowerBound(s.gaps, oldest));
  }
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
  private listeners = new Set<Listener>();
  private clock: Clock | null = null;
  private readonly opts: StoreOptions;
  constructor(opts: StoreOptions = {}) {
    this.opts = opts;
  }

  getSeries(symbol: string, timeframe: Timeframe): Series | undefined {
    return this.series.get(seriesKey(symbol, timeframe));
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

  applyClock(clock: Clock): void {
    this.clock = clock;
    this.emit();
  }

  clear(symbol?: string, timeframe?: Timeframe): void {
    if (symbol && timeframe) this.series.delete(seriesKey(symbol, timeframe));
    else this.series.clear();
    this.emit();
  }

  subscribe(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit(): void {
    for (const fn of this.listeners) fn();
  }
}
