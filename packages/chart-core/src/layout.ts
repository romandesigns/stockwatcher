import { hatch, sortByLayer, type ChartTheme, type DisplayList, type Rect } from "./displayList.ts";
import { layoutDraft, layoutDrawings, type Drawing, type DrawingDraft } from "./drawings.ts";
import { indicatorColor, indicatorLabel, type IndicatorResult } from "./indicators.ts";
import { withAlpha } from "./theme.ts";
import { layoutContext, type ContextLayer } from "./levels.ts";
import { layoutTrading, type TradingLayer } from "./trading.ts";
import { formatPrice } from "./format.ts";
import { sessionOf } from "./time.ts";
import {
  buildSlots,
  formatAxisValue,
  indexScale,
  priceAxisTicks,
  priceRange,
  priceY,
  slotAt,
  slotX,
  timeTicks,
  yPrice,
  type IndexScale,
  type PriceScale,
  type PriceScaleType,
  type PriceTicks,
  type Slot,
  type Viewport,
} from "./scale.ts";
import { lowerBound } from "./store.ts";
import { TIMEFRAME_MS, type Candle, type Gap, type ProfileWire, type Timeframe } from "./types.ts";

export interface ChartOptions {
  priceAxisWidth: number;
  timeAxisHeight: number;
  /** 0 hides the volume pane. */
  volumeHeight: number;
  paneGap: number;
  /** Draw the session profile inside the price pane, anchored right. */
  showProfile: boolean;
  /** Profile overlay width as a fraction of the plot width. */
  profileWidth: number;
  fontSize: number;
  hatchSpacing: number;
  /** Minimum px between time labels. */
  timeLabelSpacing: number;
  /** Height of each oscillator pane (RSI, MACD) below the volume pane. */
  indicatorPaneHeight: number;
  /** Space kept free at the top-left of the plot for a host overlay (the ticker and candle readout); the indicator legend starts below it. */
  legendTop: number;
  /** Space kept free along the top-right of the plot for a host overlay (the timeframe picker); top-edge tags start below it. */
  topRightInset: number;
}

export const DEFAULT_CHART_OPTIONS: ChartOptions = {
  priceAxisWidth: 56,
  timeAxisHeight: 22,
  volumeHeight: 64,
  paneGap: 6,
  showProfile: false,
  profileWidth: 0.22,
  fontSize: 10,
  hatchSpacing: 7,
  timeLabelSpacing: 72,
  indicatorPaneHeight: 64,
  legendTop: 0,
  topRightInset: 0,
};

export interface Size {
  width: number;
  height: number;
}

export interface Point {
  x: number;
  y: number;
}

/** Everything the chart layout reads. Plain data, so it can be built anywhere. */
export interface ChartState {
  timeframe: Timeframe;
  candles: readonly Candle[];
  gaps: readonly Gap[];
  profile: ProfileWire | null;
  /** Precomputed buildSlots(candles, gaps); pass it to avoid rebuilding per frame. */
  slots?: Slot[];
  /** t of the inspected slot; null inspects the latest. */
  inspectT: number | null;
  /** Pointer position for the horizontal crosshair; null when the pointer is away. */
  pointer: Point | null;
  /** Price axis scale type; default "linear". Whether the range is auto or manual lives in the Viewport. */
  priceScaleType?: PriceScaleType;
  options?: Partial<ChartOptions>;
  /** computeIndicators(slots, specs): overlays on the plot, oscillators in panes. */
  indicators?: readonly IndicatorResult[];
  drawings?: readonly Drawing[];
  /** Id of the selected drawing (handles shown), or null. */
  selectedDrawing?: string | null;
  /** A drawing being placed, in px. */
  draft?: DrawingDraft | null;
  /** Simulated orders and position for this symbol; absent hides trading. */
  trading?: TradingLayer | null;
  /** Halts, LULD bands, key levels and fill markers (each list already filtered by its layer toggle). */
  context?: ContextLayer | null;
  /** The feed clock (ms): drives the countdown to the forming candle's close under the last-price tag. */
  now?: number | null;
  /** Oscillator pane heights in px by indicator id (dragged or expanded by the user); absent = options.indicatorPaneHeight. */
  paneHeights?: Readonly<Record<string, number>>;
}

export interface ChartGeometry {
  plot: Rect;
  volume: Rect | null;
  /** Oscillator panes, top to bottom, between the volume pane and the time axis. */
  panes: Rect[];
  priceAxis: Rect;
  timeAxis: Rect;
}

export interface ChartModel {
  options: ChartOptions;
  geometry: ChartGeometry;
  slots: Slot[];
  index: IndexScale;
  price: PriceScale | null;
  priceTicks: PriceTicks;
  volumeMax: number;
  timeframeMs: number;
  inspected: number | null;
}

/** The smallest an oscillator pane can be dragged to, and the share of the chart's height the panes may take together. */
export const MIN_PANE_HEIGHT = 32;
export const MAX_PANES_SHARE = 0.6;

/** Room the price plot and the oscillator panes share: everything but the time axis and the volume pane. */
function paneRoom(size: Size, o: ChartOptions): number {
  const volH = o.volumeHeight > 0 ? o.volumeHeight + o.paneGap : 0;
  return Math.max(0, size.height - o.timeAxisHeight - volH);
}

