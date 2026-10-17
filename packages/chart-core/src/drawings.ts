import type { DisplayList, Rect } from "./displayList.ts";
import type { ChartTheme } from "./displayList.ts";
import { formatPrice } from "./format.ts";
import type { ChartModel, Point } from "./layout.ts";
import { priceY, slotX, yPrice, type Slot } from "./scale.ts";
import { dragPosition, hitPosition, isPositionKind, layoutPosition, newPosition } from "./position.ts";
import { lowerBound } from "./store.ts";

// User drawings: trend lines, rays, horizontal levels, vertical markers and
// ranges. Anchors are DATA coordinates (time, price), never pixels, so a
// drawing stays on its candles through pans, zooms, timeframe switches and
// new data. Time maps to the index-based x axis through the slots (gaps take
// one slot, see scale.ts); a time between two slots interpolates, and a time
// past either end extrapolates one slot per timeframe interval.
//
// Drawings are data marks like the candles, so they are lines and fills on
// the plot. The no-borders rule (UI chrome) does not apply to them, but a
// range is still a fill only, with no outline.

/** long and short are the position tool (position.ts). */
export type DrawingKind = "trend" | "ray" | "hline" | "vline" | "rect" | "long" | "short";

export const DRAWING_KINDS: readonly DrawingKind[] = ["trend", "ray", "hline", "vline", "rect", "long", "short"];

export const DRAWING_LABELS: Readonly<Record<DrawingKind, string>> = {
  trend: "Trend line",
  ray: "Ray",
  hline: "Horizontal line",
  vline: "Vertical line",
  rect: "Range",
  long: "Long position",
  short: "Short position",
};

/** One anchor: epoch ms and price. */
export interface Anchor {
  t: number;
  p: number;
}

export interface Drawing {
  id: string;
  kind: DrawingKind;
  a: Anchor;
  /** Second anchor; hline and vline ignore it (it equals a). A position: the target (its t ends the zones). */
  b: Anchor;
  /** A position's stop (its t equals b's). */
  c?: Anchor;
  /** A position's risk budget in dollars (sizes the quantity). */
  risk?: number;
  /** A position's share count when set by hand; absent = sized from risk. */
  qty?: number | null;
}

/** Whether a kind is placed with one point (a level, a marker) or two. */
export const isOnePoint = (k: DrawingKind) => k === "hline" || k === "vline" || isPositionKind(k);

/**
 * The part of a drawing under the pointer: its body, or one of its handles.
 * A position adds c (its stop) and end (where its zones end in time).
 */
export type DrawingPart = "body" | "a" | "b" | "c" | "end";

/** Hit tolerance in px: fingers need more room than a mouse. */
export const DRAWING_HIT_PX = { mouse: 6, touch: 14 } as const;

/** Fractional slot index of time t (see the header for between-slot and off-end times). */
export function tToIndex(slots: readonly Slot[], t: number, timeframeMs: number): number {
  const n = slots.length;
  if (n === 0) return 0;
  const first = slots[0]!.t;
  const last = slots[n - 1]!.t;
  if (t <= first) return (t - first) / timeframeMs;
  if (t >= last) return n - 1 + (t - last) / timeframeMs;
  const i = lowerBound(slots, t);
  const hi = slots[i]!;
  if (hi.t === t) return i;
  const lo = slots[i - 1]!;
  return i - 1 + (t - lo.t) / (hi.t - lo.t);
}

/** Inverse of tToIndex. */
export function indexToT(slots: readonly Slot[], idx: number, timeframeMs: number): number {
  const n = slots.length;
  if (n === 0) return idx * timeframeMs;
  if (idx <= 0) return slots[0]!.t + idx * timeframeMs;
  if (idx >= n - 1) return slots[n - 1]!.t + (idx - (n - 1)) * timeframeMs;
  const i = Math.floor(idx);
  const f = idx - i;
  const lo = slots[i]!;
  const hi = slots[i + 1]!;
  return lo.t + f * (hi.t - lo.t);
}

/** Pixel position of an anchor. */
export function anchorXY(model: ChartModel, a: Anchor): Point | null {
  if (!model.price) return null;
  return { x: slotX(model.index, tToIndex(model.slots, a.t, model.timeframeMs)), y: priceY(model.price, a.p) };
}

/**
 * The anchor under a pixel. With `snap`, time snaps to the nearest slot
 * centre (a new point lands on a candle, as in every charting package);
 * price is never snapped.
 */
export function anchorAt(model: ChartModel, x: number, y: number, snap: boolean): Anchor | null {
  if (!model.price) return null;
  const ix = model.index;
  let idx = ix.edge - (ix.plotRight - x) / ix.barSpacing - 0.5;
  if (snap) idx = Math.round(idx);
  return { t: indexToT(model.slots, idx, model.timeframeMs), p: yPrice(model.price, y) };
}

