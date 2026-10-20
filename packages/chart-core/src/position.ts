import type { ChartTheme, DisplayList } from "./displayList.ts";
import type { Anchor, Drawing, DrawingPart } from "./drawings.ts";
import { indexToT, tToIndex } from "./drawings.ts";
import { formatPrice, formatSize } from "./format.ts";
import type { ChartModel, Point } from "./layout.ts";
import { formatAxisTag, priceY, slotX, yPrice } from "./scale.ts";
import { withAlpha } from "./theme.ts";
import { snapPrice, tickSize } from "./trading.ts";

// The position tool (owner decision 2026-10-17; design canvas
// https://claude.ai/artifact/UkBuQdEMyZjAYmdMsTzatW): a Long or Short
// drawing with an entry, a target zone and a stop zone, sized from a risk
// amount, which the chart can place as one simulated bracket order
// (docs/architecture/chart-trading-sim.md, "Bracket orders").
//
// It is a drawing like the others (time and price anchors, so it stays on its
// candles), with three anchors: a = entry (its t is where the zones start),
// b = target (its t is where they end), c = stop (same t as b). Prices are
// always on the broker's tick grid, so what the tool shows is what Place
// sends.

export const DEFAULT_POSITION_RISK = 100;

/** The kinds that are position tools. */
export const isPositionKind = (k: string): k is "long" | "short" => k === "long" || k === "short";

export interface PositionPlan {
  side: "long" | "short";
  entry: number;
  target: number;
  stop: number;
  /** Shares: the override, else floor(risk ÷ risk per share). */
  qty: number;
  /** Dollars at risk the qty was sized from. */
  riskBudget: number;
  riskPerShare: number;
  rewardPerShare: number;
  /** Dollars if the target fills / if the stop fills, at qty. */
  reward: number;
  risk: number;
  /** Reward per share ÷ risk per share. */
  rr: number;
  value: number;
  targetPct: number;
  stopPct: number;
  /** Target and stop on the right sides of the entry, and at least one share. */
  valid: boolean;
}

export function positionPlan(d: Drawing): PositionPlan {
  const side = d.kind === "short" ? "short" : "long";
  const entry = d.a.p;
  const target = d.b.p;
  const stop = d.c?.p ?? d.a.p;
  const riskPerShare = Math.abs(entry - stop);
  const rewardPerShare = Math.abs(target - entry);
  const riskBudget = d.risk ?? DEFAULT_POSITION_RISK;
  // A 1e-9 nudge so $100 ÷ $0.07 is 1428, not 1428.5714 floored through float noise to 1428 anyway,
  // and $100 ÷ $0.05 is exactly 2000 rather than 1999.
  const sized = riskPerShare > 0 ? Math.floor(riskBudget / riskPerShare + 1e-9) : 0;
  const qty = d.qty ?? sized;
  const ordered = side === "long" ? target > entry && entry > stop : target < entry && entry < stop;
  return {
    side,
    entry,
    target,
    stop,
    qty,
    riskBudget,
    riskPerShare,
    rewardPerShare,
    reward: rewardPerShare * qty,
    risk: riskPerShare * qty,
    rr: riskPerShare > 0 ? rewardPerShare / riskPerShare : 0,
    value: entry * qty,
    targetPct: entry ? (target / entry - 1) * 100 : 0,
    stopPct: entry ? (stop / entry - 1) * 100 : 0,
    valid: ordered && qty > 0,
  };
}

/**
 * A new position from a tap at `at`: entry at the tap, the stop one eighth of
 * the visible price range away (at least 1% of the price) and the target at
 * twice that (2R, the usual
 * starting point), the zones reaching a quarter of the plot to the right.
 */
