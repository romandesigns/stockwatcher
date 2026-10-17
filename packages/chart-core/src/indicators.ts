import type { ChartTheme } from "./displayList.ts";
import type { Slot } from "./scale.ts";
import { formatEtDateKey, tradingDayOf } from "./time.ts";

// Technical indicators, computed over the chart's slots so every value lines
// up with the candle it belongs to. A gap slot (no candle) holds NaN and
// breaks the drawn line rather than bridging it: the chart never invents a
// price where nothing traded, and the indicators follow the same rule. The
// running state (EMA, RSI averages, VWAP sums) carries ACROSS a gap, since
// the gap only means no prints, not a reset.
//
// Scope (owner decision 2026-10-17): drawings, indicators and simulated
// chart trading move into this phase. The set here is the day-trading core,
// kept small on purpose; every one is a pure function of the candles.
//
// Colour: overlay indicators take categorical slots in a FIXED order
// (theme.series1..3), never cycled. Three slots validate all-pairs on the dark
// surface (dataviz validator: worst CVD ΔE 13.2, normal 19.3), so at most
// MAX_OVERLAYS overlays can be on at once; an oscillator lives in its own
// pane, where its lines use the same slots from the start.

export type IndicatorKind = "sma" | "ema" | "vwap" | "bb" | "rsi" | "macd";

export interface IndicatorSpec {
  /** Stable id, unique per chart (e.g. "ema-9"). */
  id: string;
  kind: IndicatorKind;
  /** sma/ema/bb/rsi: lookback in candles. */
  period?: number;
  /** bb: band width in standard deviations. */
  mult?: number;
  /** macd: fast, slow and signal periods. */
  fast?: number;
  slow?: number;
  signal?: number;
}

export type IndicatorPlacement = "overlay" | "pane";

export const INDICATOR_PLACEMENT: Readonly<Record<IndicatorKind, IndicatorPlacement>> = {
  sma: "overlay",
  ema: "overlay",
  vwap: "overlay",
  bb: "overlay",
  rsi: "pane",
  macd: "pane",
};

export const MAX_OVERLAYS = 3;
export const MAX_PANES = 2;

/** The catalogue the indicator menus offer, with day-trading defaults. */
export const INDICATOR_PRESETS: readonly IndicatorSpec[] = [
  { id: "vwap", kind: "vwap" },
  { id: "ema-9", kind: "ema", period: 9 },
  { id: "ema-20", kind: "ema", period: 20 },
  { id: "sma-50", kind: "sma", period: 50 },
  { id: "bb-20", kind: "bb", period: 20, mult: 2 },
  { id: "rsi-14", kind: "rsi", period: 14 },
  { id: "macd", kind: "macd", fast: 12, slow: 26, signal: 9 },
];

export function indicatorLabel(s: IndicatorSpec): string {
  switch (s.kind) {
    case "sma":
      return `SMA ${s.period ?? 20}`;
    case "ema":
      return `EMA ${s.period ?? 9}`;
    case "vwap":
      return "VWAP";
    case "bb":
      return `BB ${s.period ?? 20} ${s.mult ?? 2}`;
    case "rsi":
      return `RSI ${s.period ?? 14}`;
    case "macd":
      return `MACD ${s.fast ?? 12} ${s.slow ?? 26} ${s.signal ?? 9}`;
  }
}

/** One drawn series, aligned to the slots (NaN = nothing at that slot). */
export interface IndicatorLine {
  name: string;
  values: Float64Array;
  /** "line" draws a polyline; "histogram" draws bars from zero (MACD). */
  style: "line" | "histogram";
}

