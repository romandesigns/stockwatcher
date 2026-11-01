import { describe, expect, test } from "bun:test";
import { tToIndex } from "./drawings.ts";
import { layoutEventMarkers, type EventMarker } from "./eventMarkers.ts";
import { computeChartModel, layout, type ChartState } from "./layout.ts";
import { defaultViewport, manualPrice, priceY, slotX } from "./scale.ts";
import { fixtureSnapshot } from "./testFixture.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";

const snapshot = fixtureSnapshot("1m", 20, []);
const state: ChartState = { timeframe: "1m", candles: snapshot.candles, gaps: snapshot.gaps, profile: null, inspectT: null, pointer: null };
const vp = manualPrice(defaultViewport(16), { lo: 2, hi: 3 });
const size = { width: 700, height: 400 };
const at = snapshot.candles[8]!.t + 17_000;
const alert: EventMarker = { kind: "alert", at, price: 2.5, n: 3 };
const missed: EventMarker = { kind: "missed", at: at + 60_000, price: 2.6 };

describe("server-placed event markers (Insights ticker page)", () => {
  test("an alert is a numbered accent dot at its own time and price, not its bar's open", () => {
    const model = computeChartModel(state, vp, size);
    const list = layoutEventMarkers(model, [alert], DEFAULT_CHART_THEME);
    const dot = list.find((p) => p.kind === "rect");
    const x = slotX(model.index, tToIndex(model.slots, at, model.timeframeMs)) - model.index.barSpacing / 2;
    expect(dot?.kind === "rect" && dot.x + dot.w / 2).toBeCloseTo(x);
    expect(dot?.kind === "rect" && dot.y + dot.h / 2).toBeCloseTo(priceY(model.price!, 2.5));
    expect(dot?.kind === "rect" && dot.fill).toBe(DEFAULT_CHART_THEME.alert);
    expect(list.flatMap((p) => (p.kind === "text" ? [p.text] : []))).toEqual(["3"]);
  });

  test("a missed setup is an unfilled-looking ring, drawn under the alerts; off-plot markers are skipped, never clamped", () => {
    const model = computeChartModel(state, vp, size);
    const list = layoutEventMarkers(model, [alert, missed, { kind: "alert", at, price: 9, n: 1 }], DEFAULT_CHART_THEME);
    const rects = list.filter((p) => p.kind === "rect");
    expect(rects).toHaveLength(2);
    expect(rects[0]?.kind === "rect" && rects[0].stroke).toBe(DEFAULT_CHART_THEME.textDim);
    expect(rects[1]?.kind === "rect" && rects[1].fill).toBe(DEFAULT_CHART_THEME.alert);
  });

  test("markers in the same bar never cover each other: the later one is lifted, with a leader to its point", () => {
    const model = computeChartModel(state, vp, size);
    const two: EventMarker[] = [{ kind: "alert", at, price: 2.5, n: 1 }, { kind: "alert", at: at + 21_000, price: 2.51, n: 2 }];
    const list = layoutEventMarkers(model, two, DEFAULT_CHART_THEME);
    const dots = list.flatMap((p) => (p.kind === "rect" ? [{ x: p.x + p.w / 2, y: p.y + p.h / 2, r: p.w / 2 }] : []));
    expect(dots).toHaveLength(2);
    expect(dots[0]!.y).toBeCloseTo(priceY(model.price!, 2.5));
    expect(Math.hypot(dots[0]!.x - dots[1]!.x, dots[0]!.y - dots[1]!.y)).toBeGreaterThanOrEqual(dots[0]!.r * 2);
    expect(dots[1]!.y).toBeLessThan(dots[0]!.y);
    const leader = list.find((p) => p.kind === "line");
    expect(leader?.kind === "line" && leader.y1).toBeCloseTo(priceY(model.price!, 2.51));
    // Apart already: nothing moves and no leader is drawn.
    const apart = layoutEventMarkers(model, [alert, missed], DEFAULT_CHART_THEME);
    expect(apart.some((p) => p.kind === "line")).toBe(false);
  });

  test("the chart layout draws them from its state", () => {
    const list = layout({ ...state, eventMarkers: [alert] }, vp, DEFAULT_CHART_THEME, size);
    expect(list.some((p) => p.kind === "text" && p.text === "3" && p.layer === "overlay")).toBe(true);
  });
});