export function newPosition(model: ChartModel, id: string, kind: "long" | "short", at: Point): Drawing | null {
  const price = model.price;
  if (!price) return null;
  const ix = model.index;
  const idx = Math.round(ix.edge - (ix.plotRight - at.x) / ix.barSpacing - 0.5);
  const span = Math.max(8, Math.round(model.geometry.plot.w / ix.barSpacing / 4));
  const entry = snapPrice(yPrice(price, at.y));
  // The distance in whole ticks of the ENTRY's grid (snapPrice on a sub-dollar distance would use
  // the $0.0001 grid and put the target off 2R).
  const tick = tickSize(entry);
  // An eighth of the visible range, but at least 1% of the price: in a quiet stretch the range can
  // be a few cents, and a one-cent stop would size an absurd position.
  const d = Math.max(1, Math.round(Math.max((price.hi - price.lo) / 8, entry * 0.01) / tick)) * tick;
  const dir = kind === "long" ? 1 : -1;
  const t0 = indexToT(model.slots, idx, model.timeframeMs);
  const t1 = indexToT(model.slots, idx + span, model.timeframeMs);
  return {
    id,
    kind,
    a: { t: t0, p: entry },
    b: { t: t1, p: snapPrice(entry + dir * 2 * d) },
    c: { t: t1, p: snapPrice(Math.max(tickSize(entry), entry - dir * d)) },
    risk: DEFAULT_POSITION_RISK,
  };
}

interface Geo {
  x0: number;
  x1: number;
  yE: number;
  yT: number;
  yS: number;
}

function geo(model: ChartModel, d: Drawing): Geo | null {
  const price = model.price;
  if (!price || !d.c) return null;
  const x = (t: number) => slotX(model.index, tToIndex(model.slots, t, model.timeframeMs));
  return { x0: x(d.a.t), x1: x(d.b.t), yE: priceY(price, d.a.p), yT: priceY(price, d.b.p), yS: priceY(price, d.c.p) };
}

const HANDLE_R = 6;

/** Handles of a position, in px: entry (start), target, stop, and the end of the time span. */
function handles(g: Geo): { part: DrawingPart; x: number; y: number }[] {
  return [
    { part: "b", x: g.x1, y: g.yT },
    { part: "c", x: g.x1, y: g.yS },
    { part: "a", x: g.x0, y: g.yE },
    { part: "end", x: g.x1, y: g.yE },
  ];
}

export function hitPosition(model: ChartModel, d: Drawing, x: number, y: number, tol: number): DrawingPart | null {
  const g = geo(model, d);
  if (!g) return null;
  for (const h of handles(g)) if (Math.hypot(h.x - x, h.y - y) <= tol + 2) return h.part;
  const left = Math.min(g.x0, g.x1);
  const right = Math.max(g.x0, g.x1);
  const top = Math.min(g.yT, g.yS);
  const bottom = Math.max(g.yT, g.yS);
  return x >= left && x <= right && y >= top - tol && y <= bottom + tol ? "body" : null;
}

/**
 * Where a drag moves the ORIGINAL position. Prices snap to the tick grid and
 * keep their order (a long's target above its entry above its stop, by at
 * least one tick), so the tool can never show a plan the broker would reject.
 */
