// FIXTURE DATA: NOT MARKET DATA.
//
// A deterministic stand-in for the server until it forwards chart-engine
// output over /ws. It generates synthetic prints for one fictional symbol
// from a seeded PRNG and emits EXACTLY the engine's wire shapes
// (crates/chart-engine/src/wire.rs: chart.snapshot / chart.update /
// chart.gap / tape.trade), on a virtual clock with play/pause/step/speed.
//
// Its aggregation follows the engine's documented rules (Appendix A of
// docs/architecture/simulator-and-chart.md) closely enough to exercise the
// UI, and exists ONLY to feed it; the chart packages never aggregate:
// - O/H/L/C from OHLC-qualifying prints, V from volume-qualifying sizes,
//   half-open epoch-aligned intervals, final once the clock passes the end;
// - empty runs are `no_trades` gaps, split at session boundaries;
// - the profile counts OHLC prints and odd lots, not other volume-only prints
//   (average price), with POC/value area computed by chart-core's
//   engine-identical helpers;
// - tape `counted`/`reason` use the engine's vocabulary.
// Not modelled: corrections and cancels (so no chart.remove), late-but-
// applied prints, volume_only_trades gaps (volume-only prints are generated
// only after an OHLC print in the same 15 s interval), early closes.

import {
  TIMEFRAME_MS,
  TIMEFRAMES,
  etToUtc,
  pointOfControl,
  profileTotal,
  sessionBands,
  valueArea,
  type Candle,
  type ChartMessage,
  type ChartSnapshot,
  type Clock,
  type Counted,
  type DataQuality,
  type Gap,
  type ProfileRow,
  type SessionBand,
  type TapeTrade,
  type Timeframe,
} from "@stockwatcher/chart-core";
import type { ChartFeed, ChartSubscription, ReplayCommand } from "../feed/types.ts";

export const FIXTURE_SYMBOL = "SMPL";
export const FIXTURE_DAY = { year: 2026, month: 10, day: 15 } as const;
const DAY_ID = "2026-10-15";

/** A print as the engine would report it on the tape (minus type/symbol). */
export interface FixtureTrade {
  seq: number;
  t: number;
  p: number;
  s: number;
  exchange: string;
  conditions: string[];
  counted: Counted;
  reason: string | null;
  session: string;
}

