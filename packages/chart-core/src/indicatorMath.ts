// The clients' former indicator math, kept OUT of the runtime: nothing in index.ts exports it, and
// no chart imports it. The server computes every value (`indicators.v1`,
// crates/chart-engine/src/indicators.rs, bit-identical to this code); this module stays for
//   - indicatorFixtures.ts, which writes the parity fixtures the Rust port is tested against
//     (crates/chart-engine/tests/indicator_parity.rs), and indicators.parity.test.ts, which
//     checks the committed fixtures still match this code;
//   - tests that need real indicator values (layout, panes, serverIndicators).
// Change a formula here only together with the Rust one and the fixtures.
//
// Computed over the chart's slots: a gap slot (no candle) holds NaN, and the running state (EMA,
// RSI averages, VWAP sums) carries ACROSS a gap, since the gap only means no prints, not a reset.

import { assembleIndicators, specKey, type IndicatorResult, type IndicatorSpec } from "./indicators.ts";
import type { Slot } from "./scale.ts";
import { formatEtDateKey, tradingDayOf } from "./time.ts";

const nanArray = (n: number) => new Float64Array(n).fill(Number.NaN);

/** Candle closes per slot, NaN on gap slots. */
function closes(slots: readonly Slot[]): Float64Array {
  const out = nanArray(slots.length);
  slots.forEach((s, i) => {
    if (s.kind === "candle") out[i] = s.candle.c;
  });
  return out;
}

/** Simple moving average over the last `period` CANDLES (gap slots are skipped, not counted). */
export function sma(values: Float64Array, period: number): Float64Array {
  const out = nanArray(values.length);
  const win: number[] = [];
  let sum = 0;
  for (let i = 0; i < values.length; i++) {
    const v = values[i]!;
    if (Number.isNaN(v)) continue;
    win.push(v);
    sum += v;
    if (win.length > period) sum -= win.shift()!;
    if (win.length === period) out[i] = sum / period;
  }
  return out;
}

/** Exponential moving average seeded with the SMA of the first `period` values. */
export function ema(values: Float64Array, period: number): Float64Array {
  const out = nanArray(values.length);
  const k = 2 / (period + 1);
  let seed = 0;
  let count = 0;
  let prev = Number.NaN;
  for (let i = 0; i < values.length; i++) {
    const v = values[i]!;
    if (Number.isNaN(v)) continue;
    if (count < period) {
      seed += v;
      count++;
      if (count === period) {
        prev = seed / period;
        out[i] = prev;
      }
      continue;
    }
    prev = v * k + prev * (1 - k);
    out[i] = prev;
  }
  return out;
}

/**
 * Session VWAP from candles: Σ(typical price × volume) / Σ volume, reset at
 * each trading day (tradingDayOf, so the overnight session that starts at
 * 20:00 ET opens the next day, as the timeline shows it). Built from candles,
 * so it is the bar approximation of the tick VWAP, as every charting package
 * does it.
 */
export function vwap(slots: readonly Slot[]): Float64Array {
  const out = nanArray(slots.length);
  let day = "";
  let pv = 0;
  let vol = 0;
  for (let i = 0; i < slots.length; i++) {
    const s = slots[i]!;
    if (s.kind !== "candle") continue;
    const key = formatEtDateKey(tradingDayOf(s.t));
    if (key !== day) {
      day = key;
      pv = 0;
      vol = 0;
    }
    const c = s.candle;
    pv += ((c.h + c.l + c.c) / 3) * c.v;
    vol += c.v;
    out[i] = vol > 0 ? pv / vol : c.c;
  }
  return out;
}

/** Bollinger Bands: middle SMA, upper and lower at ± mult population standard deviations. */
export function bollinger(values: Float64Array, period: number, mult: number): [Float64Array, Float64Array, Float64Array] {
  const mid = nanArray(values.length);
  const up = nanArray(values.length);
  const lo = nanArray(values.length);
  const win: number[] = [];
  for (let i = 0; i < values.length; i++) {
    const v = values[i]!;
    if (Number.isNaN(v)) continue;
    win.push(v);
    if (win.length > period) win.shift();
    if (win.length < period) continue;
    let m = 0;
    for (const x of win) m += x;
    m /= period;
    let sd = 0;
    for (const x of win) sd += (x - m) * (x - m);
    sd = Math.sqrt(sd / period);
    mid[i] = m;
    up[i] = m + mult * sd;
    lo[i] = m - mult * sd;
  }
  return [mid, up, lo];
}

