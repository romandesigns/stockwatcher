import { describe, expect, test } from "bun:test";
import { describeSlot, intervalEnd } from "./inspect.ts";
import { computeChartModel, type ChartState } from "./layout.ts";
import { buildSlots, defaultViewport } from "./scale.ts";
import { applyIncrement, applySnapshot } from "./store.ts";
import { etToUtc } from "./time.ts";
import { fixtureCandles, fixtureSnapshot } from "./testFixture.ts";
import { TIMEFRAME_MS, TIMEFRAMES, type Candle, type Timeframe } from "./types.ts";

// 10s and 1h (2026-11-22). The server builds them (crates/chart-engine/src/timeframe.rs);
// these pin what the clients assume about them.

describe("timeframes", () => {
  test("the list, finest first, and the nominal lengths", () => {
    expect(TIMEFRAMES).toEqual(["10s", "15s", "30s", "1m", "5m", "1h"]);
    expect(TIMEFRAME_MS["10s"]).toBe(10_000);
    expect(TIMEFRAME_MS["1h"]).toBe(3_600_000);
  });

  test("a 1h candle ends on the hour, except the 09:00 one at the open", () => {
    // Both sides of the 2026-11-01 DST change.
    for (const [y, m, d] of [[2026, 10, 30], [2026, 11, 2]] as const) {
      const at = (h: number, min = 0) => etToUtc(y, m, d, h, min);
      expect(intervalEnd(at(8), "1h")).toBe(at(9));
      expect(intervalEnd(at(9), "1h")).toBe(at(9, 30));
      expect(intervalEnd(at(9, 30), "1h")).toBe(at(10));
      expect(intervalEnd(at(10), "1h")).toBe(at(11));
      expect(intervalEnd(at(9), "5m")).toBe(at(9, 5));
      expect(intervalEnd(at(9, 0), "10s")).toBe(at(9) + 10_000);
    }
  });

  test("the inspector labels the clipped hour", () => {
    const t = etToUtc(2026, 10, 15, 9, 0);
    const c: Candle = { t, o: 1, h: 2, l: 0.5, c: 1.5, v: 10, n: 1, final: true };
    expect(describeSlot({ kind: "candle", t, candle: c }, "1h").timeLabel).toBe("09:00:00 – 09:30:00");
    const s = describeSlot({ kind: "candle", t: t + 10_000, candle: { ...c, t: t + 10_000 } }, "10s");
    expect(s.timeLabel).toBe("09:00:10 – 09:00:20");
    expect(s.time).toBe("09:00:10");
  });

  for (const tf of ["10s", "1h"] as const satisfies readonly Timeframe[]) {
    test(`${tf}: snapshot, increment and model`, () => {
      let series = applySnapshot(undefined, fixtureSnapshot(tf, 12, [4]));
      expect(series.timeframe).toBe(tf);
      const last = series.candles[series.candles.length - 1]!;
      const next = { ...last, t: last.t + TIMEFRAME_MS[tf], final: false };
      series = applyIncrement(series, { type: "chart.update", symbol: "SMPL", timeframe: tf, session: "2026-10-15/regular", candle: next, profileBin: 0.01, profile: [] });
      expect(series.candles.length).toBe(12);
      const { candles, gaps } = fixtureCandles(12, tf);
      const slots = buildSlots(candles, gaps);
      const state: ChartState = { timeframe: tf, candles, gaps, slots, profile: null, inspectT: null, pointer: null };
      const model = computeChartModel(state, defaultViewport(slots.length), { width: 800, height: 400 });
      expect(model.timeframeMs).toBe(TIMEFRAME_MS[tf]);
      expect(model.price).not.toBeNull();
    });
  }
});