/** An expanded pane (double tap / double click): 40 % of the shared room, never less than its default. */
export function expandedPaneHeight(size: Size, o: ChartOptions): number {
  return Math.max(o.indicatorPaneHeight, Math.round(paneRoom(size, o) * 0.4));
}

/**
 * `paneSizes` is a pane count (every pane at options.indicatorPaneHeight) or each pane's height. Heights
 * are clamped to MIN_PANE_HEIGHT, and scaled down together if they would take more than
 * MAX_PANES_SHARE of the room, so the price plot always keeps the larger part of the chart.
 */
export function chartGeometry(size: Size, o: ChartOptions, paneSizes: number | readonly number[] = 0): ChartGeometry {
  const plotW = Math.max(0, size.width - o.priceAxisWidth);
  const volH = o.volumeHeight > 0 ? o.volumeHeight : 0;
  let heights = typeof paneSizes === "number" ? Array.from({ length: paneSizes }, () => o.indicatorPaneHeight) : paneSizes.map((h) => Math.max(MIN_PANE_HEIGHT, h));
  const cap = paneRoom(size, o) * MAX_PANES_SHARE - heights.length * o.paneGap;
  const sum = heights.reduce((a, b) => a + b, 0);
  if (heights.length && sum > cap && cap > 0) heights = heights.map((h) => Math.max(1, Math.floor((h * cap) / sum)));
  const paneH = heights.reduce((a, h) => a + h + o.paneGap, 0);
  const plotH = Math.max(0, size.height - o.timeAxisHeight - (volH ? volH + o.paneGap : 0) - paneH);
  const plot = { x: 0, y: 0, w: plotW, h: plotH };
  const volume = volH ? { x: 0, y: plotH + o.paneGap, w: plotW, h: volH } : null;
  let y = volume ? volume.y + volume.h : plotH;
  const panes: Rect[] = [];
  for (const h of heights) {
    panes.push({ x: 0, y: y + o.paneGap, w: plotW, h });
    y += o.paneGap + h;
  }
  return {
    plot,
    volume,
    panes,
    priceAxis: { x: plotW, y: 0, w: o.priceAxisWidth, h: plotH },
    timeAxis: { x: 0, y, w: plotW, h: o.timeAxisHeight },
  };
}

/** The oscillator panes' indicator ids, top to bottom: the keys of ChartState.paneHeights. */
export function paneIdsOf(state: Pick<ChartState, "indicators">): string[] {
  return state.indicators?.filter((r) => r.placement === "pane").map((r) => r.spec.id) ?? [];
}

/**
 * Applies a pane gesture to the stored heights (pure, shared by web and phone). A resize sets the
 * pane to its drawn height minus dy (dragging the border up grows it), never below MIN_PANE_HEIGHT;
 * a toggle expands a pane to expandedPaneHeight, or drops its entry so it returns to the default.
 */
export function applyPaneGesture(
  heights: Readonly<Record<string, number>>,
  action: { kind: "resizePane"; pane: number; dy: number } | { kind: "togglePane"; pane: number },
  model: ChartModel,
  paneIds: readonly string[],
  size: Size,
): Record<string, number> {
  const id = paneIds[action.pane];
  const drawn = model.geometry.panes[action.pane]?.h;
  if (id === undefined || drawn === undefined) return { ...heights };
  const next = { ...heights };
  if (action.kind === "resizePane") {
    // Build on the stored height, not the drawn one: several moves can arrive before the next render.
    // The ceiling is what the share cap leaves this pane beside the others, so a drag past it has no
    // dead zone on the way back.
    const o = model.options;
    const others = model.geometry.panes.reduce((a, r, k) => (k === action.pane ? a : a + r.h), 0);
    const max = Math.max(MIN_PANE_HEIGHT, Math.floor(paneRoom(size, o) * MAX_PANES_SHARE - model.geometry.panes.length * o.paneGap - others));
    const current = Math.min(heights[id] ?? drawn, max);
    next[id] = Math.round(Math.min(max, Math.max(MIN_PANE_HEIGHT, current - action.dy)));
    return next;
  }
  const expanded = expandedPaneHeight(size, model.options);
  if (drawn >= expanded - 1) delete next[id];
  else next[id] = expanded;
  return next;
}

/** Index of the slot with exactly t, or null. */
export function slotIndexOf(slots: readonly Slot[], t: number): number | null {
  const i = lowerBound(slots, t);
  return slots[i]?.t === t ? i : null;
}

/** Smallest price column, so a short label still leaves room for the crosshair tag. */
const MIN_PRICE_AXIS = 30;
/** Monospaced digits: advance per character as a share of the font size (Geist Mono / Menlo / monospace). */
const MONO_ADVANCE = 0.62;

/**
 * The width the price column needs: its widest label (tick labels, the last-price tag in bold at
 * fontSize + 1, a crosshair value at the top of the range) plus padding. ChartOptions.priceAxisWidth
 * is the maximum, so a $3 stock's "3.38" column is narrower than a sub-$1 stock's "0.9179".
 */
