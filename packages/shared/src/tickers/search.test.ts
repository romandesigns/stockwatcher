import { expect, test } from "bun:test";
import { tickerUniverse } from "./mockTickers.ts";
import { applyQuotesMessage, EMPTY_QUOTE_BOARD } from "./quotes.ts";
import { activeTickerFilters, pushRecentTicker, refreshTickerRows, searchTickers, visibleTickerFilterGroups, type TickerFilterId } from "./search.ts";

// A fixture source: two openable symbols plus the mock universe.
const universe = tickerUniverse(["SMPL", "FXLO"], EMPTY_QUOTE_BOARD, { mock: true });
const on = (...ids: TickerFilterId[]) => new Set(ids);

// A real source: live rows from the quote board, no mock rows.
const board = applyQuotesMessage(EMPTY_QUOTE_BOARD, {
  type: "quotes.snapshot",
  t: 0,
  quotes: [
    { s: "AAL", p: 12.5, o: 12.0, v: 900_000, n: 4000, name: "American Airlines Group Inc.", x: "NASDAQ" },
    { s: "F", p: 9.5, o: 10.0, v: 5_000_000, n: 20000, name: "Ford Motor Company", x: "NYSE" },
    { s: "SNDL", p: 0.8, o: 0.7, v: 2_000_000, n: 9000, x: "NASDAQ" },
    { s: "XYZ", p: 3.0, o: 3.0, v: 10, n: 1 },
  ],
});
const real = tickerUniverse(["AAL", "F", "SNDL", "XYZ"], board);

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

test("real data: live quote fields, real names, no mock rows and no Simulator/Mock chips", () => {
  expect(real.some((t) => t.mock)).toBe(false);
  const aal = real.find((t) => t.symbol === "AAL")!;
  expect(aal).toMatchObject({ name: "American Airlines Group Inc.", exchange: "NASDAQ", price: 12.5, volume: 900_000, available: true });
  expect(aal.changePct).toBeCloseTo(4.1667, 3);
  // No metadata: the ticker stands in for the name, and no exchange is invented.
  expect(real.find((t) => t.symbol === "XYZ")).toMatchObject({ name: "XYZ", exchange: "" });
  expect(visibleTickerFilterGroups(real).map((g) => g.id)).toEqual(["price", "movers", "exchange"]);
  expect(visibleTickerFilterGroups(universe).map((g) => g.id)).toContain("status");
  // A Mock chip remembered from a fixture session does not empty a real list.
  expect([...activeTickerFilters(real, on("mock", "nyse"))]).toEqual(["nyse"]);
  expect(searchTickers(real, "", on("mock"), []).length).toBe(real.length);
});

test("real data, no query: recents, then most active first", () => {
  const rows = searchTickers(real, "", on(), ["XYZ"]);
  expect(rows.map((r) => r.item.symbol)).toEqual(["XYZ", "F", "SNDL", "AAL"]);
  expect(rows[1]!.section).toBe("sim");
});

test("the board's symbols are listed even before the candidate list has them", () => {
  const u = tickerUniverse(["AAL"], board);
  expect(u.map((t) => t.symbol).sort()).toEqual(["AAL", "F", "SNDL", "XYZ"]);
});

test("price and mover chips test the live quote", () => {
  expect(searchTickers(real, "", on("under1"), []).map((r) => r.item.symbol)).toEqual(["SNDL"]);
  expect(searchTickers(real, "", on("5to25"), []).map((r) => r.item.symbol)).toEqual(["F", "AAL"]);
  // Gainers: biggest move first (SNDL +14 % before AAL +4 %); XYZ (flat) is neither.
  expect(searchTickers(real, "", on("gainers"), []).map((r) => r.item.symbol)).toEqual(["SNDL", "AAL"]);
  expect(searchTickers(real, "", on("losers"), []).map((r) => r.item.symbol)).toEqual(["F"]);
  expect(searchTickers(real, "", on("nasdaq", "gainers"), []).map((r) => r.item.symbol)).toEqual(["SNDL", "AAL"]);
  expect(searchTickers(real, "", on("nyse"), []).map((r) => r.item.symbol)).toEqual(["F"]);
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
  expect(searchTickers(real, "ford", on(), [])[0]!.item.symbol).toBe("F");
});

test("refresh keeps the order, takes the new values and drops symbols that left the band", () => {
  const rows = searchTickers(real, "", on(), []);
  const next = applyQuotesMessage(board, {
    type: "quotes.update",
    t: 1,
    // AAL's volume now beats F's: the open list must NOT reshuffle under the pointer.
    quotes: [{ s: "AAL", p: 13, o: 12, v: 9_000_000, n: 5000 }],
    gone: ["SNDL"],
  });
  const refreshed = refreshTickerRows(rows, tickerUniverse(["AAL", "F", "XYZ"], next), true);
  expect(refreshed.map((r) => r.item.symbol)).toEqual(["F", "AAL", "XYZ"]);
  const aal = refreshed.find((r) => r.item.symbol === "AAL")!.item;
  expect(aal.price).toBe(13);
  expect(aal.name).toBe("American Airlines Group Inc.");
  // Unchanged rows keep their identity, so a list can skip re-rendering them.
  expect(refreshed[0]).toBe(rows[0]!);
});

test("refresh moves a section title off a row that left", () => {
  const rows = searchTickers(real, "", on(), ["XYZ"]);
  expect(rows[1]!.section).toBe("sim");
  const gone = applyQuotesMessage(board, { type: "quotes.update", t: 1, quotes: [], gone: ["F"] });
  const refreshed = refreshTickerRows(rows, tickerUniverse(["AAL", "SNDL", "XYZ"], gone), true);
  expect(refreshed.map((r) => [r.item.symbol, r.section ?? null])).toEqual([
    ["XYZ", "recent"],
    ["SNDL", "sim"],
    ["AAL", null],
  ]);
});

test("recents: newest first, no duplicates, capped", () => {
  let r: string[] = [];
  for (const s of ["A", "B", "C", "A", "D", "E", "F"]) r = pushRecentTicker(r, s);
  expect(r).toEqual(["F", "E", "D", "A", "C"]);
});

test("Gainers / Losers and their order use the previous-close change when the board has it", () => {
  const b = applyQuotesMessage(EMPTY_QUOTE_BOARD, {
    type: "quotes.snapshot",
    t: 0,
    quotes: [
      // Up on its open, down on yesterday's close: a loser, as a broker lists it.
      { s: "GAPDN", p: 4.4, o: 4.0, pc: 5.0, v: 10, n: 1 },
      // Flat on its open, up 50% on yesterday's close: the biggest gainer.
      { s: "GAPUP", p: 1.5, o: 1.5, pc: 1.0, v: 10, n: 1 },
      // No close known: the open fallback (+10%).
      { s: "NOPC", p: 2.2, o: 2.0, v: 10, n: 1 },
    ],
  });
  const t = tickerUniverse([], b);
  expect(searchTickers(t, "", on("gainers"), []).map((r) => r.item.symbol)).toEqual(["GAPUP", "NOPC"]);
  expect(searchTickers(t, "", on("losers"), []).map((r) => r.item.symbol)).toEqual(["GAPDN"]);
});
