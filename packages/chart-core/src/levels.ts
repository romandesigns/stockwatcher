import type { ChartTheme, DisplayList } from "./displayList.ts";
import { tToIndex } from "./drawings.ts";
import { formatPrice } from "./format.ts";
import type { ChartModel } from "./layout.ts";
import { priceY, slotX, type Slot } from "./scale.ts";
import { withAlpha } from "./theme.ts";
import { formatEtDateKey, sessionOf, tradingDayOf } from "./time.ts";
import type { OrderRole, OrderSide } from "./trading.ts";
import type { Halt, LuldPoint } from "./types.ts";

// Context layers for small-cap day trading (owner, 2026-10-17): the levels
// traders watch (premarket high/low, high/low of day, the open, yesterday's
// close), LULD bands and trading halts, and your own simulated fills. Each
// one can be switched on and off (ChartLayers); the chart draws only what
// is on. Levels come from the candles the chart already has; halts and LULD
// bands come from the server (docs/architecture/halts-luld-fills.md); fills
// come from paper.state.

export type LayerId = "profile" | "halts" | "luld" | "premarket" | "dayRange" | "open" | "prevClose" | "fills";

export const LAYER_IDS: readonly LayerId[] = ["profile", "halts", "luld", "premarket", "dayRange", "open", "prevClose", "fills"];

/**
 * The layers a chart can show. "profile" is the session profile drawn inside
 * the chart (a phone, the web's narrow layout); a chart that shows the
 * profile in a side panel instead (options.showProfile off) does not list it.
 */
export function layerIdsFor(inChartProfile: boolean): readonly LayerId[] {
  return inChartProfile ? LAYER_IDS : LAYER_IDS.filter((id) => id !== "profile");
}

export const LAYER_INFO: Readonly<Record<LayerId, { name: string; description: string }>> = {
  profile: { name: "Volume profile", description: "Shares traded at each price today" },
  halts: { name: "Halts", description: "Shaded while trading was halted, with its length" },
  luld: { name: "LULD bands", description: "Limit up / limit down price bands (modelled)" },
  premarket: { name: "Premarket high / low", description: "Extremes of 04:00–09:30" },
  dayRange: { name: "High / low of day", description: "Regular-session extremes so far" },
  open: { name: "Open", description: "First regular-session price" },
  prevClose: { name: "Previous close", description: "Last regular price of the prior day" },
  fills: { name: "My fills", description: "Your simulated buys and sells" },
};

/** A swatch shape in a 24×16 box; `color` names a theme role each renderer maps to its own palette. */
export interface LayerSwatchShape {
  d: string;
  paint: "fill" | "stroke";
  color: "warning" | "dim" | "text" | "good" | "critical";
  opacity?: number;
  dash?: readonly number[];
}

/**
 * How each layer looks on the chart, drawn small: the row icon in the web's
 * Levels menu and the phone's Levels sheet, so the menu is also the key.
 * Shared here (like INDICATOR_GLYPHS) so both platforms show the same marks.
 */
export const LAYER_SWATCHES: Readonly<Record<LayerId, readonly LayerSwatchShape[]>> = {
  profile: [
    { d: "M13 3.5h9M15 12.5h7", paint: "stroke", color: "dim", opacity: 0.8 },
    { d: "M7 8h15", paint: "stroke", color: "text", opacity: 0.9 },
  ],
  halts: [
    { d: "M6 1h12a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V3a2 2 0 0 1 2-2z", paint: "fill", color: "warning", opacity: 0.18 },
    { d: "M4 1h2v14H4z", paint: "fill", color: "warning", opacity: 0.6 },
  ],
  luld: [{ d: "M2 4h20M2 12h20", paint: "stroke", color: "warning", opacity: 0.7, dash: [3, 2] }],
  premarket: [{ d: "M2 8h20", paint: "stroke", color: "dim", opacity: 0.7, dash: [2, 2.5] }],
  dayRange: [{ d: "M2 4h20M2 12h20", paint: "stroke", color: "text", opacity: 0.6, dash: [5, 2.5] }],
  open: [{ d: "M2 8h20", paint: "stroke", color: "dim", opacity: 0.7, dash: [2, 2.5] }],
  prevClose: [{ d: "M2 8h20", paint: "stroke", color: "dim", opacity: 0.7, dash: [2, 2.5] }],
  fills: [
    { d: "M7 6 10 11H4z", paint: "fill", color: "good" },
    { d: "M17 10 20 5h-6z", paint: "fill", color: "critical" },
  ],
};

export type ChartLayers = Readonly<Record<LayerId, boolean>>;