/** Bottom of everything time-aligned (plot, volume and indicator panes): where a vertical line ends. */
export function timeAlignedBottom(model: ChartModel): number {
  const g = model.geometry;
  const last = g.panes.at(-1) ?? g.volume ?? g.plot;
  return last.y + last.h;
}

/** Liang–Barsky: the part of segment p→q inside r, or null. */
export function clipSegment(p: Point, q: Point, r: Rect): [Point, Point] | null {
  let t0 = 0;
  let t1 = 1;
  const dx = q.x - p.x;
  const dy = q.y - p.y;
  const edges: [number, number][] = [
    [-dx, p.x - r.x],
    [dx, r.x + r.w - p.x],
    [-dy, p.y - r.y],
    [dy, r.y + r.h - p.y],
  ];
  for (const [pk, qk] of edges) {
    if (pk === 0) {
      if (qk < 0) return null;
      continue;
    }
    const t = qk / pk;
    if (pk < 0) {
      if (t > t1) return null;
      if (t > t0) t0 = t;
    } else {
      if (t < t0) return null;
      if (t < t1) t1 = t;
    }
  }
  return [
    { x: p.x + t0 * dx, y: p.y + t0 * dy },
    { x: p.x + t1 * dx, y: p.y + t1 * dy },
  ];
}

/** The pixel segments of a drawing, already clipped to the plot (rects: their box). */
function geometryOf(model: ChartModel, d: Drawing): { segments: [Point, Point][]; box: Rect | null; a: Point | null; b: Point | null } {
  const plot = model.geometry.plot;
  const a = anchorXY(model, d.a);
  const b = anchorXY(model, d.b);
  const none = { segments: [], box: null, a, b };
  if (!a || !b) return none;
  switch (d.kind) {
    case "trend": {
      const s = clipSegment(a, b, plot);
      return { segments: s ? [s] : [], box: null, a, b };
    }
    case "ray": {
      // From a through b to well past the plot edge, then clipped.
      const len = Math.hypot(b.x - a.x, b.y - a.y) || 1;
      const far = (plot.w + plot.h) * 4;
      const end = { x: a.x + ((b.x - a.x) / len) * far, y: a.y + ((b.y - a.y) / len) * far };
      const s = clipSegment(a, end, plot);
      return { segments: s ? [s] : [], box: null, a, b };
    }
    case "hline": {
      if (a.y < plot.y || a.y > plot.y + plot.h) return none;
      return { segments: [[{ x: plot.x, y: a.y }, { x: plot.x + plot.w, y: a.y }]], box: null, a, b: a };
    }
    case "vline": {
      if (a.x < plot.x || a.x > plot.x + plot.w) return none;
      return { segments: [[{ x: a.x, y: plot.y }, { x: a.x, y: timeAlignedBottom(model) }]], box: null, a, b: a };
    }
    case "long":
    case "short":
      // Drawn and hit-tested by position.ts.
      return none;
    case "rect": {
      const x0 = Math.max(plot.x, Math.min(a.x, b.x));
      const x1 = Math.min(plot.x + plot.w, Math.max(a.x, b.x));
      const y0 = Math.max(plot.y, Math.min(a.y, b.y));
      const y1 = Math.min(plot.y + plot.h, Math.max(a.y, b.y));
      return { segments: [], box: x1 > x0 && y1 > y0 ? { x: x0, y: y0, w: x1 - x0, h: y1 - y0 } : null, a, b };
    }
  }
}

function distToSegment(p: Point, [a, b]: [Point, Point]): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len2 = dx * dx + dy * dy;
  const t = len2 ? Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2)) : 0;
  return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
}

export interface DrawingHit {
  id: string;
  part: DrawingPart;
}

/**
 * The topmost drawing under (x, y). Handles win over bodies, and a selected
 * drawing's handles are checked first, so grabbing an end of the selected
 * line never grabs a different line crossing it.
 */
export function hitDrawing(model: ChartModel, drawings: readonly Drawing[], x: number, y: number, tol: number, selectedId: string | null = null): DrawingHit | null {
  const p = { x, y };
  const ordered = [...drawings].reverse().sort((m, n) => Number(n.id === selectedId) - Number(m.id === selectedId));
  for (const d of ordered) {
    if (isPositionKind(d.kind)) {
      const part = hitPosition(model, d, x, y, tol);
      if (part) return { id: d.id, part };
      continue;
    }
    const g = geometryOf(model, d);
    if (!isOnePoint(d.kind)) {
      if (g.a && Math.hypot(g.a.x - x, g.a.y - y) <= tol) return { id: d.id, part: "a" };
      if (g.b && Math.hypot(g.b.x - x, g.b.y - y) <= tol) return { id: d.id, part: "b" };
    }
    if (g.segments.some((s) => distToSegment(p, s) <= tol)) return { id: d.id, part: "body" };
    if (g.box && x >= g.box.x - tol && x <= g.box.x + g.box.w + tol && y >= g.box.y - tol && y <= g.box.y + g.box.h + tol) return { id: d.id, part: "body" };
  }
  return null;
}