function priceAxisNeed(model: ChartModel): number {
  const o = model.options;
  const price = model.price;
  if (!price) return o.priceAxisWidth;
  const w = (text: string, fs: number) => text.length * fs * MONO_ADVANCE;
  let need = 0;
  for (const label of model.priceTicks.labels) need = Math.max(need, AXIS_LABEL_PAD + w(label, o.fontSize) + 4);
  const last = lastCandle(model.slots);
  if (last) need = Math.max(need, 2 + 5 + w(formatAxisValue(price, last.c), o.fontSize + 1) * 1.04 + 6);
  need = Math.max(need, 4 + w(formatAxisValue(price, price.hi), o.fontSize) + 6);
  return Math.min(o.priceAxisWidth, Math.max(MIN_PRICE_AXIS, Math.ceil(need)));
}

/** Gap between the plot's edge and a price label. */
const AXIS_LABEL_PAD = 5;

export function computeChartModel(state: ChartState, viewport: Viewport, size: Size, fitted = false): ChartModel {
  const options = { ...DEFAULT_CHART_OPTIONS, ...state.options };
  const paneIds = state.indicators?.filter((r) => r.placement === "pane").map((r) => r.spec.id) ?? [];
  const geometry = chartGeometry(size, options, paneIds.map((id) => state.paneHeights?.[id] ?? options.indicatorPaneHeight));
  const slots = state.slots ?? buildSlots(state.candles, state.gaps);
  const index = indexScale(viewport, slots, geometry.plot.x, geometry.plot.x + geometry.plot.w);
  const type = state.priceScaleType ?? "linear";
  const auto = priceRange(slots, index.from, index.to, 0.08, type);
  // A manual range (from a price-axis drag) holds until auto-scale; it still
  // needs candles to exist, so an empty chart has no price scale either way.
  let range = auto && viewport.price?.mode === "manual" ? { lo: viewport.price.lo, hi: viewport.price.hi } : auto;
  // A manual range dragged below zero on a linear axis cannot be drawn in log.
  if (range && type === "log" && range.lo <= 0) range = { lo: Math.max(range.hi * 1e-3, 1e-6), hi: range.hi };
  const base = type === "percent" ? firstVisibleOpen(slots, index.from, index.to) : undefined;
  const price: PriceScale | null = range && range.hi > range.lo ? { lo: range.lo, hi: range.hi, top: geometry.plot.y, bottom: geometry.plot.y + geometry.plot.h, type, ...(base !== undefined ? { base } : {}) } : null;
  const priceTicks = price ? priceAxisTicks(price, Math.max(2, Math.floor(geometry.plot.h / 48))) : { step: 0, decimals: 2, values: [], labels: [] };
  let volumeMax = 0;
  for (let i = index.from; i <= index.to; i++) {
    const s = slots[i];
    if (s?.kind === "candle" && s.candle.v > volumeMax) volumeMax = s.candle.v;
  }
  const inspected = state.inspectT === null ? (slots.length ? slots.length - 1 : null) : slotIndexOf(slots, state.inspectT);
  const model = { options, geometry, slots, index, price, priceTicks, volumeMax, timeframeMs: TIMEFRAME_MS[state.timeframe], inspected };
  // Second pass with the price column fitted to its labels (narrower than the maximum when it can be).
  if (!fitted) {
    const need = priceAxisNeed(model);
    if (need < options.priceAxisWidth) return computeChartModel({ ...state, options: { ...state.options, priceAxisWidth: need } }, viewport, size, true);
  }
  return model;
}

/** Percent mode's base: the open of the first candle in [from, to]. */
export function firstVisibleOpen(slots: readonly Slot[], from: number, to: number): number | undefined {
  for (let i = Math.max(0, from); i <= to; i++) {
    const s = slots[i];
    if (s?.kind === "candle") return s.candle.o;
  }
  return undefined;
}

/** paneBorder: the strip on an oscillator pane's top edge that resizes it when dragged. */
export type HitRegion = "plot" | "volume" | "pane" | "paneBorder" | "priceAxis" | "timeAxis" | "none";

/** How far around a pane's top edge a press grabs its border: a touch-sized strip, mostly inside the pane. */
const BORDER_ABOVE = 6;
const BORDER_BELOW = 8;

export interface Hit {
  region: HitRegion;
  /** The oscillator pane under the pointer (or whose border it is on), else null. */
  pane: number | null;
  index: number | null;
  slot: Slot | null;
  /** Price under the pointer (plot and price axis only). */
  price: number | null;
}

const inside = (r: Rect | null, x: number, y: number) => !!r && x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h;

/** x, y (CSS px on the chart surface) → region, slot under x, and price under y. */
export function hitTest(model: ChartModel, x: number, y: number): Hit {
  const g = model.geometry;
  // A pane's border wins over whatever it overlaps (the gap, the volume pane's last pixels, the pane's top).
  const border = x >= 0 && x <= g.plot.w ? g.panes.findIndex((r) => y >= r.y - model.options.paneGap - BORDER_ABOVE && y <= r.y + BORDER_BELOW) : -1;
  const paneIdx = border >= 0 ? border : g.panes.findIndex((r) => inside(r, x, y));
  const region: HitRegion = border >= 0
    ? "paneBorder"
    : inside(g.plot, x, y)
    ? "plot"
    : inside(g.volume, x, y)
      ? "volume"
      : g.panes.some((r) => inside(r, x, y))
        ? "pane"
        : inside(g.priceAxis, x, y)
        ? "priceAxis"
        : inside(g.timeAxis, x, y)
          ? "timeAxis"
          : "none";
  const index = region === "none" || region === "priceAxis" ? null : slotAt(model.index, x, model.slots.length);
  const price = model.price && (region === "plot" || region === "priceAxis") ? yPrice(model.price, y) : null;
  return { region, pane: paneIdx >= 0 ? paneIdx : null, index, slot: index === null ? null : model.slots[index]!, price };
}

