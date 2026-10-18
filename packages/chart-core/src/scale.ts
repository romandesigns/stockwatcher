import { formatPrice } from "./format.ts";
import { lowerBound } from "./store.ts";
import { etParts, formatEtDate, formatEtTime, tradingDayOf } from "./time.ts";
import type { Candle, Gap } from "./types.ts";

// The time axis is INDEX-based: every candle and every gap entry takes one
// slot of equal width, so empty stretches (a halt, a quiet pre-market) are
// visible as hatched slots instead of either vanishing or stretching the
// chart. x is computed from the right edge, which makes "follow latest" the
// natural state.

export type Slot = { kind: "candle"; t: number; candle: Candle } | { kind: "gap"; t: number; gap: Gap };

/** Merges candles and gaps (both sorted by t) into one ordered slot list. */
export function buildSlots(candles: readonly Candle[], gaps: readonly Gap[]): Slot[] {
  const out: Slot[] = [];
  let i = 0;
  let j = 0;
  while (i < candles.length || j < gaps.length) {
    const c = candles[i];
    const g = gaps[j];
    if (c && (!g || c.t <= g.t)) {
      out.push({ kind: "candle", t: c.t, candle: c });
      i++;
      // A candle supersedes a gap at the same t (late data filled it).
      if (g && g.t === c.t) j++;
    } else if (g) {
      out.push({ kind: "gap", t: g.t, gap: g });
      j++;
    }
  }
  return out;
}

/**
 * Zoom/pan state. `anchorT === null` means follow the latest slot. Otherwise
 * the right edge is pinned to the slot at anchorT (+ anchorFrac slots), so a
 * panned-back view stays put while new candles arrive.
 */
export interface Viewport {
  /** Pixels per slot. */
  barSpacing: number;
  anchorT: number | null;
  anchorFrac: number;
  /** Empty slots kept right of the latest candle while following. */
  rightMargin: number;
  /**
   * Price axis range. Absent or "auto" fits the visible candles (the default,
   * and what a double-tap / double-click / "A" returns to); "manual" holds
   * the explicit range a price-axis drag produced, in price units whatever
   * the scale type, so switching linear/percent/log keeps the range.
   */
  price?: PriceRangeMode;
}

export type PriceRangeMode = { mode: "auto" } | { mode: "manual"; lo: number; hi: number };

/**
 * How price maps to y. "linear": y is affine in price. "percent": the same
 * mapping as linear (a percent of a fixed base is affine in price), but ticks
 * and labels are percent changes from the first visible candle's open, so
 * the base moves as you pan (Robinhood Legend's "Linear (%)"). "log": y is
 * affine in ln(price), so equal percent moves have equal heights.
 */
export type PriceScaleType = "linear" | "percent" | "log";

/**
 * Zoom limits in px per slot. The floor was 2 until the session zoom presets
 * (chartSettings.ts, 2026-10-18): "100 %" must fit a whole 15 s day (1,560
 * regular slots, 3,840 extended) into a laptop-wide plot, which needs well
 * under 1 px a slot. Below ~2 px every body is the 1 px minimum (bodyWidth),
 * so a candle reads as a wick-wide mark, as TradingView draws a zoomed-out day.
 */
export const MIN_BAR_SPACING = 0.25;
export const MAX_BAR_SPACING = 48;

export function defaultViewport(barSpacing = 9): Viewport {
  return { barSpacing, anchorT: null, anchorFrac: 0, rightMargin: 1.5 };
}

/** Float slot index at the plot's right edge (slot i spans [i, i+1)). */
export function rightEdge(vp: Viewport, slots: readonly Slot[]): number {
  if (vp.anchorT === null || slots.length === 0) return slots.length + vp.rightMargin;
  const i = Math.min(lowerBound(slots, vp.anchorT), slots.length - 1);
  return i + vp.anchorFrac;
}

function withEdge(vp: Viewport, slots: readonly Slot[], edge: number): Viewport {
  const follow = slots.length + vp.rightMargin;
  // Keep at least a few slots on screen at the left limit.
  const minEdge = Math.min(follow, Math.max(1, Math.min(slots.length, 3)));
  const e = Math.max(minEdge, edge);
  if (e >= follow - 1e-6 || slots.length === 0) return { ...vp, anchorT: null, anchorFrac: 0 };
  const i = Math.max(0, Math.min(slots.length - 1, Math.floor(e)));
  return { ...vp, anchorT: slots[i]!.t, anchorFrac: e - i };
}