export interface IndicatorResult {
  spec: IndicatorSpec;
  placement: IndicatorPlacement;
  lines: IndicatorLine[];
  /** bb: fill between lines[1] (upper) and lines[2] (lower). */
  band?: [number, number];
  /** pane: a fixed value range (RSI 0–100); absent = fit the visible values. */
  fixedRange?: { lo: number; hi: number };
  /** pane: horizontal guide values (RSI 30/70, MACD 0). */
  guides?: number[];
  /** Categorical colour slot of the first line (overlays only; panes start at 0). */
  colorSlot: number;
}

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
 * Computes every spec over the slots. Overlays beyond MAX_OVERLAYS and panes
 * beyond MAX_PANES are dropped (the menus never offer more), so the colour
 * slots stay inside the validated set.
 */
export function computeIndicators(slots: readonly Slot[], specs: readonly IndicatorSpec[]): IndicatorResult[] {
  const c = closes(slots);
  const out: IndicatorResult[] = [];
  let overlays = 0;
  let panes = 0;
  for (const spec of specs) {
    const placement = INDICATOR_PLACEMENT[spec.kind];
    if (placement === "overlay" ? overlays >= MAX_OVERLAYS : panes >= MAX_PANES) continue;
    const colorSlot = placement === "overlay" ? overlays++ : (panes++, 0);
    const line = (name: string, values: Float64Array): IndicatorLine => ({ name, values, style: "line" });
    switch (spec.kind) {
      case "sma":
        out.push({ spec, placement, colorSlot, lines: [line("SMA", sma(c, spec.period ?? 20))] });
        break;
      case "ema":
        out.push({ spec, placement, colorSlot, lines: [line("EMA", ema(c, spec.period ?? 9))] });
        break;
      case "vwap":
        out.push({ spec, placement, colorSlot, lines: [line("VWAP", vwap(slots))] });
        break;
      case "bb": {
        const [mid, up, lo] = bollinger(c, spec.period ?? 20, spec.mult ?? 2);
        out.push({ spec, placement, colorSlot, lines: [line("Basis", mid), line("Upper", up), line("Lower", lo)], band: [1, 2] });
        break;
      }
      case "rsi":
        out.push({ spec, placement, colorSlot, lines: [line("RSI", rsi(c, spec.period ?? 14))], fixedRange: { lo: 0, hi: 100 }, guides: [30, 70] });
        break;
      case "macd": {
        const [l, s, h] = macd(c, spec.fast ?? 12, spec.slow ?? 26, spec.signal ?? 9);
        out.push({ spec, placement, colorSlot, lines: [{ name: "Hist", values: h, style: "histogram" }, line("MACD", l), line("Signal", s)], guides: [0] });
        break;
      }
    }
  }
  return out;
}

/** Colour of a result's k-th line: its slot plus k, for pane indicators; overlays use one colour for all their lines. */
export function indicatorColor(theme: ChartTheme, r: IndicatorResult, k: number): string {
  const slot = r.placement === "overlay" ? r.colorSlot : Math.max(0, k - (r.lines[0]?.style === "histogram" ? 1 : 0));
  const series = [theme.series1, theme.series2, theme.series3];
  return series[slot % series.length]!;
}

/** Toggles a preset on or off, respecting the overlay and pane caps. Returns the specs unchanged when the cap blocks it. */
export function toggleIndicator(specs: readonly IndicatorSpec[], preset: IndicatorSpec): IndicatorSpec[] {
  if (specs.some((s) => s.id === preset.id)) return specs.filter((s) => s.id !== preset.id);
  const placement = INDICATOR_PLACEMENT[preset.kind];
  const used = specs.filter((s) => INDICATOR_PLACEMENT[s.kind] === placement).length;
  if (used >= (placement === "overlay" ? MAX_OVERLAYS : MAX_PANES)) return [...specs];
  return [...specs, preset];
}

/** Whether a preset can be switched on now (false when its placement is full). */
export function canAddIndicator(specs: readonly IndicatorSpec[], preset: IndicatorSpec): boolean {
  if (specs.some((s) => s.id === preset.id)) return true;
  const placement = INDICATOR_PLACEMENT[preset.kind];
  return specs.filter((s) => INDICATOR_PLACEMENT[s.kind] === placement).length < (placement === "overlay" ? MAX_OVERLAYS : MAX_PANES);
}
