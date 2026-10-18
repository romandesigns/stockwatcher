import { expect, test } from "bun:test";
import { createChartRouter } from "./chartRouter.ts";
import type { ChartMessage } from "./types.ts";

const update = (symbol: string, timeframe: "1m" | "5m", profileBin = 0.01) =>
  ({ type: "chart.update", symbol, timeframe, session: "s", candle: { t: 1, o: 1, h: 1, l: 1, c: 1, v: 1, n: 1, final: false }, profileBin, profile: [] }) as unknown as ChartMessage;
const tape = (symbol: string) => ({ type: "tape.trade", symbol, t: 1 }) as unknown as ChartMessage;
const clock = { type: "replay.clock", clock: { mode: "replay", now: 1, speed: 1, playing: true } } as unknown as ChartMessage;

function rig() {
  const r = createChartRouter();
  const got: Record<string, string[]> = { a: [], b: [], c: [] };
  const on = (id: string) => (m: ChartMessage) => got[id]!.push(`${m.type}:${"symbol" in m ? m.symbol : ""}:${"timeframe" in m ? m.timeframe : ""}`);
  return { r, got, on };
}

test("candle frames go to the slots on that symbol, timeframe and bin", () => {
  const { r, got, on } = rig();
  r.add("a", { symbol: "aal", timeframe: "1m", profileBin: 0.01 }, on("a"));
  r.add("b", { symbol: "AAL", timeframe: "5m", profileBin: 0.01 }, on("b"));
  r.add("c", { symbol: "TSLA", timeframe: "1m", profileBin: 0.01 }, on("c"));
  r.dispatch(update("AAL", "1m"));
  r.dispatch(update("AAL", "1m", 0.05));
  expect(got.a).toEqual(["chart.update:AAL:1m"]);
  expect(got.b).toEqual([]);
  expect(got.c).toEqual([]);
});

test("one frame reaches every slot showing the same chart (the server sends it once)", () => {
  const { r, got, on } = rig();
  r.add("a", { symbol: "AAL", timeframe: "1m", profileBin: 0.01 }, on("a"));
  r.add("b", { symbol: "AAL", timeframe: "1m", profileBin: 0.01 }, on("b"));
  r.dispatch(update("AAL", "1m"));
  expect(got.a).toHaveLength(1);
  expect(got.b).toHaveLength(1);
});

test("tape and halts go by symbol whatever the timeframe; the clock goes everywhere", () => {
  const { r, got, on } = rig();
  r.add("a", { symbol: "AAL", timeframe: "1m", profileBin: 0.01 }, on("a"));
  r.add("b", { symbol: "AAL", timeframe: "5m", profileBin: 0.01 }, on("b"));
  r.add("c", { symbol: "TSLA", timeframe: "1m", profileBin: 0.01 }, on("c"));
  r.dispatch(tape("AAL"));
  r.dispatch(clock);
  expect(got.a).toEqual(["tape.trade:AAL:", "replay.clock::"]);
  expect(got.b).toEqual(["tape.trade:AAL:", "replay.clock::"]);
  expect(got.c).toEqual(["replay.clock::"]);
});

test("re-adding a slot replaces its subscription; removing stops it", () => {
  const { r, got, on } = rig();
  r.add("a", { symbol: "AAL", timeframe: "1m", profileBin: 0.01 }, on("a"));
  r.add("a", { symbol: "TSLA", timeframe: "1m", profileBin: 0.01 }, on("a"));
  r.dispatch(update("AAL", "1m"));
  r.dispatch(update("TSLA", "1m"));
  r.remove("a");
  r.dispatch(update("TSLA", "1m"));
  expect(got.a).toEqual(["chart.update:TSLA:1m"]);
  expect(r.entries()).toEqual([]);
});

test("a snapshot without a profile matches on symbol and timeframe", () => {
  const { r, got, on } = rig();
  r.add("a", { symbol: "AAL", timeframe: "1m", profileBin: 0.02 }, on("a"));
  r.dispatch({ type: "chart.snapshot", symbol: "AAL", timeframe: "1m", profile: null } as unknown as ChartMessage);
  r.dispatch({ type: "chart.snapshot", symbol: "AAL", timeframe: "1m", profile: { bin: 0.01 } } as unknown as ChartMessage);
  expect(got.a).toEqual(["chart.snapshot:AAL:1m"]);
});