/**
 * Keyboard inspection: moves `delta` slots from the inspected one (null =
 * latest) and returns the new slot's t, clamped to the ends.
 */
export function stepInspection(slots: readonly Slot[], currentT: number | null, delta: number): number | null {
  if (slots.length === 0) return null;
  const cur = currentT === null ? slots.length - 1 : (slotIndexOf(slots, currentT) ?? slots.length - 1);
  const next = Math.max(0, Math.min(slots.length - 1, cur + delta));
  return slots[next]!.t;
}

const crisp = (v: number) => Math.round(v) + 0.5;

/** Odd body width so a 1 px wick sits exactly in the middle. */
export function bodyWidth(barSpacing: number): number {
  return Math.max(1, 2 * Math.floor(barSpacing * 0.34) + 1);
}

export function layoutChart(model: ChartModel, state: ChartState, theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const { geometry: g, slots, index: ix, price, options: o } = model;
  if (g.plot.w <= 0 || g.plot.h <= 0) return out;
  const mono = theme.fontMono;
  const fs = o.fontSize;

  // Session shading: one faint band per run of consecutive slots outside regular hours, over the
  // price and volume panes, behind everything (gaps take their session from their start).
  {
    const bottom = (g.volume ?? g.plot).y + (g.volume ?? g.plot).h;
    const shade = { pre: theme.shadePre, after: theme.shadeAfter, overnight: theme.shadeOvernight } as const;
    let runStart = -1;
    let runKind: keyof typeof shade | null = null;
    const flush = (end: number) => {
      if (runKind === null || runStart < 0) return;
      const x0 = Math.max(g.plot.x, slotX(ix, runStart) - ix.barSpacing / 2);
      const x1 = Math.min(g.plot.x + g.plot.w, slotX(ix, end) + ix.barSpacing / 2);
      if (x1 > x0) out.push({ kind: "rect", x: x0, y: g.plot.y, w: x1 - x0, h: bottom - g.plot.y, fill: shade[runKind], layer: "background" });
    };
    for (let i = ix.from; i <= ix.to; i++) {
      const s = sessionOf(slots[i]!.t);
      const kind = s === "regular" ? null : s;
      if (kind !== runKind) {
        flush(i - 1);
        runKind = kind;
        runStart = i;
      }
    }
    // The last run reaches the plot's right edge when it is the live end of the data.
    flush(ix.to === slots.length - 1 ? ix.to + 2 : ix.to);
  }

  // grid + price labels
  if (price) {
    for (const v of model.priceTicks.values) {
      const y = crisp(priceY(price, v));
      if (y < g.plot.y || y > g.plot.y + g.plot.h) continue;
      out.push({ kind: "line", x1: g.plot.x, y1: y, x2: g.plot.x + g.plot.w, y2: y, stroke: theme.grid, lineWidth: 1, layer: "grid" });
    }
  }

  const bw = bodyWidth(ix.barSpacing);
  const half = Math.floor(bw / 2);

  // gaps: hatched slot in the price pane, never a fake candle
  for (let i = ix.from; i <= ix.to; i++) {
    const s = slots[i]!;
    if (s.kind !== "gap") continue;
    const cx = slotX(ix, i);
    const left = Math.max(g.plot.x, cx - ix.barSpacing / 2);
    const right = Math.min(g.plot.x + g.plot.w, cx + ix.barSpacing / 2);
    out.push(...hatch({ x: left, y: g.plot.y, w: right - left, h: g.plot.h }, o.hatchSpacing, theme.gapHatch, 1, "gaps"));
  }

  // session profile overlay, right-anchored inside the price pane
  if (o.showProfile && price && state.profile && state.profile.rows.length) {
    // Behind the candles, so it takes the overlay's quieter colours: the panel's solid POC would hide price.
    out.push(...profileBars(state.profile, price, { x: g.plot.x + g.plot.w * (1 - o.profileWidth), y: g.plot.y, w: g.plot.w * o.profileWidth, h: g.plot.h }, "right", theme, true));
  }

  // volume + candles
  for (let i = ix.from; i <= ix.to; i++) {
    const s = slots[i]!;
    if (s.kind !== "candle") continue;
    const c = s.candle;
    const cx = Math.round(slotX(ix, i));
    const up = c.c >= c.o;
    const color = up ? theme.up : theme.down;
    if (g.volume && model.volumeMax > 0 && c.v > 0) {
      const h = Math.max(1, (c.v / model.volumeMax) * g.volume.h);
      out.push({ kind: "rect", x: cx - half, y: g.volume.y + g.volume.h - h, w: bw, h, fill: up ? theme.upVolume : theme.downVolume, layer: "volume" });
    }
    if (!price) continue;
    const yh = priceY(price, c.h);
    const yl = priceY(price, c.l);
    out.push({ kind: "line", x1: cx + 0.5, y1: yh, x2: cx + 0.5, y2: Math.max(yl, yh + 1), stroke: color, lineWidth: 1, layer: "candles" });
    const top = priceY(price, Math.max(c.o, c.c));
    const h = Math.max(1, priceY(price, Math.min(c.o, c.c)) - top);
    if (c.final) {
      out.push({ kind: "rect", x: cx - half, y: top, w: bw, h, fill: color, layer: "candles" });
    } else {
      // Forming: dashed outline, hollow, so it never reads as final.
      out.push({ kind: "rect", x: cx - half + 0.5, y: top + 0.5, w: Math.max(0, bw - 1), h: Math.max(0, h - 1), stroke: color, lineWidth: 1, dash: [3, 2], layer: "candles" });
    }
  }

  // indicators: overlays on the plot, oscillators in their panes
  const results = state.indicators ?? [];
  if (price) for (const r of results) if (r.placement === "overlay") out.push(...overlayIndicator(model, r, theme));
  const paneResults = results.filter((r) => r.placement === "pane");
  paneResults.forEach((r, k) => {
    const rect = g.panes[k];
    if (rect) out.push(...paneIndicator(model, r, rect, theme));
  });

  // drawings, a drawing being placed, then simulated orders on top
  if (state.drawings?.length) out.push(...layoutDrawings(model, state.drawings, state.selectedDrawing ?? null, theme));
  if (state.draft) out.push(...layoutDraft(model, state.draft, theme));
  if (state.context) out.push(...layoutContext(model, state.context, theme));
  if (state.trading) out.push(...layoutTrading(model, state.trading, theme));
  if (price && results.some((r) => r.placement === "overlay")) out.push(...overlayLegend(model, results, theme));

  // inspection line at the inspected (or latest) slot, through every time-aligned pane
  if (model.inspected !== null && model.inspected >= ix.from && model.inspected <= ix.to) {
    const x = Math.round(slotX(ix, model.inspected)) + 0.5;
    const last = g.panes.at(-1) ?? g.volume ?? g.plot;
    const bottom = last.y + last.h;
    out.push({ kind: "line", x1: x, y1: g.plot.y, x2: x, y2: bottom, stroke: theme.crosshair, lineWidth: 1, layer: "crosshair" });
  }

  // price axis labels, last-price tag, pointer crosshair
  if (price) {
    const last = lastCandle(slots);
    const lastY = last ? clamp(priceY(price, last.c), g.plot.y + 9, g.plot.y + g.plot.h - 9) : null;
    model.priceTicks.values.forEach((v, k) => {
      const y = priceY(price, v);
      if (y < g.plot.y + 5 || y > g.plot.y + g.plot.h - 5) return;
      if (lastY !== null && Math.abs(y - lastY) < 11) return;
      out.push({ kind: "text", x: g.priceAxis.x + AXIS_LABEL_PAD, y, text: model.priceTicks.labels[k] ?? formatPrice(v, model.priceTicks.decimals), fill: theme.axisText, fontSize: fs, fontFamily: mono, fontWeight: 400, align: "left", baseline: "middle", layer: "axes" });
    });
    if (last && lastY !== null) {
      const ax = g.priceAxis.x + 2;
      const right = g.priceAxis.x + g.priceAxis.w - 2;
      const fill = last.c >= last.o ? theme.up : theme.down;
      out.push({ kind: "path", points: [ax, lastY, ax + 5, lastY - 9, right, lastY - 9, right, lastY + 9, ax + 5, lastY + 9], closed: true, fill, layer: "axes" });
      out.push({ kind: "text", x: (ax + 5 + right) / 2, y: lastY, text: formatAxisValue(price, last.c), fill: theme.onTag, fontSize: fs + 1, fontFamily: mono, fontWeight: 600, align: "center", baseline: "middle", layer: "axes" });
      // Countdown to the forming candle's close, tucked under the tag (above it near the bottom edge).
      const left = candleCountdown(last, model.timeframeMs, state.now ?? null);
      if (left !== null) {
        const below = lastY + 9 + 16 <= g.plot.y + g.plot.h;
        const cy = below ? lastY + 9 + 8 : lastY - 9 - 8;
        out.push({ kind: "rect", x: ax + 5, y: cy - 7, w: right - ax - 5, h: 14, radius: 3, fill: theme.crosshairLabelBg, layer: "axes" });
        out.push({ kind: "text", x: (ax + 5 + right) / 2, y: cy, text: formatCountdown(left), fill: theme.textDim, fontSize: fs - 1, fontFamily: mono, fontWeight: 500, align: "center", baseline: "middle", layer: "axes" });
      }
    }
    const p = state.pointer;
    if (p && inside(g.plot, p.x, p.y)) {
      const y = Math.round(p.y) + 0.5;
      out.push({ kind: "line", x1: g.plot.x, y1: y, x2: g.plot.x + g.plot.w, y2: y, stroke: theme.crosshair, lineWidth: 1, dash: [3, 3], layer: "crosshair" });
      out.push({ kind: "rect", x: g.priceAxis.x + 2, y: y - 9, w: g.priceAxis.w - 4, h: 18, fill: theme.crosshairLabelBg, radius: 3, layer: "crosshair" });
      out.push({ kind: "text", x: g.priceAxis.x + g.priceAxis.w / 2, y, text: formatAxisValue(price, yPrice(price, p.y)), fill: theme.crosshairLabelText, fontSize: fs, fontFamily: mono, fontWeight: 500, align: "center", baseline: "middle", layer: "crosshair" });
    }
  }

  // time axis
  for (const tick of timeTicks(slots, ix, model.timeframeMs, o.timeLabelSpacing)) {
    const x = slotX(ix, tick.index);
    if (x < g.timeAxis.x + 24 || x > g.timeAxis.x + g.timeAxis.w - 24) continue;
    out.push({ kind: "text", x, y: g.timeAxis.y + 5, text: tick.label, fill: theme.axisText, fontSize: fs, fontFamily: mono, fontWeight: 400, align: "center", baseline: "top", layer: "axes" });
  }

  return sortByLayer(out);
}

