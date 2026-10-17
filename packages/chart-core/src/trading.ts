import type { ChartTheme, DisplayList } from "./displayList.ts";
import { formatPrice, formatSize } from "./format.ts";
import type { ChartModel } from "./layout.ts";
import { priceY } from "./scale.ts";
import { withAlpha } from "./theme.ts";
import type { FillMark } from "./levels.ts";

// Simulated chart trading, chart side: the wire types of the paper broker
// (docs/architecture/chart-trading-sim.md; crates/paper-broker is the source
// of truth), and the order and position lines the chart draws, with their
// hit testing. The chart never sends anything: it emits intents (place,
// modify, cancel) and the host app forwards them. Owner decision 2026-10-17:
// simulator only; every state carries `simulated: true` and the UI says so.

export type OrderSide = "buy" | "sell";
export type OrderType = "market" | "limit" | "stop" | "stop_limit";
/** pending: a bracket exit waiting for its entry to fill (it neither fills nor triggers yet). */
export type OrderStatus = "pending" | "open" | "triggered" | "filled" | "cancelled" | "rejected";

export type OrderRole = "entry" | "take_profit" | "stop_loss";

export interface PaperOrder {
  id: string;
  clientOrderId: string;
  symbol: string;
  side: OrderSide;
  orderType: OrderType;
  qty: number;
  filledQty: number;
  avgFillPrice: number | null;
  limitPrice: number | null;
  stopPrice: number | null;
  status: OrderStatus;
  rejectReason: string | null;
  createdAt: number;
  updatedAt: number;
  /** Bracket fields (null outside a bracket): the exits' entry, the order's role, and the group shared by all three. */
  parentId?: string | null;
  role?: OrderRole | null;
  ocoGroup?: string | null;
}

export interface PaperPosition {
  symbol: string;
  /** Signed: negative is short. */
  qty: number;
  avgPrice: number;
  realizedPnl: number;
}

export interface PaperAccount {
  startingCash: number;
  cash: number;
  realizedPnl: number;
}

export interface PaperState {
  type: "paper.state";
  simulated: true;
  seq: number;
  account: PaperAccount;
  orders: PaperOrder[];
  positions: PaperPosition[];
  /** The last 200 fills, oldest first (absent from servers that predate it). */
  fills?: FillMark[];
}

export interface PaperFill {
  type: "paper.fill";
  simulated: true;
  orderId: string;
  symbol: string;
  side: OrderSide;
  qty: number;
  price: number;
  t: number;
}

export interface OrderPlace {
  type: "order.place";
  clientOrderId: string;
  symbol: string;
  side: OrderSide;
  orderType: OrderType;
  qty: number;
  limitPrice?: number;
  stopPrice?: number;
  /** Attach a take-profit limit and a stop-loss stop that cancel each other once the entry fills. */
  bracket?: { takeProfit: number; stopLoss: number };
}

export interface OrderModify {
  type: "order.modify";
  id: string;
  qty?: number;
  limitPrice?: number;
  stopPrice?: number;
}

export interface OrderCancel {
  type: "order.cancel";
  id: string;
}

export type PaperMessage = PaperState | PaperFill;

/** Parses a paper broker frame, or null when it is not one. */
export function parsePaperMessage(raw: unknown): PaperMessage | null {
  let v: unknown = raw;
  if (typeof raw === "string") {
    try {
      v = JSON.parse(raw);
    } catch {
      return null;
    }
  }
  if (typeof v !== "object" || v === null) return null;
  const type = (v as { type?: unknown }).type;
  return type === "paper.state" || type === "paper.fill" ? (v as PaperMessage) : null;
}

/** The tick grid the broker accepts: $0.01 at or above $1.00, $0.0001 below (Reg NMS sub-penny rule). */
export function tickSize(p: number): number {
  return p >= 1 ? 0.01 : 0.0001;
}

/** Snaps a price onto the tick grid (and to a clean float, so 3.4 never goes out as 3.4000000000000004). */
export function snapPrice(p: number): number {
  const tick = tickSize(p);
  const snapped = Math.round(p / tick) * tick;
  // A price just under $1 can round up to exactly 1.00, which is on both grids.
  return Number(snapped.toFixed(tick === 0.01 ? 2 : 4));
}

