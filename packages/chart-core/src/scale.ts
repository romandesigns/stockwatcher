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
}

export const MIN_BAR_SPACING = 2;
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
}

/** Visible price range over candles [from, to] with fractional padding. */
export function priceRange(slots: readonly Slot[], from: number, to: number, pad = 0.08): { lo: number; hi: number } | null {
  let lo = Infinity;
  let hi = -Infinity;
  for (let i = from; i <= to; i++) {
    const s = slots[i];
    if (s?.kind !== "candle") continue;
    if (s.candle.l < lo) lo = s.candle.l;
    if (s.candle.h > hi) hi = s.candle.h;
  }
  if (!Number.isFinite(lo)) return null;
  if (hi - lo < hi * 1e-4) {
    // A flat range (one print) still needs some height.
    const half = Math.max(hi * 0.005, 0.0005);
    lo -= half;
    hi += half;
  }
  const p = (hi - lo) * pad;
  return { lo: lo - p, hi: hi + p };
}

export function priceY(s: PriceScale, p: number): number {
  return s.top + ((s.hi - p) / (s.hi - s.lo)) * (s.bottom - s.top);
}

export function yPrice(s: PriceScale, y: number): number {
  return s.hi - ((y - s.top) / (s.bottom - s.top)) * (s.hi - s.lo);
}

export interface NiceTicks {
  step: number;
  decimals: number;
  values: number[];
}

/** "Nice" steps of 1, 2, 2.5 and 5 × 10^k, at most ~maxCount ticks in [lo, hi]. */
export function niceTicks(lo: number, hi: number, maxCount: number): NiceTicks {
  const range = hi - lo;
  if (!(range > 0) || maxCount < 1) return { step: 0, decimals: 2, values: [] };
  const raw = range / maxCount;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const norm = raw / mag;
  const nice = norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 2.5 ? 2.5 : norm <= 5 ? 5 : 10;
  const step = nice * mag;
  const stepDecimals = Math.max(0, -Math.floor(Math.log10(step) + 1e-9) + (nice === 2.5 ? 1 : 0));
  // Prices always show at least cents.
  const decimals = Math.max(2, stepDecimals);
  const values: number[] = [];
  const first = Math.ceil(lo / step - 1e-9);
  for (let k = first; k * step <= hi + step * 1e-9; k++) values.push(Number((k * step).toFixed(decimals + 2)));
  return { step, decimals, values };
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
