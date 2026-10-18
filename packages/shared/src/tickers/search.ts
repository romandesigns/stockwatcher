// The ticker quick-open's logic, shared by the web panel and the phone
// sheet so both list, filter, order and section exactly the same way. Pure:
// the apps keep the recents (localStorage / AsyncStorage) and pass them in.

import { rankTickers, type Ranked } from "./fuzzy.ts";
import type { TickerInfo } from "./mockTickers.ts";

// Filters under the input. Within a group the chips are alternatives (any
// of them), across groups they narrow (all groups). Price and mover chips
// test the live quote (TickerInfo.price / changePct from the server's quote
// board), so a stock moves in and out of "Gainers" as the replay plays.
// Rows without a quote yet drop out while a price or mover filter is on.
export type TickerFilterId = "sim" | "mock" | "under1" | "1to5" | "5to25" | "gainers" | "losers" | "nasdaq" | "nyse";
export const TICKER_FILTER_GROUPS: readonly { id: string; chips: readonly { id: TickerFilterId; label: string }[] }[] = [
  { id: "status", chips: [{ id: "sim", label: "Simulator" }, { id: "mock", label: "Mock" }] },
  { id: "price", chips: [{ id: "under1", label: "Under $1" }, { id: "1to5", label: "$1–5" }, { id: "5to25", label: "$5–25" }] },
  { id: "movers", chips: [{ id: "gainers", label: "Gainers" }, { id: "losers", label: "Losers" }] },
  { id: "exchange", chips: [{ id: "nasdaq", label: "NASDAQ" }, { id: "nyse", label: "NYSE" }] },
];
const TEST: Record<TickerFilterId, (t: TickerInfo) => boolean> = {
  sim: (t) => t.available,
  mock: (t) => t.mock,
  under1: (t) => t.price !== undefined && t.price < 1,
  "1to5": (t) => t.price !== undefined && t.price >= 1 && t.price < 5,
  "5to25": (t) => t.price !== undefined && t.price >= 5 && t.price <= 25,
  gainers: (t) => (t.changePct ?? 0) > 0,
  losers: (t) => (t.changePct ?? 0) < 0,
  nasdaq: (t) => t.exchange === "NASDAQ",
  nyse: (t) => t.exchange === "NYSE",
};

/**
 * The chip groups worth showing for this list. Simulator/Mock only means
 * something when mock rows are listed (a fixture source); against real
 * data every row is openable and the group would be noise.
 */
export function visibleTickerFilterGroups(tickers: readonly TickerInfo[]): typeof TICKER_FILTER_GROUPS {
  return tickers.some((t) => t.mock) ? TICKER_FILTER_GROUPS : TICKER_FILTER_GROUPS.filter((g) => g.id !== "status");
}

/** The filters that apply: a chip remembered from a fixture session whose group is now hidden does not silently empty the list. */
export function activeTickerFilters(tickers: readonly TickerInfo[], filters: ReadonlySet<TickerFilterId>): Set<TickerFilterId> {
  const shown = new Set(visibleTickerFilterGroups(tickers).flatMap((g) => g.chips.map((c) => c.id)));
  return new Set([...filters].filter((f) => shown.has(f)));
}

export function passesTickerFilters(t: TickerInfo, on: ReadonlySet<TickerFilterId>): boolean {
  for (const g of TICKER_FILTER_GROUPS) {
    const active = g.chips.filter((c) => on.has(c.id));
    if (active.length && !active.some((c) => TEST[c.id](t))) return false;
  }
  return true;
}

export type TickerSection = "recent" | "sim" | "mock" | null;
export type TickerRow = Ranked<TickerInfo> & { recent?: boolean; /** Set on the first row of a titled section. */ section?: TickerSection };

export const TICKER_SECTION_TITLES: Readonly<Record<Exclude<TickerSection, null>, string>> = {
  recent: "Recently opened",
  sim: "Most active",
  mock: "Mock universe · not in the simulator",
};

/**
 * Most active first: day volume, biggest first, as a trader scans a list of
 * thousands. Rows without a quote yet follow in the order given (the
 * server's, alphabetical), and mock rows always come last, so a made-up
 * volume never outranks a real one.
 */
