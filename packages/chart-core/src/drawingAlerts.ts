import type { ChartTheme, DisplayList } from "./displayList.ts";
import { anchorXY, clipSegment, drawingSegments, tToIndex, type Drawing } from "./drawings.ts";
import { formatPrice } from "./format.ts";
import type { ChartModel, Point } from "./layout.ts";
import { isPositionKind } from "./position.ts";
import { formatAxisTag, priceY, slotX } from "./scale.ts";
import { withAlpha } from "./theme.ts";

// Drawing alerts on the chart (owner request 2026-11-09: "If the price line merges with the drawing
// line, I want some visual cue to indicate exactly that"; docs/architecture/chart-packages.md
// "Drawing alerts"). The server decides fires (crates/server/src/alerts, a `drawing` condition);
// this file is the geometry both sides share and the cue every renderer draws.
//
// - **The level at a candle** (`drawingLevelAt`) is the TS twin of crates/server/src/alerts/
//   geometry.rs `level_at`, pinned to the same golden cases (drawing_alert_fixtures.json). The x
//   axis is the slot index (scale.ts: a gap run is one slot), so a trend line or a ray is straight
//   in slots and its price at a candle is interpolated in slots, never on the clock.
// - **Touching**: the last price within the alert's tolerance of the level at the latest slot.
//   The watched drawing glows (pulsing, or steady under reduced motion) and a "touching" pill rides
//   its axis tag.
// - **Fired**: a flash and a ring on the candle where it fired, kept until the person
//   acknowledges it (the host clears `fired`).
// Colours: the alert accent (10%: it is what should draw the eye), the down hue for a stop (a
// status colour that carries meaning). Nothing else.

/** The level of a drawing an alert watches: a line's own, or a position's entry, target or stop. */
export type DrawingAlertPart = "line" | "entry" | "target" | "stop";
export const DRAWING_ALERT_PARTS: readonly DrawingAlertPart[] = ["line", "entry", "target", "stop"];
export const DRAWING_PART_LABELS: Readonly<Record<DrawingAlertPart, string>> = { line: "Line", entry: "Entry", target: "Target", stop: "Stop" };

/** How close counts as touching: whole ticks of the level's grid, or a % of the level. */
export interface DrawingTolerance {
  unit: "ticks" | "pct";
  value: number;
}
export const DEFAULT_DRAWING_TOLERANCE: DrawingTolerance = { unit: "ticks", value: 1 };
export const MAX_TOLERANCE_TICKS = 100;
export const MAX_TOLERANCE_PCT = 10;

/** Which way a drawing alert was met (its tone): up into it, down into it, or a position's stop. */
export type DrawingTone = "up" | "down" | "stop";

/** The kinds v1 watches. Ranges, supply/demand zones and vertical lines are the documented second pass. */
export const canAlertOnDrawing = (d: Pick<Drawing, "kind">): boolean => d.kind === "hline" || d.kind === "trend" || d.kind === "ray" || isPositionKind(d.kind);

/** The levels of a drawing that can be watched, the most important first (a position's stop: the after-hours mental stop). */
export function drawingAlertParts(d: Pick<Drawing, "kind">): DrawingAlertPart[] {
  if (isPositionKind(d.kind)) return ["stop", "target", "entry"];
  return canAlertOnDrawing(d) ? ["line"] : [];
}

/** A tolerance from stored JSON: junk falls back to one tick, out-of-range values clamp (as the server's Tolerance::parse). */
export function parseDrawingTolerance(v: unknown): DrawingTolerance {
  if (typeof v !== "object" || v === null) return { ...DEFAULT_DRAWING_TOLERANCE };
  const o = v as Record<string, unknown>;
  const x = typeof o.value === "number" && Number.isFinite(o.value) ? o.value : null;
  if (o.unit === "pct") return x !== null && x > 0 ? { unit: "pct", value: Math.min(MAX_TOLERANCE_PCT, x) } : { ...DEFAULT_DRAWING_TOLERANCE };
  return x !== null && x >= 0 ? { unit: "ticks", value: Math.min(MAX_TOLERANCE_TICKS, Math.round(x)) } : { ...DEFAULT_DRAWING_TOLERANCE };
}

