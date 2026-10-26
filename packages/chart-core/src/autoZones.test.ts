import { describe, expect, test } from "bun:test";
import { autoZones, layoutAutoZones } from "./autoZones.ts";
import { computeChartModel } from "./layout.ts";
import { buildSlots, defaultViewport } from "./scale.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";
import type { Candle } from "./types.ts";

const T0 = Date.UTC(2026, 9, 26, 14, 0);
const MIN = 60_000;

/** A candle from open/high/low/close, one minute apart. */
const bar = (i: number, o: number, h: number, l: number, c: number): Candle => ({ t: T0 + i * MIN, o, h, l, c, v: 1000, n: 10, final: true });

/** 14 ordinary candles (range 0.10, ATR ≈ 0.10) around `p`, so ATR is known before the base. */
function warmup(p: number): Candle[] {
  return Array.from({ length: 14 }, (_, i) => bar(i, p, p + 0.06, p - 0.04, i % 2 ? p + 0.04 : p - 0.02));
}

const slotsOf = (cs: Candle[]) => buildSlots(cs, []);

describe("autoZones", () => {
  test("a quiet base then a decisive rally is a demand zone (base low to highest body top)", () => {
    const cs = [...warmup(5), bar(14, 5.0, 5.04, 4.96, 5.01), bar(15, 5.01, 5.05, 4.98, 5.0), bar(16, 5.0, 5.4, 4.99, 5.38), bar(17, 5.38, 5.5, 5.36, 5.48)];
    const z = autoZones(slotsOf(cs));
    expect(z).toEqual([{ kind: "demand", lo: 4.96, hi: 5.01, t: T0 + 14 * MIN, tested: false }]);
  });

  test("a quiet base then a decisive drop is a supply zone (lowest body bottom to base high)", () => {
    const cs = [...warmup(5), bar(14, 5.0, 5.05, 4.97, 4.99), bar(15, 4.99, 5.04, 4.98, 5.0), bar(16, 4.99, 5.0, 4.6, 4.62), bar(17, 4.62, 4.63, 4.5, 4.52)];
    const z = autoZones(slotsOf(cs));
    expect(z).toEqual([{ kind: "supply", lo: 4.99, hi: 5.05, t: T0 + 14 * MIN, tested: false }]);
  });

  test("price trading back into the zone marks it tested; a close through its far edge removes it", () => {
    const rally = [...warmup(5), bar(14, 5.0, 5.04, 4.96, 5.01), bar(15, 5.01, 5.05, 4.98, 5.0), bar(16, 5.0, 5.4, 4.99, 5.38), bar(17, 5.38, 5.5, 5.36, 5.48)];
    const back = [...rally, bar(18, 5.3, 5.31, 5.0, 5.05)];
    expect(autoZones(slotsOf(back))[0]?.tested).toBe(true);
    const through = [...back, bar(19, 5.05, 5.06, 4.8, 4.85)];
    expect(autoZones(slotsOf(through))).toEqual([]);
  });

  test("a move that is not far enough (under 2 ATR) makes no zone; too few candles make none", () => {
    const weak = [...warmup(5), bar(14, 5.0, 5.04, 4.96, 5.01), bar(15, 5.01, 5.15, 5.0, 5.14), bar(16, 5.14, 5.18, 5.12, 5.16)];
    expect(autoZones(slotsOf(weak))).toEqual([]);
    expect(autoZones(slotsOf(warmup(5).slice(0, 10)))).toEqual([]);
  });

  test("drawn as a background band to the right edge, labelled auto, fainter when tested", () => {
    const cs = [...warmup(5), bar(14, 5.0, 5.04, 4.96, 5.01), bar(15, 5.01, 5.05, 4.98, 5.0), bar(16, 5.0, 5.4, 4.99, 5.38), bar(17, 5.38, 5.5, 5.36, 5.48)];
    const slots = slotsOf(cs);
    const model = computeChartModel({ timeframe: "1m", candles: cs, gaps: [], profile: null, inspectT: null, pointer: null }, defaultViewport(cs.length), { width: 800, height: 400 });
    const list = layoutAutoZones(model, autoZones(slots), DEFAULT_CHART_THEME);
    const band = list.find((p) => p.kind === "rect");
    expect(band?.layer).toBe("background");
    expect(band && band.kind === "rect" ? band.x + band.w : 0).toBe(model.geometry.plot.x + model.geometry.plot.w);
    expect(list.some((p) => p.kind === "text" && p.text === "Demand · auto")).toBe(true);
  });
});
