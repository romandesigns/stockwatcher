import { describe, expect, test } from "bun:test";
import { computeChartModel, layoutChart, type ChartState } from "./layout.ts";
import { layoutContext, layoutVolumeShelves } from "./levels.ts";
import { defaultViewport, priceY, slotX } from "./scale.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";
import { etToUtc } from "./time.ts";
import type { Candle } from "./types.ts";

// Volume shelves (server Runners frames' `zones`): faint borderless bands from when the shelf
// formed to the right edge, under the candles, labelled "Volume shelf".

const ET = (h: number, m: number) => etToUtc(2026, 10, 15, h, m, 0);
const candles: Candle[] = Array.from({ length: 30 }, (_, k) => {
  const mid = 3 + k * 0.01;
  return { t: ET(9, 30) + k * 60_000, o: mid, h: mid + 0.05, l: mid - 0.05, c: mid + 0.01, v: 1000, n: 5, final: true };
});
const theme = DEFAULT_CHART_THEME;
const state: ChartState = { timeframe: "1m", candles, gaps: [], profile: null, inspectT: null, pointer: null };
const model = computeChartModel(state, defaultViewport(8), { width: 600, height: 300 });
const plot = model.geometry.plot;

describe("volume shelves", () => {
  test("a faint borderless band from the formed slot to the plot's right edge, labelled", () => {
    const list = layoutVolumeShelves(model, [{ lo: 3.05, hi: 3.12, formed: ET(9, 40) }], theme);
    const r = list.find((p) => p.kind === "rect");
    if (r?.kind !== "rect") throw new Error("expected a rect");
    expect(r).toMatchObject({ fill: theme.volumeShelf, layer: "background" });
    expect(r.stroke).toBeUndefined();
    expect(r.x).toBeCloseTo(slotX(model.index, 10) - model.index.barSpacing / 2);
    expect(r.x + r.w).toBe(plot.x + plot.w);
    expect(r.y).toBeCloseTo(priceY(model.price!, 3.12));
    expect(r.y + r.h).toBeCloseTo(priceY(model.price!, 3.05));
    expect(list.some((p) => p.kind === "text" && p.text === "Volume shelf" && p.fill === theme.textDim)).toBe(true);
  });

  test("never the accent", () => {
    expect(theme.volumeShelf).not.toBe(theme.profilePoc);
    expect(theme.volumeShelf.startsWith("rgba(152, 162, 174")).toBe(true);
  });

  test("off-range prices, bad bands and none draw nothing", () => {
    expect(layoutVolumeShelves(model, [], theme)).toEqual([]);
    expect(layoutVolumeShelves(model, [{ lo: 50, hi: 51, formed: ET(9, 31) }], theme)).toEqual([]);
    expect(layoutVolumeShelves(model, [{ lo: 3.2, hi: 3.1, formed: ET(9, 31) }], theme)).toEqual([]);
  });

  test("through layoutContext and layoutChart: before the candles", () => {
    const ctx = { levels: [], halts: [], luld: [], fills: [], now: null, shelves: [{ lo: 3.05, hi: 3.12, formed: ET(9, 35) }] };
    const isBand = (p: { kind: string; fill?: string }) => p.kind === "rect" && p.fill === theme.volumeShelf;
    expect(layoutContext(model, { ...ctx, shelves: [] }, theme).some(isBand)).toBe(false);
    expect(layoutContext(model, ctx, theme).some(isBand)).toBe(true);
    const list = layoutChart(model, { ...state, context: ctx }, theme);
    const band = list.findIndex(isBand);
    const candle = list.findIndex((p) => p.layer === "candles");
    expect(band).toBeGreaterThanOrEqual(0);
    expect(band).toBeLessThan(candle);
  });
});
