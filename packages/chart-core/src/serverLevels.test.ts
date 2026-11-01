import { expect, test } from "bun:test";
import { createChartRouter } from "./chartRouter.ts";
import { indicatorDemand, sameIndicatorDemand } from "./indicators.ts";
import { keyLevels, openingRanges, serverKeyLevels } from "./levels.ts";
import { buildSlots } from "./scale.ts";
import { createServerIndicators } from "./serverIndicators.ts";
import { ChartStore } from "./store.ts";
import type { Candle, ChartIndicatorsSet, ChartLevels, ChartMessage, ChartZones } from "./types.ts";

// The client half of `levels.v1` / `zones.v1` (crates/server/src/chart_context.rs): the store keeps
// the server's whole-state frames, the router hands them to the charts they belong to, the feed
// channel asks for them only from a server that offers them, and the chart turns a levels frame
// into the same KeyLevel list it used to read off the candles. The server's parity with
// keyLevels/openingRanges/autoZones is proven on the Rust side (level_fixtures.json).

const T0 = Date.UTC(2026, 9, 15, 8, 0); // 04:00 ET
const MIN = 60_000;

function candles(): Candle[] {
  const at = (i: number, o: number, h: number, l: number, c: number): Candle => ({ t: T0 + i * MIN, o, h, l, c, v: 100, n: 1, final: true });
  // A premarket print at 04:00 and 09:10, then the open at 09:30 and a high at 09:31.
  return [at(0, 3, 3, 3, 3), at(310, 2.8, 2.8, 2.8, 2.8), at(330, 3.1, 3.2, 3.1, 3.2), at(331, 3.2, 3.4, 3.15, 3.3)];
}

function levelsFrame(over: Partial<ChartLevels> = {}): ChartLevels {
  return { type: "chart.levels", symbol: "SMPL", timeframe: "1m", day: "2026-10-15", pmHigh: 3, pmLow: 2.8, hod: 3.4, lod: 3.1, open: 3.1, prevClose: 2.5, openingRanges: [{ day: "2026-10-15", t: T0 + 330 * MIN, end: Date.UTC(2026, 9, 15, 20, 0), high: 3.2, low: 3.1, forming: false }], ...over };
}

test("a levels frame becomes the list keyLevels would give for the same candles", () => {
  const slots = buildSlots(candles(), []);
  expect(serverKeyLevels(levelsFrame())).toEqual(keyLevels(slots, 2.5));
  expect(levelsFrame().openingRanges).toEqual(openingRanges(slots, null));
  // Nothing known yet (no candle): no level, never a guess.
  expect(serverKeyLevels(levelsFrame({ day: null, pmHigh: null, pmLow: null, hod: null, lod: null, open: null, prevClose: null, openingRanges: [] }))).toEqual([]);
});

test("the store keeps levels per chart and zones per chart key, whole states replacing", () => {
  const store = new ChartStore();
  let emits = 0;
  store.subscribe(() => emits++);
  store.applyLevels(levelsFrame());
  store.applyLevels(levelsFrame({ hod: 3.5 }));
  expect(store.getLevels("SMPL", "1m")?.hod).toBe(3.5);
  expect(store.getLevels("SMPL", "5m")).toBeUndefined();
  const z: ChartZones = { type: "chart.zones", symbol: "SMPL", timeframe: "1m", rth: true, zones: [{ kind: "demand", lo: 3, hi: 3.1, t: T0, tested: false }] };
  store.applyZones(z);
  expect(store.getZones("SMPL", "1m", true)?.zones).toHaveLength(1);
  expect(store.getZones("SMPL", "1m", false)).toBeUndefined();
  store.dropZones("SMPL", "1m", true);
  expect(store.getZones("SMPL", "1m", true)).toBeUndefined();
  store.applyZones(z);
  store.clear("SMPL", "1m");
  expect(store.getLevels("SMPL", "1m")).toBeUndefined();
  expect(store.getZones("SMPL", "1m", true)).toBeUndefined();
  expect(emits).toBe(6);
});

test("levels and zones frames go to the slots on that symbol and timeframe, whatever the bin", () => {
  const router = createChartRouter();
  const got: Record<string, string[]> = { a: [], b: [], c: [] };
  router.add("a", { symbol: "SMPL", timeframe: "1m", profileBin: 0.01 }, (m) => got.a!.push(m.type));
  router.add("b", { symbol: "SMPL", timeframe: "1m", profileBin: 0.05 }, (m) => got.b!.push(m.type));
  router.add("c", { symbol: "SMPL", timeframe: "5m", profileBin: 0.01 }, (m) => got.c!.push(m.type));
  router.dispatch(levelsFrame() as ChartMessage);
  router.dispatch({ type: "chart.zones", symbol: "SMPL", timeframe: "5m", rth: false, zones: [] });
  expect(got).toEqual({ a: ["chart.levels"], b: ["chart.levels"], c: ["chart.zones"] });
});

test("the channel asks for levels and zones only from a server that offers them", () => {
  const sent: ChartIndicatorsSet[] = [];
  const held = new Set<string>(["d0"]);
  const si = createServerIndicators((f) => sent.push(f), (id) => held.has(id));
  const settings = { extendedHours: false, visibility: { indicators: true } };
  const off = indicatorDemand([], settings, { autoZones: false });
  const on = indicatorDemand([], settings, { autoZones: true });
  expect(on.zones).toBe(true);
  expect(sameIndicatorDemand(off, on)).toBe(false);
  si.held("d0", on);
  // An indicators.v1 server from before levels.v1: nothing about them on the wire.
  si.setAvailable(true);
  expect(si.subscribeFields("d0")).toEqual({ indicators: [], rth: true });
  si.channel.set("d0", off);
  expect(sent.at(-1)).toEqual({ type: "chart.indicators.set", id: "d0", indicators: [], rth: true });
  let changes = 0;
  si.channel.subscribe(() => changes++);
  si.setContext(true, true);
  expect(changes).toBe(1);
  expect(si.channel.levels() && si.channel.zones()).toBe(true);
  // Levels always; zones only while the layer is on.
  expect(si.subscribeFields("d0")).toEqual({ indicators: [], rth: true, levels: true });
  si.channel.set("d0", on);
  expect(sent.at(-1)).toEqual({ type: "chart.indicators.set", id: "d0", indicators: [], rth: true, zones: true });
  expect(si.subscribeFields("d0")).toEqual({ indicators: [], rth: true, levels: true, zones: true });
  si.setContext(true, true);
  expect(changes).toBe(1);
});
