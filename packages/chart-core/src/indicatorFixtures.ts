// Parity fixtures for the server's port of the indicators (crates/chart-engine/src/indicators.rs):
// the values THIS package's indicator code gives, as f64 bit patterns, for
//   (a) the candles the chart engine builds from the synthetic fixture day
//       (crates/chart-engine/tests/fixtures/indicator_candles.json, written by
//       `cargo run -p chart-engine --example indicator_candles`), extended hours on and off;
//   (b) hand-made edge cases: fewer candles than the period, period 1 and 500, flat and
//       monotone prices, zero volume, the 20:00 ET rollover, both 2026 DST weekends, a weekend,
//       Bollinger at mult 0.5 and 10, MACD with fast >= slow, sub-penny prices.
// The Rust test (crates/chart-engine/tests/indicator_parity.rs) requires bit equality with these;
// indicators.parity.test.ts checks the committed file still matches this code, so changing a
// formula without regenerating fails here first.
//
//   bun packages/chart-core/src/indicatorFixtures.ts --write
//
// Values go through computeIndicators() one spec at a time over sessionSlots(buildSlots(..)), the
// path the charts take, so the fixtures pin what a client drew, not just the inner functions.
// Hex rather than decimal so no float formatting or parsing can get between the two sides.

import { sessionSlots } from "./chartTypes.ts";
import { specKey, type IndicatorSpec } from "./indicators.ts";
import { computeIndicators } from "./indicatorMath.ts";
import { buildSlots } from "./scale.ts";
import { formatEtDateKey, tradingDayOf } from "./time.ts";
import type { Candle } from "./types.ts";

const FIXTURES = "../../../crates/chart-engine/tests/fixtures/";
export const CANDLES_PATH = new URL(`${FIXTURES}indicator_candles.json`, import.meta.url);
export const FIXTURE_PATH = new URL(`${FIXTURES}indicator_fixtures.json`, import.meta.url);
export const SPEC_KEYS_PATH = new URL(`${FIXTURES}indicator_spec_keys.json`, import.meta.url);

/** `[t, h, l, c, v, day]`: `day` is the trading day VWAP resets on, `formatEtDateKey(tradingDayOf(t))`. */
export type FixtureCandle = [number, number, number, number, number, string];

export interface FixtureLine {
  name: string;
  /** Index of the first value that is not NaN (= the candle count when there is none). */
  from: number;
  /** 16 hex digits per value (big-endian f64 bits) from `from` to the end. */
  hex: string;
}

export interface FixtureCase {
  name: string;
  /** Extended hours off: only `sessionOf(t) === "regular"` candles are computed over. */
  rth: boolean;
  /** A series of indicator_candles.json, or `candles` inline. */
  source?: string;
  candles?: FixtureCandle[];
  /** How many candles the values are over (after the rth filter). */
  n: number;
  studies: { spec: IndicatorSpec; key: string; lines: FixtureLine[] }[];
}

/** mulberry32, as in the other fixture generators, so the cases are the same on every machine. */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const view = new DataView(new ArrayBuffer(8));
function hex(x: number): string {
  view.setFloat64(0, x);
  return view.getBigUint64(0).toString(16).padStart(16, "0");
}

const tick = (p: number) => (p < 1 ? Math.round(p * 10_000) / 10_000 : Math.round(p * 100) / 100);

/** A seeded walk of `n` candles every `stepMs` from `t0`. */
function walk(seed: number, t0: number, n: number, stepMs: number, start: number, opts: { volume?: (i: number) => number } = {}): FixtureCandle[] {
  const r = rng(seed);
  const out: FixtureCandle[] = [];
  let p = start;
  for (let i = 0; i < n; i++) {
    const c = tick(Math.max(start * 0.2, p * (1 + (r() - 0.5) * 0.02)));
    const h = tick(Math.max(c, p) * (1 + r() * 0.004));
    const l = tick(Math.min(c, p) * (1 - r() * 0.004));
    const t = t0 + i * stepMs;
    const v = opts.volume ? opts.volume(i) : Math.round(100 + r() * 5000);
    out.push([t, h, l, c, v, formatEtDateKey(tradingDayOf(t))]);
    p = c;
  }
  return out;
}

