import { describe, expect, test } from "bun:test";
import { changeReference, formatChange } from "./format.ts";
import { computeChartModel, type ChartState } from "./layout.ts";
import { DEFAULT_LAYERS, keyLevels, layoutFills, layoutHalts, layoutKeyLevels, layoutLuld, parseLayers } from "./levels.ts";
import { buildSlots, defaultViewport } from "./scale.ts";
import { ChartStore } from "./store.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";
import { etToUtc } from "./time.ts";
import { fixtureSnapshot, ZERO_QUALITY } from "./testFixture.ts";
import type { Candle } from "./types.ts";

const c = (t: number, o: number, h: number, l: number, cl: number): Candle => ({ t, o, h, l, c: cl, v: 1000, n: 5, final: true });

describe("keyLevels", () => {
  const prev = [c(etToUtc(2026, 10, 14, 15, 58), 3.0, 3.05, 2.98, 3.02), c(etToUtc(2026, 10, 14, 15, 59), 3.02, 3.04, 3.0, 3.01)];
  const pre = [c(etToUtc(2026, 10, 15, 8, 0), 3.1, 3.3, 3.05, 3.2), c(etToUtc(2026, 10, 15, 9, 0), 3.2, 3.35, 3.15, 3.3)];
  const reg = [c(etToUtc(2026, 10, 15, 9, 30), 3.32, 3.4, 3.28, 3.38), c(etToUtc(2026, 10, 15, 9, 31), 3.38, 3.5, 3.36, 3.45)];

  test("premarket high/low, high/low of day, open and previous close of the latest trading day", () => {
    const levels = keyLevels(buildSlots([...prev, ...pre, ...reg], []));
    const by = Object.fromEntries(levels.map((l) => [l.id, l.price]));
    expect(by).toEqual({ pmHigh: 3.35, pmLow: 3.05, hod: 3.5, lod: 3.28, open: 3.32, prevClose: 3.01 });
  });

  test("a level with no candles behind it is left out, not guessed", () => {
    const ids = keyLevels(buildSlots(pre, [])).map((l) => l.id);
    expect(ids).toEqual(["pmHigh", "pmLow"]);
  });

  test("the server's official previous close draws the level a single-day chart cannot derive, and wins over the candles'", () => {
    const one = keyLevels(buildSlots([...pre, ...reg], []), 3.07);
    expect(one.find((l) => l.id === "prevClose")).toEqual({ id: "prevClose", label: "Prev close", price: 3.07, layer: "prevClose" });
    const both = keyLevels(buildSlots([...prev, ...pre, ...reg], []), 3.07);
    expect(both.find((l) => l.id === "prevClose")!.price).toBe(3.07);
    // Unknown (null) falls back to the candles, and still nothing is guessed without them.
    expect(keyLevels(buildSlots([...prev, ...reg], []), null).find((l) => l.id === "prevClose")!.price).toBe(3.01);
    expect(keyLevels(buildSlots(reg, []), null).some((l) => l.id === "prevClose")).toBe(false);
  });
});

describe("changeReference (the header's change %)", () => {
  const session = { start: etToUtc(2026, 10, 15, 9, 30) };
  const candles = [c(etToUtc(2026, 10, 15, 8, 0), 3.1, 3.3, 3.05, 3.2), c(etToUtc(2026, 10, 15, 9, 30), 3.32, 3.4, 3.28, 3.38)];
  test("the previous close when known, else the anchor session's first candle", () => {
    expect(changeReference({ prevClose: 3.01, session, candles })).toEqual({ price: 3.01, kind: "prevClose" });
    expect(changeReference({ prevClose: null, session, candles })).toEqual({ price: 3.32, kind: "sessionOpen" });
    expect(changeReference({ prevClose: 0, session, candles })!.kind).toBe("sessionOpen");
    expect(changeReference({ prevClose: null, session: null, candles })).toBeUndefined();
    expect(changeReference(null)).toBeUndefined();
    // The store carries the snapshot's prevClose to the series the header reads.
    const snap = fixtureSnapshot("1m", 3);
    const store = new ChartStore();
    store.applySnapshot({ ...snap, prevClose: 2.5 });
    const s = store.getSeries(snap.symbol, "1m")!;
    expect(s.prevClose).toBe(2.5);
    expect(formatChange(changeReference(s)!.price, 2.75)).toBe("+10.00%");
    // Static for the day: an increment keeps it, a snapshot without it clears it.
    const last = snap.candles[snap.candles.length - 1]!;
    store.applyIncrement({ type: "chart.update", symbol: snap.symbol, timeframe: "1m", session: snap.session!.id, candle: { ...last, c: last.c + 0.01 }, profileBin: 0.01, profile: [] });
    expect(store.getSeries(snap.symbol, "1m")!.prevClose).toBe(2.5);
    store.applySnapshot(snap);
    expect(store.getSeries(snap.symbol, "1m")!.prevClose).toBeNull();
  });
});

test("parseLayers keeps defaults for anything missing or malformed", () => {
  expect(parseLayers({ halts: false, luld: "yes" })).toEqual({ ...DEFAULT_LAYERS, halts: false });
  expect(parseLayers(null)).toEqual(DEFAULT_LAYERS);
});

