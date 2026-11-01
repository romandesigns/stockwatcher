import { expect, test } from "bun:test";
import { createChartRouter } from "./chartRouter.ts";
import { computeIndicators, indicatorDemand, INDICATOR_PRESETS, indicatorParity, serverIndicatorResults, specKey, type IndicatorSpec } from "./indicators.ts";
import { buildSlots, type Slot } from "./scale.ts";
import { createServerIndicators } from "./serverIndicators.ts";
import { applyIndicatorIncrement, applyIndicatorsSnapshot, ChartStore } from "./store.ts";
import { sessionSlots } from "./chartTypes.ts";
import type { Candle, ChartIndicatorsSnapshot, ChartMessage, Gap } from "./types.ts";

// The client half of `indicators.v1`: the store folds the server's frames, and the chart draws
// the values through the same metadata as the local math. The server's own parity with
// chart-core is proven in crates/chart-engine/tests/indicator_parity.rs; here the question is
// only whether the client puts every value it is given on the right slot.

const T0 = Date.UTC(2026, 9, 15, 13, 30); // 09:30 ET
const MIN = 60_000;

function candles(n: number, from = T0): Candle[] {
  return Array.from({ length: n }, (_, i) => {
    const c = 10 + Math.sin(i / 3) + i * 0.01;
    return { t: from + i * MIN, o: c - 0.02, h: c + 0.05, l: c - 0.05, c, v: 100 + ((i * 37) % 900), n: 3, final: true };
  });
}

/** What the server would send for `specs` over the candle slots: the local values, keyed and trimmed like the wire. */
function serverSnapshot(slots: readonly Slot[], specs: readonly IndicatorSpec[], rth = false): ChartIndicatorsSnapshot {
  const local = computeIndicators(slots, specs);
  const idx = slots.flatMap((s, i) => (s.kind === "candle" ? [i] : []));
  return {
    type: "chart.indicators",
    symbol: "SMPL",
    timeframe: "1m",
    rth,
    t: idx.map((i) => slots[i]!.t),
    studies: local.map((r) => ({
      key: specKey(r.spec),
      lines: r.lines.map((l) => {
        const vals = idx.map((i) => (Number.isNaN(l.values[i]!) ? null : l.values[i]!));
        const from = Math.max(0, vals.findIndex((v) => v !== null));
        return { name: l.name, style: l.style, from, values: vals.slice(from) };
      }),
    })),
  };
}

const ALL: IndicatorSpec[] = [INDICATOR_PRESETS[0]!, INDICATOR_PRESETS[1]!, INDICATOR_PRESETS[4]!, INDICATOR_PRESETS[5]!, INDICATOR_PRESETS[6]!];

test("server values land on the same slots as the local math, gaps and all, with the same metadata", () => {
  const cs = candles(120);
  // A gap slot in the middle: the server's `t` has no entry for it, the slot must stay NaN.
  const gaps: Gap[] = [{ t: cs[60]!.t + MIN / 2, end: cs[61]!.t, reason: "no_trades", v: 0, intervals: 1 }];
  const slots = buildSlots(cs, gaps);
  const local = computeIndicators(slots, ALL);
  const values = applyIndicatorsSnapshot(undefined, serverSnapshot(slots, ALL));
  const server = serverIndicatorResults(slots, ALL, values);
  expect(server.map(({ lines, ...meta }) => ({ ...meta, lines: lines.map((l) => [l.name, l.style]) }))).toEqual(local.map(({ lines, ...meta }) => ({ ...meta, lines: lines.map((l) => [l.name, l.style]) })));
  server.forEach((r, k) => r.lines.forEach((l, j) => expect(Array.from(l.values)).toEqual(Array.from(local[k]!.lines[j]!.values))));
  expect(Object.values(indicatorParity(server, local)).every((p) => p.maxAbs === 0 && p.onlyOne === 0 && p.compared > 0)).toBe(true);
});