/** layout(state, viewport, theme, size) → DisplayList: the one-call form. */
export function layout(state: ChartState, viewport: Viewport, theme: ChartTheme, size: Size): DisplayList {
  return layoutChart(computeChartModel(state, viewport, size), state, theme);
}

function clamp(v: number, lo: number, hi: number) {
  return Math.max(lo, Math.min(hi, v));
}

export function lastCandle(slots: readonly Slot[]): Candle | null {
  for (let i = slots.length - 1; i >= 0; i--) {
    const s = slots[i]!;
    if (s.kind === "candle") return s.candle;
  }
  return null;
}

/**
 * Horizontal profile bars over a price scale. `align` "left" grows bars from
 * the left edge (standalone panel), "right" from the right (chart overlay).
 * POC and the value area (the profile's own, from the server or recomputed
 * by the store with the engine's rules) get their own colors.
 */
export function profileBars(profile: ProfileWire, price: PriceScale, area: Rect, align: "left" | "right", theme: ChartTheme, overlay = false): DisplayList {
  // Inside the chart the profile is a quiet backdrop: no value-area tone (its numbers are in the Levels
  // menu), and each bin a thin bar centred in its band instead of a block filling it, so a coarse bin on
  // a sub-$1 stock reads as lines, not slabs.
  const colors = overlay
    ? { bar: theme.profileOverlayBar, va: theme.profileOverlayBar, poc: theme.profileOverlayPoc }
    : { bar: theme.profileBar, va: theme.profileValueArea, poc: theme.profilePoc };
  const out: DisplayList = [];
  let max = 0;
  for (const r of profile.rows) if (r.v > max) max = r.v;
  if (max <= 0) return out;
  const poc = profile.poc;
  const va = profile.valueArea;
  const eps = profile.bin * 1e-6;
  for (const r of profile.rows) {
    if (r.v <= 0) continue;
    const yTop = priceY(price, r.p + profile.bin);
    const yBot = priceY(price, r.p);
    if (yBot < area.y || yTop > area.y + area.h) continue;
    const top = Math.max(area.y, yTop);
    const fullH = Math.min(area.y + area.h, yBot) - top;
    // 1 px separation between bins once they are tall enough to afford it; an overlay bar is at most 3 px.
    const h = overlay ? Math.min(3, Math.max(1, fullH - 1)) : fullH > 3 ? fullH - 1 : Math.max(0.5, fullH);
    const y = overlay ? top + (fullH - h) / 2 : top;
    const w = Math.max(1, (r.v / max) * area.w);
    const isPoc = poc !== null && Math.abs(r.p - poc) < eps;
    // low and high are bin starts, both inside the value area.
    const inVa = va !== null && r.p >= va.low - eps && r.p <= va.high + eps;
    const fill = isPoc ? colors.poc : inVa ? colors.va : colors.bar;
    out.push({ kind: "rect", x: align === "left" ? area.x : area.x + area.w - w, y, w, h, fill, radius: overlay ? h / 2 : h > 3 ? 1.5 : 0, layer: "profile" });
  }
  return out;
}

