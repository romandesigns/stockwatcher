import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { canAlertOnDrawing, drawingAlertParts, drawingCueStates, drawingLevelAt, glowAlpha, isTouching, layoutDrawingCues, parseDrawingTolerance, toleranceDollars, toleranceText, type DrawingAlertPart, type DrawingCueLayer, type WatchedDrawing } from "./drawingAlerts.ts";
import type { Drawing } from "./drawings.ts";
import { computeChartModel, layoutChart, type ChartState } from "./layout.ts";
import { defaultViewport, manualPrice } from "./scale.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";
import { fixtureSnapshot } from "./testFixture.ts";

// The server reads the same file (crates/server/src/alerts/geometry.rs golden_fixtures_match):
// a sloped line must trigger at the price the chart draws.
const FIXTURES = new URL("../../../crates/server/src/alerts/drawing_alert_fixtures.json", import.meta.url);

interface Case {
  name: string;
  tfMs: number;
  slots: number[];
  drawing: Drawing;
  queries: { t: number; part: DrawingAlertPart; expect: number | null }[];
}

describe("drawing level at a candle (cross-language fixtures)", () => {
  const cases = (JSON.parse(readFileSync(FIXTURES, "utf8")) as { cases: Case[] }).cases;
  test("every golden case matches the Rust evaluator's", () => {
    let n = 0;
    for (const c of cases) {
      const slots = c.slots.map((t) => ({ t }));
      for (const q of c.queries) {
        const got = drawingLevelAt(c.drawing, q.part, slots, q.t, c.tfMs);
        if (q.expect === null) expect([c.name, got]).toEqual([c.name, null]);
        else {
          expect(got).not.toBeNull();
          expect(Math.abs(got! - q.expect)).toBeLessThan(1e-9);
        }
        n++;
      }
    }
    expect(n).toBeGreaterThanOrEqual(20);
  });
});

describe("tolerance", () => {
  test("parses and clamps like the server, measures on the level's grid", () => {
    expect(parseDrawingTolerance(null)).toEqual({ unit: "ticks", value: 1 });
    expect(parseDrawingTolerance({ unit: "ticks", value: 500 })).toEqual({ unit: "ticks", value: 100 });
    expect(parseDrawingTolerance({ unit: "pct", value: 0.5 })).toEqual({ unit: "pct", value: 0.5 });
    expect(parseDrawingTolerance({ unit: "pct", value: -1 })).toEqual({ unit: "ticks", value: 1 });
    expect(toleranceDollars({ unit: "ticks", value: 2 }, 0.45)).toBeCloseTo(0.0002, 12);
    expect(isTouching(1.86, 1.85, { unit: "ticks", value: 1 })).toBe(true);
    expect(isTouching(1.87, 1.85, { unit: "ticks", value: 1 })).toBe(false);
    expect(isTouching(2.01, 2, { unit: "pct", value: 0.5 })).toBe(true);
    expect(toleranceText({ unit: "ticks", value: 1 })).toBe("±1 tick");
    expect(toleranceText({ unit: "pct", value: 0.5 })).toBe("±0.5%");
  });

  test("which drawings and levels can be watched", () => {
    expect(canAlertOnDrawing({ kind: "hline" })).toBe(true);
    expect(canAlertOnDrawing({ kind: "rect" })).toBe(false);
    expect(canAlertOnDrawing({ kind: "vline" })).toBe(false);
    expect(drawingAlertParts({ kind: "long" })).toEqual(["stop", "target", "entry"]);
    expect(drawingAlertParts({ kind: "ray" })).toEqual(["line"]);
    expect(drawingAlertParts({ kind: "supply" })).toEqual([]);
  });
});

