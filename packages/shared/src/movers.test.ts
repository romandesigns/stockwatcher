import { describe, expect, test } from "bun:test";
import { applyMoversMessage, createMoversChannel, DEFAULT_MOVER_SORT, EMPTY_MOVERS, moverLine, moversRows, nextMoverSort, parseMoverSort, sortMovers } from "./movers.ts";
import type { MoverRowWire } from "./protocol.ts";

const row = (symbol: string, o: Partial<MoverRowWire> = {}): MoverRowWire => ({ symbol, price: 2_000_000, ref: 1_000_000, chg: 100, dv: 1_000_000, volume: 500_000, trades: 900, ...o });

const snap = applyMoversMessage(EMPTY_MOVERS, {
  type: "movers.snapshot",
  t: 1,
  gainers: ["AAA"],
  bull: ["AAA", "BBB"],
  bear: ["CCC"],
  rows: [row("AAA", { day_line_start: 10, day_line_step_s: 30, day_line_from: 0, day_line: [1, 2, 3] }), row("BBB", { chg: 5, dv: 3_000_000 }), row("CCC", { chg: -12, dv: 2_000_000, price: 800_000 })],
}).state;

describe("applyMoversMessage", () => {
  test("a snapshot sets the lists and the rows", () => {
    expect(moversRows(snap, "bull").map((r) => r.symbol)).toEqual(["AAA", "BBB"]);
    expect(moversRows(snap, "bear").map((r) => r.symbol)).toEqual(["CCC"]);
    expect(snap.connected).toBe(true);
  });

  test("an update keeps unchanged rows by identity, merges slices, replaces only the lists it carries, drops gone", () => {
    const { state, resync } = applyMoversMessage(snap, {
      type: "movers.update",
      t: 2,
      bull: ["BBB", "AAA", "DDD"],
      rows: [row("AAA", { price: 2_100_000, day_line_start: 10, day_line_step_s: 30, day_line_from: 3, day_line: [4] }), row("DDD", { day_line_start: 10, day_line_from: 0, day_line: [9, 9] })],
      gone: ["CCC"],
    });
    expect(resync).toBe(false);
    expect(state.rows!.get("AAA")!.day_line).toEqual([1, 2, 3, 4]);
    expect(state.rows!.get("BBB")).toBe(snap.rows!.get("BBB"));
    expect(state.rows!.has("CCC")).toBe(false);
    expect(state.gainers).toEqual(["AAA"]);
    expect(state.bear).toEqual(["CCC"]);
    // A list member without a row is skipped.
    expect(moversRows(state, "bear")).toEqual([]);
    expect(moversRows(state, "bull").map((r) => r.symbol)).toEqual(["BBB", "AAA", "DDD"]);
  });

  test("a slice it cannot place drops the line and asks for a snapshot", () => {
    const { state, resync } = applyMoversMessage(snap, { type: "movers.update", t: 2, rows: [row("AAA", { day_line_start: 10, day_line_from: 7, day_line: [8] })] });
    expect(resync).toBe(true);
    expect(state.rows!.get("AAA")!.day_line).toBeUndefined();
  });

  test("the channel opts in with the first listener, out with the last, and re-subscribes on a gap", () => {
    const calls: string[] = [];
    const ch = createMoversChannel({ subscribe: () => calls.push("sub"), unsubscribe: () => calls.push("unsub") });
    const off = ch.subscribe(() => {});
    ch.apply({ type: "movers.snapshot", t: 1, gainers: [], bull: ["AAA"], bear: [], rows: [row("AAA", { day_line_start: 1, day_line_from: 0, day_line: [1] })] });
    ch.apply({ type: "movers.update", t: 2, rows: [row("AAA", { day_line_start: 2, day_line_from: 3, day_line: [1] })] });
    off();
    expect(calls).toEqual(["sub", "sub", "unsub"]);
    expect(ch.get()).toBe(EMPTY_MOVERS);
  });
});

describe("sorting", () => {
  const rows = [row("BBB", { dv: 2, chg: 5, price: 3 }), row("AAA", { dv: 3, chg: 1, price: 1 }), row("CCC", { dv: 1, chg: 9, price: 2 })];
  const syms = (r: MoverRowWire[]) => r.map((x) => x.symbol);

  test("each key in its natural order, and reversed", () => {
    expect(syms(sortMovers(rows, DEFAULT_MOVER_SORT, "bull"))).toEqual(["AAA", "BBB", "CCC"]);
    expect(syms(sortMovers(rows, { key: "chg", dir: 1 }, "bull"))).toEqual(["CCC", "BBB", "AAA"]);
    expect(syms(sortMovers(rows, { key: "price", dir: 1 }, "gainers"))).toEqual(["BBB", "CCC", "AAA"]);
    expect(syms(sortMovers(rows, { key: "symbol", dir: 1 }, "bull"))).toEqual(["AAA", "BBB", "CCC"]);
    expect(syms(sortMovers(rows, { key: "symbol", dir: -1 }, "bull"))).toEqual(["CCC", "BBB", "AAA"]);
    expect(syms(sortMovers(rows, { key: "dv", dir: -1 }, "bull"))).toEqual(["CCC", "BBB", "AAA"]);
  });

  test("Bear by change puts the most negative first", () => {
    const bear = [row("A", { chg: -3 }), row("B", { chg: -20 }), row("C", { chg: -8 })];
    expect(syms(sortMovers(bear, { key: "chg", dir: 1 }, "bear"))).toEqual(["B", "C", "A"]);
    expect(syms(sortMovers(bear, { key: "chg", dir: -1 }, "bear"))).toEqual(["A", "C", "B"]);
  });

  test("ties keep the server's order", () => {
    const tied = [row("Z", { dv: 1 }), row("A", { dv: 1 })];
    expect(syms(sortMovers(tied, DEFAULT_MOVER_SORT, "bull"))).toEqual(["Z", "A"]);
  });

  test("a second press flips; another key starts natural; junk parses to the default", () => {
    expect(nextMoverSort(DEFAULT_MOVER_SORT, "dv")).toEqual({ key: "dv", dir: -1 });
    expect(nextMoverSort({ key: "dv", dir: -1 }, "dv")).toEqual({ key: "dv", dir: 1 });
    expect(nextMoverSort({ key: "dv", dir: -1 }, "chg")).toEqual({ key: "chg", dir: 1 });
    expect(parseMoverSort({ key: "price", dir: -1 })).toEqual({ key: "price", dir: -1 });
    expect(parseMoverSort({ key: "rvol", dir: 1 })).toBe(DEFAULT_MOVER_SORT);
    expect(parseMoverSort("dv")).toBe(DEFAULT_MOVER_SORT);
  });
});

test("the row line ends at the live price and tones against the previous close", () => {
  const l = moverLine(row("AAA", { price: 900_000, pc: 1_000_000, day_line: [1_000_000, 1_100_000] }))!;
  expect(l.values).toEqual([1_000_000, 1_100_000, 900_000]);
  expect(l.tone).toBe("critical");
  expect(moverLine(row("AAA"))).toBeNull();
});
