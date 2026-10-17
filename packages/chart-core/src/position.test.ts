import { describe, expect, test } from "bun:test";
import { anchorXY, hitDrawing, layoutDrawings, type Drawing } from "./drawings.ts";
import { computeChartModel, type ChartState } from "./layout.ts";
import { dragPosition, flipPosition, newPosition, positionPlan } from "./position.ts";
import { defaultViewport, priceY, slotX } from "./scale.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";
import { fixtureSnapshot } from "./testFixture.ts";
import { tickSize } from "./trading.ts";

const snap = fixtureSnapshot("1m", 40);
const state: ChartState = { timeframe: "1m", candles: snap.candles, gaps: snap.gaps, profile: snap.profile, inspectT: null, pointer: null };
const model = computeChartModel(state, defaultViewport(8), { width: 500, height: 300 });
const price = model.price!;

const long = (entry: number, target: number, stop: number, extra: Partial<Drawing> = {}): Drawing => ({
  id: "p",
  kind: "long",
  a: { t: model.slots[20]!.t, p: entry },
  b: { t: model.slots[32]!.t, p: target },
  c: { t: model.slots[32]!.t, p: stop },
  risk: 100,
  ...extra,
});

describe("positionPlan", () => {
  test("sizes from risk: $100 at $0.07 a share is 1,428 shares, R:R 1.71", () => {
    const p = positionPlan(long(3.38, 3.5, 3.31));
    expect(p.qty).toBe(1428);
    expect(p.reward).toBeCloseTo(171.36, 6);
    expect(p.risk).toBeCloseTo(99.96, 6);
    expect(p.rr).toBeCloseTo(0.12 / 0.07, 9);
    expect(p.valid).toBe(true);
  });

  test("an exact division is not lost to float noise ($100 at $0.05 = 2,000)", () => {
    expect(positionPlan(long(3.4, 3.5, 3.35)).qty).toBe(2000);
  });

  test("a share override wins over sizing; a short mirrors the checks", () => {
    expect(positionPlan(long(3.38, 3.5, 3.31, { qty: 500 })).qty).toBe(500);
    const s = positionPlan({ ...long(3.38, 3.2, 3.45), kind: "short" });
    expect(s.valid).toBe(true);
    expect(positionPlan({ ...long(3.38, 3.5, 3.31), kind: "short" }).valid).toBe(false);
  });
});

describe("placing and editing", () => {
  test("a new long: stop below, target at twice the distance above, prices on the tick grid", () => {
    const at = { x: slotX(model.index, 30), y: priceY(price, (price.lo + price.hi) / 2) };
    const d = newPosition(model, "n", "long", at)!;
    const p = positionPlan(d);
    expect(p.valid).toBe(true);
    expect(p.rr).toBeCloseTo(2, 6);
    for (const v of [p.entry, p.target, p.stop]) expect(Math.abs(v / tickSize(v) - Math.round(v / tickSize(v)))).toBeLessThan(1e-6);
  });

  test("dragging the target below the entry stops one tick above it", () => {
    const d = long(3.38, 3.5, 3.31);
    const moved = dragPosition(model, d, "b", { x: 0, y: 0 }, { x: 0, y: priceY(price, 3.0) });
    expect(moved.b.p).toBe(3.39);
    expect(moved.a).toEqual(d.a);
  });

  test("dragging the stop above the entry stops one tick below it", () => {
    const moved = dragPosition(model, long(3.38, 3.5, 3.31), "c", { x: 0, y: 0 }, { x: 0, y: priceY(price, 3.6) });
    expect(moved.c!.p).toBe(3.37);
  });

  test("moving the body keeps the distances exactly", () => {
    const d = long(3.38, 3.5, 3.31);
    const moved = dragPosition(model, d, "body", { x: 100, y: 100 }, { x: 116, y: 80 });
    expect(Number((moved.b.p - moved.a.p).toFixed(4))).toBe(0.12);
    expect(Number((moved.a.p - moved.c!.p).toFixed(4))).toBe(0.07);
  });

  test("the end handle never crosses the start", () => {
    const d = long(3.38, 3.5, 3.31);
    const moved = dragPosition(model, d, "end", { x: 0, y: 0 }, { x: 0, y: 0 });
    expect(moved.b.t).toBeGreaterThan(d.a.t);
    expect(moved.c!.t).toBe(moved.b.t);
  });

  test("flipping mirrors target and stop around the entry", () => {
    const f = flipPosition(long(3.38, 3.5, 3.31));
    expect(f.kind).toBe("short");
    expect(f.b.p).toBe(3.26);
    expect(f.c!.p).toBe(3.45);
  });
});

describe("hit testing and drawing", () => {
  const lo = price.lo + (price.hi - price.lo) * 0.3;
  const hi = price.lo + (price.hi - price.lo) * 0.7;
  const mid = (lo + hi) / 2;
  const d = long(Number(mid.toFixed(2)), Number(hi.toFixed(2)), Number(lo.toFixed(2)));

  test("handles first, then the zones", () => {
    const t = anchorXY(model, d.b)!;
    expect(hitDrawing(model, [d], t.x, t.y, 6)).toEqual({ id: "p", part: "b" });
    const s = anchorXY(model, d.c!)!;
    expect(hitDrawing(model, [d], s.x, s.y, 6)).toEqual({ id: "p", part: "c" });
    expect(hitDrawing(model, [d], (anchorXY(model, d.a)!.x + t.x) / 2, (t.y + s.y) / 2 + 10, 6)?.part).toBe("body");
  });

  test("draws two zones and the entry pill; selected adds three axis tags (above the tick labels)", () => {
    const plain = layoutDrawings(model, [d], null, DEFAULT_CHART_THEME);
    expect(plain.filter((p) => p.kind === "rect" && p.layer === "overlay" && !p.radius).length).toBe(2);
    const sel = layoutDrawings(model, [d], "p", DEFAULT_CHART_THEME);
    expect(sel.filter((p) => p.kind === "rect" && p.layer === "crosshair").length).toBe(3);
  });
});
