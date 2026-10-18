import { describe, expect, test } from "bun:test";
import { layoutChart, computeChartModel, type ChartState } from "./layout.ts";
import { DEFAULT_LAYERS, layoutContext, layoutOpeningRanges, openingRanges, parseLayers, regularSessionEnd } from "./levels.ts";
import { buildSlots, defaultViewport, priceY, slotX } from "./scale.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";
import { etToUtc } from "./time.ts";
import { TIMEFRAME_MS, type Candle, type Gap, type Timeframe } from "./types.ts";

// The opening-range layer (owner request 2026-10-18): a dimmed box around the
// first regular candle of the CURRENT timeframe, from that candle to the close.

const ET = (h: number, m: number, s = 0, day = 15, month = 10) => etToUtc(2026, month, day, h, m, s);
const c = (t: number, o: number, h: number, l: number, cl: number, final = true): Candle => ({ t, o, h, l, c: cl, v: 1000, n: 5, final });

/**
 * `count` candles of one timeframe from `from`, with known extremes: interval
 * k has high base + k*0.01 + 0.05 and low base + k*0.01 - 0.05.
 */
function run(tf: Timeframe, from: number, count: number, base = 3): Candle[] {
  const ms = TIMEFRAME_MS[tf];
  return Array.from({ length: count }, (_, k) => {
    const mid = Number((base + k * 0.01).toFixed(2));
    return c(from + k * ms, mid, Number((mid + 0.05).toFixed(2)), Number((mid - 0.05).toFixed(2)), Number((mid + 0.01).toFixed(2)));
  });
}