/** Everything on except the open, which most small-cap traders read off the candles anyway. */
export const DEFAULT_LAYERS: ChartLayers = { profile: true, halts: true, luld: true, premarket: true, dayRange: true, open: false, prevClose: true, fills: true };

export function parseLayers(raw: unknown): ChartLayers {
  const out: Record<LayerId, boolean> = { ...DEFAULT_LAYERS };
  if (typeof raw === "object" && raw !== null) for (const id of LAYER_IDS) if (typeof (raw as Record<string, unknown>)[id] === "boolean") out[id] = (raw as Record<string, boolean>)[id]!;
  return out;
}

export type KeyLevelId = "pmHigh" | "pmLow" | "hod" | "lod" | "open" | "prevClose";

export interface KeyLevel {
  id: KeyLevelId;
  label: string;
  price: number;
  layer: LayerId;
}

/**
 * The key levels of the trading day of the latest candle: premarket high
 * and low (04:00–09:30), regular-session high and low so far, the regular
 * open, and the previous trading day's last regular close when the chart
 * still holds it. A level with no candles behind it is left out rather than
 * guessed.
 */
export function keyLevels(slots: readonly Slot[]): KeyLevel[] {
  let last: Slot | undefined;
  for (let i = slots.length - 1; i >= 0; i--) if (slots[i]!.kind === "candle") { last = slots[i]; break; }
  if (!last) return [];
  const day = formatEtDateKey(tradingDayOf(last.t));
  let pmHi = -Infinity, pmLo = Infinity, hi = -Infinity, lo = Infinity;
  let open: number | null = null;
  let prevClose: number | null = null;
  for (const s of slots) {
    if (s.kind !== "candle") continue;
    const c = s.candle;
    const session = sessionOf(c.t);
    const d = formatEtDateKey(tradingDayOf(c.t));
    if (d !== day) {
      // The latest regular candle before today is yesterday's close (slots are ascending).
      if (d < day && session === "regular") prevClose = c.c;
      continue;
    }
    if (session === "pre") {
      pmHi = Math.max(pmHi, c.h);
      pmLo = Math.min(pmLo, c.l);
    } else if (session === "regular") {
      if (open === null) open = c.o;
      hi = Math.max(hi, c.h);
      lo = Math.min(lo, c.l);
    }
  }
  const out: KeyLevel[] = [];
  if (Number.isFinite(pmHi)) out.push({ id: "pmHigh", label: "PM H", price: pmHi, layer: "premarket" }, { id: "pmLow", label: "PM L", price: pmLo, layer: "premarket" });
  if (Number.isFinite(hi)) out.push({ id: "hod", label: "HOD", price: hi, layer: "dayRange" }, { id: "lod", label: "LOD", price: lo, layer: "dayRange" });
  if (open !== null) out.push({ id: "open", label: "Open", price: open, layer: "open" });
  if (prevClose !== null) out.push({ id: "prevClose", label: "Prev close", price: prevClose, layer: "prevClose" });
  return out;
}

/** A simulated fill for the chart's markers and trade list (paper.state fills). */
export interface FillMark {
  orderId: string;
  symbol: string;
  side: OrderSide;
  qty: number;
  price: number;
  t: number;
  role?: OrderRole | null;
}

/** What the chart draws for the context layers; each list is already filtered by the layers that are on. */
export interface ContextLayer {
  levels: readonly KeyLevel[];
  halts: readonly Halt[];
  luld: readonly LuldPoint[];
  fills: readonly FillMark[];
  /** The clock, for an ongoing halt's length and the right end of open spans. */
  now: number | null;
}

const xOfT = (model: ChartModel, t: number) => slotX(model.index, tToIndex(model.slots, t, model.timeframeMs)) - model.index.barSpacing / 2;

function duration(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  return r ? `${m}m ${String(r).padStart(2, "0")}s` : `${m}m`;
}