export function dragPosition(model: ChartModel, original: Drawing, part: DrawingPart, from: Point, to: Point): Drawing {
  const price = model.price;
  if (!price || !original.c) return original;
  const long = original.kind !== "short";
  const at = (y: number) => snapPrice(yPrice(price, y));
  const tick = (p: number) => tickSize(p);
  const slotT = (x: number) => {
    const ix = model.index;
    return indexToT(model.slots, Math.round(ix.edge - (ix.plotRight - x) / ix.barSpacing - 0.5), model.timeframeMs);
  };
  const above = (p: number, floor: number) => Math.max(p, snapPrice(floor + tick(floor)));
  const below = (p: number, ceil: number) => Math.max(tick(ceil), Math.min(p, snapPrice(ceil - tick(ceil))));
  const { a, b, c } = { a: original.a, b: original.b, c: original.c };
  switch (part) {
    case "rotate":
      // Positions do not turn: their zones are horizontal price bands.
      return original;
    case "b": {
      const p = at(to.y);
      return { ...original, b: { ...b, p: long ? above(p, a.p) : below(p, a.p) } };
    }
    case "c": {
      const p = at(to.y);
      return { ...original, c: { ...c, p: long ? below(p, a.p) : above(p, a.p) } };
    }
    case "a": {
      const hi = long ? b.p : c.p;
      const lo = long ? c.p : b.p;
      const p = Math.min(snapPrice(hi - tick(hi)), Math.max(snapPrice(lo + tick(lo)), at(to.y)));
      const t = Math.min(slotT(to.x), indexToT(model.slots, tToIndex(model.slots, b.t, model.timeframeMs) - 1, model.timeframeMs));
      return { ...original, a: { t, p } };
    }
    case "end": {
      const minT = indexToT(model.slots, tToIndex(model.slots, a.t, model.timeframeMs) + 1, model.timeframeMs);
      const t = Math.max(minT, slotT(to.x));
      return { ...original, b: { ...b, t }, c: { ...c, t } };
    }
    case "body": {
      const dIdx = (to.x - from.x) / model.index.barSpacing;
      const dy = to.y - from.y;
      const move = (an: Anchor): Anchor => ({ t: indexToT(model.slots, tToIndex(model.slots, an.t, model.timeframeMs) + dIdx, model.timeframeMs), p: at(priceY(price, an.p) + dy) });
      const na = move(a);
      // Moving the whole plan keeps its distances exactly, in ticks.
      return { ...original, a: na, b: { t: move(b).t, p: snapPrice(na.p + (b.p - a.p)) }, c: { t: move(c).t, p: snapPrice(na.p + (c.p - a.p)) } };
    }
  }
}

const money = (v: number) => `${v >= 0 ? "+" : "−"}$${Math.abs(v).toFixed(2)}`;
const pct = (v: number) => `${v >= 0 ? "+" : "−"}${Math.abs(v).toFixed(2)}%`;