/** mulberry32: tiny, fast, deterministic. */
export function rng(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const et = (h: number, m: number, s = 0) => etToUtc(FIXTURE_DAY.year, FIXTURE_DAY.month, FIXTURE_DAY.day, h, m, s);
const BANDS: SessionBand[] = sessionBands(FIXTURE_DAY);
const bandOf = (t: number) => BANDS.find((b) => t >= b.start && t < b.end) ?? BANDS[BANDS.length - 1]!;
const sessionIdOf = (t: number) => `${DAY_ID}/${bandOf(t).kind}`;

/** Piecewise-linear "story" the walk is pulled toward: quiet pre, a 10:20 expansion, a fade. */
function target(t: number): number {
  const pts: [number, number][] = [
    [BANDS[0]!.start, 3.02],
    [et(4, 0), 3.06],
    [et(9, 29), 3.16],
    [et(10, 15), 3.21],
    [et(10, 34), 3.47],
    [et(11, 30), 3.39],
    [et(16, 0), 3.36],
    [BANDS[3]!.end, 3.33],
  ];
  for (let i = 1; i < pts.length; i++) {
    const [t1, p1] = pts[i]!;
    const [t0, p0] = pts[i - 1]!;
    if (t <= t1) return p0 + ((p1 - p0) * (t - t0)) / (t1 - t0);
  }
  return pts[pts.length - 1]![1];
}

/** Mean seconds between prints per session; a halt-like silence at 10:05–10:06:30. */
const MEAN_GAP_S = { overnight: 55, pre: 7, regular: 1.1, after: 9 } as const;
const SILENCE: [number, number] = [et(10, 5), et(10, 6, 30)];
const EXCHANGES = ["V", "P", "K", "Q", "Z", "N", "D"];

export function generateTrades(seed = 7): FixtureTrade[] {
  const r = rng(seed);
  const end = BANDS[3]!.end;
  const out: FixtureTrade[] = [];
  let t = BANDS[0]!.start + 1_000;
  let p = 3.02;
  let seq = 0;
  let lastOhlc15s = -1; // 15 s interval of the last OHLC print
  const officials: [number, string][] = [
    [et(9, 30), "Q"],
    [et(16, 0), "M"],
  ];
  const push = (tr: Omit<FixtureTrade, "seq" | "exchange" | "session">) => out.push({ ...tr, seq: seq++, exchange: EXCHANGES[Math.floor(r() * EXCHANGES.length)]!, session: sessionIdOf(tr.t) });
  while (t < end) {
    const kind = bandOf(t).kind;
    const burst = t >= et(10, 18) && t < et(10, 36);
    t += Math.max(1, Math.round(-Math.log(1 - r()) * MEAN_GAP_S[kind] * (burst ? 0.35 : 1) * 1000));
    if (t >= SILENCE[0] && t < SILENCE[1]) t = SILENCE[1] + Math.round(r() * 800);
    if (t >= end) break;
    // Official open/close prints update nothing (conditions Q / M).
    while (officials.length && officials[0]![0] <= t) {
      const [ot, code] = officials.shift()!;
      push({ t: ot, p: Math.round(p * 100) / 100, s: 100 * (50 + Math.floor(r() * 300)), conditions: [code], counted: `excluded:condition_${code}`, reason: `condition_${code}` });
    }
    const vol = kind === "regular" ? 0.004 : 0.0025;
    p += (target(t) - p) * 0.03 + (r() - 0.5) * vol * p * (burst ? 1.8 : 1);
    const price = Math.round(p * 100) / 100;
    const base = kind === "regular" ? ["@"] : ["@", "T"]; // T: extended hours, counts fully
    const u = r();
    const afterOhlc = Math.floor(t / 15_000) === lastOhlc15s;
    if (u < 0.12 && afterOhlc) {
      push({ t, p: price, s: 1 + Math.floor(r() * 99), conditions: base, counted: "volume_only", reason: "odd_lot" });
    } else if (u < 0.13 && afterOhlc) {
      push({ t, p: Math.round((p + (r() - 0.5) * 0.06) * 100) / 100, s: 100 * (5 + Math.floor(r() * 20)), conditions: ["W"], counted: "volume_only", reason: "condition_W" });
    } else if (u < 0.145) {
      // Arrived after the settle window: shown, never counted.
      push({ t, p: Math.round((p - 0.02 - r() * 0.04) * 100) / 100, s: 100 * (1 + Math.floor(r() * 9)), conditions: base, counted: "excluded:late", reason: null });
    } else {
      const lots = Math.ceil(r() ** 3 * (burst ? 60 : 25));
      push({ t, p: price, s: lots * 100, conditions: base, counted: "ohlc_and_volume", reason: null });
      lastOhlc15s = Math.floor(t / 15_000);
      // Now and then the feed repeats a print: same identity, ignored.
      if (r() < 0.006) out.push({ ...out[out.length - 1]!, counted: "excluded:duplicate" });
    }
  }
  return out;
}

const countsOhlc = (tr: FixtureTrade) => tr.counted === "ohlc_and_volume";
const countsVolume = (tr: FixtureTrade) => tr.counted === "ohlc_and_volume" || tr.counted === "volume_only";
/** Engine rule: the profile takes OHLC prints and odd lots, not other volume-only prints. */
const countsProfile = (tr: FixtureTrade) => countsOhlc(tr) || tr.reason === "odd_lot";

/** No-trade gap(s) over [from, to), split at session boundaries (engine: a gap lies inside one session). */
function gapsBetween(from: number, to: number, tfMs: number): Gap[] {
  const out: Gap[] = [];
  let t = from;
  while (t < to) {
    const band = bandOf(t);
    const end = Math.min(to, Math.ceil(band.end / tfMs) * tfMs);
    out.push({ t, end, reason: "no_trades", v: 0, intervals: Math.round((end - t) / tfMs) });
    t = end;
  }
  return out;
}

/** Incremental per-timeframe aggregation (the fixture's stand-in for chart-engine). */
class TfAggregator {
  readonly candles: Candle[] = [];
  readonly gaps: Gap[] = [];
  readonly tf: Timeframe;
  constructor(tf: Timeframe) {
    this.tf = tf;
  }

  /** Returns what changed: the candle, and any gaps the print closed. */
  add(tr: FixtureTrade): { candle?: Candle; gaps: Gap[] } {
    if (!countsVolume(tr)) return { gaps: [] };
    const ms = TIMEFRAME_MS[this.tf];
    const start = Math.floor(tr.t / ms) * ms;
    const last = this.candles[this.candles.length - 1];
    if (last && last.t === start) {
      const c: Candle = { ...last, v: last.v + tr.s, n: last.n + 1 };
      if (countsOhlc(tr)) {
        c.h = Math.max(c.h, tr.p);
        c.l = Math.min(c.l, tr.p);
        c.c = tr.p;
      }
      this.candles[this.candles.length - 1] = c;
      return { candle: c, gaps: [] };
    }
    // generateTrades never lets a volume-only print open an interval.
    if (!countsOhlc(tr)) return { gaps: [] };
    const gaps = last && start > last.t + ms ? gapsBetween(last.t + ms, start, ms) : [];
    this.gaps.push(...gaps);
    const c: Candle = { t: start, o: tr.p, h: tr.p, l: tr.p, c: tr.p, v: tr.s, n: 1, final: false };
    this.candles.push(c);
    return { candle: c, gaps };
  }

  /** Finalizes the forming candle once the clock reaches its end (half-open interval). */
  finalize(now: number): Candle | undefined {
    const f = this.candles[this.candles.length - 1];
    if (f && !f.final && now >= f.t + TIMEFRAME_MS[this.tf]) {
      const c = { ...f, final: true };
      this.candles[this.candles.length - 1] = c;
      return c;
    }
    return undefined;
  }
}

const ZERO_QUALITY: DataQuality = {
  duplicates: 0,
  lateExcluded: 0,
  lateApplied: 0,
  outOfOrderReordered: 0,
  correctionsApplied: 0,
  correctionsUnapplied: 0,
  cancelsApplied: 0,
  cancelsUnapplied: 0,
  conditionExcluded: 0,
  volumeOnly: 0,
  unrecognizedConditions: 0,
  outsideSession: 0,
};

export interface FixtureOptions {
  seed?: number;
  /** Virtual start time; default 10:40:00 ET on the fixture day. */
  start?: number;
  speed?: number;
  playing?: boolean;
  /** Wall-clock tick in ms. */
  tickMs?: number;
  /** Virtual ms per wall ms at speed "Max" (0). */
  maxSpeed?: number;
  /** Inject timers (tests). */
  setInterval?: (fn: () => void, ms: number) => unknown;
  clearInterval?: (h: unknown) => void;
}

export const TAPE_HISTORY = 40;

/** The fixture feed: the same ChartFeed interface as the server feed. */
export function createFixtureFeed(opts: FixtureOptions = {}): ChartFeed & { now(): number; trades: FixtureTrade[] } {
  const trades = generateTrades(opts.seed ?? 7);
  const aggs = new Map<Timeframe, TfAggregator>(TIMEFRAMES.map((tf) => [tf, new TfAggregator(tf)]));
  /** Profile volume per exact cent price for the current session (exact for any bin, like the engine). */
  let perPrice = new Map<number, number>();
  let quality: DataQuality = { ...ZERO_QUALITY };
  let session = "";
  let cursor = 0;
  let now = opts.start ?? et(10, 40);
  let speed = opts.speed ?? 10;
  let playing = opts.playing ?? true;
  const tickMs = opts.tickMs ?? 50;
  const maxSpeed = opts.maxSpeed ?? 1200;
  const subs = new Set<{ sub: ChartSubscription; emit: (m: ChartMessage) => void }>();
  let lastClockEmit = 0;

  const clock = (): Clock => ({ mode: "replay", now, speed, playing });
  const cents = (p: number) => Math.round(p * 100);
  const binCents = (bin: number) => Math.max(1, Math.round(bin * 100));
  const binKey = (c: number, bin: number) => Math.floor(c / binCents(bin));
  const binRows = (bin: number, onlyKeys?: Set<number>): ProfileRow[] => {
    const acc = new Map<number, number>();
    for (const [c, v] of perPrice) {
      const k = binKey(c, bin);
      if (!onlyKeys || onlyKeys.has(k)) acc.set(k, (acc.get(k) ?? 0) + v);
    }
    return [...acc.entries()].sort((a, b) => a[0] - b[0]).map(([k, v]) => ({ p: Number(((k * binCents(bin)) / 100).toFixed(2)), v }));
  };

  const snapshot = (sub: ChartSubscription): ChartSnapshot => {
    const agg = aggs.get(sub.timeframe)!;
    const band = bandOf(now);
    const rows = binRows(sub.profileBin);
    return {
      type: "chart.snapshot",
      symbol: FIXTURE_SYMBOL,
      timeframe: sub.timeframe,
      session: session ? { id: session, start: band.start, end: band.end } : null,
      candles: agg.candles.slice(),
      gaps: agg.gaps.slice(),
      profile: session ? { bin: sub.profileBin, anchor: session, rows, total: profileTotal(rows), poc: pointOfControl(rows), valueArea: valueArea(rows, sub.profileBin) } : null,
      quality: { ...quality },
      clock: clock(),
    };
  };

  const tape = (tr: FixtureTrade): TapeTrade => ({
    type: "tape.trade",
    symbol: FIXTURE_SYMBOL,
    t: tr.t,
    tsNs: tr.t * 1_000_000,
    p: tr.p,
    s: tr.s,
    exchange: tr.exchange,
    seq: tr.seq,
    conditions: tr.conditions,
    counted: tr.counted,
    reason: tr.reason,
    session: tr.session,
  });

  type Change = { candle?: Candle; gaps: Gap[]; cents: Set<number> };

  /** Processes prints up to `until` (at most `maxTrades`) and finalizes intervals. */
  const advance = (until: number, maxTrades = Infinity) => {
    const changed = new Map<Timeframe, Map<number, Change>>(TIMEFRAMES.map((tf) => [tf, new Map()]));
    const touch = (tf: Timeframe, t: number) => {
      const m = changed.get(tf)!;
      let c = m.get(t);
      if (!c) m.set(t, (c = { gaps: [], cents: new Set() }));
      return c;
    };
    const tapeOut: FixtureTrade[] = [];
    let rolled = false;
    let processed = 0;
    while (cursor < trades.length && trades[cursor]!.t <= until && processed < maxTrades) {
      const tr = trades[cursor++]!;
      processed++;
      now = Math.max(now, tr.t);
      for (const [tf, agg] of aggs) {
        const fin = agg.finalize(tr.t);
        if (fin) touch(tf, fin.t).candle = fin;
      }
      if (tr.session !== session) {
        session = tr.session; // new session: profile and counters start over
        perPrice = new Map();
        quality = { ...ZERO_QUALITY };
        rolled = true;
      }
      if (tr.counted === "excluded:duplicate") quality.duplicates++;
      else if (tr.counted === "excluded:late") quality.lateExcluded++;
      else if (tr.counted.startsWith("excluded:condition_")) quality.conditionExcluded++;
      else if (tr.counted === "volume_only") quality.volumeOnly++;
      if (countsProfile(tr)) perPrice.set(cents(tr.p), (perPrice.get(cents(tr.p)) ?? 0) + tr.s);
      for (const [tf, agg] of aggs) {
        const res = agg.add(tr);
        for (const g of res.gaps) touch(tf, g.t).gaps.push(g);
        if (res.candle) {
          const c = touch(tf, res.candle.t);
          c.candle = res.candle;
          if (countsProfile(tr)) c.cents.add(cents(tr.p));
        }
      }
      tapeOut.push(tr);
    }
    if (maxTrades === Infinity) now = Math.max(now, until);
    for (const [tf, agg] of aggs) {
      const fin = agg.finalize(now);
      if (fin) touch(tf, fin.t).candle = fin;
    }
    return { changed, tapeOut, rolled };
  };

  // History up to the start time, silently.
  advance(now);

  const publish = (res: ReturnType<typeof advance>, forceClock: boolean) => {
    const wall = Date.now();
    const sendClock = forceClock || wall - lastClockEmit > 200;
    if (sendClock) lastClockEmit = wall;
    for (const { sub, emit } of subs) {
      if (res.rolled) {
        // A new session re-anchors the profile; a fresh authoritative snapshot is simplest.
        emit(snapshot(sub));
      } else {
        const entries = [...res.changed.get(sub.timeframe)!.entries()].sort((a, b) => a[0] - b[0]);
        for (const [, ch] of entries) {
          for (const gap of ch.gaps) emit({ type: "chart.gap", symbol: FIXTURE_SYMBOL, timeframe: sub.timeframe, session, gap, profileBin: sub.profileBin, profile: [] });
          if (!ch.candle) continue;
          const keys = new Set([...ch.cents].map((c) => binKey(c, sub.profileBin)));
          emit({ type: "chart.update", symbol: FIXTURE_SYMBOL, timeframe: sub.timeframe, session, candle: ch.candle, profileBin: sub.profileBin, profile: keys.size ? binRows(sub.profileBin, keys) : [] });
        }
      }
      for (const tr of res.tapeOut.slice(-TAPE_HISTORY)) emit(tape(tr));
      if (sendClock) emit({ type: "replay.clock", clock: clock() });
    }
  };

  const setI = opts.setInterval ?? ((fn: () => void, ms: number) => setInterval(fn, ms));
  const clearI = opts.clearInterval ?? ((h: unknown) => clearInterval(h as ReturnType<typeof setInterval>));
  const end = BANDS[3]!.end;
  const timer = setI(() => {
    if (!playing || subs.size === 0) return;
    const until = Math.min(now + tickMs * (speed === 0 ? maxSpeed : speed), end);
    if (until <= now) {
      playing = false; // end of the fixture day
      publish(advance(now), true);
      return;
    }
    publish(advance(until), false);
  }, tickMs);

  return {
    kind: "fixture",
    trades,
    now: () => now,
    subscribe(sub, emit) {
      const entry = { sub, emit };
      subs.add(entry);
      emit(snapshot(sub));
      // Recent tape so the panel is not empty, oldest first.
      for (const tr of trades.slice(Math.max(0, cursor - TAPE_HISTORY), cursor)) emit(tape(tr));
      return () => subs.delete(entry);
    },
    control(cmd: ReplayCommand) {
      if (cmd.action === "play") playing = true;
      else if (cmd.action === "pause") playing = false;
      else if (cmd.action === "speed" && cmd.speed !== undefined) speed = cmd.speed;
      else if (cmd.action === "step") {
        playing = false;
        publish(advance(Infinity, 1), true);
        return;
      }
      publish(advance(now), true);
    },
    close() {
      clearI(timer);
      subs.clear();
    },
  };
}
