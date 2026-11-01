import type { DisplayList } from "./displayList.ts";
import type { ChartTheme } from "./displayList.ts";
import { formatPrice } from "./format.ts";
import type { ChartModel, Point } from "./layout.ts";
import { formatAxisTag, priceY, yPrice } from "./scale.ts";
import { placeButtonX, snapPrice, tickSize } from "./trading.ts";

// Price alerts on the chart (owner-approved design 2026-10-19, docs/architecture/chart-packages.md
// "Alerts"): the geometry, drawing and hit testing every renderer shares. The chart never talks
// to the server: the host passes the alerts in (`ChartAlert`, already resolved to a price line)
// and gets callbacks out; what an alert IS lives in @stockwatcher/app-state alerts.ts.
//
// - A price alert is a dashed accent line across the plot, a label on its right end ("above 2.50
//   · once") and an accent price tag on the axis. The axis tag is also the touch handle: a 44 pt
//   tall target, dragged directly (no long press).
// - Dragging shows "old → new · % from last"; Shift (mouse) moves a tenth as fast; it snaps to
//   the tick ($0.0001 under $1, $0.01 above). Dragging the tag off the right edge deletes.
// - The bell sits beside the hover "+" (place an order) at the same y, mouse only, and opens a
//   new alert at that exact price.
// - Grab priority (the renderer's hitObject): orders and positions first, then alerts, then
//   drawings, so an alert never steals an order's drag and a drawing never steals an alert's.

export interface ChartAlert {
  id: string;
  price: number;
  dir: "above" | "below";
  /** The line's tag text, e.g. "above 2.50 · once". */
  label: string;
  /** Snoozed or cooling: drawn at half strength. */
  muted?: boolean;
}

export interface AlertLayer {
  alerts: readonly ChartAlert[];
  /** An alert being dragged or nudged: drawn at `price`; `deleting` = released here it is deleted. */
  dragging?: { id: string; price: number; deleting?: boolean } | null;
  /** y of the bell (the hover crosshair's, mouse only), or null to hide it. */
  bellY?: number | null;
  /** The "+" place button shows at the same y: the bell sits just right of it. */
  besidePlace?: boolean;
  /** The last price, for the drag label's %. */
  last: number | null;
}

/** What hitAlert reports (part of gestures.ts ObjectHit). */
export type AlertPart = { kind: "alert"; id: string; part: "line" | "tag" } | { kind: "placeAlert"; y: number };

/** The "+" button's radius in trading.ts; the bell matches it. */
const BUTTON_R = 9;
const TAG_H = 18;
/** The axis tag as a finger target: Apple's and Material's 44 pt minimum. */
export const ALERT_HANDLE_PX = 44;
/** A drag this far right, ending at the surface's right edge, deletes the alert. */
export const DELETE_DRAG_PX = 24;
const EDGE_PX = 10;

/** The price at a height on the chart's own scale (linear, log or percent), snapped to the tick. */
export function alertPriceAt(model: ChartModel, y: number): number | null {
  if (!model.price) return null;
  const p = snapPrice(yPrice(model.price, y));
  return p > 0 ? p : null;
}

/** One tick up or down from `price` (the grid changes at $1). */
export function stepTick(price: number, dir: 1 | -1): number {
  const down = dir < 0;
  // At exactly $1 a step down uses the sub-dollar tick.
  const tick = down && price <= 1 ? 0.0001 : tickSize(price);
  return snapPrice(Math.max(0.0001, price + dir * tick));
}

/**
 * The price an alert dragged by `dy` px lands on: from its own line, snapped. `fine` (Shift):
 * a tenth of the pointer's travel, for single-tick moves on a tall axis.
 */
export function alertDragPrice(model: ChartModel, start: number, dy: number, fine = false): number | null {
  if (!model.price) return null;
  return alertPriceAt(model, priceY(model.price, start) + (fine ? dy / 10 : dy));
}

/** Released here, the drag deletes: dragged right past DELETE_DRAG_PX and ending at the right edge. */
export function alertDragDeletes(model: ChartModel, from: Point, to: Point): boolean {
  const right = model.geometry.priceAxis.x + model.geometry.priceAxis.w;
  return to.x - from.x >= DELETE_DRAG_PX && to.x >= right - EDGE_PX;
}

/** "2.50 → 2.55 · +2.0%" (% from the last price). */
export function alertDragText(from: number, to: number, last: number | null): string {
  const pct = last ? ((to - last) / last) * 100 : null;
  return `${formatPrice(from)} → ${formatPrice(to)}${pct === null ? "" : ` · ${pct >= 0 ? "+" : "−"}${Math.abs(pct).toFixed(1)}%`}`;
}

function labelWidth(text: string, fs: number): number {
  return Math.ceil(text.length * fs * 0.6) + 12;
}

interface Placed {
  a: ChartAlert;
  price: number;
  y: number;
  /** The line's label tag, in the plot. */
  label: { x: number; w: number; text: string };
  dragging: boolean;
}

function placed(model: ChartModel, layer: AlertLayer): Placed[] {
  const price = model.price;
  if (!price) return [];
  const plot = model.geometry.plot;
  const fs = model.options.fontSize;
  const out: Placed[] = [];
  for (const a of layer.alerts) {
    const dragging = layer.dragging?.id === a.id;
    const p = dragging ? layer.dragging!.price : a.price;
    const y = priceY(price, p);
    if (y < plot.y || y > plot.y + plot.h) continue;
    const text = dragging ? (layer.dragging!.deleting ? "Release to delete" : alertDragText(a.price, p, layer.last)) : a.label;
    const w = labelWidth(text, fs);
    out.push({ a, price: p, y, label: { x: plot.x + plot.w - w - 6, w, text }, dragging });
  }
  return out;
}