const fromCloses = (t0: number, stepMs: number, closes: number[]): FixtureCandle[] =>
  closes.map((c, i) => {
    const t = t0 + i * stepMs;
    return [t, c, c, c, 100, formatEtDateKey(tradingDayOf(t))];
  });

const MIN = 60_000;
/** 2026-10-15 09:30 ET (EDT). */
const OPEN_1015 = Date.UTC(2026, 9, 15, 13, 30);

const PRESETS: IndicatorSpec[] = [
  { id: "vwap", kind: "vwap" },
  { id: "ema-9", kind: "ema", period: 9 },
  { id: "ema-20", kind: "ema", period: 20 },
  { id: "sma-50", kind: "sma", period: 50 },
  { id: "bb-20", kind: "bb", period: 20, mult: 2 },
  { id: "rsi-14", kind: "rsi", period: 14 },
  { id: "macd", kind: "macd", fast: 12, slow: 26, signal: 9 },
];
const DEFAULTS: IndicatorSpec[] = [
  { id: "sma", kind: "sma" },
  { id: "ema", kind: "ema" },
  { id: "vwap", kind: "vwap" },
  { id: "bb", kind: "bb" },
  { id: "rsi", kind: "rsi" },
  { id: "macd", kind: "macd" },
];

/** The cases about the trading-day key only need the study that reads it (and one that does not). */
const DAY_KEYED: IndicatorSpec[] = [
  { id: "vwap", kind: "vwap" },
  { id: "ema", kind: "ema" },
];

function toCandles(rows: FixtureCandle[]): Candle[] {
  return rows.map(([t, h, l, c, v]) => ({ t, o: c, h, l, c, v, n: 1, final: true }));
}

function compute(name: string, rows: FixtureCandle[], rth: boolean, specs: IndicatorSpec[], source?: string): FixtureCase {
  const slots = sessionSlots(buildSlots(toCandles(rows), []), !rth);
  const studies = specs.map((spec) => {
    const [r] = computeIndicators(slots, [spec]);
    const lines = r!.lines.map((line) => {
      const vals = Array.from(line.values);
      let from = vals.findIndex((x) => !Number.isNaN(x));
      if (from < 0) from = vals.length;
      return { name: line.name, from, hex: vals.slice(from).map(hex).join("") };
    });
    return { spec, key: specKey(spec), lines };
  });
  const c: FixtureCase = { name, rth, n: slots.length, studies };
  if (source) c.source = source;
  else c.candles = rows;
  return c;
}