function byActivity(a: TickerInfo, b: TickerInfo): number {
  return Number(a.mock) - Number(b.mock) || (b.volume ?? -1) - (a.volume ?? -1);
}

/** A mover filter sorts by the size of the move, biggest first. */
function byMove(a: TickerInfo, b: TickerInfo): number {
  return Math.abs(b.changePct ?? 0) - Math.abs(a.changePct ?? 0);
}

/**
 * Tags the rows that start a section, so both UIs title them identically.
 * Titles only in the unqueried, unfiltered list, where the grouping is real;
 * a ranked list keeps one title, over the first mock row, so a not-openable
 * row is never unannounced.
 */
function withSections(rows: readonly TickerRow[], grouped: boolean): TickerRow[] {
  let firstMock = true;
  return rows.map((r, i) => {
    const prev = rows[i - 1];
    let section: TickerSection = null;
    if (grouped && r.recent && i === 0) section = "recent";
    else if (grouped && !r.recent && !r.item.mock && prev?.recent) section = "sim";
    if (r.item.mock && firstMock) {
      firstMock = false;
      if (i > 0) section = "mock";
    }
    // A refreshed list may carry a title from before a row left: clear it.
    return section ? { ...r, section } : r.section ? { ...r, section: null } : r;
  });
}

/**
 * The rows for a query, filters and the recents. A query ranks fuzzily; no
 * query lists the recents first, then the rest most active first (a mover
 * filter orders by the size of the move instead); filters narrow either.
 */
export function searchTickers(tickers: readonly TickerInfo[], query: string, filters: ReadonlySet<TickerFilterId>, recents: readonly string[]): TickerRow[] {
  const on = activeTickerFilters(tickers, filters);
  const pool = on.size ? tickers.filter((t) => passesTickerFilters(t, on)) : tickers;
  const plain = (item: TickerInfo): TickerRow => ({ item, score: 0, symbolIdx: [], nameIdx: [] });
  let rows: TickerRow[];
  if (query.trim()) rows = rankTickers(pool, query);
  else if (on.size) {
    const mover = on.has("gainers") || on.has("losers");
    rows = [...pool].sort(mover ? byMove : byActivity).map(plain);
  } else {
    const bySym = new Map(pool.map((t) => [t.symbol, t]));
    const recent = recents.map((s) => bySym.get(s)).filter((t): t is TickerInfo => !!t);
    const taken = new Set(recent.map((t) => t.symbol));
    rows = [...recent.map((item) => ({ ...plain(item), recent: true })), ...pool.filter((t) => !taken.has(t.symbol)).sort(byActivity).map(plain)];
  }
  return withSections(rows, !query.trim() && !on.size);
}

/**
 * The same rows with each item's latest data (live quotes), keeping their
 * order. The UIs rank when the query, filters or the opening change and
 * refresh on every quote update in between, so the list does not reshuffle
 * under the pointer once a second while the values in it stay live. A row
 * whose symbol is gone (it left the price band) is dropped.
 */
export function refreshTickerRows(rows: readonly TickerRow[], tickers: readonly TickerInfo[], grouped: boolean): TickerRow[] {
  const bySym = new Map(tickers.map((t) => [t.symbol, t]));
  const out: TickerRow[] = [];
  for (const r of rows) {
    const item = bySym.get(r.item.symbol);
    // An unchanged row keeps its identity, so a memoized row component skips it: an update
    // touches a few hundred of thousands of rows.
    if (item) out.push(sameTicker(item, r.item) ? r : { ...r, item });
  }
  return out.length === rows.length ? out : withSections(out, grouped);
}

function sameTicker(a: TickerInfo, b: TickerInfo): boolean {
  return a === b || (a.price === b.price && a.changePct === b.changePct && a.volume === b.volume && a.name === b.name && a.exchange === b.exchange && a.available === b.available);
}

export const MAX_TICKER_RECENTS = 5;

/** The recents after opening `symbol`: it moves to the front, the list stays short. */
export function pushRecentTicker(recents: readonly string[], symbol: string): string[] {
  return [symbol, ...recents.filter((s) => s !== symbol)].slice(0, MAX_TICKER_RECENTS);
}