/** Wilder's RSI. 100 when there were no losses in the window, 50 when nothing moved. */
export function rsi(values: Float64Array, period: number): Float64Array {
  const out = nanArray(values.length);
  let prev = Number.NaN;
  let gain = 0;
  let loss = 0;
  let n = 0;
  for (let i = 0; i < values.length; i++) {
    const v = values[i]!;
    if (Number.isNaN(v)) continue;
    if (Number.isNaN(prev)) {
      prev = v;
      continue;
    }
    const d = v - prev;
    prev = v;
    const g = Math.max(d, 0);
    const l = Math.max(-d, 0);
    if (n < period) {
      gain += g;
      loss += l;
      n++;
      if (n < period) continue;
      gain /= period;
      loss /= period;
    } else {
      gain = (gain * (period - 1) + g) / period;
      loss = (loss * (period - 1) + l) / period;
    }
    out[i] = loss === 0 ? (gain === 0 ? 50 : 100) : 100 - 100 / (1 + gain / loss);
  }
  return out;
}

/** MACD line (EMA fast − EMA slow), its signal EMA, and the histogram (line − signal). */
export function macd(values: Float64Array, fast: number, slow: number, signal: number): [Float64Array, Float64Array, Float64Array] {
  const f = ema(values, fast);
  const s = ema(values, slow);
  const line = nanArray(values.length);
  for (let i = 0; i < values.length; i++) if (!Number.isNaN(f[i]!) && !Number.isNaN(s[i]!)) line[i] = f[i]! - s[i]!;
  const sig = ema(line, signal);
  const hist = nanArray(values.length);
  for (let i = 0; i < values.length; i++) if (!Number.isNaN(sig[i]!)) hist[i] = line[i]! - sig[i]!;
  return [line, sig, hist];
}

/**
 * Computes every spec over the slots, as the clients did before `indicators.v1`: what the parity
 * fixtures pin (indicatorFixtures.ts) and the tests compare the server's values with.
 */
export function computeIndicators(slots: readonly Slot[], specs: readonly IndicatorSpec[]): IndicatorResult[] {
  const c = closes(slots);
  return assembleIndicators(specs, (spec) => {
    switch (spec.kind) {
      case "sma":
        return [sma(c, spec.period ?? 20)];
      case "ema":
        return [ema(c, spec.period ?? 9)];
      case "vwap":
        return [vwap(slots)];
      case "bb":
        return bollinger(c, spec.period ?? 20, spec.mult ?? 2);
      case "rsi":
        return [rsi(c, spec.period ?? 14)];
      case "macd": {
        const [l, s, h] = macd(c, spec.fast ?? 12, spec.slow ?? 26, spec.signal ?? 9);
        return [h, l, s];
      }
    }
  });
}

/**
 * The largest |server − local| per spec over the slots where both have a value, where it is
 * (`fromEnd`: slots before the last, so 0 = the forming bar), and the slots where only one has
 * a value (the former `?indicatorParity=1` shadow, now only tests). Expect 0 / 0 except
 * for the documented divergences (crates/chart-engine/src/indicators.rs): RTH on an early-close
 * day and the warm-up at the history's left edge.
 */
export function indicatorParity(server: readonly IndicatorResult[], local: readonly IndicatorResult[]): Record<string, { maxAbs: number; fromEnd: number; onlyOne: number; compared: number }> {
  const out: Record<string, { maxAbs: number; fromEnd: number; onlyOne: number; compared: number }> = {};
  for (const r of server) {
    const l = local.find((x) => x.spec.id === r.spec.id);
    if (!l) continue;
    const acc = { maxAbs: 0, fromEnd: -1, onlyOne: 0, compared: 0 };
    r.lines.forEach((line, k) => {
      const other = l.lines[k]!.values;
      for (let i = 0; i < line.values.length; i++) {
        const a = line.values[i]!;
        const b = other[i]!;
        if (Number.isNaN(a) !== Number.isNaN(b)) acc.onlyOne++;
        else if (!Number.isNaN(a)) {
          acc.compared++;
          const d = Math.abs(a - b);
          if (d > acc.maxAbs) {
            acc.maxAbs = d;
            acc.fromEnd = line.values.length - 1 - i;
          }
        }
      }
    });
    out[`${r.spec.id} (${specKey(r.spec)})`] = acc;
  }
  return out;
}