export interface ProfileLayoutInput {
  profile: ProfileWire;
  /** Align bins to an external price range (e.g. the chart's); default: the profile's own range. */
  range?: { lo: number; hi: number };
}

/** Standalone session profile panel: bars grow from the left. */
export function layoutProfile(input: ProfileLayoutInput, theme: ChartTheme, size: Size): DisplayList {
  const { profile } = input;
  if (!profile.rows.length || size.width <= 0 || size.height <= 0) return [];
  let range = input.range;
  if (!range) {
    let lo = Infinity;
    let hi = -Infinity;
    for (const r of profile.rows) {
      if (r.v <= 0) continue;
      lo = Math.min(lo, r.p);
      hi = Math.max(hi, r.p + profile.bin);
    }
    if (!Number.isFinite(lo)) return [];
    range = { lo, hi };
  }
  const price: PriceScale = { lo: range.lo, hi: range.hi, top: 0, bottom: size.height };
  return profileBars(profile, price, { x: 0, y: 0, w: size.width, h: size.height }, "left", theme);
}

/** Visible slot range with one slot of margin each side, so lines run off the plot edge instead of stopping short. */
function visibleRange(model: ChartModel): [number, number] {
  return [Math.max(0, model.index.from - 1), Math.min(model.slots.length - 1, model.index.to + 1)];
}

