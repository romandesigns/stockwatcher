import { describe, expect, test } from "bun:test";
import { anchorXY, clipSegment, measure, ROTATE_HANDLE_GAP, dragDrawing, drawingFromPoints, hitDrawing, indexToT, isColorable, layoutDrawings, tToIndex, type Drawing } from "./drawings.ts";
import { parseDrawing } from "./syncedSettings.ts";
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
    expect(list.some((p) => p.kind === "text" && p.layer === "crosshair")).toBe(true);
    expect(list.filter((p) => p.kind === "rect" && p.radius === 5).length).toBe(2);
  });
});

test("a coloured drawing draws in its colour, selected or not; a range fills with it", () => {
  const a = { x: slotX(model.index, 8), y: 60 };
  const b = { x: slotX(model.index, 14), y: 120 };
  const line = { ...drawingFromPoints(model, "c1", "trend", a, b)!, color: "#d55181" };
  for (const sel of [null, "c1"]) {
    const list = layoutDrawings(model, [line], sel, DEFAULT_CHART_THEME);
    expect(list.some((p) => p.kind === "line" && p.stroke === "#d55181")).toBe(true);
  }
  const range = { ...drawingFromPoints(model, "r1", "rect", a, b)!, color: "#3987e5" };
  // A range is drawn as a closed path, so it can be turned.
  const fill = layoutDrawings(model, [range], null, DEFAULT_CHART_THEME).find((p) => p.kind === "path");
  expect(fill && "fill" in fill ? fill.fill : "").toBe("rgba(57, 135, 229, 0.14)");
});

describe("rotating a range", () => {
  const a = { x: slotX(model.index, 6), y: 60 };
  const b = { x: slotX(model.index, 14), y: 140 };
  const box = drawingFromPoints(model, "r", "rect", a, b)!;
  const pa = anchorXY(model, box.a)!;
  const pb = anchorXY(model, box.b)!;
  const c = { x: (pa.x + pb.x) / 2, y: (pa.y + pb.y) / 2 };
  const top = Math.min(pa.y, pb.y);

  test("selected, it has a rotate handle above its top edge; unselected, it does not", () => {
    expect(hitDrawing(model, [box], c.x, top - ROTATE_HANDLE_GAP, 6, "r")).toEqual({ id: "r", part: "rotate" });
    expect(hitDrawing(model, [box], c.x, top - ROTATE_HANDLE_GAP, 6, null)).toBeNull();
  });

  test("dragging the handle turns it about its centre, snapping near 15 degree steps", () => {
    const right = dragDrawing(model, box, "rotate", { x: c.x, y: top - ROTATE_HANDLE_GAP }, { x: c.x + 100, y: c.y + 2 });
    expect(right.angle).toBeCloseTo(Math.PI / 2, 9);
    expect(right.a).toEqual(box.a);
    // About 37.6 degrees: more than 4 degrees from both 30 and 45, so it does not snap.
    const free = dragDrawing(model, box, "rotate", { x: 0, y: 0 }, { x: c.x + 100, y: c.y - 130 });
    expect(free.angle! * (180 / Math.PI)).toBeCloseTo(90 - (Math.atan2(130, 100) * 180) / Math.PI, 6);
    // About 59 degrees is within 4 of 60, so it snaps there.
    const near = dragDrawing(model, box, "rotate", { x: 0, y: 0 }, { x: c.x + 100, y: c.y - 60 });
    expect(near.angle! * (180 / Math.PI)).toBeCloseTo(60, 9);
  });

  test("hit testing follows the turn: a turned box covers what the plain one did not", () => {
    const turned = { ...box, angle: Math.PI / 4 };
    const hw = Math.abs(pb.x - pa.x) / 2;
    // The plain box's corner region is outside once turned 45 degrees...
    expect(hitDrawing(model, [turned], c.x + hw - 1, top + 1, 0)).toBeNull();
    // ...and the centre is always inside.
    expect(hitDrawing(model, [turned], c.x, c.y, 0)?.part).toBe("body");
  });

  test("drawn as a turned closed path, with the angle shown while selected", () => {
    const list = layoutDrawings(model, [{ ...box, angle: Math.PI / 6 }], "r", DEFAULT_CHART_THEME);
    const path = list.find((p) => p.kind === "path");
    expect(path && "points" in path ? path.points.length : 0).toBe(8);
    expect(list.some((p) => p.kind === "text" && p.text === "30°")).toBe(true);
  });
});

describe("measure", () => {
  test("reads change, percent, bars and time between two points", () => {
    const d = drawingFromPoints(model, "m", "measure", { x: slotX(model.index, 4), y: 150 }, { x: slotX(model.index, 12), y: 70 })!;
    const m = measure(model, d);
    expect(m.bars).toBe(8);
    expect(m.ms).toBe(8 * 60_000); // the one-interval gap at slot 5 is one minute, like a candle
    expect(m.change).toBeGreaterThan(0);
    const list = layoutDrawings(model, [d], null, DEFAULT_CHART_THEME);
    expect(list.some((p) => p.kind === "text" && p.text.includes("bars"))).toBe(true);
  });
});

describe("supply and demand zones", () => {
  const a = { x: slotX(model.index, 4), y: 60 };
  const b = { x: slotX(model.index, 7), y: 100 };
  const supply = drawingFromPoints(model, "z1", "supply", a, b)!;
  const plot = model.geometry.plot;

  test("a zone runs from its earlier anchor to the plot's right edge, tinted by its kind and labelled", () => {
    const list = layoutDrawings(model, [supply], null, DEFAULT_CHART_THEME);
    const band = list.find((p) => p.kind === "rect" && p.layer === "overlay");
    expect(band && band.kind === "rect" ? [band.x, band.x + band.w] : null).toEqual([a.x, plot.x + plot.w]);
    expect(band && band.kind === "rect" ? band.fill : "").toBe("rgba(240, 100, 92, 0.16)");
    expect(list.some((p) => p.kind === "text" && p.text.startsWith("Supply "))).toBe(true);
    const demand = { ...supply, id: "z2", kind: "demand" as const };
    const fill = layoutDrawings(model, [demand], null, DEFAULT_CHART_THEME).find((p) => p.kind === "rect");
    expect(fill && fill.kind === "rect" ? fill.fill : "").toBe("rgba(63, 185, 122, 0.16)");
  });

  test("the band is hit to the right of where it was drawn; selected, both edges get axis tags", () => {
    expect(hitDrawing(model, [supply], slotX(model.index, 14), 80, 6)).toEqual({ id: "z1", part: "body" });
    expect(hitDrawing(model, [supply], slotX(model.index, 14), 140, 6)).toBeNull();
    const tags = layoutDrawings(model, [supply], "z1", DEFAULT_CHART_THEME).filter((p) => p.kind === "text" && p.layer === "crosshair");
    expect(tags.length).toBe(2);
  });

  test("a zone keeps its hue (not recolourable) and survives the sync parser", () => {
    expect(isColorable(supply)).toBe(false);
    expect(parseDrawing(JSON.parse(JSON.stringify(supply)))).toEqual(supply);
  });
});
