import { describe, expect, test } from "bun:test";
import { MOCK_TICKERS } from "./mockTickers.ts";
import { applySearchMessage, createQuoteSearchChannel, directoryStale, directoryUniverse, EMPTY_SEARCH_STATE, parseDirectory, quoteSearchQuery, searchRankRows, serverTickerRows, withSearchQuotes, type QuoteSearchClientMessage } from "./quoteSearch.ts";

const dir = parseDirectory({ type: "quotes.directory", rows: [["AAA", "Alpha Corp", "NASDAQ"], ["BBB", "Beta Inc"], ["CCC"]] });
const snap = applySearchMessage(EMPTY_SEARCH_STATE, {
  type: "quotes.search.snapshot",
  id: "",
  t: 1,
  total: 3,
  rank: ["CCC", "AAA"],
  rows: [
    { s: "AAA", p: 2, chg: 5, v: 900 },
    { s: "CCC", p: 1, chg: -2, v: 1200 },
    { s: "BBB", p: 3, chg: 0, v: 10 },
  ],
});

describe("server-ranked ticker search", () => {
  test("a snapshot replaces and an update patches, keeping names", () => {
    expect(snap.snapshots).toBe(1);
    const named = applySearchMessage(EMPTY_SEARCH_STATE, { type: "quotes.search.snapshot", id: "", t: 1, total: 1, rank: ["AAA"], rows: [{ s: "AAA", p: 2, chg: 5, v: 9, name: "Alpha Corp", x: "NASDAQ" }] });
    const up = applySearchMessage(named, { type: "quotes.search.update", id: "", t: 2, rows: [{ s: "AAA", p: 2.5, chg: 6, v: 12 }], rank: ["AAA"] });
    expect(up.rows.get("AAA")).toEqual({ price: 2.5, changePct: 6, volume: 12, name: "Alpha Corp", exchange: "NASDAQ" });
    expect(up.snapshots).toBe(1);
    const gone = applySearchMessage(up, { type: "quotes.search.update", id: "", t: 3, rows: [], gone: ["AAA"], total: 0, rank: [] });
    expect(gone.rows.size).toBe(0);
    expect(gone.total).toBe(0);
  });

  test("the directory names the pool and says when a candidate is missing", () => {
    const u = directoryUniverse(["AAA", "CCC", "DDD"], dir);
    expect(u.map((t) => [t.symbol, t.name, t.exchange])).toEqual([
      ["AAA", "Alpha Corp", "NASDAQ"],
      ["CCC", "CCC", ""],
      ["DDD", "DDD", ""],
      ["BBB", "Beta Inc", ""],
    ]);
    expect(directoryStale(dir, ["AAA", "CCC"])).toBe(false);
    expect(directoryStale(dir, ["AAA", "DDD"])).toBe(true);
    expect(directoryStale(null, [])).toBe(true);
  });

  test("no text: recents first, then the server's rank, mock rows last", () => {
    const tickers = withSearchQuotes(directoryUniverse(["AAA", "BBB", "CCC"], dir, MOCK_TICKERS.slice(0, 1)), snap);
    const rows = serverTickerRows(tickers, snap, "", new Set(), ["BBB"]);
    expect(rows.map((r) => r.item.symbol)).toEqual(["BBB", "CCC", "AAA", MOCK_TICKERS[0]!.symbol]);
    expect(rows[0]!.section).toBe("recent");
    expect(rows[1]!.section).toBe("sim");
    expect(rows[1]!.item.changePct).toBe(-2);
  });

  test("text: fuzzy over the pool, chips tested on the pinned rows' live quotes", () => {
    const tickers = withSearchQuotes(directoryUniverse(["AAA", "BBB", "CCC"], dir), snap);
    expect(serverTickerRows(tickers, snap, "alpha", new Set(), []).map((r) => r.item.symbol)).toEqual(["AAA"]);
    expect(serverTickerRows(tickers, snap, "a", new Set(["losers"]), []).length).toBe(0);
  });

  test("the query pins the matches with text and the recents without, and sorts by move under a mover chip", () => {
    const u = directoryUniverse(["AAA", "BBB", "CCC"], dir);
    expect(quoteSearchQuery(u, "beta", new Set(), ["CCC"], 200)).toEqual({ filters: [], sort: "activity", limit: 1, pins: ["BBB"] });
    expect(quoteSearchQuery(u, "", new Set(["gainers", "under1"]), ["CCC"], 200)).toEqual({ filters: ["gainers", "under1"], sort: "move", limit: 200, pins: ["CCC"] });
    // A chip of a hidden group (Simulator/Mock against real data) is not sent.
    expect(quoteSearchQuery(u, "", new Set(["sim"]), [], 50).filters).toEqual([]);
  });

  test("the channel sends only to a server that ranks, resends on reconnect, and stops", () => {
    const sent: QuoteSearchClientMessage[] = [];
    const ch = createQuoteSearchChannel({ send: (m) => sent.push(m) });
    const h = ch.open("s", { filters: [], sort: "activity", limit: 50, pins: [] });
    expect(sent).toEqual([]);
    ch.setAvailable(true);
    ch.resend();
    expect(sent).toEqual([{ type: "quotes.search", id: "s", filters: [], sort: "activity", limit: 50, pins: [] }]);
    h.set({ filters: [], sort: "activity", limit: 50, pins: [] });
    expect(sent.length).toBe(1);
    h.set({ filters: [], sort: "activity", limit: 250, pins: [] });
    expect(sent.length).toBe(2);
    let calls = 0;
    h.subscribe(() => calls++);
    ch.apply({ type: "quotes.search.snapshot", id: "s", t: 1, total: 0, rank: [], rows: [] });
    ch.apply({ type: "quotes.search.snapshot", id: "other", t: 1, total: 0, rank: [], rows: [] });
    expect(calls).toBe(1);
    h.close();
    expect(sent.at(-1)).toEqual({ type: "quotes.search.stop", id: "s" });
    ch.apply({ type: "quotes.directory", rows: [["AAA"]] });
    expect(ch.meta().directory?.covered.has("AAA")).toBe(true);
  });

  test("the Most active list reads the rank with names from the directory", () => {
    expect(searchRankRows(snap, dir).map((t) => [t.symbol, t.name, t.volume])).toEqual([
      ["CCC", "CCC", 1200],
      ["AAA", "Alpha Corp", 900],
    ]);
  });
});
