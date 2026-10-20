import { hatch, sortByLayer, type ChartTheme, type DisplayList, type Rect } from "./displayList.ts";
import { layoutDraft, layoutDrawings, type Drawing, type DrawingDraft } from "./drawings.ts";
import { indicatorColor, indicatorLabel, type IndicatorResult } from "./indicators.ts";
import { withAlpha } from "./theme.ts";
import { layoutContext, type ContextLayer } from "./levels.ts";
import { layoutTrading, type TradingLayer } from "./trading.ts";
import { layoutAlerts, type AlertLayer } from "./alerts.ts";
import { formatPrice } from "./format.ts";
import { sessionOf } from "./time.ts";
import { heikinAshi, hollowStyle, isCloseOnly, prevCloseAt, sessionSlots, startsTradingDay, type ChartType, type Ohlc } from "./chartTypes.ts";
import {
  buildSlots,
  padRange,
  formatAxisTag,
  indexScale,
  priceAxisTicks,
  priceToUnit,
  priceY,
  slotAt,
  slotX,
  timeTicks,
  unitToPrice,
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
  /** This symbol's price alert lines and the hover bell (alerts.ts); absent hides them. */
  alerts?: AlertLayer | null;
  /** Halts, LULD bands, key levels and fill markers (each list already filtered by its layer toggle). */
  context?: ContextLayer | null;
  /** The feed clock (ms): drives the countdown to the forming candle's close under the last-price tag. */
  now?: number | null;
  /** Oscillator pane heights in px by indicator id (dragged or expanded by the user); absent = options.indicatorPaneHeight. */
  paneHeights?: Readonly<Record<string, number>>;
  /** How the price series is drawn (chartTypes.ts); default "candles". Changes only the marks, never the prices read elsewhere. */
  chartType?: ChartType;
  /** The chart settings' display switches (chartSettings.ts); each absent field keeps today's look. */
  chartStyle?: ChartStyle;
  /** The baseline chart's reference: the previous close when known; absent = the first visible close. */
  baselinePrice?: number | null;
  /**
   * false drops pre-market, after-hours and overnight slots (sessionSlots).
   * Applied here only when computeChartModel builds the slots itself: a
   * renderer that passes `slots` passes them already filtered, because its
   * indicators must be computed on the same list to line up by index.
   */
  extendedHours?: boolean;
}

/** Display switches from the chart settings panel. Absent = on for grid and session shading, off for the rest (today's look). */
export interface ChartStyle {
  grid?: boolean;
  sessionShading?: boolean;
  sessionBreaks?: boolean;
  highLow?: boolean;
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
  chartType: ChartType;
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
  if (last) need = Math.max(need, 2 + 5 + w(formatAxisTag(model, last.c), o.fontSize + 1) * 1.04 + 6);
  need = Math.max(need, 4 + w(formatAxisTag(model, price.hi), o.fontSize) + 6);
  return Math.min(o.priceAxisWidth, Math.max(MIN_PRICE_AXIS, Math.ceil(need)));
}

/**
 * Height the overlay legend takes at the top of the plot (its offset, then one row per wrapped
 * line, as overlayLegend flows them), or 0 without overlays. Capped at a quarter of the plot, so a
 * short chart still gives the candles most of its height.
 */
function legendHeadroom(results: readonly IndicatorResult[], plot: Rect, o: ChartOptions, inspected: number | null): number {
  const fs = o.fontSize;
  const widths: number[] = [];
  for (const r of results) {
    if (r.placement !== "overlay") continue;
    const main = r.lines[0]!.values;
    const v = inspected === null ? Number.NaN : main[inspected]!;
    const value = Number.isNaN(v) ? "—" : v.toFixed(v < 1 ? 4 : 2);
    widths.push(LEGEND_SWATCH + LEGEND_GAP + sansWidth(indicatorLabel(r.spec), fs) + LEGEND_GAP + textWidth(value, fs));
  }
  if (widths.length === 0) return 0;
  const lines = legendLines(widths, plot.w);
  const h = o.legendTop + 8 + (lines - 1) * (fs + 6) + fs / 2 + 3 + 4;
  return Math.min(Math.round(h), Math.round(plot.h / 4));
}

