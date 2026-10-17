import { describe, expect, test } from "bun:test";
import { backToLatestMs, backToLatestStep, buildSlots, defaultViewport, ensureVisible, indexScale, isPannedBack, niceTicks, pan, panSlots, rightEdge, slotAt, slotsBehindLatest, slotX, timeTicks, zoom } from "./scale.ts";
import { fixtureCandles, T0 } from "./testFixture.ts";

describe("buildSlots", () => {
  test("merges candles and gaps in time order; a candle supersedes a gap at the same t", () => {
    const { candles, gaps } = fixtureCandles(6, "1m", T0, [2]);
    const slots = buildSlots(candles, [...gaps, { t: candles[0]!.t, end: candles[0]!.t + 60_000, reason: "no_trades" as const, v: 0, intervals: 1 }].sort((a, b) => a.t - b.t));
    expect(slots.map((s) => s.kind)).toEqual(["candle", "candle", "gap", "candle", "candle", "candle"]);
  });
});

describe("niceTicks", () => {
  test("cents for a $3 stock", () => {
    const t = niceTicks(3.17, 3.53, 6);
    expect(t.step).toBeCloseTo(0.1);
    expect(t.values).toEqual([3.2, 3.3, 3.4, 3.5]);
    expect(t.decimals).toBe(2);
  });
  test("sub-penny steps for a $0.15 stock", () => {
    const t = niceTicks(0.1512, 0.1588, 5);
    expect(t.step).toBeCloseTo(0.002);
    expect(t.decimals).toBe(3);
    expect(t.values).toEqual([0.152, 0.154, 0.156, 0.158]);
  });
  test("2.5 steps carry an extra decimal", () => {
    const t = niceTicks(10, 20, 4);
    expect(t.step).toBe(2.5);
    expect(t.values).toEqual([10, 12.5, 15, 17.5, 20]);
  });
  test("degenerate ranges give no ticks", () => {
    expect(niceTicks(3, 3, 5).values).toEqual([]);
  });
});

describe("viewport", () => {
  const { candles, gaps } = fixtureCandles(100, "1m", T0, [10]);
  const slots = buildSlots(candles, gaps);

  test("follows the latest slot by default", () => {
    const vp = defaultViewport(10);
    expect(rightEdge(vp, slots)).toBe(100 + vp.rightMargin);
    const s = indexScale(vp, slots, 0, 500);
    expect(s.to).toBe(99);
    expect(slotX(s, 99)).toBeCloseTo(500 - (vp.rightMargin + 0.5) * 10);
  });

  test("panning back pins the view; new slots do not move it; panning forward resumes follow", () => {
    let vp = pan(defaultViewport(10), slots, 200);
    expect(vp.anchorT).not.toBeNull();
    const edge = rightEdge(vp, slots);
    expect(edge).toBeCloseTo(100 + 1.5 - 20);
    const more = buildSlots(fixtureCandles(110, "1m", T0, [10]).candles, gaps);
    expect(rightEdge(vp, more)).toBeCloseTo(edge);
    vp = pan(vp, slots, -10_000);
    expect(vp.anchorT).toBeNull();
  });

  test("zoom keeps the slot under the pointer fixed and clamps spacing", () => {
    const vp0 = pan(defaultViewport(10), slots, 300);
    const s0 = indexScale(vp0, slots, 0, 600);
    const under = slotAt(s0, 250, slots.length)!;
    const vp1 = zoom(vp0, slots, 2, 250, 600);
    const s1 = indexScale(vp1, slots, 0, 600);
    expect(slotAt(s1, 250, slots.length)).toBe(under);
    expect(zoom(vp0, slots, 1000, 0, 600).barSpacing).toBe(48);
    expect(zoom(vp0, slots, 0.0001, 0, 600).barSpacing).toBe(2);
  });

  test("ensureVisible pans just enough, and back to follow at the end", () => {
    const vp = defaultViewport(10);
    const back = ensureVisible(vp, slots, 20, 300);
    const s = indexScale(back, slots, 0, 300);
    expect(s.from).toBeLessThanOrEqual(20);
    expect(s.to).toBeGreaterThanOrEqual(20);
    expect(ensureVisible(back, slots, 99, 300).anchorT).toBeNull();
    expect(ensureVisible(vp, slots, 95, 300)).toBe(vp);
  });

  test("slotAt inverts slotX", () => {
    const s = indexScale(defaultViewport(8), slots, 0, 400);
    for (const i of [s.from + 1, 60, 99]) expect(slotAt(s, slotX(s, i), slots.length)).toBe(i);
    expect(slotAt(s, 401, slots.length)).toBeNull();
  });
});

describe("timeTicks", () => {
  test("15s candles at 9 px get labels every 2 minutes, with seconds hidden on whole minutes", () => {
    const { candles } = fixtureCandles(120, "15s", T0);
    const slots = buildSlots(candles, []);
    const s = indexScale(defaultViewport(9), slots, 0, 1000);
    const ticks = timeTicks(slots, s, 15_000, 64);
    expect(ticks.length).toBeGreaterThan(3);
    for (const t of ticks) expect((t.t - T0) % 120_000).toBe(0);
    expect(ticks.map((t) => t.label)).toContain("10:20");
  });

  test("labels stay at least minSpacing apart", () => {
    const { candles } = fixtureCandles(300, "1m", T0);
    const slots = buildSlots(candles, []);
    const s = indexScale(defaultViewport(3), slots, 0, 900);
    const xs = timeTicks(slots, s, 60_000, 72).map((t) => slotX(s, t.index));
    for (let i = 1; i < xs.length; i++) expect(xs[i]! - xs[i - 1]!).toBeGreaterThanOrEqual(72);
  });
});

describe("back to latest", () => {
  test("is shown only when panned back, and slides the whole way in eased steps", () => {
    const slots = Array.from({ length: 100 }, (_, i) => ({ kind: "gap" as const, t: i * 60_000, gap: { t: i * 60_000, end: (i + 1) * 60_000, reason: "no_trades" as const, v: 0, intervals: 1 } }));
    const vp = defaultViewport(8);
    expect(isPannedBack(vp)).toBe(false);
    const back = panSlots(vp, slots, 40);
    expect(isPannedBack(back)).toBe(true);
    const total = slotsBehindLatest(back, slots);
    expect(total).toBeCloseTo(40, 6);
    const ms = backToLatestMs(total, 8);
    let v = back;
    let panned = 0;
    for (let t = 16; t <= ms + 16; t += 16) {
      const d = backToLatestStep(total, panned, t, ms);
      panned -= d;
      v = panSlots(v, slots, d);
    }
    expect(panned).toBeCloseTo(total, 6);
    expect(isPannedBack(v)).toBe(false);
  });
});