/** The tick a level sits on: $0.0001 under $1, $0.01 at and above. */
export const levelTick = (price: number) => (price < 1 ? 0.0001 : 0.01);

/** The band half-width in dollars around a level. */
export function toleranceDollars(tol: DrawingTolerance, level: number): number {
  return tol.unit === "pct" ? (Math.abs(level) * tol.value) / 100 : tol.value * levelTick(level);
}

const PRICE_EPS = 1e-9;
const INDEX_EPS = 1e-6;

export function isTouching(price: number, level: number, tol: DrawingTolerance): boolean {
  return Math.abs(price - level) <= toleranceDollars(tol, level) + PRICE_EPS;
}

/** "±1 tick", "±3 ticks", "±0.5%". */
export function toleranceText(tol: DrawingTolerance): string {
  if (tol.unit === "pct") return `±${tol.value}%`;
  return tol.value === 0 ? "exactly" : `±${tol.value} tick${tol.value === 1 ? "" : "s"}`;
}

/**
 * The price `part` of drawing `d` stands at, at the candle that starts at `t`, on a chart with
 * these slots (crates/server/src/alerts/geometry.rs `level_at`, the same rules): null when the
 * level is not there (a trend line outside its anchors, a ray before its start, a vertical line, a
 * part the drawing does not have).
 */
export function drawingLevelAt(d: Pick<Drawing, "kind" | "a" | "b" | "c">, part: DrawingAlertPart, slots: readonly { t: number }[], t: number, timeframeMs: number): number | null {
  if (isPositionKind(d.kind)) {
    if (part === "entry") return d.a.p;
    if (part === "target") return d.b.p;
    if (part === "stop") return d.c?.p ?? d.a.p;
    return null;
  }
  if (part !== "line") return null;
  if (d.kind === "hline") return d.a.p;
  if (d.kind !== "trend" && d.kind !== "ray") return null;
  const ia = tToIndex(slots, d.a.t, timeframeMs);
  const ib = tToIndex(slots, d.b.t, timeframeMs);
  const i = tToIndex(slots, t, timeframeMs);
  const span = ib - ia;
  if (Math.abs(span) < INDEX_EPS) return null;
  const f = (i - ia) / span;
  const inside = d.kind === "trend" ? f >= -INDEX_EPS && f <= 1 + INDEX_EPS : f >= -INDEX_EPS;
  return inside ? d.a.p + (d.b.p - d.a.p) * f : null;
}

/** One drawing alert as the host hands it to the chart. */
export interface WatchedDrawing {
  alertId: string;
  drawingId: string;
  part: DrawingAlertPart;
  tol: DrawingTolerance;
  /** Snoozed, cooling, finished or orphaned: no glow (the cue means "this alert is live and the price is on it"). */
  muted?: boolean;
  /** A fire not yet acknowledged: where and how it fired. Kept highlighted until the host clears it. */
  fired?: { t: number; price: number; tone: DrawingTone } | null;
}

export interface DrawingCueLayer {
  watched: readonly WatchedDrawing[];
  /** The last price (the forming candle's close). */
  last: number | null;
  /** The pulse's phase, 0..1 (a sine of it sets the glow); null = reduced motion: a steady glow. */
  pulse: number | null;
  /** The newest fire's flash, 1 (just fired) → 0 (gone); 0 or absent under reduced motion. */
  flash?: number;
}

/** A watched drawing's state at the latest slot. */
export interface DrawingCueState {
  alertId: string;
  level: number | null;
  touching: boolean;
}

/** Each watched drawing's level at the latest slot and whether the last price touches it (hosts run the pulse only while one does). */
export function drawingCueStates(drawings: readonly Drawing[], watched: readonly WatchedDrawing[], slots: readonly { t: number }[], timeframeMs: number, last: number | null): DrawingCueState[] {
  const latest = slots.at(-1)?.t;
  const byId = new Map(drawings.map((d) => [d.id, d]));
  return watched.map((w) => {
    const d = byId.get(w.drawingId);
    const level = d && latest !== undefined ? drawingLevelAt(d, w.part, slots, latest, timeframeMs) : null;
    return { alertId: w.alertId, level, touching: !w.muted && level !== null && last !== null && isTouching(last, level, w.tol) };
  });
}

