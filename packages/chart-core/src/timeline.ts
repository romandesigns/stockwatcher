import { sortByLayer, type ChartTheme, type DisplayList } from "./displayList.ts";
import type { Size } from "./layout.ts";
import { formatEtTime, type SessionBand } from "./time.ts";
import type { Candle, SessionKind } from "./types.ts";

export interface VolumeBucket {
  t: number;
  v: number;
}

/**
 * Groups candle volume into fixed buckets for the day minimap. This only
 * re-groups the server's candle volumes for display (any timeframe gives the
 * same totals, since all are built from the same trades); it never touches
 * trades. A dedicated day-volume series from the server would replace it.
 */
export function volumeBuckets(candles: readonly Candle[], start: number, end: number, bucketMs: number): VolumeBucket[] {
  const n = Math.max(0, Math.ceil((end - start) / bucketMs));
  const sums = Array.from({ length: n }, () => 0);
  for (const c of candles) {
    if (c.t < start || c.t >= end) continue;
    sums[Math.floor((c.t - start) / bucketMs)]! += c.v;
  }
  return sums.map((v, i) => ({ t: start + i * bucketMs, v }));
}

export interface TimelineInput {
  bands: readonly SessionBand[];
  buckets: readonly VolumeBucket[];
  bucketMs: number;
  /** Replay head / live clock position; null hides it. */
  now: number | null;
  /** full: labelled bands (desktop). compact: short labels under the bands. minimal: no labels, a thin band
   *  strip under the minimap (the phone's playback card, where the clock already names the session). */
  variant: "full" | "compact" | "minimal";
}

export interface TimelineGeometry {
  start: number;
  end: number;
  headLabelH: number;
  minimap: { y: number; h: number };
  bands: { y: number; h: number };
  labelsY: number | null;
}

export function timelineGeometry(input: TimelineInput, size: Size): TimelineGeometry {
  const start = input.bands[0]?.start ?? 0;
  const end = input.bands[input.bands.length - 1]?.end ?? start + 1;
  if (input.variant === "full") {
    const headLabelH = 16;
    const bandH = 22;
    const gap = 10;
    const minimapH = Math.max(8, size.height - headLabelH - gap - bandH);
    return { start, end, headLabelH, minimap: { y: headLabelH, h: minimapH }, bands: { y: headLabelH + minimapH + gap, h: bandH }, labelsY: null };
  }
  if (input.variant === "minimal") {
    const bandH = 4;
    const gap = 4;
    const minimapH = Math.max(8, size.height - gap - bandH);
    return { start, end, headLabelH: 4, minimap: { y: 4, h: minimapH - 4 }, bands: { y: minimapH + gap, h: bandH }, labelsY: null };
  }
  const bandH = 6;
  const gap = 8;
  const labelsH = 14;
  const minimapH = Math.max(8, size.height - gap - bandH - labelsH - 4);
  return { start, end, headLabelH: 4, minimap: { y: 4, h: minimapH - 4 }, bands: { y: minimapH + gap, h: bandH }, labelsY: minimapH + gap + bandH + 4 };
}

/** Epoch ms → x on the timeline. */
export function timelineX(geo: TimelineGeometry, width: number, t: number): number {
  return ((t - geo.start) / (geo.end - geo.start)) * width;
}

/** x on the timeline → epoch ms, clamped to the day: what a tap or click seeks to. */
export function timelineTimeAt(bands: readonly SessionBand[], width: number, x: number): number | null {
  const start = bands[0]?.start;
  const end = bands[bands.length - 1]?.end;
  if (start === undefined || end === undefined || width <= 0) return null;
  return Math.round(start + Math.min(1, Math.max(0, x / width)) * (end - start));
}

const BAND_FILL: Record<SessionKind, keyof ChartTheme> = {
  overnight: "sessionOvernight",
  pre: "sessionPre",
  regular: "sessionRegular",
  after: "sessionAfter",
};

const LONG_LABEL: Record<SessionKind, string> = {
  overnight: "Overnight (BOATS)",
  pre: "Premarket",
  regular: "Regular",
  after: "After",
};