function handMade(): FixtureCase[] {
  const out: FixtureCase[] = [];
  const add = (name: string, rows: FixtureCandle[], specs: IndicatorSpec[] = DEFAULTS) => out.push(compute(name, rows, false, specs));

  add("fewer-than-period", walk(1, OPEN_1015, 5, MIN, 3.1));
  add("period-1", walk(2, OPEN_1015, 40, MIN, 7.25), [
    { id: "s", kind: "sma", period: 1 },
    { id: "e", kind: "ema", period: 1 },
    { id: "b", kind: "bb", period: 1, mult: 2 },
    { id: "r", kind: "rsi", period: 1 },
    { id: "m", kind: "macd", fast: 1, slow: 1, signal: 1 },
  ]);
  add("period-500", walk(3, Date.UTC(2026, 9, 15, 8, 0), 560, 15_000, 12.4), [
    { id: "s", kind: "sma", period: 500 },
    { id: "e", kind: "ema", period: 500 },
    { id: "b", kind: "bb", period: 500, mult: 2 },
    { id: "r", kind: "rsi", period: 500 },
    { id: "m", kind: "macd", fast: 100, slow: 500, signal: 50 },
  ]);
  add("flat", fromCloses(OPEN_1015, MIN, Array.from({ length: 60 }, () => 4.2)));
  add(
    "monotone-rise",
    fromCloses(OPEN_1015, MIN, Array.from({ length: 60 }, (_, i) => tick(2 + i * 0.01))),
  );
  add(
    "monotone-fall",
    fromCloses(OPEN_1015, MIN, Array.from({ length: 60 }, (_, i) => tick(9 - i * 0.03))),
  );
  // Candles with holes between them (the chart's gap slots): the state carries across.
  add(
    "holes",
    walk(4, OPEN_1015, 120, MIN, 5.5).filter((_, i) => i % 7 !== 3 && !(i >= 40 && i < 55)),
  );
  // Zero-volume candles: VWAP is the close until the day has volume.
  add("zero-volume", walk(5, OPEN_1015, 40, MIN, 2.2, { volume: (i) => (i < 3 || i === 20 ? 0 : 100 * i) }));
  // 19:30 → 20:30 ET: the overnight session from 20:00 belongs to the next day, so VWAP resets.
  add("rollover-2000", walk(6, Date.UTC(2026, 9, 15, 23, 30), 60, MIN, 8.8), DAY_KEYED);
  // Both DST weekends, 15-minute candles straight through Saturday to Monday: the trading-day
  // key around 20:00 ET has to follow the offset change.
  add("dst-spring-2026-03-08", walk(7, Date.UTC(2026, 2, 6, 12, 0), 4 * 96, 15 * MIN, 3.3), DAY_KEYED);
  add("dst-fall-2026-11-01", walk(8, Date.UTC(2026, 9, 30, 12, 0), 4 * 96, 15 * MIN, 15.7), DAY_KEYED);
  // Friday's after hours, then Sunday's overnight (Monday's trading day).
  add("weekend", [
    ...walk(9, Date.UTC(2026, 9, 16, 22, 0), 120, MIN, 6.4),
    ...walk(10, Date.UTC(2026, 9, 19, 0, 0), 120, MIN, 6.5),
  ], DAY_KEYED);
  add("bollinger-mults", walk(11, OPEN_1015, 150, MIN, 1.85), [
    { id: "b1", kind: "bb", period: 20, mult: 0.5 },
    { id: "b2", kind: "bb", period: 20, mult: 10 },
    { id: "b3", kind: "bb", period: 13, mult: 2.25 },
  ]);
  add("macd-fast-not-below-slow", walk(12, OPEN_1015, 150, MIN, 11.1), [
    { id: "m1", kind: "macd", fast: 26, slow: 12, signal: 9 },
    { id: "m2", kind: "macd", fast: 12, slow: 12, signal: 9 },
    { id: "m3", kind: "macd", fast: 3, slow: 5, signal: 2 },
  ]);
  add("sub-penny", walk(13, OPEN_1015, 200, MIN, 0.137), DEFAULTS);
  return out;
}

interface CandleFile {
  series: Record<string, [number, number, number, number, number, string][]>;
}

export async function buildFixtures(): Promise<FixtureCase[]> {
  const input = (await Bun.file(CANDLES_PATH).json()) as CandleFile;
  const out: FixtureCase[] = [];
  const day = (name: string, rth: boolean) => {
    // The engine's session id is replaced by the TS trading-day key: the client never saw the
    // session, and the Rust test checks the two agree.
    const rows = input.series[name]!.map(([t, h, l, c, v]): FixtureCandle => [t, h, l, c, v, formatEtDateKey(tradingDayOf(t))]);
    out.push(compute(`${name}${rth ? " rth" : ""}`, rows, rth, PRESETS, name));
  };
  day("SMPL/1m", true);
  day("SMPL/5m", false);
  day("SMPL/5m", true);
  day("SMPL/15s", true);
  day("FXLO/5m", false);
  out.push(...handMade());
  return out;
}

if (import.meta.main && process.argv.includes("--write")) {
  const cases = await buildFixtures();
  await Bun.write(FIXTURE_PATH, JSON.stringify({ note: "Generated by packages/chart-core/src/indicatorFixtures.ts --write; do not edit.", cases }) + "\n");
  console.log(`wrote ${cases.length} cases to ${FIXTURE_PATH.pathname}`);
}