/** The range with its top raised so the price `headroom` px below the plot's top is the old high. */
function liftTop(r: { lo: number; hi: number }, headroom: number, plotH: number, type: PriceScaleType): { lo: number; hi: number } {
  if (headroom <= 0 || plotH <= headroom) return r;
  const lo = priceToUnit(type, r.lo);
  const hi = priceToUnit(type, r.hi);
  return { lo: r.lo, hi: unitToPrice(type, hi + ((hi - lo) * headroom) / (plotH - headroom)) };
}

/** Gap between the plot's edge and a price label. */
const AXIS_LABEL_PAD = 5;

export function computeChartModel(state: ChartState, viewport: Viewport, size: Size, fitted = false): ChartModel {
  const options = { ...DEFAULT_CHART_OPTIONS, ...state.options };
  const paneIds = state.indicators?.filter((r) => r.placement === "pane").map((r) => r.spec.id) ?? [];
  const geometry = chartGeometry(size, options, paneIds.map((id) => state.paneHeights?.[id] ?? options.indicatorPaneHeight));
  const slots = state.slots ?? sessionSlots(buildSlots(state.candles, state.gaps), state.extendedHours !== false);
  const index = indexScale(viewport, slots, geometry.plot.x, geometry.plot.x + geometry.plot.w);
  const type = state.priceScaleType ?? "linear";
  const chartType = state.chartType ?? "candles";
  const overlays = state.indicators?.filter((r) => r.placement === "overlay").flatMap((r) => r.lines.map((l) => l.values)) ?? [];
  const inspected = state.inspectT === null ? (slots.length ? slots.length - 1 : null) : slotIndexOf(slots, state.inspectT);
  const fit = autoFitRange(slots, index.from, index.to, { chartType, overlays }, 0.08, type);
  // Auto scale keeps the overlay legend's rows clear: without it the visible high sits right under
  // the legend (and the HOD tag beside it), unreadable on a phone (owner, 2026-10-19).
  const auto = fit ? liftTop(fit, legendHeadroom(state.indicators ?? [], geometry.plot, options, inspected), geometry.plot.h, type) : null;
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
  const model = { options, geometry, slots, index, price, priceTicks, volumeMax, timeframeMs: TIMEFRAME_MS[state.timeframe], inspected, chartType };
  // Second pass with the price column fitted to its labels (narrower than the maximum when it can be).
  if (!fitted) {
    const need = priceAxisNeed(model);
    if (need < options.priceAxisWidth) return computeChartModel({ ...state, options: { ...state.options, priceAxisWidth: need } }, viewport, size, true);
  }
  return model;
}

/**
 * Auto stretch: the price range that fits what is drawn in slots [from, to],
 * plus `pad` of the span on each side (in log space on a log axis).
 * - Candles, hollow candles and bars fit the real highs and lows; Heikin-Ashi
 *   fits its own (averaged) highs and lows, which is what is on screen; the
 *   line, area and baseline types fit closes only, so a long wick that is
 *   not drawn does not shrink the line.
 * - Overlay indicators on the price pane (VWAP, EMAs, SMA, Bollinger) are
 *   included, as TradingView's auto scale does for its main pane: a band you
 *   turned on is part of what you asked to see. Key levels, LULD bands,
 *   orders and drawings are NOT: they are often far from price, and the
 *   chart already pins an off-screen level to the edge with an arrow.
 * - Only candles make a range: overlays alone (a gap-only window) never do.
 * - A flat window (one print) still gets a small height (padRange).
 */