/** Pans by dx pixels (positive = drag right = look back in time). */
export function pan(vp: Viewport, slots: readonly Slot[], dx: number): Viewport {
  return withEdge(vp, slots, rightEdge(vp, slots) - dx / vp.barSpacing);
}

/** Pans by whole or fractional slots (positive = look back in time). */
export function panSlots(vp: Viewport, slots: readonly Slot[], dxSlots: number): Viewport {
  return withEdge(vp, slots, rightEdge(vp, slots) - dxSlots);
}

/** Zooms by `factor` keeping the slot under `anchorX` (px from plot left) fixed. */
export function zoom(vp: Viewport, slots: readonly Slot[], factor: number, anchorX: number, plotWidth: number): Viewport {
  const bs = Math.min(MAX_BAR_SPACING, Math.max(MIN_BAR_SPACING, vp.barSpacing * factor));
  if (bs === vp.barSpacing) return vp;
  const edge = rightEdge(vp, slots);
  const fromRight = plotWidth - anchorX;
  const u = edge - fromRight / vp.barSpacing;
  const following = vp.anchorT === null;
  const next = { ...vp, barSpacing: bs };
  // While following, zoom around the right edge so the latest candle stays put.
  if (following) return next;
  return withEdge(next, slots, u + fromRight / bs);
}

/** Pans the minimum needed so slot i is fully on screen (keyboard inspection). */
export function ensureVisible(vp: Viewport, slots: readonly Slot[], i: number, plotWidth: number): Viewport {
  const edge = rightEdge(vp, slots);
  const visible = plotWidth / vp.barSpacing;
  // Reaching the latest slot resumes following.
  if (i >= slots.length - 1) return vp.anchorT === null ? vp : { ...vp, anchorT: null, anchorFrac: 0 };
  if (i + 1 > edge) return withEdge(vp, slots, i + 1.5);
  if (i < edge - visible) return withEdge(vp, slots, i + visible - 0.5);
  return vp;
}

export function followLatest(vp: Viewport): Viewport {
  return { ...vp, anchorT: null, anchorFrac: 0 };
}

/** Whether the view is panned back from the latest candle (the "back to latest" button shows then). */
export function isPannedBack(vp: Viewport): boolean {
  return vp.anchorT !== null;
}

/** Slots between the view's right edge and where following the latest would put it (0 when following). */
export function slotsBehindLatest(vp: Viewport, slots: readonly Slot[]): number {
  if (vp.anchorT === null) return 0;
  return Math.max(0, slots.length + vp.rightMargin - rightEdge(vp, slots));
}

/** Duration of the slide back to the latest candle: quick from nearby, capped from far back. */
export function backToLatestMs(behind: number, barSpacing: number): number {
  return Math.round(Math.min(520, 220 + Math.sqrt(behind * barSpacing) * 6));
}

/**
 * One frame of the slide back to the latest candle: the slots to pan
 * (negative = toward now) after `elapsed` ms of `duration`, given the
 * distance `total` at the start and what has been panned so far. Ease-out
 * cubic, so it starts fast and settles; the renderer calls followLatest at
 * the end, so new candles keep the view pinned from then on.
 */
export function backToLatestStep(total: number, panned: number, elapsed: number, duration: number): number {
  const t = Math.min(1, elapsed / duration);
  const eased = 1 - (1 - t) ** 3;
  return -(total * eased - panned);
}

export interface IndexScale {
  edge: number;
  barSpacing: number;
  plotRight: number;
  plotLeft: number;
  /** First and last slot index intersecting the plot (inclusive); from > to if none. */
  from: number;
  to: number;
}

export function indexScale(vp: Viewport, slots: readonly Slot[], plotLeft: number, plotRight: number): IndexScale {
  const edge = rightEdge(vp, slots);
  const visible = (plotRight - plotLeft) / vp.barSpacing;
  const from = Math.max(0, Math.floor(edge - visible));
  const to = Math.min(slots.length - 1, Math.ceil(edge) - 1);
  return { edge, barSpacing: vp.barSpacing, plotRight, plotLeft, from, to };
}

/** x of slot i's center. */
export function slotX(s: IndexScale, i: number): number {
  return s.plotRight - (s.edge - (i + 0.5)) * s.barSpacing;
}