/**
 * The resting order a click at `price` means for a side, relative to the
 * last trade (Robinhood Legend's rule): below the market a buy limit or a
 * sell stop, above it a sell limit or a buy stop.
 */
export function orderTypeAt(side: OrderSide, price: number, last: number): "limit" | "stop" {
  const below = price < last;
  return side === "buy" ? (below ? "limit" : "stop") : below ? "stop" : "limit";
}

/** The price an order line sits at, and the field a drag modifies. */
export function orderLinePrice(o: PaperOrder): { price: number; field: "limitPrice" | "stopPrice" } | null {
  if (o.orderType === "limit" && o.limitPrice !== null) return { price: o.limitPrice, field: "limitPrice" };
  if (o.orderType === "stop" && o.stopPrice !== null) return { price: o.stopPrice, field: "stopPrice" };
  if (o.orderType === "stop_limit") {
    // Until it triggers the stop is what matters; after, the limit.
    if (o.status === "triggered" && o.limitPrice !== null) return { price: o.limitPrice, field: "limitPrice" };
    if (o.stopPrice !== null) return { price: o.stopPrice, field: "stopPrice" };
  }
  return null;
}

const TYPE_SHORT: Readonly<Record<OrderType, string>> = { market: "MKT", limit: "LMT", stop: "STP", stop_limit: "STP LMT" };

export function orderLabel(o: PaperOrder): string {
  const role = o.role === "take_profit" ? "TP · " : o.role === "stop_loss" ? "SL · " : "";
  return `${role}${o.side === "buy" ? "Buy" : "Sell"} ${TYPE_SHORT[o.orderType]}${o.status === "triggered" ? " ·trig" : ""} ${formatSize(o.qty)}`;
}

/** What the chart draws for trading: this symbol's working orders and position. */
export interface TradingLayer {
  orders: readonly PaperOrder[];
  position: PaperPosition | null;
  /** Last trade, for the position's unrealized P&L. */
  last: number | null;
  /** An order being dragged: drawn at this price instead of its own. */
  dragging?: { id: string; price: number } | null;
  /** y of the "+" place button (the hover crosshair's y), or null to hide it. */
  placeY?: number | null;
}

/** Hit regions of trading objects, in chart surface px. */
export type TradingPart = { kind: "order"; id: string; part: "drag" | "cancel" } | { kind: "place"; y: number };

interface Pill {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  cancelX: number;
}

const PILL_H = 18;
const CANCEL_W = 18;
const PLACE_R = 9;

/** Only working orders have lines: open ones, triggered stop-limits, and bracket exits waiting for their entry. */
export const isWorking = (o: PaperOrder) => o.status === "open" || o.status === "triggered" || o.status === "pending";

function pills(model: ChartModel, layer: TradingLayer): Pill[] {
  const out: Pill[] = [];
  const price = model.price;
  if (!price) return out;
  const plot = model.geometry.plot;
  const fs = model.options.fontSize;
  for (const o of layer.orders) {
    if (!isWorking(o)) continue;
    const line = orderLinePrice(o);
    if (!line) continue;
    const p = layer.dragging?.id === o.id ? layer.dragging.price : line.price;
    // An order off the visible range pins to the plot edge, so it is never lost.
    const y = Math.min(plot.y + plot.h - PILL_H / 2, Math.max(plot.y + PILL_H / 2, priceY(price, p)));
    const w = Math.ceil(orderLabel(o).length * fs * 0.62) + 14 + CANCEL_W;
    const x = plot.x + plot.w - w - 22;
    out.push({ id: o.id, x, y: y - PILL_H / 2, w, h: PILL_H, cancelX: x + w - CANCEL_W });
  }
  return out;
}

