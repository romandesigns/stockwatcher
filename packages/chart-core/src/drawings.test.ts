import { describe, expect, test } from "bun:test";
import { anchorXY, clipSegment, dragDrawing, drawingFromPoints, hitDrawing, indexToT, layoutDrawings, tToIndex, type Drawing } from "./drawings.ts";
import { computeChartModel, type ChartState } from "./layout.ts";
import { defaultViewport, slotX } from "./scale.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";
import { fixtureSnapshot } from "./testFixture.ts";

const snap = fixtureSnapshot("1m", 20, [5]);
const state: ChartState = { timeframe: "1m", candles: snap.candles, gaps: snap.gaps, profile: snap.profile, inspectT: null, pointer: null };
const model = computeChartModel(state, defaultViewport(16), { width: 400, height: 300 });
const slots = model.slots;
const TF = 60_000;

describe("time ↔ fractional slot index", () => {
  test("exact slot times map to their index and back", () => {
    for (const i of [0, 3, 5, 19]) {
      expect(tToIndex(slots, slots[i]!.t, TF)).toBe(i);
      expect(indexToT(slots, i, TF)).toBe(slots[i]!.t);
    }
  });

  test("between slots interpolates; past either end extrapolates per interval", () => {
    const mid = (slots[2]!.t + slots[3]!.t) / 2;
    expect(tToIndex(slots, mid, TF)).toBeCloseTo(2.5, 9);
    expect(tToIndex(slots, slots[19]!.t + 3 * TF, TF)).toBeCloseTo(22, 9);
    expect(indexToT(slots, -2, TF)).toBe(slots[0]!.t - 2 * TF);
  });
});

describe("clipSegment", () => {
  const r = { x: 0, y: 0, w: 10, h: 10 };
  test("clips a crossing segment to the rect and drops one fully outside", () => {
    expect(clipSegment({ x: -5, y: 5 }, { x: 15, y: 5 }, r)).toEqual([
      { x: 0, y: 5 },
      { x: 10, y: 5 },
    ]);
    expect(clipSegment({ x: -5, y: -5 }, { x: -1, y: -1 }, r)).toBeNull();
  });
});

describe("drawings on the chart", () => {
  const a = { x: slotX(model.index, 8), y: 60 };
  const b = { x: slotX(model.index, 14), y: 120 };
  const trend = { ...drawingFromPoints(model, "t1", "trend", a, b)! };

  test("anchors are snapped to slots and round-trip to the same pixels", () => {
    expect(trend.a.t).toBe(slots[8]!.t);
    const pa = anchorXY(model, trend.a)!;
    expect(pa.x).toBeCloseTo(a.x, 6);
    expect(pa.y).toBeCloseTo(a.y, 6);
  });

  test("hit testing finds handles before the body, and misses empty space", () => {
    expect(hitDrawing(model, [trend], a.x, a.y, 6)).toEqual({ id: "t1", part: "a" });
    const mx = (a.x + b.x) / 2;
    const my = (a.y + b.y) / 2;
    expect(hitDrawing(model, [trend], mx, my + 2, 6)).toEqual({ id: "t1", part: "body" });
    expect(hitDrawing(model, [trend], mx, my + 40, 6)).toBeNull();
  });

  test("dragging the body moves both anchors by the same slots and pixels", () => {
    const moved = dragDrawing(model, trend, "body", { x: 100, y: 100 }, { x: 132, y: 90 });
    expect(tToIndex(slots, moved.a.t, TF)).toBeCloseTo(10, 6);
    expect(tToIndex(slots, moved.b.t, TF)).toBeCloseTo(16, 6);
    expect(anchorXY(model, moved.a)!.y).toBeCloseTo(a.y - 10, 6);
  });

  test("dragging a handle moves only that anchor", () => {
    const moved = dragDrawing(model, trend, "b", b, { x: slotX(model.index, 17), y: 30 });
    expect(moved.a).toEqual(trend.a);
    expect(moved.b.t).toBe(slots[17]!.t);
  });

  test("a level draws across the plot with a tag on the axis; the selected trend gets handles", () => {
    const level: Drawing = { id: "h", kind: "hline", a: trend.a, b: trend.a };
    const list = layoutDrawings(model, [level, trend], "t1", DEFAULT_CHART_THEME);
    expect(list.some((p) => p.kind === "line" && p.x1 === 0 && p.x2 === model.geometry.plot.w)).toBe(true);
    expect(list.some((p) => p.kind === "text" && p.layer === "axes")).toBe(true);
    expect(list.filter((p) => p.kind === "rect" && p.radius === 5).length).toBe(2);
  });
});
