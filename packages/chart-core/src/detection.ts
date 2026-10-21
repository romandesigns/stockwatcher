// Read-only annotations from the Rust detector. These are presentation coordinates, never
// user drawings, alert instructions, or a second detector. Prices are dollars; times are ms.
import type { ChartTheme, DisplayList } from "./displayList.ts";
import { tToIndex } from "./drawings.ts";
import { formatPrice } from "./format.ts";
import type { ChartModel } from "./layout.ts";
import { priceY, slotX } from "./scale.ts";

export interface DetectionOverlayLevel {
  id: string;
  version: number;
  knownAt: number;
  armedAt: number | null;
  validTo: number | null;
  trigger: number;
  invalidation: number | null;
}

export interface DetectionOverlay {
  episodeId: string;
  label: string;
  recognizedAt: number;
  terminal: boolean;
  levels: readonly DetectionOverlayLevel[];
}

/** Annotation positions use the actual recognition/known time, even on a larger display bar.
 * They are never backdated to a pivot or to that display bar's opening time. */
export function layoutDetections(model: ChartModel, episodes: readonly DetectionOverlay[], theme: ChartTheme): DisplayList {
  if (!model.price || !model.slots.length) return [];
  const out: DisplayList = [];
  const plot = model.geometry.plot;
  const right = plot.x + plot.w;
  const fs = model.options.fontSize;
  const xAt = (t: number) => slotX(model.index, tToIndex(model.slots, t, model.timeframeMs)) - model.index.barSpacing / 2;
  const text = (x: number, y: number, label: string, fill: string) => {
    out.push({ kind: "text", x, y, text: label, fill, fontSize: fs, fontFamily: theme.fontMono, fontWeight: 600, align: "left", baseline: "bottom", layer: "overlay" });
  };
  for (const e of episodes) {
    const recognizedX = xAt(e.recognizedAt);
    const color = e.terminal ? theme.textDim : theme.orderBuy;
    if (recognizedX >= plot.x && recognizedX <= right) {
      out.push({ kind: "line", x1: recognizedX, y1: plot.y + model.options.legendTop + 22, x2: recognizedX, y2: plot.y + plot.h, stroke: color, lineWidth: 1, dash: [2, 4], clip: plot, layer: "overlay" });
      text(Math.min(recognizedX + 4, Math.max(plot.x, right - 160)), plot.y + model.options.legendTop + 20, `${e.label} · experimental`, color);
    }
    for (const level of e.levels) {
      // A version is visible only from when it was known. Its predecessor stops at the
      // supplied validity end; a chart timeframe change cannot extend a level backwards.
      const start = xAt(level.knownAt);
      const end = level.validTo === null ? right : xAt(level.validTo);
      if (end < plot.x || start > right || end < start) continue;
      const x1 = Math.max(plot.x, start);
      const x2 = Math.min(right, end);
      for (const [name, price, stroke] of [["T", level.trigger, color], ["I", level.invalidation, theme.orderSell]] as const) {
        if (price === null || !Number.isFinite(price) || price <= 0) continue;
        const y = priceY(model.price, price);
        if (y < plot.y || y > plot.y + plot.h) continue;
        out.push({ kind: "line", x1, y1: y, x2, y2: y, stroke, lineWidth: 1, dash: name === "T" ? [6, 3] : [2, 3], clip: plot, layer: "overlay" });
        if (x2 - x1 > 90) text(x1 + 4, y - 3, `${name} ${formatPrice(price)} · v${level.version}`, stroke);
      }
    }
  }
  return out;
}
