import { describe, expect, test } from "bun:test";
import { type ChartTheme, type TextPrim } from "./displayList.ts";
import { computeChartModel, hitTest, layoutChart, type ChartState } from "./layout.ts";
import {
  autoScalePrice,
  defaultViewport,
  formatAxisValue,
  formatPercentAxis,
  isAutoPrice,
  manualPrice,
  panSlots,
  priceAxisTicks,
  priceRange,
  priceY,
  scalePriceRange,
  yPrice,
  type PriceScale,
  type PriceScaleType,
} from "./scale.ts";
import { fixtureSnapshot } from "./testFixture.ts";

const TYPES: PriceScaleType[] = ["linear", "percent", "log"];

describe("price ↔ y mapping", () => {
  for (const type of TYPES) {
    test(`${type}: price → y → price round-trips and the ends map to top/bottom`, () => {
      const s: PriceScale = { lo: 0.15, hi: 24.5, top: 10, bottom: 410, type, base: 3.2 };
      expect(priceY(s, s.hi)).toBeCloseTo(10);
      expect(priceY(s, s.lo)).toBeCloseTo(410);
      for (const p of [0.15, 0.1512, 0.99, 1, 3.17, 12.5, 24.5]) expect(yPrice(s, priceY(s, p))).toBeCloseTo(p, 9);
      for (const y of [10, 57.3, 210, 409]) expect(priceY(s, yPrice(s, y))).toBeCloseTo(y, 9);
    });
  }

  test("log: equal ratios get equal heights; linear and percent share one mapping", () => {
    const log: PriceScale = { lo: 1, hi: 16, top: 0, bottom: 400, type: "log" };
    expect(priceY(log, 2) - priceY(log, 4)).toBeCloseTo(priceY(log, 4) - priceY(log, 8));
    expect(priceY(log, 4)).toBeCloseTo(200);
    const lin: PriceScale = { lo: 3, hi: 4, top: 0, bottom: 100 };
    for (const p of [3, 3.25, 3.9]) expect(priceY({ ...lin, type: "percent", base: 3.5 }, p)).toBeCloseTo(priceY(lin, p));
  });
});

describe("auto range", () => {
  const snap = fixtureSnapshot("1m", 20, [5]);
  test("log padding is applied in log space, so lo stays positive even for a wide range", () => {
    const slots = [
      { kind: "candle" as const, t: 0, candle: { t: 0, o: 0.2, h: 0.3, l: 0.15, c: 0.25, v: 1, n: 1, final: true } },
      { kind: "candle" as const, t: 1, candle: { t: 1, o: 20, h: 24, l: 19, c: 23, v: 1, n: 1, final: true } },
    ];
    expect(priceRange(slots, 0, 1, 0.08, "linear")!.lo).toBeLessThan(0);
    const r = priceRange(slots, 0, 1, 0.08, "log")!;
    expect(r.lo).toBeGreaterThan(0);
    expect(Math.log(0.15 / r.lo)).toBeCloseTo(Math.log(r.hi / 24));
  });

  test("auto vs manual: a manual range holds through time pans until auto-scale", () => {
    const state: ChartState = { timeframe: "1m", candles: snap.candles, gaps: snap.gaps, profile: null, inspectT: null, pointer: null };
    const size = { width: 400, height: 300 };
    const vp0 = defaultViewport(16);
    const auto = computeChartModel(state, vp0, size).price!;
    const manual = manualPrice(vp0, { lo: 3.0, hi: 3.6 });
    expect(isAutoPrice(manual)).toBe(false);
    const m = computeChartModel(state, manual, size).price!;
    expect([m.lo, m.hi]).toEqual([3.0, 3.6]);
    const panned = computeChartModel(state, panSlots(manual, computeChartModel(state, manual, size).slots, 8), size).price!;
    expect([panned.lo, panned.hi]).toEqual([3.0, 3.6]);
    const back = computeChartModel(state, autoScalePrice(manual), size).price!;
    expect([back.lo, back.hi]).toEqual([auto.lo, auto.hi]);
    // Hit testing reads the same (manual) mapping the candles are drawn with.
    const model = computeChartModel(state, manual, size);
    expect(hitTest(model, 100, priceY(model.price!, 3.3)).price).toBeCloseTo(3.3);
  });

  test("percent mode's base is the first visible candle's open, and moves with the view", () => {
    const state: ChartState = { timeframe: "1m", candles: snap.candles, gaps: snap.gaps, profile: null, inspectT: null, pointer: null, priceScaleType: "percent" };
    const size = { width: 400, height: 300 };
    const vp0 = defaultViewport(16);
    const m = computeChartModel(state, vp0, size);
    expect(m.price!.base).toBe(snap.candles.find((c) => c.t === m.slots[m.index.from]!.t)!.o);
    const back = panSlots(vp0, m.slots, 10);
    const m2 = computeChartModel(state, back, size);
    expect(m2.price!.base).toBe((m2.slots[m2.index.from] as { candle: { o: number } }).candle.o);
  });
});