/** The pixel segments of a watched level, clipped to the plot. */
function levelSegments(model: ChartModel, d: Drawing, part: DrawingAlertPart): [Point, Point][] {
  if (!model.price) return [];
  if (!isPositionKind(d.kind)) return drawingSegments(model, d);
  const a = anchorXY(model, d.a);
  const b = anchorXY(model, d.b);
  if (!a || !b) return [];
  const p = part === "target" ? d.b.p : part === "stop" ? (d.c?.p ?? d.a.p) : d.a.p;
  const y = priceY(model.price, p);
  const s = clipSegment({ x: Math.min(a.x, b.x), y }, { x: Math.max(a.x, b.x), y }, model.geometry.plot);
  return s ? [s] : [];
}

/** The glow's alpha: 0.18–0.42 on the pulse, a steady 0.32 under reduced motion. */
export function glowAlpha(pulse: number | null): number {
  if (pulse === null) return 0.32;
  return 0.3 + 0.12 * Math.sin(pulse * 2 * Math.PI);
}

const TAG_H = 18;

/**
 * The cue, drawn over the drawings: a glow and a "touching" pill on the axis for each live alert
 * whose level the last price touches; a ring (and, just after a fire, an expanding flash) on the
 * candle of each fire not yet acknowledged; a small bell at the middle of every other
 * watched level, so a watched drawing reads as watched.
 */
export function layoutDrawingCues(model: ChartModel, drawings: readonly Drawing[], layer: DrawingCueLayer, theme: ChartTheme): DisplayList {
  const out: DisplayList = [];
  const price = model.price;
  if (!price || !layer.watched.length) return out;
  const g = model.geometry;
  const plot = g.plot;
  const fs = model.options.fontSize;
  const byId = new Map(drawings.map((d) => [d.id, d]));
  const states = drawingCueStates(drawings, layer.watched, model.slots, model.timeframeMs, layer.last);
  layer.watched.forEach((w, k) => {
    const d = byId.get(w.drawingId);
    const st = states[k]!;
    if (!d) return;
    const hue = w.part === "stop" ? theme.down : theme.alert;
    const levelY = st.level === null ? null : priceY(price, st.level);
    const inPlot = levelY !== null && levelY >= plot.y && levelY <= plot.y + plot.h;
    if (st.touching) {
      const a = glowAlpha(layer.pulse);
      for (const [p, q] of levelSegments(model, d, w.part)) {
        out.push({ kind: "line", x1: p.x, y1: p.y, x2: q.x, y2: q.y, stroke: withAlpha(hue, a), lineWidth: 8, clip: plot, layer: "overlay" });
        out.push({ kind: "line", x1: p.x, y1: p.y, x2: q.x, y2: q.y, stroke: hue, lineWidth: 2, clip: plot, layer: "overlay" });
      }
      if (inPlot && st.level !== null) {
        // The axis tag at the level, and the pill at the middle of the line. Seen on screen
        // (2026-11-09): beside the axis it covered the newest candles, the very thing being
        // watched; at the left end the drawing toolbox (default there) hid it.
        out.push({ kind: "rect", x: g.priceAxis.x + 2, y: levelY - TAG_H / 2, w: g.priceAxis.w - 2, h: TAG_H, radius: 4, fill: hue, layer: "crosshair" });
        out.push({ kind: "text", x: g.priceAxis.x + 1 + g.priceAxis.w / 2, y: levelY, text: formatAxisTag(model, st.level), fill: theme.onAlert, fontSize: fs, fontFamily: theme.fontMono, fontWeight: 600, align: "center", baseline: "middle", layer: "crosshair" });
        const text = w.part === "stop" ? "touching stop" : "touching";
        const pw = Math.ceil(text.length * fs * 0.6) + 14;
        const px = plot.x + (plot.w - pw) / 2;
        out.push({ kind: "rect", x: px, y: levelY - 10, w: pw, h: 20, radius: 10, fill: hue, layer: "crosshair" });
        out.push({ kind: "text", x: px + pw / 2, y: levelY, text, fill: theme.onAlert, fontSize: fs, fontFamily: theme.fontMono, fontWeight: 600, align: "center", baseline: "middle", layer: "crosshair" });
      }
    } else if (inPlot && !w.fired) {
      // Watched, not touching: a small bell at the middle of the level (the right end holds the key
      // levels' labels and the newest candles, the left end the drawing toolbox).
      const cx = plot.x + plot.w / 2;
      out.push({ kind: "rect", x: cx - 8, y: levelY - 8, w: 16, h: 16, radius: 8, fill: w.muted ? theme.alertMuted : withAlpha(hue, 0.85), layer: "overlay" });
      out.push({ kind: "path", points: bellShape(cx, levelY - 0.5, 5.6), closed: true, fill: theme.onAlert, layer: "overlay" });
    }
    if (w.fired) {
      const fhue = w.fired.tone === "stop" ? theme.down : theme.alert;
      const bucket = Math.floor(w.fired.t / model.timeframeMs) * model.timeframeMs;
      const x = slotX(model.index, tToIndex(model.slots, bucket, model.timeframeMs));
      const y = priceY(price, w.fired.price);
      if (x >= plot.x && x <= plot.x + plot.w && y >= plot.y && y <= plot.y + plot.h) {
        const flash = layer.flash ?? 0;
        if (flash > 0) {
          const r = 10 + (1 - flash) * 26;
          out.push({ kind: "rect", x: x - r, y: y - r, w: 2 * r, h: 2 * r, radius: r, fill: withAlpha(fhue, 0.35 * flash), layer: "overlay" });
        }
        out.push({ kind: "rect", x: x - 9, y: y - 9, w: 18, h: 18, radius: 9, fill: withAlpha(fhue, 0.28), layer: "overlay" });
        out.push({ kind: "rect", x: x - 5, y: y - 5, w: 10, h: 10, radius: 5, fill: fhue, layer: "overlay" });
        out.push({ kind: "rect", x: x - 2, y: y - 2, w: 4, h: 4, radius: 2, fill: theme.onAlert, layer: "overlay" });
      }
    }
  });
  return out;
}

