// Read-only event markers placed by the server (the Insights ticker page, crates/server/src/
// insights): each SENT alert as a numbered dot at its reference price and time, each missed setup
// as a hollow ring at its arming. The server decides what is marked, where and with which number;
// this only turns its (time, price) into pixels. Prices are dollars, times epoch ms.
import type { ChartTheme, DisplayList } from "./displayList.ts";
import { tToIndex } from "./drawings.ts";
import type { ChartModel } from "./layout.ts";
import { priceY, slotX } from "./scale.ts";

export interface EventMarker {
  /** `alert`: a sent notice (filled, numbered); `missed`: a setup that was not notified (a ring). */
  kind: "alert" | "missed";
  /** Epoch ms (the event's own time, not its bar's open). */
  at: number;
  price: number;
  /** The alert's number, as in the page's notification history. */
  n?: number;
  /** Drawn larger with a ring around it (the row the person is pointing at). */
  focused?: boolean;
}

/** Marker radius in px: the dot holds a two-digit number at the chart's font size. */
export function eventMarkerRadius(fontSize: number): number {
  return Math.max(7, Math.round(fontSize * 0.8));
}

/**
 * The markers over the candles: rings first, so a numbered alert on the same spot stays readable.
 * Off-plot markers are skipped (never clamped to an edge: a dot at the wrong price is worse than
 * none). The alert's fill is the accent (`theme.alert`, the colour that already means "a thing
 * you set to catch your eye"); a ring is the quiet level grey on the chart background.
 *
 * Markers never cover each other: two alerts 20 s apart share a 1-minute bar (INO's 1 and 2), and
 * at a zoomed-out width neighbours do too. A marker that would overlap one already placed is
 * lifted a dot at a time until it is clear (lowered only when the plot has no room above), oldest first,
 * and a short leader joins it to its true time and price, so the number stays readable and the
 * point it marks is still exact.
 */
export function layoutEventMarkers(model: ChartModel, markers: readonly EventMarker[], theme: ChartTheme): DisplayList {
  if (!model.price || !model.slots.length || markers.length === 0) return [];
  const out: DisplayList = [];
  const plot = model.geometry.plot;
  const fs = model.options.fontSize;
  const r = eventMarkerRadius(fs);
  const xAt = (t: number) => slotX(model.index, tToIndex(model.slots, t, model.timeframeMs)) - model.index.barSpacing / 2;
  const ordered = [...markers].sort((a, b) => (a.kind === b.kind ? a.at - b.at : a.kind === "missed" ? -1 : 1));
  const placed: { x: number; y: number; r: number }[] = [];
  const clear = (x: number, y: number, rr: number) => placed.every((p) => Math.hypot(p.x - x, p.y - y) >= p.r + rr + 2);
  for (const m of ordered) {
    if (!Number.isFinite(m.price) || m.price <= 0 || !Number.isFinite(m.at)) continue;
    const x = xAt(m.at);
    const y0 = priceY(model.price, m.price);
    if (x < plot.x - r || x > plot.x + plot.w + r || y0 < plot.y - r || y0 > plot.y + plot.h + r) continue;
    const rr = m.focused ? r + 2 : r;
    let y = y0;
    if (!clear(x, y, rr)) {
      const step = rr + 2;
      const spots: number[] = [];
      // Above first, as far as it takes (a run's markers climb with it); below only when the top is full.
      for (let k = 1; k <= 8; k++) spots.push(y0 - k * step);
      for (let k = 1; k <= 8; k++) spots.push(y0 + k * step);
      y = spots.find((c) => c - rr >= plot.y && c + rr <= plot.y + plot.h && clear(x, c, rr)) ?? y0;
    }
    placed.push({ x, y, r: rr });
    if (y !== y0) {
      const dir = Math.sign(y - y0);
      out.push({ kind: "line", x1: x, y1: y0, x2: x, y2: y - dir * rr, stroke: m.kind === "alert" ? theme.alert : theme.textDim, lineWidth: 1.5, layer: "overlay" });
    }
    if (m.focused) out.push({ kind: "rect", x: x - rr - 3, y: y - rr - 3, w: (rr + 3) * 2, h: (rr + 3) * 2, radius: rr + 3, fill: theme.crosshairLabelBg, layer: "overlay" });
    if (m.kind === "missed") {
      const ring = Math.max(5, rr - 2);
      out.push({ kind: "rect", x: x - ring, y: y - ring, w: ring * 2, h: ring * 2, radius: ring, fill: theme.handleFill, stroke: theme.textDim, lineWidth: 2, layer: "overlay" });
      continue;
    }
    out.push({ kind: "rect", x: x - rr, y: y - rr, w: rr * 2, h: rr * 2, radius: rr, fill: theme.alert, layer: "overlay" });
    if (m.n !== undefined) out.push({ kind: "text", x, y: y + 0.5, text: String(m.n), fill: theme.onAlert, fontSize: m.n >= 10 ? fs - 2 : fs - 1, fontFamily: theme.fontSans, fontWeight: 700, align: "center", baseline: "middle", layer: "overlay" });
  }
  return out;
}