describe("the cue", () => {
  // 400×300, a manual range 2.00–3.00 (alerts.test.ts's frame).
  const snap = fixtureSnapshot("1m", 20, [5]);
  const lastT = snap.candles.at(-1)!.t;
  const hline: Drawing = { id: "h", kind: "hline", a: { t: lastT, p: 2.5 }, b: { t: lastT, p: 2.5 } };
  const pos: Drawing = { id: "p", kind: "long", a: { t: snap.candles[10]!.t, p: 2.6 }, b: { t: lastT, p: 2.9 }, c: { t: lastT, p: 2.4 } };
  const state = (cues: DrawingCueLayer | null): ChartState => ({ timeframe: "1m", candles: snap.candles, gaps: snap.gaps, profile: snap.profile, inspectT: null, pointer: null, drawings: [hline, pos], drawingCues: cues });
  const model = computeChartModel(state(null), manualPrice(defaultViewport(16), { lo: 2, hi: 3 }), { width: 400, height: 300 });
  const watch = (extra: Partial<WatchedDrawing> = {}): WatchedDrawing => ({ alertId: "a", drawingId: "h", part: "line", tol: { unit: "ticks", value: 1 }, ...extra });
  const texts = (l: ReturnType<typeof layoutDrawingCues>) => l.flatMap((p) => (p.kind === "text" ? [p.text] : []));

  test("touching: a glow on the drawing and a pill on its axis tag", () => {
    const l = layoutDrawingCues(model, [hline, pos], { watched: [watch()], last: 2.51, pulse: 0.25 }, DEFAULT_CHART_THEME);
    expect(texts(l)).toContain("touching");
    const glow = l.find((p) => p.kind === "line" && p.lineWidth === 8);
    expect(glow).toBeDefined();
    expect(glow!.kind === "line" && glow!.stroke).toContain("rgba(139, 156, 255, 0.42");
  });

  test("not touching: only the watched bell; muted never glows", () => {
    const far = layoutDrawingCues(model, [hline], { watched: [watch()], last: 2.53, pulse: 0 }, DEFAULT_CHART_THEME);
    expect(texts(far)).toEqual([]);
    expect(far.some((p) => p.kind === "path")).toBe(true);
    const muted = layoutDrawingCues(model, [hline], { watched: [watch({ muted: true })], last: 2.5, pulse: 0 }, DEFAULT_CHART_THEME);
    expect(texts(muted)).toEqual([]);
    expect(muted.some((p) => p.kind === "line")).toBe(false);
  });

  test("a position's stop glows in the down hue and says so", () => {
    const l = layoutDrawingCues(model, [pos], { watched: [watch({ drawingId: "p", part: "stop" })], last: 2.4, pulse: null }, DEFAULT_CHART_THEME);
    expect(texts(l)).toContain("touching stop");
    const glow = l.find((p) => p.kind === "line" && p.lineWidth === 2);
    expect(glow!.kind === "line" && glow!.stroke).toBe(DEFAULT_CHART_THEME.down);
  });

  test("a fire stays marked on its candle until acknowledged; the flash expands and fades", () => {
    const fired = { t: lastT + 5000, price: 2.5, tone: "down" as const };
    const now = layoutDrawingCues(model, [hline], { watched: [watch({ fired })], last: 2.7, pulse: null, flash: 1 }, DEFAULT_CHART_THEME);
    const later = layoutDrawingCues(model, [hline], { watched: [watch({ fired })], last: 2.7, pulse: null, flash: 0 }, DEFAULT_CHART_THEME);
    const rings = (l: typeof now) => l.filter((p) => p.kind === "rect" && p.radius !== undefined && p.w === p.h);
    expect(rings(now).length).toBe(rings(later).length + 1);
    expect(rings(later).length).toBe(3);
  });

  test("reduced motion: a steady glow; the pulse stays within its band", () => {
    expect(glowAlpha(null)).toBe(0.32);
    for (const ph of [0, 0.25, 0.5, 0.75]) {
      expect(glowAlpha(ph)).toBeGreaterThanOrEqual(0.18 - 1e-9);
      expect(glowAlpha(ph)).toBeLessThanOrEqual(0.42 + 1e-9);
    }
  });

  test("states for the host, and the cue reaches the chart's display list", () => {
    const st = drawingCueStates([hline, pos], [watch(), watch({ alertId: "b", drawingId: "gone" })], model.slots, model.timeframeMs, 2.5);
    expect(st).toEqual([
      { alertId: "a", level: 2.5, touching: true },
      { alertId: "b", level: null, touching: false },
    ]);
    const list = layoutChart(model, state({ watched: [watch()], last: 2.5, pulse: null }), DEFAULT_CHART_THEME);
    expect(list.some((p) => p.kind === "text" && p.text === "touching")).toBe(true);
  });
});
