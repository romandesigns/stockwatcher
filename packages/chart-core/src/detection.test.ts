import { describe, expect, test } from "bun:test";
import { layoutDetections, type DetectionOverlay } from "./detection.ts";
import { tToIndex } from "./drawings.ts";
import { computeChartModel, type ChartState } from "./layout.ts";
import { defaultViewport, manualPrice, slotX } from "./scale.ts";
import { fixtureSnapshot } from "./testFixture.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";

const snapshot = fixtureSnapshot("1m", 20, []);
const state: ChartState = { timeframe: "1m", candles: snapshot.candles, gaps: snapshot.gaps, profile: null, inspectT: null, pointer: null };
const vp = manualPrice(defaultViewport(16), { lo: 2, hi: 3 });
const size = { width: 700, height: 400 };
const at = snapshot.candles[8]!.t + 17_000;
const episode: DetectionOverlay = { episodeId: "fixture/micro/1", label: "Micro-pullback · armed", recognizedAt: at, terminal: false, levels: [{ id: "fixture/level/1", version: 1, knownAt: at, armedAt: at, validTo: null, trigger: 2.7, invalidation: 2.4 }] };

describe("authoritative detection annotations", () => {
  test("a known level begins at its knowledge time inside the display bar", () => {
    const model = computeChartModel(state, vp, size);
    const list = layoutDetections(model, [episode], DEFAULT_CHART_THEME);
    const expectedX = slotX(model.index, tToIndex(model.slots, at, model.timeframeMs)) - model.index.barSpacing / 2;
    const horizontal = list.filter((p) => p.kind === "line" && p.y1 === p.y2);
    expect(horizontal).toHaveLength(2);
    for (const line of horizontal) if (line.kind === "line") expect(line.x1).toBeCloseTo(Math.max(model.geometry.plot.x, expectedX));
    expect(episode.levels[0]!.knownAt).toBe(at);
  });

  test("marks are faint and unlabelled: an event-start line and two level lines with only price tags", () => {
    const model = computeChartModel(state, vp, size);
    const list = layoutDetections(model, [episode], DEFAULT_CHART_THEME);
    const vertical = list.filter((p) => p.kind === "line" && p.x1 === p.x2);
    expect(vertical).toHaveLength(1);
    const texts = list.flatMap((p) => (p.kind === "text" ? [p.text] : []));
    // Price tags only: no family/state label, no "T"/"I", no version, no "experimental".
    expect(texts.sort()).toEqual(["2.40", "2.70"]);
    const strokes = list.flatMap((p) => (p.kind === "line" ? [p.stroke] : []));
    // The trigger is a neutral key-level grey (never a "go" green); only the invalidation is red, faint.
    expect(strokes).toContain(DEFAULT_CHART_THEME.levelStrong);
    expect(strokes).not.toContain(DEFAULT_CHART_THEME.orderBuy);
    expect(strokes).not.toContain(DEFAULT_CHART_THEME.orderSell);
    expect(strokes.some((s) => /^rgba\(240, 100, 92, 0\.45\)$/.test(s))).toBe(true);
  });

  test("a resolved episode draws nothing", () => {
    const model = computeChartModel(state, vp, size);
    expect(layoutDetections(model, [{ ...episode, terminal: true }], DEFAULT_CHART_THEME)).toEqual([]);
  });

  test("annotations neither stretch prices nor turn into editable drawings", () => {
    const before = computeChartModel(state, vp, size);
    const after = computeChartModel({ ...state, detection: [episode] }, vp, size);
    expect(after.price).toEqual(before.price);
    expect(after.slots).toEqual(before.slots);
    expect(state.drawings).toBeUndefined();
    expect(layoutDetections(after, [{ ...episode, levels: [{ ...episode.levels[0]!, knownAt: at + 10_000_000 }] }], DEFAULT_CHART_THEME).filter((p) => p.kind === "line" && p.y1 === p.y2)).toHaveLength(0);
  });

  test("a superseded level stops where its validity ends", () => {
    const model = computeChartModel(state, vp, size);
    const stop = at + 30_000;
    const list = layoutDetections(model, [{ ...episode, levels: [{ ...episode.levels[0]!, validTo: stop }] }], DEFAULT_CHART_THEME);
    const expected = slotX(model.index, tToIndex(model.slots, stop, model.timeframeMs)) - model.index.barSpacing / 2;
    const horizontal = list.filter((p) => p.kind === "line" && p.y1 === p.y2);
    expect(horizontal.length).toBeGreaterThan(0);
    for (const p of horizontal) if (p.kind === "line") expect(p.x2).toBeCloseTo(Math.min(model.geometry.plot.x + model.geometry.plot.w, expected));
  });

  test("stale price tags keep exact detector precision and dim to grey", () => {
    const model = computeChartModel(state, vp, size);
    const list = layoutDetections(model, [{ ...episode, stale: true, recognizedAt: at - 1_000_000, levels: [{ ...episode.levels[0]!, trigger: 2.700001 }] }], DEFAULT_CHART_THEME);
    const tag = list.find((p) => p.kind === "text" && p.text === "2.700001");
    expect(tag?.kind === "text" && tag.fill).toBe(DEFAULT_CHART_THEME.textDim);
    for (const p of list) if (p.kind === "line") expect(p.stroke).toBe(DEFAULT_CHART_THEME.level);
  });
});
