import { CHART_TYPES, type ChartType } from "./chartTypes.ts";
import { computeChartModel, type ChartState, type Size } from "./layout.ts";
import { MAX_BAR_SPACING, MIN_BAR_SPACING, manualPrice, type PriceScaleType, type Slot, type Viewport } from "./scale.ts";
import { tradingDayOf } from "./time.ts";
import { TIMEFRAME_MS, type Timeframe } from "./types.ts";

// The chart settings panel (owner request 2026-10-18): a "Chart" button left
// of Indicators with Robinhood's chart settings, plus the zoom the owner
// asked for so that opening a ticker never needs a manual resize: "set it to
// auto stretch, or change its zooming from 25%, 50%, 100% ... in order to
// capture the full scope of the price action."
//
// The two zoom halves answer different axes (confirmed with the owner):
// - autoStretch is VERTICAL: the price axis fits what is on screen, and keeps
//   re-fitting on pan, zoom, new data, a timeframe change and a ticker switch
//   (Viewport.price "auto"). A price-axis drag or a vertical chart drag makes
//   the range manual, as before; manual holds until the next ticker (or
//   timeframe) or a double-click / double-tap on the price axis, or the small
//   "A" button the chart shows while manual (TradingView's and Legend's
//   behaviour). With autoStretch OFF a newly opened chart is fitted ONCE and
//   then frozen (a manual range), so panning keeps the owner's scale.
// - zoom is HORIZONTAL: how much of the trading day fits, anchored at the
//   latest candle. 100 % = the whole day so far, 50 % its most recent half,
//   25 % its most recent quarter. Applied on a ticker switch, a timeframe
//   switch and when chosen; wheel/pinch zoom and pan still work after it.
//
// Persisted beside the layers: the web under `${persistKey}:settings`
// (`sw-chart:settings` next to `sw-chart:layers`), the phone under the
// preference `chart.settings` next to `chart.layers`.

export type ZoomPreset = 25 | 50 | 100;
export const ZOOM_PRESETS: readonly ZoomPreset[] = [25, 50, 100];

/** Robinhood's "Visibility" group: hide whole families of chart objects without deleting them. */
export interface ChartVisibility {
  indicators: boolean;
  drawings: boolean;
  /** Simulated order lines, the position line and the "+" place button. */
  trading: boolean;
}

export interface ChartSettings {
  /** Bumped when a field's meaning changes; parseChartSettings migrates older saves. */
  version: 1;
  chartType: ChartType;
  priceScale: PriceScaleType;
  /** Show pre-market, after-hours and overnight candles (off: the regular session only, no gaps). */
  extendedHours: boolean;
  volume: boolean;
  /** Horizontal price grid lines. */
  grid: boolean;
  /** The faint backgrounds behind pre-market / after-hours / overnight candles ("highlight extended hours"). */
  sessionShading: boolean;
  /** A dashed vertical line where a new trading day starts. */
  sessionBreaks: boolean;
  /** Labels on the highest high and lowest low on screen (Legend's "display the high and low price"). */
  highLow: boolean;
  autoStretch: boolean;
  zoom: ZoomPreset;
  visibility: ChartVisibility;
}

/**
 * Today's look is the default, so nothing changes until the owner opens the
 * panel: candles, a linear scale, every session shown and shaded, volume and
 * grid on, no session-break lines and no high/low labels (neither existed),
 * plus the new zoom: auto stretch on and the whole day (100 %).
 */
export const DEFAULT_CHART_SETTINGS: ChartSettings = {
  version: 1,
  chartType: "candles",
  priceScale: "linear",
  extendedHours: true,
  volume: true,
  grid: true,
  sessionShading: true,
  sessionBreaks: false,
  highLow: false,
  autoStretch: true,
  zoom: 100,
  visibility: { indicators: true, drawings: true, trading: true },
};

const SCALES: readonly PriceScaleType[] = ["linear", "percent", "log"];
const BOOLS = ["extendedHours", "volume", "grid", "sessionShading", "sessionBreaks", "highLow", "autoStretch"] as const;

/**
 * A saved value → valid settings. Storage outlives code: a save from before a
 * field existed lacks it, a hand-edited or corrupted one has junk, so every
 * field is checked on its own and falls back to its default (the parseLayers
 * rule). Migration of older shapes:
 * - no `version` (a pre-release save) is read field by field like v1;
 * - `zoom` saved as a fraction (0.25 / 0.5 / 1) or a string ("50", "50%")
 *   becomes the percent preset;
 * - the chart type "candle" / "candlestick" (Robinhood's own words) maps to
 *   "candles", "ha" to "heikinAshi";
 * - the scale "logarithmic" maps to "log", "percentage" to "percent".
 */
export function parseChartSettings(raw: unknown): ChartSettings {
  const d = DEFAULT_CHART_SETTINGS;
  if (typeof raw !== "object" || raw === null) return { ...d, visibility: { ...d.visibility } };
  const r = raw as Record<string, unknown>;
  const out: ChartSettings = { ...d, visibility: { ...d.visibility } };
  const type = typeof r.chartType === "string" ? ({ candle: "candles", candlestick: "candles", ha: "heikinAshi" } as Record<string, string>)[r.chartType] ?? r.chartType : null;
  if (type && (CHART_TYPES as readonly string[]).includes(type)) out.chartType = type as ChartType;
  const scale = typeof r.priceScale === "string" ? ({ logarithmic: "log", percentage: "percent" } as Record<string, string>)[r.priceScale] ?? r.priceScale : null;
  if (scale && (SCALES as readonly string[]).includes(scale)) out.priceScale = scale as PriceScaleType;
  for (const k of BOOLS) if (typeof r[k] === "boolean") out[k] = r[k] as boolean;
  const z = parseZoom(r.zoom);
  if (z !== null) out.zoom = z;
  if (typeof r.visibility === "object" && r.visibility !== null) {
    const v = r.visibility as Record<string, unknown>;
    for (const k of ["indicators", "drawings", "trading"] as const) if (typeof v[k] === "boolean") out.visibility[k] = v[k] as boolean;
  }
  return out;
}