describe("openingRanges: the first regular candle of the current timeframe", () => {
  for (const tf of ["15s", "30s", "1m", "5m"] as const) {
    test(`${tf}: the 09:30 candle's low to high, from its start to 16:00 ET`, () => {
      const ms = TIMEFRAME_MS[tf];
      const pre = run(tf, ET(9, 30) - 4 * ms, 4, 2.5);
      const reg = run(tf, ET(9, 30), 6, 3);
      const [r, ...rest] = openingRanges(buildSlots([...pre, ...reg], []));
      expect(rest).toEqual([]);
      expect(r).toEqual({ day: "2026-10-15", t: ET(9, 30), end: ET(16, 0), high: 3.05, low: 2.95, forming: false });
    });
  }

  test("switching timeframe re-derives it: 1m encloses 09:30–09:31, 5m the whole 09:30–09:35 candle", () => {
    const oneMinute = [c(ET(9, 30), 3.0, 3.1, 2.95, 3.05), c(ET(9, 31), 3.05, 3.4, 3.0, 3.3), c(ET(9, 32), 3.3, 3.35, 2.8, 2.9), c(ET(9, 33), 2.9, 3.0, 2.85, 2.95), c(ET(9, 34), 2.95, 3.0, 2.9, 2.98), c(ET(9, 35), 2.98, 3.5, 2.97, 3.45)];
    // The 5m candle the server builds from the same prints: the extremes of 09:30–09:34.
    const fiveMinute = [c(ET(9, 30), 3.0, 3.4, 2.8, 2.98), c(ET(9, 35), 2.98, 3.5, 2.97, 3.45)];
    expect(openingRanges(buildSlots(oneMinute, []))[0]).toMatchObject({ t: ET(9, 30), high: 3.1, low: 2.95 });
    expect(openingRanges(buildSlots(fiveMinute, []))[0]).toMatchObject({ t: ET(9, 30), high: 3.4, low: 2.8 });
  });

  test("before 09:30 there is no box (premarket and overnight candles never start one)", () => {
    const overnight = run("1m", ET(3, 50), 5, 2.4);
    const pre = run("1m", ET(9, 25), 5, 2.6);
    expect(openingRanges(buildSlots([...overnight, ...pre], []))).toEqual([]);
    expect(openingRanges([])).toEqual([]);
  });

  test("while the first candle is forming (09:30:xx) the box follows its growing high and low", () => {
    const pre = run("1m", ET(9, 27), 3, 2.6);
    const at = (h: number, l: number) => openingRanges(buildSlots([...pre, c(ET(9, 30), 3, h, l, 3.01, false)], []))[0]!;
    expect(at(3.02, 2.99)).toMatchObject({ high: 3.02, low: 2.99, forming: true });
    // The next update of the same candle widens it; nothing is cached between calls.
    expect(at(3.2, 2.9)).toMatchObject({ high: 3.2, low: 2.9, forming: true });
    // Once 09:31 opens, the first candle is final and the range stops moving.
    const done = openingRanges(buildSlots([...pre, c(ET(9, 30), 3, 3.2, 2.9, 3.1, true), c(ET(9, 31), 3.1, 3.6, 3.05, 3.5, false)], []))[0]!;
    expect(done).toMatchObject({ high: 3.2, low: 2.9, forming: false });
  });

  test("a 09:30 interval with no trades (a gap slot) starts the range at the first interval that traded", () => {
    const gaps: Gap[] = [{ t: ET(9, 30), end: ET(9, 31), reason: "no_trades", v: 0, intervals: 1 }];
    const r = openingRanges(buildSlots([c(ET(9, 29), 3, 3.1, 2.9, 3), c(ET(9, 31), 3, 3.3, 2.95, 3.2)], gaps))[0]!;
    expect(r).toMatchObject({ t: ET(9, 31), high: 3.3, low: 2.95 });
  });

  test("one box per trading day the chart holds; a day without regular data gets none", () => {
    const day14 = [...run("5m", ET(8, 0, 0, 14), 3, 2.0), ...run("5m", ET(9, 30, 0, 14), 3, 2.2)];
    const day15PreOnly = run("5m", ET(7, 0, 0, 15), 4, 2.5);
    const day16 = run("5m", ET(9, 30, 0, 16), 2, 2.7);
    const ranges = openingRanges(buildSlots([...day14, ...day15PreOnly, ...day16], []));
    expect(ranges.map((r) => [r.day, r.t, r.end])).toEqual([
      ["2026-10-14", ET(9, 30, 0, 14), ET(16, 0, 0, 14)],
      ["2026-10-16", ET(9, 30, 0, 16), ET(16, 0, 0, 16)],
    ]);
  });

  test("an early close the server reports ends the box at 13:00 ET; without it the standard 16:00 holds", () => {
    const day = { year: 2026, month: 11, day: 27 };
    const regular = { id: "2026-11-27/regular", start: etToUtc(2026, 11, 27, 9, 30), end: etToUtc(2026, 11, 27, 13, 0) };
    const after = { id: "2026-11-27/after", start: etToUtc(2026, 11, 27, 13, 0), end: etToUtc(2026, 11, 27, 17, 0) };
    expect(regularSessionEnd(day, regular)).toBe(etToUtc(2026, 11, 27, 13, 0));
    expect(regularSessionEnd(day, after)).toBe(etToUtc(2026, 11, 27, 13, 0));
    expect(regularSessionEnd(day, null)).toBe(etToUtc(2026, 11, 27, 16, 0));
    // Another day's session ref says nothing about this one.
    expect(regularSessionEnd(day, { ...regular, id: "2026-11-26/regular" })).toBe(etToUtc(2026, 11, 27, 16, 0));
    const candles = run("1m", etToUtc(2026, 11, 27, 9, 30), 3);
    expect(openingRanges(buildSlots(candles, []), regular)[0]!.end).toBe(etToUtc(2026, 11, 27, 13, 0));
  });

  test("the layer exists, defaults off, and a stored set from before it existed parses to off", () => {
    expect(DEFAULT_LAYERS.openingRange).toBe(false);
    expect(parseLayers({ halts: true }).openingRange).toBe(false);
    expect(parseLayers({ openingRange: true }).openingRange).toBe(true);
  });
});

