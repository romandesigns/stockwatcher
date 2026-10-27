import { expect, test } from "bun:test";
import { breakoutOverlays } from "../../shared/src/breakout.ts";
import { breakoutTestFixture as b } from "../../shared/src/breakoutTestFixture.ts";
import { layoutDetections } from "./detection.ts";
import { tToIndex } from "./drawings.ts";
import { computeChartModel } from "./layout.ts";
import { defaultViewport, manualPrice, slotX } from "./scale.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";
import type { Timeframe } from "./types.ts";

test("15s / 1m / 5m charts project identical frozen event geometry, clipped to knowledge time", () => {
  const context = { event_id: "runners-breakout-fixture", symbol: "CJMB", breakout: b };
  const overlays = breakoutOverlays(context, "CJMB");
  const original = structuredClone(overlays);
  for (const [timeframe, ms] of [["15s", 15000], ["1m", 60000], ["5m", 300000]] as const) {
    const start = Math.floor(b.bar_start_ms / ms) * ms - 10 * ms;
    const candles = Array.from({ length: 20 }, (_, i) => ({ t: start + i * ms, o: 2.02, h: 2.05, l: 2.0, c: 2.02, v: 100, n: 10, final: true }));
    const model = computeChartModel({ timeframe: timeframe as Timeframe, candles, gaps: [], profile: null, inspectT: null, pointer: null, detection: overlays }, manualPrice(defaultViewport(30), { lo: 2, hi: 2.1 }), { width: 1000, height: 400 });
    const list = layoutDetections(model, overlays, DEFAULT_CHART_THEME);
    const lines = list.filter((p) => p.kind === "line" && p.y1 === p.y2);
    expect(lines).toHaveLength(2);
    for (const [i, known] of [b.reference_known_at_ms, b.evaluated_at_ms].entries()) {
      const expectedX = slotX(model.index, tToIndex(model.slots, known, model.timeframeMs)) - model.index.barSpacing / 2;
      const line = lines[i]!;
      expect(line.kind === "line" && line.x1).toBeCloseTo(Math.max(model.geometry.plot.x, expectedX));
    }
    expect(overlays).toEqual(original);
    expect(overlays[0]!.levels.map((x) => x.trigger)).toEqual([2.03, 2.040001]);
    expect(model.price).toEqual(computeChartModel({ timeframe, candles, gaps: [], profile: null, inspectT: null, pointer: null }, manualPrice(defaultViewport(30), { lo: 2, hi: 2.1 }), { width: 1000, height: 400 }).price);
  }
});
