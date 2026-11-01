import { describe, expect, test } from "bun:test";
import { canAddIndicator, INDICATOR_PRESETS, MAX_OVERLAYS, toggleIndicator, type IndicatorSpec } from "./indicators.ts";
import { bollinger, computeIndicators, ema, macd, rsi, sma, vwap } from "./indicatorMath.ts";
import { buildSlots, type Slot } from "./scale.ts";
import { fixtureCandles, T0 } from "./testFixture.ts";
import type { Candle } from "./types.ts";

const arr = (xs: number[]) => Float64Array.from(xs);
const close = (a: number, b: number) => expect(Math.abs(a - b)).toBeLessThan(1e-9);

function candleSlots(closes: number[], start = T0, stepMs = 60_000, vols?: number[]): Slot[] {
  const candles: Candle[] = closes.map((c, i) => ({ t: start + i * stepMs, o: c, h: c, l: c, c, v: vols?.[i] ?? 100, n: 1, final: true }));
  return buildSlots(candles, []);
}

describe("moving averages", () => {
  test("SMA starts after `period` values and averages the window", () => {
    const out = sma(arr([1, 2, 3, 4, 5]), 3);
    expect(Number.isNaN(out[0]!) && Number.isNaN(out[1]!)).toBe(true);
    close(out[2]!, 2);
    close(out[4]!, 4);
  });

  test("a gap (NaN) is skipped, not counted, and stays NaN in the output", () => {
    const out = sma(arr([1, 2, Number.NaN, 3]), 3);
    expect(Number.isNaN(out[2]!)).toBe(true);
    close(out[3]!, 2);
  });

  test("EMA is seeded with the SMA, then weights 2/(n+1)", () => {
    const out = ema(arr([2, 4, 6, 8]), 3);
    close(out[2]!, 4);
    close(out[3]!, 8 * 0.5 + 4 * 0.5);
  });
});

describe("oscillators", () => {
  test("RSI is 100 with only gains, 50 when nothing moves, and Wilder-smoothed after", () => {
    expect(rsi(arr([1, 2, 3, 4]), 3)[3]).toBe(100);
    expect(rsi(arr([5, 5, 5, 5]), 3)[3]).toBe(50);
    const r = rsi(arr([10, 11, 10, 11, 12]), 2);
    // after 2 changes: avg gain 0.5, avg loss 0.5 → 50; then gain (0.5+1)/2, loss 0.5/2 → RS 3 → 75
    close(r[2]!, 50);
    close(r[3]!, 75);
  });

  test("MACD histogram = line − signal wherever the signal exists", () => {
    const xs = Array.from({ length: 60 }, (_, i) => 3 + Math.sin(i / 5));
    const [line, sig, hist] = macd(arr(xs), 12, 26, 9);
    expect(Number.isNaN(line[24]!)).toBe(true);
    expect(Number.isNaN(line[25]!)).toBe(false);
    for (let i = 0; i < xs.length; i++) if (!Number.isNaN(sig[i]!)) close(hist[i]!, line[i]! - sig[i]!);
  });

  test("Bollinger bands sit at ± mult population SD around the SMA", () => {
    const [mid, up, lo] = bollinger(arr([1, 2, 3]), 3, 2);
    const sd = Math.sqrt(2 / 3);
    close(mid[2]!, 2);
    close(up[2]!, 2 + 2 * sd);
    close(lo[2]!, 2 - 2 * sd);
  });
});

describe("VWAP", () => {
  test("is volume-weighted typical price and resets at a new trading day", () => {
    const day1 = candleSlots([2, 4], T0, 60_000, [100, 300]);
    expect(vwap(day1)[1]).toBeCloseTo((2 * 100 + 4 * 300) / 400, 9);
    // 20:00 ET opens the next trading day (the overnight session belongs to it).
    const evening = Date.UTC(2026, 9, 15, 23, 59, 0); // 19:59 ET
    const slots = candleSlots([2, 10], evening, 60_000, [100, 100]);
    expect(vwap(slots)[1]).toBe(10);
  });
});

describe("computeIndicators", () => {
  const { candles, gaps } = fixtureCandles(60, "1m", T0, [30]);
  const slots = buildSlots(candles, gaps);

  test("values align to slots and a gap slot is NaN", () => {
    const [r] = computeIndicators(slots, [{ id: "ema-9", kind: "ema", period: 9 }]);
    expect(r!.lines[0]!.values.length).toBe(slots.length);
    const gapIdx = slots.findIndex((s) => s.kind === "gap");
    expect(Number.isNaN(r!.lines[0]!.values[gapIdx]!)).toBe(true);
  });

  test("overlays take colour slots in order and the cap drops extras", () => {
    const specs: IndicatorSpec[] = [
      { id: "a", kind: "ema", period: 5 },
      { id: "b", kind: "sma", period: 5 },
      { id: "c", kind: "vwap" },
      { id: "d", kind: "ema", period: 9 },
      { id: "r", kind: "rsi", period: 14 },
    ];
    const out = computeIndicators(slots, specs);
    expect(out.filter((r) => r.placement === "overlay").map((r) => r.colorSlot)).toEqual([0, 1, 2]);
    expect(out.map((r) => r.spec.id)).toEqual(["a", "b", "c", "r"]);
    expect(out.find((r) => r.spec.id === "r")!.fixedRange).toEqual({ lo: 0, hi: 100 });
  });

  test("toggle respects the caps", () => {
    let specs: IndicatorSpec[] = [];
    const overlays = INDICATOR_PRESETS.filter((p) => p.kind !== "rsi" && p.kind !== "macd");
    for (const p of overlays.slice(0, MAX_OVERLAYS)) specs = toggleIndicator(specs, p);
    expect(specs.length).toBe(MAX_OVERLAYS);
    expect(canAddIndicator(specs, overlays[MAX_OVERLAYS]!)).toBe(false);
    expect(toggleIndicator(specs, overlays[MAX_OVERLAYS]!).length).toBe(MAX_OVERLAYS);
    expect(toggleIndicator(specs, overlays[0]!).length).toBe(MAX_OVERLAYS - 1);
  });
});