/**
 * Where a drag of `part` that started at `from` and is now at `to` puts the
 * ORIGINAL drawing. Always computed from the drawing as it was when the drag
 * began, so rounding never accumulates. A body moves both anchors by the same
 * slots and the same screen distance in price; a handle moves to the pointer
 * (snapped to a slot).
 */
export function dragDrawing(model: ChartModel, original: Drawing, part: DrawingPart, from: Point, to: Point): Drawing {
  if (!model.price) return original;
  if (isPositionKind(original.kind)) return dragPosition(model, original, part, from, to);
  const price = model.price;
  if (part === "body") {
    const dIdx = (to.x - from.x) / model.index.barSpacing;
    const dy = to.y - from.y;
    const move = (an: Anchor): Anchor => ({
      t: indexToT(model.slots, tToIndex(model.slots, an.t, model.timeframeMs) + dIdx, model.timeframeMs),
      p: yPrice(price, priceY(price, an.p) + dy),
    });
    const a = move(original.a);
    return { ...original, a, b: isOnePoint(original.kind) ? a : move(original.b) };
  }
  const at = anchorAt(model, to.x, to.y, true);
  if (!at) return original;
  return part === "a" ? { ...original, a: at } : { ...original, b: at };
}

/** Builds a drawing from two pixel points (one-point kinds use `a`). */
export function drawingFromPoints(model: ChartModel, id: string, kind: DrawingKind, a: Point, b: Point): Drawing | null {
  if (isPositionKind(kind)) return newPosition(model, id, kind, a);
  const pa = anchorAt(model, a.x, a.y, true);
  const pb = anchorAt(model, b.x, b.y, true);
  if (!pa || !pb) return null;
  return { id, kind, a: pa, b: isOnePoint(kind) ? pa : pb };
}

/** Display list for drawings; the selected one gets handles and, for a level, its price tag on the axis. */
export function layoutDrawings(model: ChartModel, drawings: readonly Drawing[], selectedId: string | null, theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const g = model.geometry;
  for (const d of drawings) {
    const sel = d.id === selectedId;
    if (isPositionKind(d.kind)) {
      out.push(...layoutPosition(model, d, sel, theme));
      continue;
    }
    const stroke = sel ? theme.drawingSelected : theme.drawing;
    const geo = geometryOf(model, d);
    if (geo.box) out.push({ kind: "rect", ...geo.box, fill: theme.drawingFill, layer: "overlay" });
    for (const [p, q] of geo.segments) out.push({ kind: "line", x1: p.x, y1: p.y, x2: q.x, y2: q.y, stroke, lineWidth: sel ? 2 : 1.5, layer: "overlay" });
    if (d.kind === "hline" && geo.a && model.price && geo.segments.length) {
      const y = geo.a.y;
      out.push({ kind: "rect", x: g.priceAxis.x + 2, y: y - 9, w: g.priceAxis.w - 4, h: 18, fill: stroke, radius: 3, layer: "crosshair" });
      out.push({ kind: "text", x: g.priceAxis.x + g.priceAxis.w / 2, y, text: formatPrice(d.a.p), fill: theme.onTag, fontSize: model.options.fontSize, fontFamily: theme.fontMono, fontWeight: 600, align: "center", baseline: "middle", layer: "crosshair" });
    }
    if (!sel) continue;
    const handles = isOnePoint(d.kind) ? [] : [geo.a, geo.b];
    for (const h of handles) {
      if (!h || h.x < g.plot.x || h.x > g.plot.x + g.plot.w || h.y < g.plot.y || h.y > g.plot.y + g.plot.h) continue;
      out.push({ kind: "rect", x: h.x - 5, y: h.y - 5, w: 10, h: 10, radius: 5, fill: theme.drawingSelected, layer: "overlay" });
      out.push({ kind: "rect", x: h.x - 3, y: h.y - 3, w: 6, h: 6, radius: 3, fill: theme.handleFill, layer: "overlay" });
    }
  }
  return out;
}

/** A drawing being placed, in pixels (before it has anchors). */
export interface DrawingDraft {
  kind: DrawingKind;
  a: Point;
  b: Point;
}

export function layoutDraft(model: ChartModel, draft: DrawingDraft, theme: ChartTheme): DisplayList {
  const d = drawingFromPoints(model, "draft", draft.kind, draft.a, draft.b);
  return d ? layoutDrawings(model, [d], "draft", theme) : [];
}