/** Zones, lines, labels and (when selected) handles and axis tags of a position. */
export function layoutPosition(model: ChartModel, d: Drawing, selected: boolean, theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const g = geo(model, d);
  if (!g) return out;
  const plot = model.geometry.plot;
  const ax = model.geometry.priceAxis;
  const fs = model.options.fontSize;
  const mono = theme.fontMono;
  const plan = positionPlan(d);
  const left = Math.max(plot.x, Math.min(g.x0, g.x1));
  const right = Math.min(plot.x + plot.w, Math.max(g.x0, g.x1));
  if (right <= left) return out;
  const clipY = (y: number) => Math.min(plot.y + plot.h, Math.max(plot.y, y));
  const zone = (y1: number, y2: number, fill: string) => {
    const t = clipY(Math.min(y1, y2));
    const b = clipY(Math.max(y1, y2));
    if (b > t) out.push({ kind: "rect", x: left, y: t, w: right - left, h: b - t, fill, layer: "overlay" });
  };
  const strength = selected ? 0.18 : 0.12;
  zone(g.yE, g.yT, withAlpha(theme.up, strength));
  zone(g.yE, g.yS, withAlpha(theme.down, strength));
  const hline = (y: number, stroke: string, width: number, dash?: number[]) => {
    if (y < plot.y || y > plot.y + plot.h) return;
    out.push({ kind: "line", x1: left, y1: y, x2: right, y2: y, stroke, lineWidth: width, ...(dash ? { dash } : {}), layer: "overlay" });
  };
  hline(g.yT, theme.up, 2);
  hline(g.yS, theme.down, 2);
  hline(g.yE, theme.text, 1.5, [5, 3]);

  // Labels in the zones: price and percent, then the dollar result at the sized quantity.
  const label = (y: number, inward: number, lines: [string, string]) => {
    const ly = clipY(y + inward);
    if (right - left < 90) return;
    out.push({ kind: "text", x: left + 8, y: ly, text: lines[0], fill: theme.text, fontSize: fs, fontFamily: mono, fontWeight: 500, align: "left", baseline: inward > 0 ? "top" : "bottom", layer: "overlay" });
    out.push({ kind: "text", x: left + 8, y: ly + (inward > 0 ? fs + 4 : -(fs + 4)), text: lines[1], fill: theme.textDim, fontSize: fs, fontFamily: mono, fontWeight: 500, align: "left", baseline: inward > 0 ? "top" : "bottom", layer: "overlay" });
  };
  const targetInward = g.yT < g.yE ? 6 : -6;
  label(g.yT, targetInward, [`Target ${formatPrice(plan.target)} · ${pct(plan.targetPct)}`, money(plan.reward)]);
  label(g.yS, -targetInward, [`Stop ${formatPrice(plan.stop)} · ${pct(plan.stopPct)}`, money(-plan.risk)]);

  // The pill on the entry: side, size, price and R:R.
  if (g.yE >= plot.y && g.yE <= plot.y + plot.h) {
    const text = `${plan.side === "long" ? "Long" : "Short"} ${formatSize(plan.qty)} @ ${formatPrice(plan.entry)}  R:R ${plan.rr.toFixed(2)}`;
    const w = Math.ceil(text.length * fs * 0.62) + 16;
    if (right - left > w + 8) {
      const cx = (left + right) / 2;
      out.push({ kind: "rect", x: cx - w / 2, y: g.yE - 9, w, h: 18, radius: 9, fill: theme.crosshairLabelBg, layer: "overlay" });
      out.push({ kind: "text", x: cx, y: g.yE, text, fill: plan.valid ? theme.text : theme.down, fontSize: fs, fontFamily: mono, fontWeight: 500, align: "center", baseline: "middle", layer: "overlay" });
    }
  }

  if (!selected) return out;
  const tag = (y: number, fill: string, p: number) => {
    if (y < plot.y || y > plot.y + plot.h) return;
    // On the crosshair layer, above the axis tick labels, which would otherwise print through the tag.
    out.push({ kind: "rect", x: ax.x + 2, y: y - 9, w: ax.w - 4, h: 18, fill, radius: 3, layer: "crosshair" });
    out.push({ kind: "text", x: ax.x + ax.w / 2, y, text: formatAxisTag(model, p), fill: theme.onTag, fontSize: fs, fontFamily: mono, fontWeight: 600, align: "center", baseline: "middle", layer: "crosshair" });
  };
  tag(g.yT, theme.up, plan.target);
  tag(g.yE, theme.text, plan.entry);
  tag(g.yS, theme.down, plan.stop);
  const colors: Record<string, string> = { a: theme.text, b: theme.up, c: theme.down, end: theme.textDim };
  for (const h of handles(g)) {
    if (h.x < plot.x - 1 || h.x > plot.x + plot.w + 1 || h.y < plot.y || h.y > plot.y + plot.h) continue;
    if (h.part === "end") {
      out.push({ kind: "rect", x: h.x - 4, y: h.y - 10, w: 8, h: 20, radius: 4, fill: theme.handleFill, layer: "overlay" });
      out.push({ kind: "rect", x: h.x - 2.5, y: h.y - 8.5, w: 5, h: 17, radius: 2.5, fill: colors.end!, layer: "overlay" });
      continue;
    }
    out.push({ kind: "rect", x: h.x - HANDLE_R - 2, y: h.y - HANDLE_R - 2, w: (HANDLE_R + 2) * 2, h: (HANDLE_R + 2) * 2, radius: HANDLE_R + 2, fill: theme.handleFill, layer: "overlay" });
    out.push({ kind: "rect", x: h.x - HANDLE_R, y: h.y - HANDLE_R, w: HANDLE_R * 2, h: HANDLE_R * 2, radius: HANDLE_R, fill: colors[h.part]!, layer: "overlay" });
  }
  return out;
}

/** Turns a long into a short (or back), mirroring target and stop around the entry. */
export function flipPosition(d: Drawing): Drawing {
  if (!d.c) return d;
  const kind = d.kind === "short" ? "long" : "short";
  const mirror = (p: number) => snapPrice(Math.max(tickSize(d.a.p), 2 * d.a.p - p));
  return { ...d, kind, b: { ...d.b, p: mirror(d.b.p) }, c: { ...d.c, p: mirror(d.c.p) } };
}