/** Order and position lines, their pills, and the "+" place button. */
export function layoutTrading(model: ChartModel, layer: TradingLayer, theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const price = model.price;
  if (!price) return out;
  const g = model.geometry;
  const plot = g.plot;
  const fs = model.options.fontSize;
  const mono = theme.fontMono;
  const inPlot = (y: number) => y >= plot.y && y <= plot.y + plot.h;

  const pos = layer.position;
  if (pos && pos.qty !== 0) {
    const y = priceY(price, pos.avgPrice);
    if (inPlot(y)) {
      out.push({ kind: "line", x1: plot.x, y1: Math.round(y) + 0.5, x2: plot.x + plot.w, y2: Math.round(y) + 0.5, stroke: theme.position, lineWidth: 1, layer: "overlay" });
      const unreal = layer.last === null ? null : (layer.last - pos.avgPrice) * pos.qty;
      const text = `${pos.qty > 0 ? "+" : ""}${formatSize(pos.qty)} @ ${formatPrice(pos.avgPrice)}${unreal === null ? "" : `  ${unreal >= 0 ? "+" : "−"}$${Math.abs(unreal).toFixed(2)}`}`;
      const w = Math.ceil(text.length * fs * 0.62) + 12;
      const fill = unreal === null ? theme.position : unreal >= 0 ? theme.up : theme.down;
      out.push({ kind: "rect", x: plot.x + 6, y: y - PILL_H / 2, w, h: PILL_H, radius: 4, fill, layer: "overlay" });
      out.push({ kind: "text", x: plot.x + 12, y, text, fill: theme.onOrder, fontSize: fs, fontFamily: mono, fontWeight: 600, align: "left", baseline: "middle", layer: "overlay" });
    }
  }

  const byId = new Map(layer.orders.map((o) => [o.id, o]));
  for (const pill of pills(model, layer)) {
    const o = byId.get(pill.id)!;
    const pending = o.status === "pending";
    // A take-profit reads as the good outcome and a stop-loss as the bad one, whatever side they sell on.
    const base = o.role === "take_profit" ? theme.up : o.role === "stop_loss" ? theme.down : o.side === "buy" ? theme.orderBuy : theme.orderSell;
    // An exit waiting for its entry is drawn faintly: it cannot fill yet.
    const color = pending ? withAlpha(base, 0.45) : base;
    const line = orderLinePrice(o)!;
    const p = layer.dragging?.id === o.id ? layer.dragging.price : line.price;
    const y = priceY(price, p);
    const cy = pill.y + pill.h / 2;
    if (inPlot(y)) out.push({ kind: "line", x1: plot.x, y1: Math.round(y) + 0.5, x2: plot.x + plot.w, y2: Math.round(y) + 0.5, stroke: color, lineWidth: 1, dash: [4, 3], layer: "overlay" });
    out.push({ kind: "rect", x: pill.x, y: pill.y, w: pill.w, h: pill.h, radius: 4, fill: color, layer: "overlay" });
    const arrow = y < plot.y ? "↑ " : y > plot.y + plot.h ? "↓ " : "";
    out.push({ kind: "text", x: pill.x + 7, y: cy, text: `${arrow}${orderLabel(o)}`, fill: pending ? theme.text : theme.onOrder, fontSize: fs, fontFamily: mono, fontWeight: 600, align: "left", baseline: "middle", layer: "overlay" });
    // The cancel cell: a darker step of the pill, so it reads as its own target without a divider line.
    out.push({ kind: "rect", x: pill.cancelX, y: pill.y, w: CANCEL_W, h: pill.h, radius: 4, fill: withAlpha(theme.onOrder, 0.28), layer: "overlay" });
    out.push({ kind: "text", x: pill.cancelX + CANCEL_W / 2, y: cy, text: "✕", fill: theme.onOrder, fontSize: fs, fontFamily: mono, fontWeight: 700, align: "center", baseline: "middle", layer: "overlay" });
    // The price tag on the axis while it is being dragged, so the new price is readable.
    if (layer.dragging?.id === o.id && inPlot(y)) {
      out.push({ kind: "rect", x: g.priceAxis.x + 2, y: y - 9, w: g.priceAxis.w - 4, h: 18, fill: color, radius: 3, layer: "crosshair" });
      out.push({ kind: "text", x: g.priceAxis.x + g.priceAxis.w / 2, y, text: formatPrice(p), fill: theme.onOrder, fontSize: fs, fontFamily: mono, fontWeight: 600, align: "center", baseline: "middle", layer: "crosshair" });
    }
  }

  // OCO: a connector between a bracket's two exits, just left of their pills (right of them is the
  // price axis), with its label left of the connector.
  const pillById = new Map(pills(model, layer).map((p) => [p.id, p]));
  const groups = new Map<string, Pill[]>();
  for (const o of layer.orders) {
    if (!o.ocoGroup || !o.role || o.role === "entry" || !isWorking(o)) continue;
    const p = pillById.get(o.id);
    if (p) groups.set(o.ocoGroup, [...(groups.get(o.ocoGroup) ?? []), p]);
  }
  for (const pair of groups.values()) {
    if (pair.length !== 2) continue;
    const [p1, p2] = pair as [Pill, Pill];
    const x = Math.min(p1.x, p2.x) - 8;
    const y1 = Math.min(p1.y, p2.y) + PILL_H / 2;
    const y2 = Math.max(p1.y, p2.y) + PILL_H / 2;
    out.push({ kind: "rect", x: x - 1, y: y1, w: 2, h: y2 - y1, radius: 1, fill: theme.crosshairLabelBg, layer: "overlay" });
    out.push({ kind: "rect", x: x - 30, y: (y1 + y2) / 2 - 8, w: 26, h: 16, radius: 4, fill: theme.crosshairLabelBg, layer: "overlay" });
    out.push({ kind: "text", x: x - 17, y: (y1 + y2) / 2, text: "OCO", fill: theme.text, fontSize: fs - 1, fontFamily: mono, fontWeight: 600, align: "center", baseline: "middle", layer: "overlay" });
  }

  if (layer.placeY !== null && layer.placeY !== undefined && inPlot(layer.placeY)) {
    const cx = plot.x + plot.w - PLACE_R - 3;
    const cy = layer.placeY;
    out.push({ kind: "rect", x: cx - PLACE_R, y: cy - PLACE_R, w: PLACE_R * 2, h: PLACE_R * 2, radius: PLACE_R, fill: theme.crosshairLabelBg, layer: "crosshair" });
    out.push({ kind: "text", x: cx, y: cy + 0.5, text: "+", fill: theme.crosshairLabelText, fontSize: fs + 3, fontFamily: mono, fontWeight: 500, align: "center", baseline: "middle", layer: "crosshair" });
  }
  return out;
}