/** A small bell, as one closed path around (cx, cy) (the price alerts' bell, alerts.ts). */
function bellShape(cx: number, cy: number, r: number): number[] {
  const u: [number, number][] = [
    [-0.55, 0.35], [-0.55, -0.05], [-0.45, -0.4], [-0.2, -0.6], [0, -0.66], [0.2, -0.6], [0.45, -0.4], [0.55, -0.05], [0.55, 0.35], [0.78, 0.52], [-0.78, 0.52],
  ];
  return u.flatMap(([x, y]) => [cx + x * r, cy + y * r]);
}

/** "Touched long stop 1.85": a fire's short label for toasts. */
export function drawingFireLabel(kind: Drawing["kind"], part: DrawingAlertPart, level: number): string {
  const name = isPositionKind(kind) ? `${kind} ${part}` : kind === "hline" ? "line" : kind === "trend" ? "trend line" : "ray";
  return `${name} ${formatPrice(level)}`;
}

// ---- Motion -----------------------------------------------------------------
// One clock for both renderers: they tick it (every CUE_FRAME_MS) only while a watched drawing is
// touched or a fire is flashing, and hand the phases to the layer. Under reduced motion they do
// not tick at all: the glow is steady (pulse null) and there is no flash, the marker just appears.

/** One breath of the touching glow. */
export const CUE_PULSE_MS = 1600;
/** A fire's flash, from the moment the client hears it. */
export const CUE_FLASH_MS = 1400;
/** The renderers' tick while animating (20 fps: a soft glow needs no more, and the chart is repainted whole). */
export const CUE_FRAME_MS = 50;

/** The pulse's phase at `now` (ms). */
export const cuePulse = (now: number): number => (((now % CUE_PULSE_MS) + CUE_PULSE_MS) % CUE_PULSE_MS) / CUE_PULSE_MS;

/** A fire's flash at `now`, heard at `heardAt`: 1 → 0, eased out. */
export function cueFlash(heardAt: number, now: number): number {
  const f = 1 - (now - heardAt) / CUE_FLASH_MS;
  return f <= 0 ? 0 : f >= 1 ? 1 : f * f;
}

/** A fire's identity for "is this one new" (an alert can fire again later). */
export const fireKey = (w: WatchedDrawing): string | null => (w.fired ? `${w.alertId}@${w.fired.t}` : null);
