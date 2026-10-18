import type { IconName } from "./icons.ts";
import type { Slot } from "./scale.ts";
import { sessionOf, tradingDayOf } from "./time.ts";
import type { Candle } from "./types.ts";

// Chart types (owner request 2026-10-18: "the same chart settings just like
// Robinhood's"). Robinhood Legend's chart-types icon offers Area, Bar,
// Baseline, Candle, Equivolume, Heikin-Ashi, Hollow candles, Line, Scatter
// and Trend (docs/research/robinhood-chart-settings.md). This module holds
// the pure maths every type needs; layout.ts draws them through the shared
// display list, so the web canvas and the phone's Skia picture match.
//
// Built: candles, hollow candles, OHLC bars, Heikin-Ashi, line, area,
// baseline. Skipped on purpose (see the research note): Equivolume (a box's
// WIDTH is its volume, which breaks the index-based time axis that gaps,
// drawings, indicators and the inspector all share), Scatter (dots of the
// close: the line already shows them, and at the 0.5–2 px candle widths the
// session presets produce, dots are unreadable) and Trend (candles coloured
// by close vs previous close: hollow candles already carry exactly that
// colour rule, with the open/close fill on top).
//
// Rule that matters everywhere: a chart type changes only what is DRAWN.
// Slots, the last-price tag, the readout, the tape, levels, drawings and
// orders keep reading the real candles, so Heikin-Ashi's synthetic prices
// can never leak into a price the owner trades on.

export type ChartType = "candles" | "hollow" | "bars" | "heikinAshi" | "line" | "area" | "baseline";

export const CHART_TYPES: readonly ChartType[] = ["candles", "hollow", "bars", "heikinAshi", "line", "area", "baseline"];

export const CHART_TYPE_INFO: Readonly<Record<ChartType, { name: string; description: string }>> = {
  candles: { name: "Candles", description: "Bodies and wicks, coloured by close vs open" },
  hollow: { name: "Hollow candles", description: "Hollow when close ≥ open; colour by close vs previous close" },
  bars: { name: "Bars", description: "OHLC bars: open tick left, close tick right" },
  heikinAshi: { name: "Heikin-Ashi", description: "Averaged candles; the price label and readout stay real" },
  line: { name: "Line", description: "Closing prices" },
  area: { name: "Area", description: "Closing prices over a soft fill" },
  baseline: { name: "Baseline", description: "Above / below the previous close" },
};

/** Types drawn from the close only (auto-fit reads closes, the high/low labels mark closes). */
export function isCloseOnly(type: ChartType): boolean {
  return type === "line" || type === "area" || type === "baseline";
}

export interface Ohlc {
  o: number;
  h: number;
  l: number;
  c: number;
}

const haCache = new WeakMap<readonly Slot[], readonly (Ohlc | null)[]>();

/**
 * Heikin-Ashi values per slot (null for a gap slot), computed over the WHOLE
 * slot list, not just the visible part:
 *   haClose = (o + h + l + c) / 4
 *   haOpen  = (prev haOpen + prev haClose) / 2, the first one (o + c) / 2
 *   haHigh  = max(h, haOpen, haClose), haLow = min(l, haOpen, haClose)
 * haOpen is recursive, so starting the recursion at the first VISIBLE candle
 * would redraw every candle as you pan; starting at the first loaded one
 * keeps them still, which is what TradingView (and so Legend) does. A gap
 * carries the previous candle's values over it. Cached per slots array (the
 * renderers rebuild that array only when the data changes), so a frame costs
 * a lookup.
 */
export function heikinAshi(slots: readonly Slot[]): readonly (Ohlc | null)[] {
  const hit = haCache.get(slots);
  if (hit) return hit;
  const out: (Ohlc | null)[] = Array.from({ length: slots.length }, () => null);
  let prev: Ohlc | null = null;
  for (let i = 0; i < slots.length; i++) {
    const s = slots[i]!;
    if (s.kind !== "candle") {
      out[i] = null;
      continue;
    }
    const k = s.candle;
    const c = (k.o + k.h + k.l + k.c) / 4;
    const o: number = prev ? (prev.o + prev.c) / 2 : (k.o + k.c) / 2;
    const v = { o, c, h: Math.max(k.h, o, c), l: Math.min(k.l, o, c) };
    out[i] = v;
    prev = v;
  }
  haCache.set(slots, out);
  return out;
}

/**
 * Hollow candles, the TradingView rule Robinhood Legend documents ("Fill
 * shows close versus open. Color shows close versus previous close"):
 * - colour: up when the close is at or above the PREVIOUS candle's close
 *   (the first candle, with nothing before it, compares with its own open);
 * - fill: hollow when the close is at or above the open, filled below it.
 * So a red hollow candle is a bar that rallied from its open but still
 * closed under the previous close.
 */
export function hollowStyle(c: Pick<Candle, "o" | "c">, prevClose: number | null): { up: boolean; hollow: boolean } {
  return { up: c.c >= (prevClose ?? c.o), hollow: c.c >= c.o };
}

/** The close of the nearest candle before slot i (gaps skipped), or null. */
export function prevCloseAt(slots: readonly Slot[], i: number): number | null {
  for (let k = i - 1; k >= 0; k--) {
    const s = slots[k]!;
    if (s.kind === "candle") return s.candle.c;
  }
  return null;
}

const sessionCache = new WeakMap<readonly Slot[], Slot[]>();

/**
 * The slots the chart shows. With extended hours on, all of them. With them
 * off, only regular-session slots (09:30–16:00 ET): pre-market, after-hours
 * and overnight candles AND gaps are dropped, so on the index-based time axis
 * one day's close sits right next to the next day's open with no empty
 * stretch (Robinhood's "Show extended hours" off, TradingView's RTH). The
 * caller computes indicators on the returned list so indices line up; key
 * levels (the premarket high/low is a regular-session level traders watch)
 * and the opening range still read the full list.
 */
export function sessionSlots(slots: Slot[], extendedHours: boolean): Slot[] {
  if (extendedHours) return slots;
  const hit = sessionCache.get(slots);
  if (hit) return hit;
  const out = slots.filter((s) => sessionOf(s.t) === "regular");
  sessionCache.set(slots, out);
  return out;
}

/** Whether slot i starts a new trading day (the session-break line goes on its left edge). */
export function startsTradingDay(slots: readonly Slot[], i: number): boolean {
  if (i <= 0 || i >= slots.length) return false;
  const a = tradingDayOf(slots[i - 1]!.t);
  const b = tradingDayOf(slots[i]!.t);
  return a.year !== b.year || a.month !== b.month || a.day !== b.day;
}

/** Each type's mark in the settings panel's picker (icons.ts, duotone like every icon). */
export const CHART_TYPE_ICONS: Readonly<Record<ChartType, IconName>> = {
  candles: "chartCandles",
  hollow: "chartHollow",
  bars: "chartBars",
  heikinAshi: "chartHeikinAshi",
  line: "chartLine",
  area: "chartArea",
  baseline: "chartBaseline",
};