/** Halts (under the candles) and the HALTED badge. */
export function layoutHalts(model: ChartModel, halts: readonly Halt[], now: number | null, theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const g = model.geometry;
  const plot = g.plot;
  const bottom = (g.volume ?? plot).y + (g.volume ?? plot).h;
  const fs = model.options.fontSize;
  for (const h of halts) {
    const x0 = Math.max(plot.x, xOfT(model, h.start));
    const x1 = h.end === null ? plot.x + plot.w : Math.min(plot.x + plot.w, xOfT(model, h.end) + model.index.barSpacing);
    if (x1 <= plot.x || x0 >= plot.x + plot.w) continue;
    const w = Math.max(2, x1 - x0);
    out.push({ kind: "rect", x: x0, y: plot.y, w, h: bottom - plot.y, fill: theme.halt, layer: "gaps" });
    out.push({ kind: "rect", x: x0, y: plot.y, w: Math.min(2, w), h: bottom - plot.y, fill: withAlpha(theme.haltText, 0.6), layer: "gaps" });
    const len = h.end === null ? (now !== null ? now - h.start : null) : h.end - h.start;
    const text = `HALT${h.reason ? ` ${h.reason}` : ""}${len !== null ? ` · ${duration(len)}` : ""}`;
    // A row down from the top edge, where an off-screen LULD band's tag is pinned.
    out.push({ kind: "text", x: x0 + 5, y: plot.y + 24 + model.options.topRightInset, text, fill: theme.haltText, fontSize: fs, fontFamily: theme.fontMono, fontWeight: 600, align: "left", baseline: "top", layer: "overlay" });
  }
  const ongoing = halts.find((h) => h.end === null);
  if (ongoing) {
    const text = `HALTED${now !== null ? ` · ${duration(now - ongoing.start)}` : ""}`;
    const w = Math.ceil(text.length * (fs + 1) * 0.62) + 20;
    const cx = plot.x + plot.w / 2;
    out.push({ kind: "rect", x: cx - w / 2, y: plot.y + 8, w, h: 22, radius: 11, fill: theme.haltText, layer: "crosshair" });
    out.push({ kind: "text", x: cx, y: plot.y + 19, text, fill: theme.onTag, fontSize: fs + 1, fontFamily: theme.fontMono, fontWeight: 700, align: "center", baseline: "middle", layer: "crosshair" });
  }
  return out;
}

/** LULD bands as stepped dashed lines, with the current pair tagged at the right edge. */
export function layoutLuld(model: ChartModel, luld: readonly LuldPoint[], theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const price = model.price;
  if (!price || !luld.length) return out;
  const plot = model.geometry.plot;
  const right = plot.x + plot.w;
  for (const key of ["up", "down"] as const) {
    for (let i = 0; i < luld.length; i++) {
      const p = luld[i]!;
      const v = p[key];
      if (v === null) continue;
      const x0 = Math.max(plot.x, xOfT(model, p.t));
      const next = luld[i + 1];
      const x1 = next ? Math.min(right, xOfT(model, next.t)) : right;
      if (x1 <= plot.x || x0 >= right) continue;
      const y = priceY(price, v);
      out.push({ kind: "line", x1: x0, y1: y, x2: x1, y2: y, stroke: theme.luld, lineWidth: 1.25, dash: [4, 3], clip: plot, layer: "overlay" });
      const nv = next?.[key];
      if (next && nv !== null && nv !== undefined && x1 < right) out.push({ kind: "line", x1, y1: y, x2: x1, y2: priceY(price, nv), stroke: theme.luld, lineWidth: 1.25, dash: [4, 3], clip: plot, layer: "overlay" });
    }
  }
  const cur = luld[luld.length - 1]!;
  if (cur.up !== null && cur.down !== null) {
    const fs = model.options.fontSize;
    for (const [v, arrow] of [[cur.up, "▲"], [cur.down, "▼"]] as const) {
      const raw = priceY(price, v);
      // A band outside the visible range (±10% usually is, zoomed in) is pinned to the
      // nearest edge with an arrow, so the limit is always readable.
      const off = raw < plot.y + 8 ? "↑ " : raw > plot.y + plot.h - 8 ? "↓ " : "";
      const y = Math.min(plot.y + plot.h - 10, Math.max(plot.y + 10 + model.options.topRightInset, raw));
      const text = `${off}LULD ${arrow} ${formatPrice(v)}`;
      const w = Math.ceil(text.length * fs * 0.62) + 10;
      out.push({ kind: "rect", x: right - w - 4, y: y - 8, w, h: 16, radius: 3, fill: theme.crosshairLabelBg, layer: "axes" });
      out.push({ kind: "text", x: right - w / 2 - 4, y, text, fill: theme.luldText, fontSize: fs, fontFamily: theme.fontMono, fontWeight: 600, align: "center", baseline: "middle", layer: "axes" });
    }
  }
  return out;
}