describe("drawing the opening range", () => {
  const size = { width: 600, height: 300 };
  const theme = DEFAULT_CHART_THEME;
  const modelOf = (candles: Candle[], timeframe: Timeframe = "1m") => {
    const state: ChartState = { timeframe, candles, gaps: [], profile: null, inspectT: null, pointer: null };
    return { state, model: computeChartModel(state, defaultViewport(8), size) };
  };

  test("a dimmed, borderless rect from the first candle's slot to the plot edge while the session runs", () => {
    const candles = [...run("1m", ET(9, 25), 5, 2.9), ...run("1m", ET(9, 30), 20, 3)];
    const { model } = modelOf(candles);
    const list = layoutOpeningRanges(model, openingRanges(model.slots), theme);
    expect(list).toHaveLength(1);
    const r = list[0]!;
    if (r.kind !== "rect") throw new Error("expected a rect");
    const plot = model.geometry.plot;
    expect(r).toMatchObject({ fill: theme.openingRange, layer: "background" });
    expect(r.stroke).toBeUndefined();
    expect(r.x).toBeCloseTo(slotX(model.index, 5) - model.index.barSpacing / 2);
    expect(r.y).toBeCloseTo(priceY(model.price!, 3.05));
    expect(r.y + r.h).toBeCloseTo(priceY(model.price!, 2.95));
    // 16:00 is far in the future of a 09:49 chart: the box runs to the plot's edge and stops there, off the price axis.
    expect(r.x + r.w).toBe(plot.x + plot.w);
  });

  test("once the session is over the box ends where the first after-hours slot begins", () => {
    const candles = [...run("5m", ET(15, 40), 4, 3.0), ...run("5m", ET(16, 0), 3, 3.0)];
    const { model } = modelOf(candles, "5m");
    const [r] = layoutOpeningRanges(model, [{ day: "2026-10-15", t: ET(15, 40), end: ET(16, 0), high: 3.05, low: 2.95, forming: false }], theme);
    if (r?.kind !== "rect") throw new Error("expected a rect");
    expect(r.x + r.w).toBeCloseTo(slotX(model.index, 4) - model.index.barSpacing / 2);
  });

  test("nothing when the layer is off, before 09:30, or for a range that ends left of the view", () => {
    const { model: pre } = modelOf(run("1m", ET(9, 20), 8, 2.9));
    expect(layoutOpeningRanges(pre, openingRanges(pre.slots), theme)).toEqual([]);
    const ctx = { levels: [], halts: [], luld: [], fills: [], now: null };
    const { model: reg } = modelOf(run("1m", ET(9, 30), 10));
    const isBox = (p: { kind: string; fill?: string }) => p.kind === "rect" && p.fill === theme.openingRange;
    expect(layoutContext(reg, ctx, theme).some(isBox)).toBe(false);
    expect(layoutContext(reg, { ...ctx, openingRanges: openingRanges(reg.slots) }, theme).some(isBox)).toBe(true);
    expect(layoutOpeningRanges(reg, [{ day: "2026-10-15", t: reg.slots[0]!.t, end: reg.slots[0]!.t, high: 3, low: 2.9, forming: false }], theme)).toEqual([]);
  });

  test("through layoutChart it paints after the session shading and before the candles", () => {
    const candles = [...run("1m", ET(9, 25), 5, 2.9), ...run("1m", ET(9, 30), 20, 3)];
    const { state, model } = modelOf(candles);
    const list = layoutChart(model, { ...state, context: { levels: [], halts: [], luld: [], fills: [], now: null, openingRanges: openingRanges(model.slots) } }, theme);
    const box = list.findIndex((p) => p.kind === "rect" && p.fill === theme.openingRange);
    const shade = list.findIndex((p) => p.kind === "rect" && p.fill === theme.shadePre);
    const firstCandle = list.findIndex((p) => p.layer === "candles");
    expect(shade).toBeGreaterThanOrEqual(0);
    expect(box).toBeGreaterThan(shade);
    expect(box).toBeLessThan(firstCandle);
  });
});
