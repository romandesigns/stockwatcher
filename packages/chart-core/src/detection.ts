// Read-only annotations from the Rust detector. These are presentation coordinates, never
// user drawings, alert instructions, or a second detector. Prices are dollars; times are ms.
import type { ChartTheme, DisplayList } from "./displayList.ts";
import { tToIndex } from "./drawings.ts";
import { formatPrice } from "./format.ts";
import type { ChartModel } from "./layout.ts";
import { priceY, slotX } from "./scale.ts";
import { withAlpha } from "./theme.ts";

export interface DetectionOverlayLevel {
  id: string;
  kind?: "trigger" | "range_high";
  version: number;
  knownAt: number;
  armedAt: number | null;
  validTo: number | null;
  trigger: number;
  invalidation: number | null;
}

export interface DetectionOverlay {
  episodeId: string;
  /** For the host's own caption and accessibility text; the chart itself draws no label. */
  label: string;
  recognizedAt: number;
  terminal: boolean;
  stale?: boolean;
  levels: readonly DetectionOverlayLevel[];
}

/** Detector prices are exact; the tag keeps every significant digit the engine gave (2.700001). */
const exact = (p: number) => {
  const full = p.toFixed(6).replace(/0+$/, "").replace(/\.$/, "");
  return full.length > formatPrice(p).length ? full : formatPrice(p);
};

/**
 * The selected Signals row's episode on the chart (design canvas "Detection Panel", 2026-10-22):
 * a faint event-start marker at the recognition time and faint trigger / invalidation lines,
 * drawn like the key levels (a quiet dashed line plus a small price tag, no words). The trigger
 * is neutral, never a "go" green: on the holdout a trigger observed failed about three times in
 * four; only the invalidation carries a hue (the critical red, faint). A resolved episode draws
 * nothing (the host passes none: `detectionOverlays`); a stale one dims to the level grey.
 *
 * Annotation positions use the actual recognition/known time, even on a larger display bar.
 * They are never backdated to a pivot or to that display bar's opening time, and a level is
 * drawn only from when it was known.
 */
export function layoutDetections(model: ChartModel, episodes: readonly DetectionOverlay[], theme: ChartTheme): DisplayList {
  if (!model.price || !model.slots.length) return [];
  const out: DisplayList = [];
  const plot = model.geometry.plot;
  const right = plot.x + plot.w;
  const fs = model.options.fontSize;
  const xAt = (t: number) => slotX(model.index, tToIndex(model.slots, t, model.timeframeMs)) - model.index.barSpacing / 2;
  for (const e of episodes) {
    if (e.terminal) continue;
    const quiet = e.stale === true;
    const recognizedX = xAt(e.recognizedAt);
    if (recognizedX >= plot.x && recognizedX <= right) {
      out.push({ kind: "line", x1: recognizedX, y1: plot.y + model.options.legendTop + 22, x2: recognizedX, y2: plot.y + plot.h, stroke: theme.level, lineWidth: 1, dash: [2, 4], clip: plot, layer: "overlay" });
    }
    for (const level of e.levels) {
      // A version is visible only from when it was known. Its predecessor stops at the
      // supplied validity end; a chart timeframe change cannot extend a level backwards.
      const start = xAt(level.knownAt);
      const end = level.validTo === null ? right : xAt(level.validTo);
      if (end < plot.x || start > right || end < start) continue;
      const x1 = Math.max(plot.x, start);
      const x2 = Math.min(right, end);
      const lines = [
        [level.trigger, quiet ? theme.level : theme.levelStrong, theme.textDim, [6, 3]],
        [level.invalidation, quiet ? theme.level : withAlpha(theme.orderSell, 0.45), quiet ? theme.textDim : withAlpha(theme.orderSell, 0.85), [2, 3]],
      ] as const;
      for (const [price, stroke, tagText, dash] of lines) {
        if (price === null || !Number.isFinite(price) || price <= 0) continue;
        const y = Math.round(priceY(model.price, price)) + 0.5;
        if (y < plot.y || y > plot.y + plot.h) continue;
        out.push({ kind: "line", x1, y1: y, x2, y2: y, stroke, lineWidth: 1, dash: [...dash], clip: plot, layer: "overlay" });
        // The price tag sits at the line's start (where the level became known), the key
        // levels' tag style; a line too short to hold it goes untagged rather than overflow.
        const text = exact(price);
        const w = Math.ceil(text.length * fs * 0.62) + 10;
        if (x2 - x1 < w + 8) continue;
        out.push({ kind: "rect", x: x1 + 4, y: y - 8, w, h: 16, radius: 3, fill: withAlpha(theme.crosshairLabelBg, 0.92), layer: "axes" });
        out.push({ kind: "text", x: x1 + 9, y, text, fill: tagText, fontSize: fs, fontFamily: theme.fontMono, fontWeight: 500, align: "left", baseline: "middle", layer: "axes" });
      }
    }
  }
  return out;
}