/** The bell's centre x: the "+" button's place (the plot's left edge), or just right of it when both show. */
function bellX(model: ChartModel, beside: boolean): number {
  const plusX = placeButtonX(model.geometry.plot);
  return beside ? plusX + BUTTON_R * 2 + 4 : plusX;
}

/** A small bell, as one closed path around (cx, cy), `r` = its half height. */
function bellPath(cx: number, cy: number, r: number): number[] {
  const u: [number, number][] = [
    [-0.55, 0.35], [-0.55, -0.05], [-0.45, -0.4], [-0.2, -0.6], [0, -0.66], [0.2, -0.6], [0.45, -0.4], [0.55, -0.05], [0.55, 0.35], [0.78, 0.52], [-0.78, 0.52],
  ];
  return u.flatMap(([x, y]) => [cx + x * r, cy + y * r]);
}

/** Alert lines, their labels and axis tags, and the bell. */
export function layoutAlerts(model: ChartModel, layer: AlertLayer, theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  if (!model.price) return out;
  const g = model.geometry;
  const plot = g.plot;
  const fs = model.options.fontSize;
  const mono = theme.fontMono;
  for (const q of placed(model, layer)) {
    const color = q.a.muted && !q.dragging ? theme.alertMuted : theme.alert;
    const y = Math.round(q.y) + 0.5;
    out.push({ kind: "line", x1: plot.x, y1: y, x2: plot.x + plot.w, y2: y, stroke: color, lineWidth: q.dragging ? 2 : 1.5, dash: [6, 5], layer: "overlay" });
    out.push({ kind: "rect", x: q.label.x, y: q.y - 10, w: q.label.w, h: 20, radius: 5, fill: q.dragging && layer.dragging?.deleting ? theme.down : theme.alertLabelBg, layer: "overlay" });
    out.push({ kind: "text", x: q.label.x + 6, y: q.y, text: q.label.text, fill: q.dragging && layer.dragging?.deleting ? theme.onAlert : q.a.muted ? theme.textDim : theme.alertLabelText, fontSize: fs, fontFamily: mono, fontWeight: 500, align: "left", baseline: "middle", layer: "overlay" });
    // The axis tag (the touch handle), over the axis labels; the crosshair layer keeps it above them.
    out.push({ kind: "rect", x: g.priceAxis.x + 2, y: q.y - TAG_H / 2, w: g.priceAxis.w - 2, h: TAG_H, radius: 4, fill: color, layer: "crosshair" });
    out.push({ kind: "text", x: g.priceAxis.x + 1 + g.priceAxis.w / 2, y: q.y, text: formatAxisTag(model, q.price), fill: theme.onAlert, fontSize: fs, fontFamily: mono, fontWeight: 600, align: "center", baseline: "middle", layer: "crosshair" });
  }
  const by = layer.bellY;
  if (by !== null && by !== undefined && by >= plot.y && by <= plot.y + plot.h) {
    const cx = bellX(model, !!layer.besidePlace);
    out.push({ kind: "rect", x: cx - BUTTON_R, y: by - BUTTON_R, w: BUTTON_R * 2, h: BUTTON_R * 2, radius: BUTTON_R, fill: theme.alert, layer: "controls" });
    out.push({ kind: "path", points: bellPath(cx, by - 0.5, BUTTON_R * 0.72), closed: true, fill: theme.onAlert, layer: "controls" });
    out.push({ kind: "rect", x: cx - 1.5, y: by + 4, w: 3, h: 2.2, radius: 1, fill: theme.onAlert, layer: "controls" });
  }
  return out;
}

/**
 * The alert part under (x, y): the bell, an axis tag (44 px tall for a finger), a line label,
 * or a line within `tol`. The renderer asks orders first (trading.ts hitTrading) and drawings
 * last, which is the grab priority.
 */
export function hitAlert(model: ChartModel, layer: AlertLayer, x: number, y: number, tol: number, touch: boolean): AlertPart | null {
  const g = model.geometry;
  const plot = g.plot;
  const by = layer.bellY;
  if (by !== null && by !== undefined && Math.hypot(x - bellX(model, !!layer.besidePlace), y - by) <= BUTTON_R + 2) return { kind: "placeAlert", y: by };
  const ps = placed(model, layer);
  const inAxis = x >= g.priceAxis.x && x <= g.priceAxis.x + g.priceAxis.w;
  const half = (touch ? ALERT_HANDLE_PX : TAG_H + 4) / 2;
  // The nearest tag wins when two alerts sit close together.
  let best: { id: string; d: number } | null = null;
  for (const q of ps) {
    const d = Math.abs(y - q.y);
    if (inAxis && d <= half && (!best || d < best.d)) best = { id: q.a.id, d };
  }
  if (best) return { kind: "alert", id: best.id, part: "tag" };
  for (const q of [...ps].reverse()) {
    if (x >= q.label.x && x <= q.label.x + q.label.w && Math.abs(y - q.y) <= 11) return { kind: "alert", id: q.a.id, part: "tag" };
  }
  if (x < plot.x || x > plot.x + plot.w) return null;
  for (const q of ps) if (Math.abs(q.y - y) <= tol) return { kind: "alert", id: q.a.id, part: "line" };
  return null;
}

/** Where each alert's axis tag is (for the renderers' focusable overlays): id → centre y, in the plot only. */
export function alertTagPositions(model: ChartModel, layer: AlertLayer): { id: string; y: number; price: number }[] {
  return placed(model, layer).map((q) => ({ id: q.a.id, y: q.y, price: q.price }));
}