/**
 * The trading object under (x, y): the place button, an order's cancel cell,
 * or the order itself (its pill, or its line within `tol`).
 */
export function hitTrading(model: ChartModel, layer: TradingLayer, x: number, y: number, tol: number): TradingPart | null {
  const price = model.price;
  if (!price) return null;
  const plot = model.geometry.plot;
  if (layer.placeY !== null && layer.placeY !== undefined) {
    const cx = plot.x + plot.w - PLACE_R - 3;
    if (Math.hypot(x - cx, y - layer.placeY) <= PLACE_R + 2) return { kind: "place", y: layer.placeY };
  }
  const ps = pills(model, layer);
  for (const pill of [...ps].reverse()) {
    if (y < pill.y - 2 || y > pill.y + pill.h + 2) continue;
    if (x >= pill.cancelX && x <= pill.cancelX + CANCEL_W) return { kind: "order", id: pill.id, part: "cancel" };
    if (x >= pill.x && x <= pill.x + pill.w) return { kind: "order", id: pill.id, part: "drag" };
  }
  if (x < plot.x || x > plot.x + plot.w) return null;
  const byId = new Map(layer.orders.map((o) => [o.id, o]));
  for (const pill of ps) {
    const line = orderLinePrice(byId.get(pill.id)!);
    if (line && Math.abs(priceY(price, line.price) - y) <= tol) return { kind: "order", id: pill.id, part: "drag" };
  }
  return null;
}

/** Unrealized P&L of a position at `last`. */
export function unrealizedPnl(pos: PaperPosition, last: number): number {
  return (last - pos.avgPrice) * pos.qty;
}