test("rth values join onto the regular-session slots only", () => {
  // 08:00–10:59 ET: premarket then regular.
  const cs = candles(180, T0 - 90 * MIN);
  const shown = sessionSlots(buildSlots(cs, []), false);
  const values = applyIndicatorsSnapshot(undefined, serverSnapshot(shown, ALL, true));
  const server = serverIndicatorResults(shown, ALL, values);
  const local = computeIndicators(shown, ALL);
  server.forEach((r, k) => r.lines.forEach((l, j) => expect(Array.from(l.values)).toEqual(Array.from(local[k]!.lines[j]!.values))));
});

test("a key the server has not answered draws nothing rather than local lines", () => {
  const slots = buildSlots(candles(40), []);
  const r = serverIndicatorResults(slots, [INDICATOR_PRESETS[1]!], null);
  expect(r).toHaveLength(1);
  expect(Array.from(r[0]!.lines[0]!.values).every(Number.isNaN)).toBe(true);
});

test("increments: forming bar, new bar, revision, insert, remove, unknown keys", () => {
  const slots = buildSlots(candles(30), []);
  const ema = INDICATOR_PRESETS[1]!; // ema:9
  let v = applyIndicatorsSnapshot(undefined, serverSnapshot(slots, [ema]));
  const last = v.t.at(-1)!;
  const line = () => v.studies.get("ema:9")!.lines[0]!.values;
  const base = { type: "chart.indicator" as const, symbol: "SMPL", timeframe: "1m" as const, rth: false };
  // Forming bar: replaced in place; an unknown key (another socket's study) is ignored.
  let next = applyIndicatorIncrement(v, { ...base, points: [{ t: last, v: { "ema:9": [1.5], "sma:50": [2] } }] });
  expect(next).not.toBe(v);
  v = next;
  expect(line().at(-1)).toBe(1.5);
  expect(v.studies.has("sma:50")).toBe(false);
  // New bar, then a revision of an older one and an insert between two.
  v = applyIndicatorIncrement(v, { ...base, points: [{ t: last + MIN, v: { "ema:9": [2.5] } }] });
  v = applyIndicatorIncrement(v, { ...base, points: [{ t: v.t[10]!, v: { "ema:9": [null] } }, { t: v.t[11]! + MIN / 2, v: {} }] });
  expect(v.t).toHaveLength(32);
  expect(Number.isNaN(line()[10]!)).toBe(true);
  expect(v.t[12]).toBe(v.t[11]! + MIN / 2);
  expect(Number.isNaN(line()[12]!)).toBe(true);
  expect(line().at(-1)).toBe(2.5);
  // Removal first, then the points.
  const gone = v.t[12]!;
  v = applyIndicatorIncrement(v, { ...base, removed: [gone], points: [] });
  expect(v.t.includes(gone)).toBe(false);
  expect(line()).toHaveLength(v.t.length);
});

test("the ring bound trims old bars and ignores revisions older than it", () => {
  const slots = buildSlots(candles(50), []);
  let v = applyIndicatorsSnapshot(undefined, serverSnapshot(slots, [INDICATOR_PRESETS[1]!]), 40);
  expect(v.t).toHaveLength(40);
  const first = v.t[0]!;
  v = applyIndicatorIncrement(v, { type: "chart.indicator", symbol: "SMPL", timeframe: "1m", rth: false, points: [{ t: first - MIN, v: { "ema:9": [9] } }] }, 40);
  expect(v.t[0]).toBe(first);
  v = applyIndicatorIncrement(v, { type: "chart.indicator", symbol: "SMPL", timeframe: "1m", rth: false, points: [{ t: v.t.at(-1)! + MIN, v: { "ema:9": [9] } }] }, 40);
  expect(v.t).toHaveLength(40);
  expect(v.t[0]).toBe(first + MIN);
});

test("the store keys the values by rth, and drops increments before a snapshot", () => {
  const store = new ChartStore();
  const slots = buildSlots(candles(20), []);
  const inc = { type: "chart.indicator" as const, symbol: "SMPL", timeframe: "1m" as const, rth: true, points: [{ t: T0, v: { "ema:9": [1] } }] };
  expect(store.applyIndicatorIncrement(inc)).toBe(false);
  store.applyIndicators(serverSnapshot(slots, [INDICATOR_PRESETS[1]!], true));
  expect(store.getIndicators("SMPL", "1m", true)).toBeDefined();
  expect(store.getIndicators("SMPL", "1m", false)).toBeUndefined();
  expect(store.applyIndicatorIncrement(inc)).toBe(true);
});