export function autoFitRange(
  slots: readonly Slot[],
  from: number,
  to: number,
  opts: { chartType?: ChartType; overlays?: readonly Float64Array[] } = {},
  pad = 0.08,
  type: PriceScaleType = "linear",
): { lo: number; hi: number } | null {
  const chartType = opts.chartType ?? "candles";
  const ha = chartType === "heikinAshi" ? heikinAshi(slots) : null;
  const closeOnly = isCloseOnly(chartType);
  let lo = Infinity;
  let hi = -Infinity;
  for (let i = Math.max(0, from); i <= to; i++) {
    const s = slots[i];
    if (s?.kind !== "candle") continue;
    const v: Ohlc = ha?.[i] ?? s.candle;
    const l = closeOnly ? v.c : v.l;
    const h = closeOnly ? v.c : v.h;
    if (l < lo) lo = l;
    if (h > hi) hi = h;
  }
  if (!Number.isFinite(lo)) return null;
  for (const values of opts.overlays ?? []) {
    for (let i = Math.max(0, from); i <= to; i++) {
      const v = values[i];
      if (v === undefined || !Number.isFinite(v)) continue;
      if (v < lo) lo = v;
      if (v > hi) hi = v;
    }
  }
  return padRange(lo, hi, pad, type);
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
  const style = state.chartStyle ?? {};

  // Session shading: one faint band per run of consecutive slots outside regular hours, over the
  // price and volume panes, behind everything (gaps take their session from their start).
  // The settings' "Highlight extended hours" switch turns it off.
  if (style.sessionShading !== false) {
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

  // Session breaks: a dashed line in the grid tone on the left edge of each trading day's first
  // slot, through the price and volume panes. Grid-quiet on purpose: it marks where the day turns
  // over, it is not a boundary between areas of the UI (the no-dividers rule is about chrome).
  if (style.sessionBreaks) {
    const bottom = (g.volume ?? g.plot).y + (g.volume ?? g.plot).h;
    for (let i = Math.max(1, ix.from); i <= ix.to; i++) {
      if (!startsTradingDay(slots, i)) continue;
      const x = crisp(slotX(ix, i) - ix.barSpacing / 2);
      if (x <= g.plot.x || x >= g.plot.x + g.plot.w) continue;
      out.push({ kind: "line", x1: x, y1: g.plot.y, x2: x, y2: bottom, stroke: theme.guide, lineWidth: 1, dash: [4, 4], layer: "grid" });
    }
  }

  // grid + price labels
  if (price && style.grid !== false) {
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

  // volume: always the real candles' volume, coloured by their real close vs open, whatever the type
  if (g.volume && model.volumeMax > 0) {
    for (let i = ix.from; i <= ix.to; i++) {
      const s = slots[i]!;
      if (s.kind !== "candle" || s.candle.v <= 0) continue;
      const c = s.candle;
      const cx = Math.round(slotX(ix, i));
      const h = Math.max(1, (c.v / model.volumeMax) * g.volume.h);
      out.push({ kind: "rect", x: cx - half, y: g.volume.y + g.volume.h - h, w: bw, h, fill: c.c >= c.o ? theme.upVolume : theme.downVolume, layer: "volume" });
    }
  }
  // the price series, in the chosen chart type
  if (price) out.push(...seriesMarks(model, state, theme));
  if (price && style.highLow) out.push(...highLowLabels(model, theme));

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
  // Alerts under the orders: an order's pill (what the grab priority favours) is never covered.
  if (state.alerts) out.push(...layoutAlerts(model, state.alerts, theme));
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
      out.push({ kind: "text", x: (ax + 5 + right) / 2, y: lastY, text: formatAxisTag(model, last.c), fill: theme.onTag, fontSize: fs + 1, fontFamily: mono, fontWeight: 600, align: "center", baseline: "middle", layer: "axes" });
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
      out.push({ kind: "text", x: g.priceAxis.x + g.priceAxis.w / 2, y, text: formatAxisTag(model, yPrice(price, p.y)), fill: theme.crosshairLabelText, fontSize: fs, fontFamily: mono, fontWeight: 500, align: "center", baseline: "middle", layer: "crosshair" });
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
  if (Number.isNaN(v)) return "—";
  return digits >= 0 ? v.toFixed(digits) : formatSignificant(v, -digits);
}

/** v to `sig` significant digits in plain notation (no exponent), at most 6 decimals; 0 reads "0". */
export function formatSignificant(v: number, sig: number): string {
  if (v === 0) return "0";
  const mag = Math.floor(Math.log10(Math.abs(v)));
  const decimals = Math.min(6, Math.max(0, sig - 1 - mag));
  return v.toFixed(decimals);
}

/** Approximate advance of `text` at `fs` px: the layout has no font metrics, so it uses the same 0.62 em per glyph as the pane legend. */
function textWidth(text: string, fs: number): number {
  return Math.ceil(text.length * fs * 0.62);
}

/**
 * Approximate advance of a sans-serif label: capitals are wider than the
 * 0.62 em average (an all-caps "VWAP" measured ~0.7 em a glyph and its value
 * crowded it), the rest are near it. Still an estimate, so callers leave a gap.
 */
function sansWidth(text: string, fs: number): number {
  let em = 0;
  for (const ch of text) em += ch >= "A" && ch <= "Z" ? 0.7 : 0.6;
  return Math.ceil(em * fs);
}

/**
 * The overlay legend, top-left of the plot: every overlay on ONE line, each
 * entry a swatch in the line's colour, then the name and the value at the
 * inspected candle in text tones (text never wears the series colour; the
 * swatch carries the identity). Entries flow left to right with a compact gap
 * and wrap to a new line only when the next entry would cross the plot's right
 * edge, so a phone-width chart still shows all of them (the owner asked for
 * "EMA 9, EMA 20 and VWAP inline" rather than one row each, 2026-10-18).
 * An entry never splits across lines; the first entry on a line always fits
 * there even if it is wider than the plot, so the loop cannot stall. Each
 * line sits on the pane legends' translucent backing (paneLegendBg).
 * Both web and phone draw this display list, so the change lands on both.
 */
const LEGEND_SWATCH = 10;
/** swatch → name and name → value. */
const LEGEND_GAP = 6;
/** value → the next entry's swatch. */
const LEGEND_ENTRY_GAP = 12;

/** How many lines entries of these widths take, flowed as overlayLegend flows them across a plot `plotW` wide. */
function legendLines(widths: readonly number[], plotW: number): number {
  const left = 8;
  const right = plotW - 8;
  let x = left;
  let lines = 1;
  for (const w of widths) {
    if (x > left && x + w > right) {
      x = left;
      lines++;
    }
    x += w + LEGEND_ENTRY_GAP;
  }
  return lines;
}

function overlayLegend(model: ChartModel, results: readonly IndicatorResult[], theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const plot = model.geometry.plot;
  const fs = model.options.fontSize;
  const left = plot.x + 8;
  const right = plot.x + plot.w - 8;
  const SWATCH = LEGEND_SWATCH;
  const SWATCH_GAP = LEGEND_GAP; // swatch → name
  const VALUE_GAP = LEGEND_GAP; // name → value
  const ENTRY_GAP = LEGEND_ENTRY_GAP; // value → next entry's swatch
  let x = left;
  let y = plot.y + 8 + model.options.legendTop;
  // Each line's right end, for its backing (one per line, so a wrapped legend's short last line gets a short one).
  const lines: { y: number; end: number }[] = [];
  const marks: DisplayList = [];
  for (const r of results) {
    if (r.placement !== "overlay") continue;
    const color = indicatorColor(theme, r, 0);
    const main = r.lines[0]!.values;
    const sample = model.inspected === null ? Number.NaN : main[model.inspected]!;
    const digits = Number.isNaN(sample) ? 2 : sample < 1 ? 4 : 2;
    const name = indicatorLabel(r.spec);
    const value = valueAt(model, main, digits);
    const nameW = sansWidth(name, fs);
    const w = SWATCH + SWATCH_GAP + nameW + VALUE_GAP + textWidth(value, fs);
    if (x > left && x + w > right) {
      x = left;
      y += fs + 6;
    }
    marks.push({ kind: "rect", x, y: y - 1, w: SWATCH, h: 2, fill: color, layer: "axes" });
    const nx = x + SWATCH + SWATCH_GAP;
    marks.push({ kind: "text", x: nx, y, text: name, fill: theme.textDim, fontSize: fs, fontFamily: theme.fontSans, fontWeight: 500, align: "left", baseline: "middle", layer: "axes" });
    marks.push({ kind: "text", x: nx + nameW + VALUE_GAP, y, text: value, fill: theme.text, fontSize: fs, fontFamily: theme.fontMono, fontWeight: 400, align: "left", baseline: "middle", layer: "axes" });
    const last = lines.at(-1);
    if (last && last.y === y) last.end = x + w;
    else lines.push({ y, end: x + w });
    x += w + ENTRY_GAP;
  }
  // The pane legends' translucent backing, behind each line, so candles and lines never run through the
  // text now that a line can reach across the plot (the price candles often sit under a long one).
  for (const l of lines) out.push({ kind: "rect", x: left - 4, y: l.y - fs / 2 - 3, w: Math.min(right + 4, l.end + 4) - (left - 4), h: fs + 6, fill: theme.paneLegendBg, radius: 4, layer: "axes" });
  out.push(...marks);
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
  // RSI reads to one decimal; MACD's scale follows the price (a sub-$1 stock's MACD is ~0.0001), so it
  // shows three significant digits rather than a fixed four decimals that would read "-0.0000".
  const digits = r.spec.kind === "rsi" ? 1 : -3;

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
  x += textWidth(name, fs) + 8;
  r.lines.forEach((l, k) => {
    const text = valueAt(model, l.values, digits);
    if (l.style === "histogram") {
      const v = model.inspected === null ? NaN : l.values[model.inspected]!;
      legend.push({ kind: "rect", x, y: ly - 3, w: 3, h: 6, fill: !Number.isNaN(v) && v < 0 ? theme.histDown : theme.histUp, layer: "axes" });
      legend.push({ kind: "rect", x: x + 4, y: ly - 3, w: 3, h: 6, fill: !Number.isNaN(v) && v < 0 ? theme.histDownFade : theme.histUpFade, layer: "axes" });
    } else legend.push({ kind: "rect", x, y: ly - 1, w: 8, h: 2, fill: indicatorColor(theme, r, k), layer: "axes" });
    legend.push({ kind: "text", x: x + 12, y: ly, text, fill: theme.text, fontSize: fs, fontFamily: theme.fontMono, fontWeight: 400, align: "left", baseline: "middle", layer: "axes" });
    x += 12 + textWidth(text, fs) + 10;
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

/**
 * The price series as marks, in the chart type the settings chose. Every type
 * reads the slots the model holds; only Heikin-Ashi swaps in averaged values,
 * and only for drawing. A forming (not yet final) candle keeps its dashed
 * outline in every candle-like type, so it never reads as final.
 */
function seriesMarks(model: ChartModel, state: ChartState, theme: ChartTheme): DisplayList {
  const type = model.chartType;
  if (type === "line" || type === "area" || type === "baseline") return closeSeries(model, state, theme, type);
  const out: DisplayList = [];
  const { slots, index: ix } = model;
  const price = model.price!;
  const bw = bodyWidth(ix.barSpacing);
  const half = Math.floor(bw / 2);
  const ha = type === "heikinAshi" ? heikinAshi(slots) : null;
  for (let i = ix.from; i <= ix.to; i++) {
    const s = slots[i]!;
    if (s.kind !== "candle") continue;
    const real = s.candle;
    const c: Ohlc = ha?.[i] ?? real;
    const cx = Math.round(slotX(ix, i));
    let up = c.c >= c.o;
    let hollow = false;
    if (type === "hollow") ({ up, hollow } = hollowStyle(real, prevCloseAt(slots, i)));
    const color = up ? theme.up : theme.down;
    const yh = priceY(price, c.h);
    const yl = priceY(price, c.l);
    const dash = real.final ? undefined : [3, 2];
    if (type === "bars") {
      // OHLC bar: the high-low line, the open as a tick to the left, the close as a tick to the right.
      const tick = Math.max(1, half);
      const yo = Math.round(priceY(price, c.o)) + 0.5;
      const yc = Math.round(priceY(price, c.c)) + 0.5;
      out.push({ kind: "line", x1: cx + 0.5, y1: yh, x2: cx + 0.5, y2: Math.max(yl, yh + 1), stroke: color, lineWidth: 1, ...(dash ? { dash } : {}), layer: "candles" });
      out.push({ kind: "line", x1: cx + 0.5 - tick, y1: yo, x2: cx + 0.5, y2: yo, stroke: color, lineWidth: 1, layer: "candles" });
      out.push({ kind: "line", x1: cx + 0.5, y1: yc, x2: cx + 0.5 + tick, y2: yc, stroke: color, lineWidth: 1, layer: "candles" });
      continue;
    }
    const top = priceY(price, Math.max(c.o, c.c));
    const h = Math.max(1, priceY(price, Math.min(c.o, c.c)) - top);
    const outlined = hollow || !real.final;
    if (hollow && h > 2) {
      // A hollow body: the wick stops at the body instead of running through the empty inside (the
      // forming candle keeps its wick through the dashed outline, as it always has).
      if (top > yh) out.push({ kind: "line", x1: cx + 0.5, y1: yh, x2: cx + 0.5, y2: top, stroke: color, lineWidth: 1, layer: "candles" });
      if (top + h < yl) out.push({ kind: "line", x1: cx + 0.5, y1: top + h, x2: cx + 0.5, y2: yl, stroke: color, lineWidth: 1, layer: "candles" });
    } else {
      out.push({ kind: "line", x1: cx + 0.5, y1: yh, x2: cx + 0.5, y2: Math.max(yl, yh + 1), stroke: color, lineWidth: 1, layer: "candles" });
    }
    if (!outlined) out.push({ kind: "rect", x: cx - half, y: top, w: bw, h, fill: color, layer: "candles" });
    else out.push({ kind: "rect", x: cx - half + 0.5, y: top + 0.5, w: Math.max(0, bw - 1), h: Math.max(0, h - 1), stroke: color, lineWidth: 1, ...(dash ? { dash } : {}), layer: "candles" });
  }
  return out;
}

/**
 * Line, area and baseline: one polyline through the closes, continuous across
 * gap slots (a gap is a stretch with no trade, so the price held). One slot of
 * margin each side so the line runs off the plot edge instead of stopping short.
 * - line and area take ONE colour, Robinhood's rule: up when the last close on
 *   screen is at or above the first one, down otherwise. The area fills from
 *   the line to the plot's bottom with a gradient of that colour that fades
 *   out downward (no stroke around it: no borders).
 * - baseline splits at a reference price: the previous close when the chart
 *   has it (what a day trader measures the day against), else the first
 *   visible close. Above it is up-coloured, below down-coloured, each side
 *   with a flat soft fill, and the reference is a dashed guide. Robinhood
 *   lets you drag that line; here it is fixed (see the research note).
 */
function closeSeries(model: ChartModel, state: ChartState, theme: ChartTheme, type: "line" | "area" | "baseline"): DisplayList {
  const out: DisplayList = [];
  const price = model.price!;
  const plot = model.geometry.plot;
  const [from, to] = visibleRange(model);
  const pts: number[] = [];
  let first: number | null = null;
  let last: number | null = null;
  for (let i = from; i <= to; i++) {
    const s = model.slots[i]!;
    if (s.kind !== "candle") continue;
    pts.push(slotX(model.index, i), priceY(price, s.candle.c));
    if (i >= model.index.from && i <= model.index.to) {
      first ??= s.candle.c;
      last = s.candle.c;
    }
  }
  if (pts.length < 4 || first === null || last === null) {
    // One candle: a dot, so a single print is still visible.
    if (pts.length === 2) out.push({ kind: "rect", x: pts[0]! - 1.5, y: pts[1]! - 1.5, w: 3, h: 3, radius: 1.5, fill: theme.up, layer: "candles" });
    return out;
  }
  const x0 = pts[0]!;
  const x1 = pts[pts.length - 2]!;
  const bottom = plot.y + plot.h;
  if (type === "baseline") {
    const base = state.baselinePrice ?? first;
    const by = clamp(priceY(price, base), plot.y, bottom);
    const above: Rect = { x: plot.x, y: plot.y, w: plot.w, h: by - plot.y };
    const below: Rect = { x: plot.x, y: by, w: plot.w, h: bottom - by };
    const poly = [...pts, x1, by, x0, by];
    if (above.h > 0) {
      out.push({ kind: "path", points: poly, closed: true, fill: withAlpha(theme.up, 0.14), clip: above, layer: "candles" });
      out.push({ kind: "path", points: pts, closed: false, stroke: theme.up, lineWidth: 1.5, clip: above, layer: "candles" });
    }
    if (below.h > 0) {
      out.push({ kind: "path", points: poly, closed: true, fill: withAlpha(theme.down, 0.14), clip: below, layer: "candles" });
      out.push({ kind: "path", points: pts, closed: false, stroke: theme.down, lineWidth: 1.5, clip: below, layer: "candles" });
    }
    const gy = Math.round(by) + 0.5;
    out.push({ kind: "line", x1: plot.x, y1: gy, x2: plot.x + plot.w, y2: gy, stroke: theme.guide, lineWidth: 1, dash: [3, 3], layer: "candles" });
    return out;
  }
  const color = last >= first ? theme.up : theme.down;
  if (type === "area") {
    let top = bottom;
    for (let k = 1; k < pts.length; k += 2) top = Math.min(top, pts[k]!);
    out.push({ kind: "path", points: [...pts, x1, bottom, x0, bottom], closed: true, gradient: { y0: top, y1: bottom, from: withAlpha(color, 0.28), to: withAlpha(color, 0.02) }, clip: plot, layer: "candles" });
  }
  out.push({ kind: "path", points: pts, closed: false, stroke: color, lineWidth: 1.5, clip: plot, layer: "candles" });
  return out;
}

/**
 * The highest high and lowest low on screen, labelled at their candles
 * (Legend's "display the high and low price"; TradingView's high/low price
 * labels): a short tick from the extreme and the price, in the dim text tone
 * (data, not an action, so no accent). REAL prices for every candle type,
 * Heikin-Ashi included; the close for the line types, because that is what
 * is drawn. The label sits on the side with room, so it never leaves the plot.
 */
function highLowLabels(model: ChartModel, theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const { slots, index: ix } = model;
  const price = model.price!;
  const plot = model.geometry.plot;
  const closeOnly = isCloseOnly(model.chartType);
  let hiI = -1;
  let loI = -1;
  let hi = -Infinity;
  let lo = Infinity;
  for (let i = ix.from; i <= ix.to; i++) {
    const s = slots[i]!;
    if (s.kind !== "candle") continue;
    const h = closeOnly ? s.candle.c : s.candle.h;
    const l = closeOnly ? s.candle.c : s.candle.l;
    if (h > hi) [hi, hiI] = [h, i];
    if (l < lo) [lo, loI] = [l, i];
  }
  if (hiI < 0 || hi === lo) return out;
  const fs = model.options.fontSize;
  const mark = (i: number, p: number, above: boolean) => {
    const x = slotX(ix, i);
    const y = clamp(priceY(price, p) + (above ? -3 : 3), plot.y + fs, plot.y + plot.h - fs);
    const right = x < plot.x + plot.w * 0.7;
    const x2 = right ? x + 10 : x - 10;
    out.push({ kind: "line", x1: x, y1: y, x2, y2: y, stroke: theme.textDim, lineWidth: 1, layer: "overlay" });
    out.push({ kind: "text", x: right ? x2 + 3 : x2 - 3, y, text: formatPrice(p), fill: theme.textDim, fontSize: fs, fontFamily: theme.fontMono, fontWeight: 500, align: right ? "left" : "right", baseline: "middle", layer: "overlay" });
  };
  mark(hiI, hi, true);
  mark(loI, lo, false);
  return out;
}