/** Key levels as quiet dashed lines with a label at the left edge; labels that would collide shift right. */
export function layoutKeyLevels(model: ChartModel, levels: readonly KeyLevel[], theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const price = model.price;
  if (!price) return out;
  const plot = model.geometry.plot;
  const fs = model.options.fontSize;
  const placed: { y: number; x1: number }[] = [];
  // Levels off the visible range are pinned to the nearest edge as tags (no line), with an
  // arrow, so the day's high is never out of mind just because the view is zoomed in.
  const above: KeyLevel[] = [];
  const below: KeyLevel[] = [];
  for (const l of [...levels].sort((a, b) => b.price - a.price)) {
    const y = Math.round(priceY(price, l.price)) + 0.5;
    if (y < plot.y) {
      above.push(l);
      continue;
    }
    if (y > plot.y + plot.h) {
      below.push(l);
      continue;
    }
    const strong = l.id === "hod" || l.id === "lod";
    out.push({ kind: "line", x1: plot.x, y1: y, x2: plot.x + plot.w, y2: y, stroke: strong ? theme.levelStrong : theme.level, lineWidth: 1, dash: strong ? [6, 3] : [2, 3], layer: "grid" });
    // Labels sit at the right end of their line (the left edge is where the drawing tools
    // usually float); labels that would collide step left.
    const text = `${l.label} ${formatPrice(l.price)}`;
    const w = Math.ceil(text.length * fs * 0.62) + 10;
    let x = plot.x + plot.w - w - 4;
    for (const p of placed) if (Math.abs(p.y - y) < fs + 6 && x + w > p.x1 - 4) x = p.x1 - 4 - w;
    placed.push({ y, x1: x });
    out.push({ kind: "rect", x, y: y - 8, w, h: 16, radius: 3, fill: withAlpha(theme.crosshairLabelBg, 0.92), layer: "axes" });
    out.push({ kind: "text", x: x + 5, y, text, fill: strong ? theme.text : theme.textDim, fontSize: fs, fontFamily: theme.fontMono, fontWeight: 500, align: "left", baseline: "middle", layer: "axes" });
  }
  // Off-screen levels: a row of tags along the edge, right-aligned like the line labels, and one
  // row in from the edge so they never sit on a LULD tag pinned to the same edge.
  const edge = (list: KeyLevel[], y: number, arrow: string) => {
    let right = plot.x + plot.w - 4;
    for (const l of list) {
      const text = `${arrow} ${l.label} ${formatPrice(l.price)}`;
      const w = Math.ceil(text.length * fs * 0.62) + 10;
      if (right - w < plot.x + plot.w * 0.35) break;
      out.push({ kind: "rect", x: right - w, y: y - 8, w, h: 16, radius: 3, fill: withAlpha(theme.crosshairLabelBg, 0.92), layer: "axes" });
      out.push({ kind: "text", x: right - w + 5, y, text, fill: theme.textDim, fontSize: fs, fontFamily: theme.fontMono, fontWeight: 500, align: "left", baseline: "middle", layer: "axes" });
      right -= w + 4;
    }
  };
  // Nearest level first, reading away from the price.
  edge(above.reverse(), plot.y + 30 + model.options.topRightInset, "↑");
  edge(below, plot.y + plot.h - 30, "↓");
  return out;
}

/** Fill markers: a buy is an up-arrow whose tip touches the fill price from below, a sell a down-arrow from above. */
export function layoutFills(model: ChartModel, fills: readonly FillMark[], theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const price = model.price;
  if (!price) return out;
  const plot = model.geometry.plot;
  const s = Math.max(5, Math.min(8, model.index.barSpacing * 0.6));
  for (const f of fills) {
    const x = xOfT(model, f.t) + model.index.barSpacing / 2;
    const y = priceY(price, f.price);
    if (x < plot.x || x > plot.x + plot.w || y < plot.y || y > plot.y + plot.h) continue;
    const buy = f.side === "buy";
    const tip = y + (buy ? 2 : -2);
    const base = tip + (buy ? 1 : -1) * s * 1.4;
    out.push({ kind: "path", points: [x, tip, x + s, base, x - s, base], closed: true, fill: buy ? theme.orderBuy : theme.orderSell, layer: "crosshair" });
    out.push({ kind: "path", points: [x, tip, x + s, base, x - s, base], closed: true, stroke: theme.handleFill, lineWidth: 1, layer: "crosshair" });
  }
  return out;
}

export function layoutContext(model: ChartModel, ctx: ContextLayer, theme: ChartTheme): DisplayList {
  return [...layoutHalts(model, ctx.halts, ctx.now, theme), ...layoutKeyLevels(model, ctx.levels, theme), ...layoutLuld(model, ctx.luld, theme), ...layoutFills(model, ctx.fills, theme)];
}

/** Today's trading-day key of a time, for grouping fills in a trade list. */
export const dayKey = (t: number) => formatEtDateKey(tradingDayOf(t));