/** Slot index under x, or null when x is outside every slot. */
export function slotAt(s: IndexScale, x: number, count: number): number | null {
  if (x < s.plotLeft || x > s.plotRight) return null;
  const i = Math.floor(s.edge - (s.plotRight - x) / s.barSpacing);
  return i >= 0 && i < count ? i : null;
}

export interface PriceScale {
  lo: number;
  hi: number;
  top: number;
  bottom: number;
  /** Absent = linear (standalone profile panels never set it). */
  type?: PriceScaleType;
  /** Percent mode's 0 %: the first visible candle's open. */
  base?: number;
}

/** Smallest price a log scale accepts; real prices here are >= $0.0001. */
const LOG_FLOOR = 1e-6;

/** Price → the space y is affine in (ln for log, price otherwise). */
export function priceToUnit(type: PriceScaleType | undefined, p: number): number {
  return type === "log" ? Math.log(Math.max(LOG_FLOOR, p)) : p;
}

export function unitToPrice(type: PriceScaleType | undefined, u: number): number {
  return type === "log" ? Math.exp(u) : u;
}

/**
 * Visible price range over candles [from, to] with fractional padding. For
 * a log scale the padding is applied in log space, so it stays a fixed share
 * of the height and lo can never reach zero.
 */
export function priceRange(slots: readonly Slot[], from: number, to: number, pad = 0.08, type: PriceScaleType = "linear"): { lo: number; hi: number } | null {
  let lo = Infinity;
  let hi = -Infinity;
  for (let i = from; i <= to; i++) {
    const s = slots[i];
    if (s?.kind !== "candle") continue;
    if (s.candle.l < lo) lo = s.candle.l;
    if (s.candle.h > hi) hi = s.candle.h;
  }
  if (!Number.isFinite(lo)) return null;
  return padRange(lo, hi, pad, type);
}

/**
 * [lo, hi] grown by `pad` of its span on each side, in log space on a log
 * axis (so the padding stays a fixed share of the height and lo never
 * reaches zero). A flat range (one print) first gets ±0.5 % (at least
 * $0.0005) so it still has a height.
 */
export function padRange(lo: number, hi: number, pad: number, type: PriceScaleType = "linear"): { lo: number; hi: number } {
  if (hi - lo < hi * 1e-4) {
    // A flat range (one print) still needs some height.
    const half = Math.max(hi * 0.005, 0.0005);
    lo -= half;
    hi += half;
  }
  if (type === "log") {
    const ul = priceToUnit("log", lo);
    const uh = priceToUnit("log", hi);
    const p = (uh - ul) * pad;
    return { lo: Math.exp(ul - p), hi: Math.exp(uh + p) };
  }
  const p = (hi - lo) * pad;
  return { lo: lo - p, hi: hi + p };
}

export function priceY(s: PriceScale, p: number): number {
  const ul = priceToUnit(s.type, s.lo);
  const uh = priceToUnit(s.type, s.hi);
  return s.top + ((uh - priceToUnit(s.type, p)) / (uh - ul)) * (s.bottom - s.top);
}

export function yPrice(s: PriceScale, y: number): number {
  const ul = priceToUnit(s.type, s.lo);
  const uh = priceToUnit(s.type, s.hi);
  return unitToPrice(s.type, uh - ((y - s.top) / (s.bottom - s.top)) * (uh - ul));
}

/** Percent change of p from the scale's base (percent mode's axis value). */
export function pricePercent(s: PriceScale, p: number): number {
  return s.base ? (p / s.base - 1) * 100 : 0;
}

/** "+1.25%", "\u22120.50%" (a true minus sign), "0.00%". */
export function formatPercentAxis(pct: number, decimals = 2): string {
  const r = Number(pct.toFixed(decimals));
  if (r === 0) return `${(0).toFixed(decimals)}%`;
  return `${r > 0 ? "+" : "\u2212"}${Math.abs(r).toFixed(decimals)}%`;
}

/** One axis value as the axis words it: a price, or a percent in percent mode. */
export function formatAxisValue(s: PriceScale, p: number, decimals?: number): string {
  if (s.type === "percent" && s.base) return formatPercentAxis(pricePercent(s, p));
  return formatPrice(p, decimals);
}

// Price zoom limits: span / |mid| for linear and percent, the ln-span for log.
const MIN_REL_SPAN = 1e-5;
const MAX_REL_SPAN = 50;
const MAX_LOG_SPAN = Math.log(1e4);