/** Polylines through `values` (one per run of non-NaN slots), clipped to `clip`. */
function polylines(model: ChartModel, values: Float64Array, y: (v: number) => number, stroke: string, lineWidth: number, clip: Rect, layer: "overlay" | "volume"): DisplayList {
  const out: DisplayList = [];
  const [from, to] = visibleRange(model);
  let pts: number[] = [];
  const flush = () => {
    if (pts.length >= 4) out.push({ kind: "path", points: pts, closed: false, stroke, lineWidth, clip, layer });
    pts = [];
  };
  for (let i = from; i <= to; i++) {
    const v = values[i];
    if (v === undefined || Number.isNaN(v)) {
      flush();
      continue;
    }
    pts.push(slotX(model.index, i), y(v));
  }
  flush();
  return out;
}

function overlayIndicator(model: ChartModel, r: IndicatorResult, theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const price = model.price!;
  const plot = model.geometry.plot;
  const color = indicatorColor(theme, r, 0);
  const y = (v: number) => priceY(price, v);
  if (r.band) {
    // Fill between the band lines, run by run, so a gap leaves a hole like the lines do.
    const up = r.lines[r.band[0]]!.values;
    const lo = r.lines[r.band[1]]!.values;
    const [from, to] = visibleRange(model);
    let run: number[] = [];
    const flush = () => {
      if (run.length >= 2) {
        const pts: number[] = [];
        for (const i of run) pts.push(slotX(model.index, i), y(up[i]!));
        for (const i of [...run].reverse()) pts.push(slotX(model.index, i), y(lo[i]!));
        out.push({ kind: "path", points: pts, closed: true, fill: withAlpha(color, 0.08), clip: plot, layer: "overlay" });
      }
      run = [];
    };
    for (let i = from; i <= to; i++) {
      if (Number.isNaN(up[i]!) || Number.isNaN(lo[i]!)) flush();
      else run.push(i);
    }
    flush();
  }
  r.lines.forEach((l, k) => {
    // The basis of a band is the quieter line; its edges carry the identity.
    const width = r.band && k === 0 ? 1 : 1.5;
    out.push(...polylines(model, l.values, y, r.band && k === 0 ? withAlpha(color, 0.7) : color, width, plot, "overlay"));
  });
  return out;
}

/** Value of a line at the inspected slot, formatted, or an em dash. */
function valueAt(model: ChartModel, values: Float64Array, digits: number): string {
  const i = model.inspected;
  const v = i === null ? Number.NaN : values[i]!;
  return Number.isNaN(v) ? "—" : v.toFixed(digits);
}

/**
 * One legend row per overlay, top-left of the plot: a swatch in the line's
 * colour, then the name and the value at the inspected candle in text tones
 * (text never wears the series colour; the swatch carries the identity).
 */
function overlayLegend(model: ChartModel, results: readonly IndicatorResult[], theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const plot = model.geometry.plot;
  const fs = model.options.fontSize;
  let y = plot.y + 8 + model.options.legendTop;
  for (const r of results) {
    if (r.placement !== "overlay") continue;
    const color = indicatorColor(theme, r, 0);
    const main = r.lines[0]!.values;
    const sample = model.inspected === null ? Number.NaN : main[model.inspected]!;
    const digits = Number.isNaN(sample) ? 2 : sample < 1 ? 4 : 2;
    const name = indicatorLabel(r.spec);
    out.push({ kind: "rect", x: plot.x + 8, y: y - 1, w: 10, h: 2, fill: color, layer: "axes" });
    out.push({ kind: "text", x: plot.x + 24, y, text: name, fill: theme.textDim, fontSize: fs, fontFamily: theme.fontSans, fontWeight: 500, align: "left", baseline: "middle", layer: "axes" });
    out.push({ kind: "text", x: plot.x + 24 + Math.ceil(name.length * fs * 0.62) + 6, y, text: valueAt(model, main, digits), fill: theme.text, fontSize: fs, fontFamily: theme.fontMono, fontWeight: 400, align: "left", baseline: "middle", layer: "axes" });
    y += fs + 5;
  }
  return out;
}