test("Volume shelves: a layer, on by default, kept on for a set stored before it existed, and its off survives a round trip", () => {
  expect(DEFAULT_LAYERS.shelves).toBe(true);
  expect(parseLayers({ halts: false, luld: true, openingRange: true }).shelves).toBe(true);
  const off = parseLayers(JSON.parse(JSON.stringify({ ...DEFAULT_LAYERS, shelves: false })));
  expect(off).toEqual({ ...DEFAULT_LAYERS, shelves: false });
});

describe("drawing the context layers", () => {
  const snap = fixtureSnapshot("1m", 40);
  const state: ChartState = { timeframe: "1m", candles: snap.candles, gaps: snap.gaps, profile: snap.profile, inspectT: null, pointer: null };
  const model = computeChartModel(state, defaultViewport(8), { width: 500, height: 300 });
  const t = (i: number) => model.slots[i]!.t;
  const mid = (model.price!.lo + model.price!.hi) / 2;

  test("a finished halt is shaded with its length; an ongoing one adds the HALTED badge", () => {
    const done = layoutHalts(model, [{ start: t(10), end: t(12), reason: "LUDP", code: "H" }], null, DEFAULT_CHART_THEME);
    expect(done.some((p) => p.kind === "text" && p.text === "HALT LUDP · 2m")).toBe(true);
    const on = layoutHalts(model, [{ start: t(30), end: null, reason: "", code: "H" }], t(30) + 95_000, DEFAULT_CHART_THEME);
    expect(on.some((p) => p.kind === "text" && p.text === "HALTED · 1m 35s")).toBe(true);
  });

  test("LULD bands step at each change and tag the current pair", () => {
    const list = layoutLuld(
      model,
      [
        { t: t(5), ref: mid, up: mid + 0.1, down: mid - 0.1 },
        { t: t(20), ref: mid + 0.02, up: mid + 0.12, down: mid - 0.08 },
      ],
      DEFAULT_CHART_THEME,
    );
    expect(list.filter((p) => p.kind === "line" && p.dash).length).toBe(6); // 2 steps × 2 bands + 2 risers
    expect(list.some((p) => p.kind === "text" && p.text.includes("LULD ▲"))).toBe(true);
    // Bands far outside the view are still tagged, pinned to the edges with arrows.
    const far = layoutLuld(model, [{ t: t(5), ref: mid, up: mid + 5, down: mid - 5 }], DEFAULT_CHART_THEME);
    expect(far.some((p) => p.kind === "text" && p.text.startsWith("↑ LULD ▲"))).toBe(true);
    expect(far.some((p) => p.kind === "text" && p.text.startsWith("↓ LULD ▼"))).toBe(true);
  });

  test("a level off the visible range is pinned to that edge as a tag, with no line", () => {
    const list = layoutKeyLevels(model, [{ id: "hod", label: "HOD", price: model.price!.hi + 1, layer: "dayRange" }], DEFAULT_CHART_THEME);
    expect(list.some((p) => p.kind === "line")).toBe(false);
    expect(list.some((p) => p.kind === "text" && p.text.startsWith("↑ HOD"))).toBe(true);
  });

  test("key levels get a labelled line each", () => {
    const list = layoutKeyLevels(model, [{ id: "hod", label: "HOD", price: mid, layer: "dayRange" }], DEFAULT_CHART_THEME);
    expect(list.some((p) => p.kind === "text" && p.text.startsWith("HOD "))).toBe(true);
  });

  test("a buy fill is an up-arrow with its tip at the price, a sell a down-arrow", () => {
    const list = layoutFills(
      model,
      [
        { orderId: "o-1", symbol: "SMPL", side: "buy", qty: 100, price: mid, t: t(30) },
        { orderId: "o-2", symbol: "SMPL", side: "sell", qty: 100, price: mid, t: t(35) },
      ],
      DEFAULT_CHART_THEME,
    );
    const shapes = list.filter((p) => p.kind === "path" && p.fill);
    expect(shapes.length).toBe(2);
    const [buy, sell] = shapes as { points: number[] }[];
    expect(buy!.points[3]!).toBeGreaterThan(buy!.points[1]!); // base below the tip
    expect(sell!.points[3]!).toBeLessThan(sell!.points[1]!); // base above the tip
  });
});

test("the store hands halts and LULD points to every timeframe of the symbol", () => {
  const store = new ChartStore();
  for (const tf of ["1m", "5m"] as const) store.applySnapshot({ ...fixtureSnapshot(tf, 5), symbol: "SMPL", quality: ZERO_QUALITY });
  store.applySymbolEvent({ type: "chart.halt", symbol: "SMPL", halt: { start: 1, end: null, reason: "", code: "H" } });
  store.applySymbolEvent({ type: "chart.halt", symbol: "SMPL", halt: { start: 1, end: 9, reason: "", code: "H" } });
  store.applySymbolEvent({ type: "chart.luld", symbol: "SMPL", point: { t: 5, ref: 3, up: 3.3, down: 2.7 } });
  for (const tf of ["1m", "5m"] as const) {
    const s = store.getSeries("SMPL", tf)!;
    expect(s.halts).toEqual([{ start: 1, end: 9, reason: "", code: "H" }]);
    expect(s.luld.length).toBe(1);
  }
});
