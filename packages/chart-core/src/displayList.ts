// The renderer-agnostic display list. layout functions produce it; each
// platform only executes it (chart-web: Canvas 2D; React Native later: Skia).
// Coordinates are CSS pixels from the top-left of the drawing surface; the
// renderer applies devicePixelRatio. Colors are whatever string the theme
// gave (any CSS color the platform's renderer accepts). Nothing here may
// reference a DOM or canvas API.

export type Layer = "background" | "grid" | "gaps" | "profile" | "volume" | "candles" | "overlay" | "axes" | "crosshair";

/** Paint order, back to front. Primitives within a layer keep list order. */
export const LAYER_ORDER: readonly Layer[] = ["background", "grid", "gaps", "profile", "volume", "candles", "overlay", "axes", "crosshair"];

interface Base {
  layer: Layer;
}

export interface RectPrim extends Base {
  kind: "rect";
  x: number;
  y: number;
  w: number;
  h: number;
  fill?: string;
  stroke?: string;
  lineWidth?: number;
  /** Dash pattern in px, e.g. [3, 2] (the forming candle). */
  dash?: number[];
  /** Corner radius in px. */
  radius?: number;
}

export interface LinePrim extends Base {
  /** Clip to this rect (a line that may cross into another pane). */
  clip?: Rect;
  kind: "line";
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  stroke: string;
  lineWidth: number;
  dash?: number[];
}

export interface TextPrim extends Base {
  kind: "text";
  x: number;
  y: number;
  text: string;
  fill: string;
  fontSize: number;
  fontFamily: string;
  fontWeight: number;
  align: "left" | "center" | "right";
  baseline: "top" | "middle" | "bottom";
}

/** Polyline or polygon: points as [x0, y0, x1, y1, …]. */
export interface PathPrim extends Base {
  kind: "path";
  points: number[];
  closed: boolean;
  /** Clip to this rect (indicator lines, which may leave their pane on a manual price range). */
  clip?: Rect;
  fill?: string;
  stroke?: string;
  lineWidth?: number;
}

export type Primitive = RectPrim | LinePrim | TextPrim | PathPrim;
export type DisplayList = Primitive[];

/** Stable sort into paint order (layout already emits in order; renderers call this defensively). */
export function sortByLayer(list: DisplayList): DisplayList {
  const rank = new Map(LAYER_ORDER.map((l, i) => [l, i]));
  return list
    .map((p, i) => ({ p, i }))
    .sort((a, b) => rank.get(a.p.layer)! - rank.get(b.p.layer)! || a.i - b.i)
    .map((x) => x.p);
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * Diagonal hatch lines (45°, rising left to right) clipped to `r`, as plain
 * line primitives, so no renderer needs clip or pattern support.
 */
export function hatch(r: Rect, spacing: number, stroke: string, lineWidth: number, layer: Layer): LinePrim[] {
  const out: LinePrim[] = [];
  if (r.w <= 0 || r.h <= 0 || spacing <= 0) return out;
  const x0 = r.x;
  const x1 = r.x + r.w;
  const bottom = r.y + r.h;
  // Each line is the set of points (c + s, bottom - s), s in [0, h]. c sits on
  // a global grid of `spacing` so the pattern does not crawl while panning.
  for (let c = Math.ceil((x0 - r.h) / spacing) * spacing; c < x1; c += spacing) {
    const sMin = Math.max(0, x0 - c);
    const sMax = Math.min(r.h, x1 - c);
    if (sMax <= sMin) continue;
    out.push({ kind: "line", x1: c + sMin, y1: bottom - sMin, x2: c + sMax, y2: bottom - sMax, stroke, lineWidth, layer });
  }
  return out;
}

/** Theme tokens the layouts need. Passed in by the app: chart-core hardcodes no colors. */
export interface ChartTheme {
  grid: string;
  axisText: string;
  text: string;
  textDim: string;
  up: string;
  down: string;
  upVolume: string;
  downVolume: string;
  gapHatch: string;
  crosshair: string;
  crosshairLabelBg: string;
  crosshairLabelText: string;
  /** Text on the last-price tag (sits on up/down color). */
  onTag: string;
  profileBar: string;
  profileValueArea: string;
  profilePoc: string;
  /** The profile drawn inside the price pane, behind the candles (a phone): thin bars, one quiet tone, the POC lightly accented. */
  /** Oscillator panes: RSI's shaded 30–70 zone and its 50 line; MACD's histogram in four shades
   * (above/below zero × growing/shrinking, as TradingView and so Robinhood Legend draw it); the
   * translucent backing behind a pane's legend so lines never run through its text. */
  oscZone: string;
  oscMid: string;
  histUp: string;
  histUpFade: string;
  histDown: string;
  histDownFade: string;
  paneLegendBg: string;
  profileOverlayBar: string;
  profileOverlayPoc: string;
  sessionOvernight: string;
  sessionPre: string;
  sessionRegular: string;
  sessionAfter: string;
  sessionText: string;
  sessionTextStrong: string;
  minimapPast: string;
  minimapPastRegular: string;
  minimapFuture: string;
  replayHead: string;
  /**
   * Categorical slots for indicator lines, in this FIXED order (never cycled).
   * Validated all-pairs on the dark surface; see indicators.ts.
   */
  series1: string;
  series2: string;
  series3: string;
  /** Indicator pane guides (RSI 30/70, MACD zero). */
  guide: string;
  /** A drawing (trend line, ray, level, range), unselected and selected. */
  drawing: string;
  drawingSelected: string;
  /** Fill of a range (rectangle) drawing. */
  drawingFill: string;
  /** Inside of a drawing handle (its ring is drawingSelected). */
  handleFill: string;
  /** Simulated order lines and their pills; text on a pill. */
  orderBuy: string;
  orderSell: string;
  onOrder: string;
  /** The position line (average price). */
  position: string;
  /** A trading halt's shading, and its label and badge. */
  halt: string;
  haltText: string;
  /** LULD band lines and their tags' text. */
  luld: string;
  luldText: string;
  /** Key level lines (premarket, open, previous close), and the stronger high/low of day. */
  level: string;
  levelStrong: string;
  /**
   * Faint backgrounds behind candles outside the regular session, so the
   * session a candle belongs to is visible in the chart itself. Regular
   * hours stay the plain background: it is the normal state.
   */
  shadePre: string;
  shadeAfter: string;
  shadeOvernight: string;
  fontSans: string;
  fontMono: string;
}
