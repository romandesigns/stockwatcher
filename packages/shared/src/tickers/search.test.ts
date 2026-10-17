import { expect, test } from "bun:test";
import { tickerUniverse } from "./mockTickers.ts";
import { pushRecentTicker, searchTickers, type TickerFilterId } from "./search.ts";

const universe = tickerUniverse(["SMPL", "FXLO"]);
const on = (...ids: TickerFilterId[]) => new Set(ids);

test("no query: recents first, then the simulator, then the mock universe, each titled", () => {
  const rows = searchTickers(universe, "", on(), ["FXLO"]);
  expect(rows[0]!.item.symbol).toBe("FXLO");
  expect(rows[0]!.section).toBe("recent");
  expect(rows[1]!.item.symbol).toBe("SMPL");
  expect(rows[1]!.section).toBe("sim");
  const firstMock = rows.findIndex((r) => r.item.mock);
  expect(rows[firstMock]!.section).toBe("mock");
  expect(rows.filter((r) => r.section).length).toBe(3);
});

test("filters: alternatives within a group, narrowing across groups", () => {
  const nyse = searchTickers(universe, "", on("nyse"), []);
  expect(nyse.every((r) => r.item.exchange === "NYSE")).toBe(true);
  const both = searchTickers(universe, "", on("nyse", "nasdaq"), []);
  expect(both.length).toBeGreaterThan(nyse.length);
  const gainersNyse = searchTickers(universe, "", on("nyse", "gainers"), []);
  expect(gainersNyse.every((r) => r.item.exchange === "NYSE" && (r.item.changePct ?? 0) > 0)).toBe(true);
});

test("a mover filter sorts by the size of the move", () => {
  const moves = searchTickers(universe, "", on("losers"), []).map((r) => Math.abs(r.item.changePct ?? 0));
  expect(moves).toEqual([...moves].sort((a, b) => b - a));
});

test("a query ranks and still honours the filters", () => {
  const rows = searchTickers(universe, "fx", on("sim"), []);
  expect(rows[0]!.item.symbol).toBe("FXLO");
  expect(rows.every((r) => r.item.available)).toBe(true);
});

test("recents: newest first, no duplicates, capped", () => {
  let r: string[] = [];
  for (const s of ["A", "B", "C", "A", "D", "E", "F"]) r = pushRecentTicker(r, s);
  expect(r).toEqual(["F", "E", "D", "A", "C"]);
});