describe("price scaling", () => {
  for (const type of TYPES) {
    test(`${type}: the anchor price stays at the anchor y; factor 2 halves the span in the scale's space`, () => {
      const s: PriceScale = { lo: 3, hi: 4, top: 0, bottom: 200, type, base: 3.5 };
      const anchorY = 50;
      const anchorP = yPrice(s, anchorY);
      const r = scalePriceRange(s, 2, anchorY);
      const next = { ...s, ...r };
      expect(yPrice(next, anchorY)).toBeCloseTo(anchorP, 9);
      const u = (p: number) => (type === "log" ? Math.log(p) : p);
      expect(u(r.hi) - u(r.lo)).toBeCloseTo((u(4) - u(3)) / 2, 9);
    });
  }

  test("zoom is clamped at both ends", () => {
    const s: PriceScale = { lo: 3, hi: 4, top: 0, bottom: 200 };
    const tight = scalePriceRange(s, 1e9, 100);
    expect(tight.hi - tight.lo).toBeGreaterThan(0);
    const wide = scalePriceRange(s, 1e-9, 100);
    expect(Number.isFinite(wide.hi - wide.lo)).toBe(true);
    expect(wide.hi - wide.lo).toBeLessThanOrEqual(3.5 * 50 + 1e-9);
  });
});

describe("axis ticks per scale type", () => {
  test("linear labels are prices, as before", () => {
    const t = priceAxisTicks({ lo: 3.17, hi: 3.53, top: 0, bottom: 300 }, 6);
    expect(t.values).toEqual([3.2, 3.3, 3.4, 3.5]);
    expect(t.labels).toEqual(["3.20", "3.30", "3.40", "3.50"]);
  });

  test("percent ticks are nice percents from the base, labelled with sign", () => {
    const s: PriceScale = { lo: 3.1, hi: 3.4, top: 0, bottom: 300, type: "percent", base: 3.2 };
    const t = priceAxisTicks(s, 6);
    expect(t.labels).toEqual(["−2.00%", "0.00%", "+2.00%", "+4.00%", "+6.00%"]);
    expect(t.values[1]).toBeCloseTo(3.2);
    expect(t.values[2]).toBeCloseTo(3.264);
    expect(formatAxisValue(s, 3.264)).toBe("+2.00%");
    expect(formatPercentAxis(-0.004)).toBe("0.00%");
  });

  test("log ticks over a wide range are 1-2-5 × 10^k, evenly spaced per decade", () => {
    const s: PriceScale = { lo: 0.15, hi: 24, top: 0, bottom: 400, type: "log" };
    const t = priceAxisTicks(s, 8);
    expect(t.values).toEqual([0.2, 0.5, 1, 2, 5, 10, 20]);
    expect(t.labels).toEqual(["0.20", "0.50", "1.00", "2.00", "5.00", "10.00", "20.00"]);
    // 1 → 10 and 0.2 → 2 are the same height on a log axis.
    expect(priceY(s, 1) - priceY(s, 10)).toBeCloseTo(priceY(s, 0.2) - priceY(s, 2));
  });

  test("log ticks: denser mantissas when they fit, decades when nothing else does", () => {
    expect(priceAxisTicks({ lo: 2, hi: 9.5, top: 0, bottom: 400, type: "log" }, 8).values).toEqual([2, 3, 4, 5, 6, 7, 8, 9]);
    expect(priceAxisTicks({ lo: 0.001, hi: 1000, top: 0, bottom: 100, type: "log" }, 3).values).toEqual([0.001, 1, 1000]);
  });

  test("a narrow log range (under 2×) uses linear nice steps", () => {
    const t = priceAxisTicks({ lo: 3.17, hi: 3.53, top: 0, bottom: 300, type: "log" }, 6);
    expect(t.values).toEqual([3.2, 3.3, 3.4, 3.5]);
  });

  test("the layout writes percent labels, a percent last-price tag and a percent crosshair label", () => {
    const snap = fixtureSnapshot("1m", 20, [5]);
    const theme = new Proxy({} as ChartTheme, { get: (_, k) => String(k) });
    const state: ChartState = { timeframe: "1m", candles: snap.candles, gaps: snap.gaps, profile: null, inspectT: null, pointer: { x: 100, y: 80 }, priceScaleType: "percent" };
    const model = computeChartModel(state, defaultViewport(16), { width: 400, height: 300 });
    const texts = layoutChart(model, state, theme).filter((p): p is TextPrim => p.kind === "text" && p.x >= model.geometry.priceAxis.x);
    expect(texts.length).toBeGreaterThan(2);
    for (const t of texts) expect(t.text).toMatch(/^([+−]?\d+\.\d+%)$/);
  });
});
