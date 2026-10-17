// Deterministic test fixture (software tests only; not market data).
import type { Candle, ChartSnapshot, DataQuality, Gap, Timeframe } from "./types.ts";
import { TIMEFRAME_MS } from "./types.ts";

/** 2026-10-15 10:00:00 ET (EDT) */
export const T0 = Date.UTC(2026, 9, 15, 14, 0, 0);

export const ZERO_QUALITY: DataQuality = {
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
  haltsUnmatched: 0,
};

/** Candles every interval except `skip` indices, which become single-interval no_trades gaps. */
export function fixtureCandles(n: number, timeframe: Timeframe = "1m", start = T0, skip: readonly number[] = []): { candles: Candle[]; gaps: Gap[] } {
  const tf = TIMEFRAME_MS[timeframe];
  const candles: Candle[] = [];
  const gaps: Gap[] = [];
  let p = 3.2;
  for (let i = 0; i < n; i++) {
    const t = start + i * tf;
    if (skip.includes(i)) {
      gaps.push({ t, end: t + tf, reason: "no_trades", v: 0, intervals: 1 });
      continue;
    }
    const o = p;
    const c = Number((o + (i % 3 === 0 ? -0.02 : 0.03)).toFixed(2));
    const h = Number((Math.max(o, c) + 0.01).toFixed(2));
    const l = Number((Math.min(o, c) - 0.01).toFixed(2));
    candles.push({ t, o, h, l, c, v: 1000 + ((i * 7919) % 5000), n: 10 + (i % 7), final: i < n - 1 });
    p = c;
  }
  return { candles, gaps };
}

export function fixtureSnapshot(timeframe: Timeframe = "1m", n = 20, skip: readonly number[] = [5]): ChartSnapshot {
  const { candles, gaps } = fixtureCandles(n, timeframe, T0, skip);
  return {
    type: "chart.snapshot",
    symbol: "SMPL",
    timeframe,
    session: { id: "2026-10-15/regular", start: Date.UTC(2026, 9, 15, 13, 30), end: Date.UTC(2026, 9, 15, 20, 0) },
    candles,
    gaps,
    profile: {
      bin: 0.01,
      anchor: "2026-10-15/regular",
      rows: [
        { p: 3.19, v: 1200 },
        { p: 3.2, v: 5400 },
        { p: 3.21, v: 3100 },
        { p: 3.22, v: 800 },
        { p: 3.24, v: 2000 },
      ],
      total: 12500,
      poc: 3.2,
      // target ceil(8750): 5400 → +3100 (3.21 > 1200) = 8500 → +1200 (3.19 > 800) = 9700
      valueArea: { low: 3.19, high: 3.21, v: 9700 },
    },
    quality: ZERO_QUALITY,
    clock: { mode: "replay", now: T0 + n * TIMEFRAME_MS[timeframe] - 1000, speed: 10, playing: true },
  };
}