function parseZoom(z: unknown): ZoomPreset | null {
  let n = typeof z === "number" ? z : typeof z === "string" ? Number(z.replace(/%$/, "")) : NaN;
  if (n > 0 && n <= 1) n *= 100;
  return (ZOOM_PRESETS as readonly number[]).includes(n) ? (n as ZoomPreset) : null;
}

/** The regular session's length, and the extended day's (04:00–20:00 ET), in minutes. */
const REGULAR_MINUTES = 390;
const EXTENDED_MINUTES = 960;

/**
 * How many slots "100 %" means: the latest trading day's slots so far (every
 * slot from the last one back to the first of its trading day, i.e. its
 * pre-market when extended hours show, its 09:30 open when they do not;
 * gaps count one slot each because they take one slot on the axis).
 *
 * Floored at a quarter of a full session in this timeframe (390 min regular,
 * 960 min extended), so at 09:31 "100 %" is not two giant candles: early in
 * the day the latest candles sit on the right and the previous session
 * fills the rest, as when you open Robinhood before the bell.
 */
export function daySlotCount(slots: readonly Slot[], timeframe: Timeframe, extendedHours: boolean): number {
  const full = ((extendedHours ? EXTENDED_MINUTES : REGULAR_MINUTES) * 60_000) / TIMEFRAME_MS[timeframe];
  if (slots.length === 0) return Math.round(full);
  const last = tradingDayOf(slots[slots.length - 1]!.t);
  let n = 0;
  for (let i = slots.length - 1; i >= 0; i--) {
    const d = tradingDayOf(slots[i]!.t);
    if (d.year !== last.year || d.month !== last.month || d.day !== last.day) break;
    n++;
  }
  return Math.max(n, Math.ceil(full / 4));
}

/** Slots a preset puts on screen (at least 1). */
export function presetSlots(slots: readonly Slot[], timeframe: Timeframe, extendedHours: boolean, preset: ZoomPreset): number {
  return Math.max(1, Math.round((daySlotCount(slots, timeframe, extendedHours) * preset) / 100));
}

/**
 * Pixels per slot that fit `presetSlots` plus the right margin into the
 * plot, clamped to the zoom limits. A full 15 s session (1,560 slots) in a
 * 900 px plot is 0.58 px a slot: MIN_BAR_SPACING is low enough for that, so
 * 100 % really is the whole day; candles then draw as 1 px marks.
 */
export function presetBarSpacing(slots: readonly Slot[], timeframe: Timeframe, extendedHours: boolean, preset: ZoomPreset, plotWidth: number, rightMargin: number): number {
  const n = presetSlots(slots, timeframe, extendedHours, preset);
  const bs = plotWidth / (n + rightMargin);
  return Math.min(MAX_BAR_SPACING, Math.max(MIN_BAR_SPACING, Number.isFinite(bs) && bs > 0 ? bs : MIN_BAR_SPACING));
}

/** A viewport following the latest candle at the preset's zoom (the price mode is left alone). */
export function presetViewport(vp: Viewport, slots: readonly Slot[], timeframe: Timeframe, extendedHours: boolean, preset: ZoomPreset, plotWidth: number): Viewport {
  return { ...vp, barSpacing: presetBarSpacing(slots, timeframe, extendedHours, preset, plotWidth, vp.rightMargin), anchorT: null, anchorFrac: 0 };
}

/**
 * The view a chart opens with: what a ticker switch, a timeframe switch, or
 * choosing a preset produce. `base` supplies the right margin. The plot width
 * comes from a model of `state` (the price column is fitted to its labels, so
 * it is not simply size − options.priceAxisWidth). Auto stretch on: the price
 * range is auto. Off: fitted once at the new zoom, then held as manual.
 */
export function openView(state: ChartState, settings: Pick<ChartSettings, "autoStretch" | "zoom" | "extendedHours">, size: Size, base: Viewport): Viewport {
  const auto: Viewport = { ...base, price: { mode: "auto" } };
  const m0 = computeChartModel(state, auto, size);
  const vp = presetViewport(auto, m0.slots, state.timeframe, settings.extendedHours, settings.zoom, m0.geometry.plot.w);
  if (settings.autoStretch) return vp;
  const m1 = computeChartModel(state, vp, size);
  return m1.price ? manualPrice(vp, { lo: m1.price.lo, hi: m1.price.hi }) : vp;
}

/**
 * A view's identity: a new symbol or timeframe opens a fresh view (openView);
 * anything else (new data, settings that do not change the window) keeps it.
 */
export function viewKey(symbol: string, timeframe: Timeframe): string {
  return `${symbol}|${timeframe}`;
}

/** One frame of an ease-out cubic from `from` to `to` (the preset zoom slide); 1 at or past the end. */
export function easeOut(from: number, to: number, elapsed: number, duration: number): number {
  const t = duration > 0 ? Math.min(1, Math.max(0, elapsed / duration)) : 1;
  return from + (to - from) * (1 - (1 - t) ** 3);
}

/** How long a settings-driven slide takes (zoom preset, volume pane): quick, like the menus. */
export const SETTINGS_MOTION_MS = 220;