function paneIndicator(model: ChartModel, r: IndicatorResult, rect: Rect, theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const g = model.geometry;
  const fs = model.options.fontSize;
  const [from, to] = visibleRange(model);
  let lo = r.fixedRange?.lo ?? Infinity;
  let hi = r.fixedRange?.hi ?? -Infinity;
  if (!r.fixedRange) {
    for (const l of r.lines)
      for (let i = from; i <= to; i++) {
        const v = l.values[i]!;
        if (Number.isNaN(v)) continue;
        if (v < lo) lo = v;
        if (v > hi) hi = v;
      }
    for (const gv of r.guides ?? []) {
      lo = Math.min(lo, gv);
      hi = Math.max(hi, gv);
    }
    if (!Number.isFinite(lo)) return out;
    if (hi === lo) {
      hi += 1;
      lo -= 1;
    }
    const pad = (hi - lo) * 0.1;
    hi += pad;
    lo -= pad;
  }
  const y = (v: number) => rect.y + rect.h - ((v - lo) / (hi - lo)) * rect.h;
  const digits = r.spec.kind === "rsi" ? 1 : 4;

  if (r.zone) {
    const top = y(r.zone.hi);
    out.push({ kind: "rect", x: rect.x, y: top, w: rect.w, h: y(r.zone.lo) - top, fill: theme.oscZone, layer: "grid" });
  }
  if (r.midline !== undefined) {
    const my = Math.round(y(r.midline)) + 0.5;
    out.push({ kind: "line", x1: rect.x, y1: my, x2: rect.x + rect.w, y2: my, stroke: theme.oscMid, lineWidth: 1, layer: "grid" });
  }
  for (const gv of r.guides ?? []) {
    const gy = Math.round(y(gv)) + 0.5;
    out.push({ kind: "line", x1: rect.x, y1: gy, x2: rect.x + rect.w, y2: gy, stroke: theme.guide, lineWidth: 1, dash: [3, 3], layer: "grid" });
    out.push({ kind: "text", x: g.priceAxis.x + AXIS_LABEL_PAD, y: gy, text: String(gv), fill: theme.axisText, fontSize: fs, fontFamily: theme.fontMono, fontWeight: 400, align: "left", baseline: "middle", layer: "axes" });
  }
  const bw = bodyWidth(model.index.barSpacing);
  r.lines.forEach((l, k) => {
    if (l.style === "histogram") {
      const zero = y(0);
      for (let i = from; i <= to; i++) {
        const v = l.values[i]!;
        if (Number.isNaN(v)) continue;
        const vy = y(v);
        const cx = Math.round(slotX(model.index, i));
        if (cx < rect.x || cx > rect.x + rect.w) continue;
        // Four shades: strong while the bar grows away from zero (momentum building), faded while it shrinks.
        const prev = i > 0 ? l.values[i - 1]! : NaN;
        const growing = Number.isNaN(prev) || Math.abs(v) >= Math.abs(prev);
        const fill = v >= 0 ? (growing ? theme.histUp : theme.histUpFade) : growing ? theme.histDown : theme.histDownFade;
        out.push({ kind: "rect", x: cx - Math.floor(bw / 2), y: Math.min(zero, vy), w: bw, h: Math.max(1, Math.abs(vy - zero)), fill, layer: "volume" });
      }
      return;
    }
    out.push(...polylines(model, l.values, y, indicatorColor(theme, r, k), 1.5, rect, "overlay"));
  });

  // Pane legend: name, then a swatch and value per line (the histogram too), at the inspected candle,
  // on a translucent backing so the lines never run through the text.
  const name = indicatorLabel(r.spec);
  const ly = rect.y + 8;
  const legend: DisplayList = [];
  let x = rect.x + 8;
  legend.push({ kind: "text", x, y: ly, text: name, fill: theme.textDim, fontSize: fs, fontFamily: theme.fontSans, fontWeight: 500, align: "left", baseline: "middle", layer: "axes" });
  x += Math.ceil(name.length * fs * 0.62) + 8;
  r.lines.forEach((l, k) => {
    const text = valueAt(model, l.values, digits);
    if (l.style === "histogram") {
      const v = model.inspected === null ? NaN : l.values[model.inspected]!;
      legend.push({ kind: "rect", x, y: ly - 3, w: 3, h: 6, fill: !Number.isNaN(v) && v < 0 ? theme.histDown : theme.histUp, layer: "axes" });
      legend.push({ kind: "rect", x: x + 4, y: ly - 3, w: 3, h: 6, fill: !Number.isNaN(v) && v < 0 ? theme.histDownFade : theme.histUpFade, layer: "axes" });
    } else legend.push({ kind: "rect", x, y: ly - 1, w: 8, h: 2, fill: indicatorColor(theme, r, k), layer: "axes" });
    legend.push({ kind: "text", x: x + 12, y: ly, text, fill: theme.text, fontSize: fs, fontFamily: theme.fontMono, fontWeight: 400, align: "left", baseline: "middle", layer: "axes" });
    x += 12 + Math.ceil(text.length * fs * 0.62) + 10;
  });
  const bgW = Math.min(rect.w - 8, x - 10 - (rect.x + 4) + 6);
  out.push({ kind: "rect", x: rect.x + 4, y: ly - fs / 2 - 3, w: bgW, h: fs + 6, fill: theme.paneLegendBg, radius: 4, layer: "axes" }, ...legend);
  return out;
}

/**
 * Milliseconds until the forming candle closes, or null when there is no
 * forming candle at `now` (it is final, the clock is unknown, or the clock
 * has left its interval: the next print will open a new one).
 */
export function candleCountdown(last: Candle, timeframeMs: number, now: number | null): number | null {
  if (now === null || last.final) return null;
  const end = last.t + timeframeMs;
  // The clock reaches clients a few times a second, so it can trail the print that opened the
  // candle: a clock just behind the candle's start counts the whole interval, it is not "no candle".
  if (now >= end || now < last.t - timeframeMs) return null;
  return end - Math.max(now, last.t);
}

/** 42_000 → "0:42", 272_000 → "4:32"; rounded up, so it reads 0:01 until the very end. */
export function formatCountdown(ms: number): string {
  const s = Math.ceil(ms / 1000);
  const m = Math.floor(s / 60);
  return `${m}:${String(s % 60).padStart(2, "0")}`;
}