const SHORT_LABEL: Record<SessionKind, string> = { overnight: "Overnight", pre: "Pre", regular: "Regular", after: "After" };

/** Rough text width for fitting labels; renderers cannot be asked from here. */
const textWidth = (s: string, fontSize: number) => s.length * fontSize * 0.56;

export function layoutTimeline(input: TimelineInput, theme: ChartTheme, size: Size): DisplayList {
  const out: DisplayList = [];
  const geo = timelineGeometry(input, size);
  const w = size.width;
  if (w <= 0 || geo.end <= geo.start) return out;
  const x = (t: number) => timelineX(geo, w, t);
  const kindAt = (t: number): SessionKind => input.bands.find((b) => t >= b.start && t < b.end)?.kind ?? "overnight";

  // volume minimap
  let max = 0;
  for (const b of input.buckets) if (b.v > max) max = b.v;
  const slotW = (input.bucketMs / (geo.end - geo.start)) * w;
  for (const b of input.buckets) {
    if (b.v <= 0 || max <= 0) continue;
    // Square-root heights: on a linear scale one burst flattens overnight and
    // premarket to nothing. Relative only; the minimap has no volume axis.
    const h = Math.max(1, Math.sqrt(b.v / max) * geo.minimap.h);
    const past = input.now !== null && b.t < input.now;
    const fill = past ? (kindAt(b.t) === "regular" ? theme.minimapPastRegular : theme.minimapPast) : theme.minimapFuture;
    out.push({ kind: "rect", x: x(b.t), y: geo.minimap.y + geo.minimap.h - h, w: Math.max(1, slotW - 1), h, fill, radius: slotW > 4 ? 1 : 0, layer: "volume" });
  }

  // session bands
  const fs = input.variant === "full" ? 10 : 9;
  for (const band of input.bands) {
    const x0 = x(band.start);
    const x1 = x(band.end);
    out.push({ kind: "rect", x: x0, y: geo.bands.y, w: x1 - x0, h: geo.bands.h, fill: theme[BAND_FILL[band.kind]], layer: "background" });
    const fill = band.kind === "regular" ? theme.sessionTextStrong : theme.sessionText;
    if (input.variant === "full") {
      const long = `${LONG_LABEL[band.kind]} ${formatEtTime(band.start)}–${formatEtTime(band.end)}`;
      const room = x1 - x0 - 12;
      const label = textWidth(long, fs) <= room ? long : textWidth(SHORT_LABEL[band.kind], fs) <= room ? SHORT_LABEL[band.kind] : null;
      if (label) out.push({ kind: "text", x: x0 + 8, y: geo.bands.y + geo.bands.h / 2, text: label, fill, fontSize: fs, fontFamily: theme.fontSans, fontWeight: 400, align: "left", baseline: "middle", layer: "axes" });
    } else if (geo.labelsY !== null) {
      const label = SHORT_LABEL[band.kind];
      const cx = Math.max(textWidth(label, fs) / 2, Math.min(w - textWidth(label, fs) / 2, (x0 + x1) / 2));
      out.push({ kind: "text", x: cx, y: geo.labelsY, text: label, fill: theme.sessionText, fontSize: fs, fontFamily: theme.fontSans, fontWeight: 400, align: "center", baseline: "top", layer: "axes" });
    }
  }

  // replay head
  if (input.now !== null && input.now >= geo.start && input.now <= geo.end) {
    const hx = Math.round(x(input.now));
    out.push({ kind: "rect", x: hx - 1, y: geo.minimap.y - 4, w: 2, h: geo.minimap.h + 8, fill: theme.replayHead, layer: "crosshair" });
    if (input.variant === "full") {
      const label = formatEtTime(input.now);
      const half = textWidth(label, 10) / 2;
      out.push({ kind: "text", x: Math.max(half, Math.min(w - half, hx)), y: 0, text: label, fill: theme.replayHead, fontSize: 10, fontFamily: theme.fontMono, fontWeight: 500, align: "center", baseline: "top", layer: "crosshair" });
    }
  }
  return sortByLayer(out);
}