test("the router hands indicator frames to every slot of the chart, whatever its bin", () => {
  const router = createChartRouter();
  const got: Record<string, ChartMessage[]> = { a: [], b: [], c: [] };
  router.add("a", { symbol: "smpl", timeframe: "1m", profileBin: 0.01 }, (m) => got.a!.push(m));
  router.add("b", { symbol: "SMPL", timeframe: "1m", profileBin: 0.05 }, (m) => got.b!.push(m));
  router.add("c", { symbol: "SMPL", timeframe: "5m", profileBin: 0.01 }, (m) => got.c!.push(m));
  router.dispatch({ type: "chart.indicator", symbol: "SMPL", timeframe: "1m", rth: false, points: [] });
  expect([got.a!.length, got.b!.length, got.c!.length]).toEqual([1, 1, 0]);
});

test("the feed side sends nothing until the server offers the feature, then carries the demand", () => {
  const sent: object[] = [];
  const held = new Set<string>();
  const si = createServerIndicators((f) => sent.push(f), (id) => held.has(id));
  const demand = indicatorDemand([INDICATOR_PRESETS[1]!], { extendedHours: false, visibility: { indicators: true } });
  let changes = 0;
  si.channel.subscribe(() => changes++);
  // An older server: the demand is kept, nothing goes on the wire, the chart computes its own.
  si.held("d0", demand);
  held.add("d0");
  expect(si.subscribeFields("d0")).toEqual({});
  si.channel.set("d0", indicatorDemand([], { extendedHours: false, visibility: { indicators: true } }));
  expect(sent).toEqual([]);
  expect(si.channel.available()).toBe(false);
  // A welcome with indicators.v1: the resubscribe carries the latest demand.
  si.setAvailable(true);
  expect(changes).toBe(1);
  expect(si.subscribeFields("d0")).toEqual({ indicators: [], rth: true });
  si.channel.set("d0", demand);
  expect(sent).toEqual([{ type: "chart.indicators.set", id: "d0", indicators: demand.indicators, rth: true }]);
  // The same demand again sends nothing; a slot not held sends nothing (its subscribe will).
  si.channel.set("d0", { ...demand, indicators: [...demand.indicators] });
  si.channel.set("d1", demand);
  expect(sent).toHaveLength(1);
  // The default slot goes without an id.
  held.add("");
  si.channel.set(undefined, demand);
  expect(sent.at(-1)).toEqual({ type: "chart.indicators.set", indicators: demand.indicators, rth: true });
  // Hidden indicators: an empty list (the server computes nothing nobody sees).
  expect(indicatorDemand(ALL, { extendedHours: true, visibility: { indicators: false } })).toEqual({ indicators: [], rth: false, zones: false });
});

test("dropIndicators forgets one rth's frozen values until its next snapshot", () => {
  const store = new ChartStore();
  const slots = buildSlots(candles(20), []);
  store.applyIndicators(serverSnapshot(slots, [INDICATOR_PRESETS[1]!], false));
  store.applyIndicators(serverSnapshot(slots, [INDICATOR_PRESETS[1]!], true));
  let emits = 0;
  store.subscribe(() => emits++);
  store.dropIndicators("SMPL", "1m", false);
  expect(store.getIndicators("SMPL", "1m", false)).toBeUndefined();
  expect(store.getIndicators("SMPL", "1m", true)).toBeDefined();
  // Nothing held: no change, no render.
  store.dropIndicators("SMPL", "1m", false);
  expect(emits).toBe(1);
  // Increments for it wait for the snapshot again.
  expect(store.applyIndicatorIncrement({ type: "chart.indicator", symbol: "SMPL", timeframe: "1m", rth: false, points: [] })).toBe(false);
});