/**
 * Scales the price range by `factor` around the price at `anchorY`. factor
 * > 1 zooms in (a smaller range, taller candles), < 1 zooms out. Works in the
 * scale's own space, so a log axis zooms evenly in percent terms.
 */
export function scalePriceRange(s: PriceScale, factor: number, anchorY: number): { lo: number; hi: number } {
  const ul = priceToUnit(s.type, s.lo);
  const uh = priceToUnit(s.type, s.hi);
  const ua = priceToUnit(s.type, yPrice(s, anchorY));
  const f = factor > 0 && Number.isFinite(factor) ? factor : 1;
  let nl = ua - (ua - ul) / f;
  let nh = ua + (uh - ua) / f;
  const span = nh - nl;
  const mid = Math.max(Math.abs((nl + nh) / 2), 1e-9);
  const minSpan = s.type === "log" ? MIN_REL_SPAN : mid * MIN_REL_SPAN;
  const maxSpan = s.type === "log" ? MAX_LOG_SPAN : mid * MAX_REL_SPAN;
  const clamped = Math.min(maxSpan, Math.max(minSpan, span));
  if (clamped !== span) {
    // Keep the anchor at the same height while clamping.
    const k = (ua - nl) / span;
    nl = ua - k * clamped;
    nh = nl + clamped;
  }
  return { lo: unitToPrice(s.type, nl), hi: unitToPrice(s.type, nh) };
}

/** Moves the price range with a vertical drag of dy pixels: the content follows the pointer. */
export function shiftPriceRange(s: PriceScale, dy: number): { lo: number; hi: number } {
  const ul = priceToUnit(s.type, s.lo);
  const uh = priceToUnit(s.type, s.hi);
  const du = (dy / (s.bottom - s.top)) * (uh - ul);
  return { lo: unitToPrice(s.type, ul + du), hi: unitToPrice(s.type, uh + du) };
}

export function manualPrice(vp: Viewport, range: { lo: number; hi: number }): Viewport {
  return { ...vp, price: { mode: "manual", lo: range.lo, hi: range.hi } };
}

export function autoScalePrice(vp: Viewport): Viewport {
  return isAutoPrice(vp) ? vp : { ...vp, price: { mode: "auto" } };
}

export function isAutoPrice(vp: Viewport): boolean {
  return vp.price === undefined || vp.price.mode === "auto";
}

export interface NiceTicks {
  step: number;
  decimals: number;
  values: number[];
}

/**
 * "Nice" steps of 1, 2, 2.5 and 5 × 10^k, at most ~maxCount ticks in [lo, hi].
 * `minDecimals` defaults to 2 because prices always show at least cents.
 */
export function niceTicks(lo: number, hi: number, maxCount: number, minDecimals = 2): NiceTicks {
  const range = hi - lo;
  if (!(range > 0) || maxCount < 1) return { step: 0, decimals: 2, values: [] };
  const raw = range / maxCount;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const norm = raw / mag;
  const nice = norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 2.5 ? 2.5 : norm <= 5 ? 5 : 10;
  const step = nice * mag;
  const stepDecimals = Math.max(0, -Math.floor(Math.log10(step) + 1e-9) + (nice === 2.5 ? 1 : 0));
  const decimals = Math.max(minDecimals, stepDecimals);
  const values: number[] = [];
  const first = Math.ceil(lo / step - 1e-9);
  for (let k = first; k * step <= hi + step * 1e-9; k++) values.push(Number((k * step).toFixed(decimals + 2)));
  return { step, decimals, values };
}

export interface PriceTicks extends NiceTicks {
  /** Axis label per value (a price, or a percent in percent mode). */
  labels: string[];
}

/** A log range narrower than this (hi / lo) is nearly linear, so it gets linear nice steps. */
const LOG_LINEAR_RATIO = 2;
const LOG_MANTISSAS: readonly (readonly number[])[] = [[1, 2, 3, 4, 5, 6, 7, 8, 9], [1, 2, 5], [1]];

/**
 * Ticks and labels for a price axis of any scale type. Values are always
 * prices (so the grid, the profile and hit testing share one mapping); only
 * which values and how they are worded depend on the type:
 * - linear: niceTicks over the price range.
 * - percent: niceTicks over the percent range from the base, mapped back to
 *   prices and labelled "+1.50%".
 * - log: m × 10^k for the densest mantissa set (1..9, then 1-2-5, then
 *   decades) that fits maxCount, which comes out roughly evenly spaced on a
 *   log axis, once the range spans at least 2×.
 */
