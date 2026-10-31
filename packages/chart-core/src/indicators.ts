import type { ChartTheme } from "./displayList.ts";
import type { IconPath } from "./icons.ts";
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

/**
 * The canonical key the server's indicator frames carry for a spec (`chart.indicators`,
 * `chart.indicator`; crates/chart-engine/src/indicators.rs `IndicatorSpec::key`): the kind and
 * its RESOLVED parameters, so two specs that compute the same values share one server
 * computation whatever ids the clients gave them. Both sides are tested against the same
 * vectors (crates/chart-engine/tests/fixtures/indicator_spec_keys.json); change one only with
 * the other.
 */
export function specKey(s: IndicatorSpec): string {
  switch (s.kind) {
    case "sma":
      return `sma:${s.period ?? 20}`;
    case "ema":
      return `ema:${s.period ?? 9}`;
    case "vwap":
      return "vwap";
    case "bb":
      return `bb:${s.period ?? 20}:${String(s.mult ?? 2)}`;
    case "rsi":
      return `rsi:${s.period ?? 14}`;
    case "macd":
      return `macd:${s.fast ?? 12}:${s.slow ?? 26}:${s.signal ?? 9}`;
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
  /** pane: a shaded zone between two values (RSI 30–70) and an unlabelled faint midline (RSI 50). */
  zone?: { lo: number; hi: number };
  midline?: number;
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

/** The line names and styles of a spec, in drawing order (the server's frames use the same names). */
export function indicatorLineNames(kind: IndicatorKind): readonly { name: string; style: IndicatorLine["style"] }[] {
  return LINE_NAMES[kind];
}

const LINE: IndicatorLine["style"] = "line";
const LINE_NAMES: Readonly<Record<IndicatorKind, readonly { name: string; style: IndicatorLine["style"] }[]>> = {
  sma: [{ name: "SMA", style: LINE }],
  ema: [{ name: "EMA", style: LINE }],
  vwap: [{ name: "VWAP", style: LINE }],
  bb: [
    { name: "Basis", style: LINE },
    { name: "Upper", style: LINE },
    { name: "Lower", style: LINE },
  ],
  rsi: [{ name: "RSI", style: LINE }],
  macd: [
    { name: "Hist", style: "histogram" },
    { name: "MACD", style: LINE },
    { name: "Signal", style: LINE },
  ],
};

/**
 * The results for `specs`, values from `valuesOf` (one array per line, in indicatorLineNames
 * order) and everything else (placement, colour slot, band, RSI range and guides) decided here
 * once, so the local math (computeIndicators) and the server's values (serverIndicatorResults)
 * draw identically. Overlays beyond MAX_OVERLAYS and panes beyond MAX_PANES are dropped (the
 * menus never offer more; the server drops the same ones), so the colour slots stay inside the
 * validated set.
 */
function assembleIndicators(specs: readonly IndicatorSpec[], valuesOf: (spec: IndicatorSpec) => Float64Array[]): IndicatorResult[] {
  const out: IndicatorResult[] = [];
  let overlays = 0;
  let panes = 0;
  for (const spec of specs) {
    const placement = INDICATOR_PLACEMENT[spec.kind];
    if (placement === "overlay" ? overlays >= MAX_OVERLAYS : panes >= MAX_PANES) continue;
    const colorSlot = placement === "overlay" ? overlays++ : (panes++, 0);
    const values = valuesOf(spec);
    const lines = LINE_NAMES[spec.kind].map((l, k): IndicatorLine => ({ name: l.name, style: l.style, values: values[k]! }));
    const r: IndicatorResult = { spec, placement, colorSlot, lines };
    if (spec.kind === "bb") r.band = [1, 2];
    if (spec.kind === "rsi") Object.assign(r, { fixedRange: { lo: 0, hi: 100 }, guides: [30, 70], zone: { lo: 30, hi: 70 }, midline: 50 });
    if (spec.kind === "macd") r.guides = [0];
    out.push(r);
  }
  return out;
}

/**
 * Computes every spec over the slots, here on the client: the fallback for a server without
 * `indicators.v1` (and the dev parity shadow). With it, the server computes them
 * (serverIndicatorResults).
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
 * The server's values (ChartStore `getIndicators`, `indicators.v1`) as results over the shown
 * slots: each line joined onto the slots by bar time (both ascending, one merge pass), NaN on
 * gap slots and wherever the server has no value. A spec whose key the server has not sent yet
 * (just toggled on: the `chart.indicators` answer is on its way) draws nothing rather than
 * falling back to local math, so a line never jumps between two sources.
 */
export function serverIndicatorResults(slots: readonly Slot[], specs: readonly IndicatorSpec[], server: IndicatorValuesLike | null): IndicatorResult[] {
  // Slot i's index in server.t, -1 = none.
  const at = new Int32Array(slots.length).fill(-1);
  if (server) {
    const t = server.t;
    let j = 0;
    for (let i = 0; i < slots.length; i++) {
      const s = slots[i]!;
      if (s.kind !== "candle") continue;
      while (j < t.length && t[j]! < s.t) j++;
      if (j < t.length && t[j] === s.t) at[i] = j;
    }
  }
  return assembleIndicators(specs, (spec) => {
    const study = server?.studies.get(specKey(spec));
    return LINE_NAMES[spec.kind].map((l) => {
      const out = nanArray(slots.length);
      const src = study?.lines.find((x) => x.name === l.name)?.values;
      if (src) for (let i = 0; i < at.length; i++) if (at[i]! >= 0) out[i] = src[at[i]!]!;
      return out;
    });
  });
}

/** What serverIndicatorResults reads: ChartStore's `IndicatorValues` (store.ts). */
export interface IndicatorValuesLike {
  /** Bar times, ascending. */
  readonly t: readonly number[];
  /** By spec key; each line's values parallel to `t`, NaN = none. */
  readonly studies: ReadonlyMap<string, { readonly lines: readonly { readonly name: string; readonly values: ArrayLike<number> }[] }>;
}

/**
 * The largest |server − local| per spec over the slots where both have a value, where it is
 * (`fromEnd`: slots before the last, so 0 = the forming bar), and the slots where only one has
 * a value (the `?indicatorParity=1` shadow, docs in chart-web CandleChart). Expect 0 / 0 except
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

/**
 * What a chart asks the server for (`indicators.v1`): its effective spec list (none while the
 * indicators layer is hidden, so the server computes nothing nobody sees) and whether it shows
 * the regular session only (extended hours off). The host sends it on `chart.subscribe` and
 * `chart.indicators.set`, and the same `rth` picks the values to draw (ChartStore `getIndicators`).
 */
export interface IndicatorDemand {
  indicators: IndicatorSpec[];
  rth: boolean;
}

export function indicatorDemand(specs: readonly IndicatorSpec[], settings: { extendedHours: boolean; visibility: { indicators: boolean } }): IndicatorDemand {
  return { indicators: settings.visibility.indicators ? [...specs] : [], rth: !settings.extendedHours };
}

/** Same demand (to skip a `chart.indicators.set` that would change nothing)? */
export function sameIndicatorDemand(a: IndicatorDemand, b: IndicatorDemand): boolean {
  return a.rth === b.rth && a.indicators.length === b.indicators.length && a.indicators.every((s, i) => s.id === b.indicators[i]!.id && specKey(s) === specKey(b.indicators[i]!));
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

/** What a menu shows for an indicator: its short name, its settings, and one line on what it measures. */
export interface IndicatorInfo {
  name: string;
  params: string;
  description: string;
}

export function indicatorInfo(s: IndicatorSpec): IndicatorInfo {
  switch (s.kind) {
    case "vwap":
      return { name: "VWAP", params: "", description: "Volume-weighted average price, resets daily" };
    case "ema":
      return { name: "EMA", params: String(s.period ?? 9), description: (s.period ?? 9) <= 10 ? "Fast exponential average" : "Exponential moving average" };
    case "sma":
      return { name: "SMA", params: String(s.period ?? 20), description: "Simple moving average" };
    case "bb":
      return { name: "Bollinger", params: `${s.period ?? 20} · ${s.mult ?? 2}σ`, description: "Volatility bands around the average" };
    case "rsi":
      return { name: "RSI", params: String(s.period ?? 14), description: "Momentum from 0 to 100, lines at 30 and 70" };
    case "macd":
      return { name: "MACD", params: `${s.fast ?? 12} ${s.slow ?? 26} ${s.signal ?? 9}`, description: "Trend momentum, with signal and histogram" };
  }
}

/**
 * Icon glyphs per indicator kind, as duotone path data on a 16 px grid
 * (icons.ts), so the web (inline <svg>) and the phone (react-native-svg)
 * draw the same icon. The study's own line is primary; the price or guides
 * it is read against, and a band's area, are secondary. Each menu tints the
 * glyph with the line's colour, so both tones follow it.
 */
export const INDICATOR_GLYPHS: Readonly<Record<IndicatorKind, readonly IconPath[]>> = {
  vwap: [
    { d: "M2 14v-2.5M5 14v-4M8 14v-1.5M11 14v-3M14 14v-2", role: "secondary" },
    { d: "M1.5 8.5c2-1.5 4-3.5 6.5-3.5s4.5 1.5 6.5 0.5", role: "primary" },
  ],
  ema: [
    { d: "M1.5 12 4.5 8.5l2.5 2 3.5-6 4 3", role: "secondary" },
    { d: "M1.5 11c3-1 4.5-4.5 7-5.5s4 0 6-1.5", role: "primary" },
  ],
  sma: [
    { d: "M1.5 12 4.5 8.5l2.5 2 3.5-6 4 3", role: "secondary" },
    { d: "M1.5 12c3.5-.5 5-3 7-4s4-.5 6-1.5", role: "primary" },
  ],
  bb: [
    { d: "M1.5 6c3-2 5-2.5 7-1.5s4 1 6-1v6c-2 1.5-4 2-6 1s-4-.5-7 1.5z", paint: "fill", role: "secondary" },
    { d: "M1.5 6c3-2 5-2.5 7-1.5s4 1 6-1M1.5 12c3-2 5-2.5 7-1.5s4 1 6-1", role: "primary" },
  ],
  rsi: [
    { d: "M1.5 4.5h13M1.5 11.5h13", role: "secondary" },
    { d: "M1.5 9c1.5-5 3-5 4.5-1s3 5 4.5 1 2.5-4.5 4-3", role: "primary" },
  ],
  macd: [
    { d: "M3 8v3M5.5 8v4.5M8 8v2M10.5 8V6M13 8V4.5", role: "secondary" },
    { d: "M1.5 10c3-1 5-5 8-5.5s4 .5 5 1", role: "primary" },
  ],
};
