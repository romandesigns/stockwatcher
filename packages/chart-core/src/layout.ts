import { hatch, sortByLayer, type ChartTheme, type DisplayList, type Rect } from "./displayList.ts";
import { formatPrice } from "./format.ts";
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
}

export interface ChartGeometry {
  plot: Rect;
  volume: Rect | null;
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

export function chartGeometry(size: Size, o: ChartOptions): ChartGeometry {
  const plotW = Math.max(0, size.width - o.priceAxisWidth);
  const volH = o.volumeHeight > 0 ? o.volumeHeight : 0;
  const plotH = Math.max(0, size.height - o.timeAxisHeight - (volH ? volH + o.paneGap : 0));
  const plot = { x: 0, y: 0, w: plotW, h: plotH };
  const volume = volH ? { x: 0, y: plotH + o.paneGap, w: plotW, h: volH } : null;
  const timeY = volume ? volume.y + volume.h : plotH;
  return {
    plot,
    volume,
    priceAxis: { x: plotW, y: 0, w: o.priceAxisWidth, h: plotH },
    timeAxis: { x: 0, y: timeY, w: plotW, h: o.timeAxisHeight },
  };
}

/** Index of the slot with exactly t, or null. */
export function slotIndexOf(slots: readonly Slot[], t: number): number | null {
  const i = lowerBound(slots, t);
  return slots[i]?.t === t ? i : null;
}

export function computeChartModel(state: ChartState, viewport: Viewport, size: Size): ChartModel {
  const options = { ...DEFAULT_CHART_OPTIONS, ...state.options };
  const geometry = chartGeometry(size, options);
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
  return { options, geometry, slots, index, price, priceTicks, volumeMax, timeframeMs: TIMEFRAME_MS[state.timeframe], inspected };
}

/** Percent mode's base: the open of the first candle in [from, to]. */
export function firstVisibleOpen(slots: readonly Slot[], from: number, to: number): number | undefined {
  for (let i = Math.max(0, from); i <= to; i++) {
    const s = slots[i];
    if (s?.kind === "candle") return s.candle.o;
  }
  return undefined;
}

export type HitRegion = "plot" | "volume" | "priceAxis" | "timeAxis" | "none";

export interface Hit {
  region: HitRegion;
  index: number | null;
  slot: Slot | null;
  /** Price under the pointer (plot and price axis only). */
  price: number | null;
}

const inside = (r: Rect | null, x: number, y: number) => !!r && x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h;

/** x, y (CSS px on the chart surface) → region, slot under x, and price under y. */
export function hitTest(model: ChartModel, x: number, y: number): Hit {
  const g = model.geometry;
  const region: HitRegion = inside(g.plot, x, y)
    ? "plot"
    : inside(g.volume, x, y)
      ? "volume"
      : inside(g.priceAxis, x, y)
        ? "priceAxis"
        : inside(g.timeAxis, x, y)
          ? "timeAxis"
          : "none";
  const index = region === "none" || region === "priceAxis" ? null : slotAt(model.index, x, model.slots.length);
  const price = model.price && (region === "plot" || region === "priceAxis") ? yPrice(model.price, y) : null;
  return { region, index, slot: index === null ? null : model.slots[index]!, price };
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
    out.push(...profileBars(state.profile, price, { x: g.plot.x + g.plot.w * (1 - o.profileWidth), y: g.plot.y, w: g.plot.w * o.profileWidth, h: g.plot.h }, "right", theme));
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

  // inspection line at the inspected (or latest) slot
  if (model.inspected !== null && model.inspected >= ix.from && model.inspected <= ix.to) {
    const x = Math.round(slotX(ix, model.inspected)) + 0.5;
    const bottom = g.volume ? g.volume.y + g.volume.h : g.plot.y + g.plot.h;
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
      out.push({ kind: "text", x: g.priceAxis.x + 8, y, text: model.priceTicks.labels[k] ?? formatPrice(v, model.priceTicks.decimals), fill: theme.axisText, fontSize: fs, fontFamily: mono, fontWeight: 400, align: "left", baseline: "middle", layer: "axes" });
    });
    if (last && lastY !== null) {
      const ax = g.priceAxis.x + 2;
      const right = g.priceAxis.x + g.priceAxis.w - 2;
      const fill = last.c >= last.o ? theme.up : theme.down;
      out.push({ kind: "path", points: [ax, lastY, ax + 5, lastY - 9, right, lastY - 9, right, lastY + 9, ax + 5, lastY + 9], closed: true, fill, layer: "axes" });
      out.push({ kind: "text", x: (ax + 5 + right) / 2, y: lastY, text: formatAxisValue(price, last.c), fill: theme.onTag, fontSize: fs + 1, fontFamily: mono, fontWeight: 600, align: "center", baseline: "middle", layer: "axes" });
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
export function profileBars(profile: ProfileWire, price: PriceScale, area: Rect, align: "left" | "right", theme: ChartTheme): DisplayList {
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
    // 1 px separation between bins once they are tall enough to afford it.
    const h = fullH > 3 ? fullH - 1 : Math.max(0.5, fullH);
    const w = Math.max(1, (r.v / max) * area.w);
    const isPoc = poc !== null && Math.abs(r.p - poc) < eps;
    // low and high are bin starts, both inside the value area.
    const inVa = va !== null && r.p >= va.low - eps && r.p <= va.high + eps;
    const fill = isPoc ? theme.profilePoc : inVa ? theme.profileValueArea : theme.profileBar;
    out.push({ kind: "rect", x: align === "left" ? area.x : area.x + area.w - w, y: top, w, h, fill, radius: h > 3 ? 1.5 : 0, layer: "profile" });
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