export function priceAxisTicks(s: PriceScale, maxCount: number): PriceTicks {
  if (s.type === "percent" && s.base) {
    const base = s.base;
    const t = niceTicks(pricePercent(s, s.lo), pricePercent(s, s.hi), maxCount, 0);
    const d = Math.min(4, Math.max(2, t.decimals));
    return { step: t.step, decimals: d, values: t.values.map((v) => base * (1 + v / 100)), labels: t.values.map((v) => formatPercentAxis(v, d)) };
  }
  if (s.type === "log" && s.lo > 0 && s.hi / s.lo >= LOG_LINEAR_RATIO) {
    for (const m of LOG_MANTISSAS) {
      const values = logCandidates(s.lo, s.hi, m);
      if (values.length <= maxCount || m.length === 1) {
        // Decades only and still too many: keep every k-th one.
        const k = Math.max(1, Math.ceil(values.length / Math.max(1, maxCount)));
        const kept = values.filter((_, i) => i % k === 0);
        return { step: 0, decimals: 2, values: kept, labels: kept.map((v) => formatPrice(v, Math.max(2, exactDecimals(v)))) };
      }
    }
  }
  const t = niceTicks(s.lo, s.hi, maxCount);
  return { ...t, labels: t.values.map((v) => formatPrice(v, t.decimals)) };
}

function logCandidates(lo: number, hi: number, mantissas: readonly number[]): number[] {
  const out: number[] = [];
  for (let e = Math.floor(Math.log10(lo)); e <= Math.ceil(Math.log10(hi)); e++) {
    for (const m of mantissas) {
      const v = Number((m * 10 ** e).toPrecision(6));
      if (v >= lo && v <= hi) out.push(v);
    }
  }
  return out;
}

/** Decimals needed to print v exactly (0.005 → 3). */
function exactDecimals(v: number): number {
  for (let d = 0; d <= 6; d++) if (Math.abs(Number(v.toFixed(d)) - v) < 1e-9) return d;
  return 6;
}

const TIME_STEPS = [15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 900_000, 1_800_000, 3_600_000, 7_200_000, 14_400_000];

export interface TimeTick {
  index: number;
  t: number;
  label: string;
}

/**
 * Picks a label step so labels are at least `minSpacing` px apart, then
 * labels each slot where t crosses a step boundary (ET offsets are whole
 * hours, so aligning on UTC equals aligning on ET for steps up to 1 h; the
 * 2 h / 4 h steps align on the ET hour explicitly). A new trading day is
 * labelled with its date.
 */
export function timeTicks(slots: readonly Slot[], s: IndexScale, timeframeMs: number, minSpacing = 64): TimeTick[] {
  const slotsPerLabel = minSpacing / s.barSpacing;
  const step = TIME_STEPS.find((st) => st >= timeframeMs && st / timeframeMs >= slotsPerLabel) ?? TIME_STEPS[TIME_STEPS.length - 1]!;
  const bucket = (t: number): number => {
    if (step <= 3_600_000) return Math.floor(t / step);
    const p = etParts(t);
    return (p.year * 10_000 + p.month * 100 + p.day) * 100 + Math.floor(p.hour / (step / 3_600_000));
  };
  const ticks: TimeTick[] = [];
  let lastX = -Infinity;
  let lastDay = "";
  const from = Math.max(0, s.from);
  for (let i = from; i <= s.to; i++) {
    const slot = slots[i]!;
    const prev = slots[i - 1];
    if (slot.kind !== "candle") continue;
    if (prev && bucket(prev.t) === bucket(slot.t)) continue;
    const x = slotX(s, i);
    if (x - lastX < minSpacing) continue;
    const d = tradingDayOf(slot.t);
    const dayKey = `${d.year}-${d.month}-${d.day}`;
    const newDay = lastDay !== "" && dayKey !== lastDay;
    const label = newDay ? formatEtDate(slot.t) : formatEtTime(slot.t, step < 60_000 || slot.t % 60_000 !== 0);
    ticks.push({ index: i, t: slot.t, label });
    lastX = x;
    lastDay = dayKey;
  }
  return ticks;
}
