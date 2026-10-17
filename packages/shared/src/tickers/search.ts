// The ticker quick-open's logic, shared by the web panel and the phone
// sheet so both list, filter and section exactly the same way. Pure: the
// apps keep the recents (localStorage / AsyncStorage) and pass them in.

import { rankTickers, type Ranked } from "./fuzzy.ts";
import type { TickerInfo } from "./mockTickers.ts";

// Filters under the input. Within a group the chips are alternatives (any
// of them), across groups they narrow (all groups). Movers also sort by the
// size of the move when there is no query. Rows without a price (the
// simulator's, until subscribed) drop out while a price or mover filter is on.
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
  sim: "In the simulator",
  mock: "Mock universe · not in the simulator",
};

/**
 * The rows for a query, filters and the recents. A query ranks fuzzily; no
 * query lists recents first, then the rest; filters narrow either. Rows
 * starting a section are tagged so both UIs title them identically.
 */
export function searchTickers(tickers: readonly TickerInfo[], query: string, filters: ReadonlySet<TickerFilterId>, recents: readonly string[]): TickerRow[] {
  const pool = filters.size ? tickers.filter((t) => passesTickerFilters(t, filters)) : tickers;
  const plain = (item: TickerInfo): TickerRow => ({ item, score: 0, symbolIdx: [], nameIdx: [] });
  let rows: TickerRow[];
  if (query.trim()) rows = rankTickers(pool, query);
  else if (filters.size) {
    // A mover filter sorts by the size of the move, biggest first.
    const mover = filters.has("gainers") || filters.has("losers");
    rows = [...pool].sort((a, b) => (mover ? Math.abs(b.changePct ?? 0) - Math.abs(a.changePct ?? 0) : 0)).map(plain);
  } else {
    const bySym = new Map(pool.map((t) => [t.symbol, t]));
    const recent = recents.map((s) => bySym.get(s)).filter((t): t is TickerInfo => !!t);
    rows = [...recent.map((item) => ({ ...plain(item), recent: true })), ...pool.filter((t) => !recents.includes(t.symbol)).map(plain)];
  }
  // Titles only in the unqueried, unfiltered list, where the grouping is real; a ranked list
  // keeps one title, over the first mock row, so a not-openable row is never unannounced.
  const grouped = !query.trim() && !filters.size;
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
    return section ? { ...r, section } : r;
  });
}

export const MAX_TICKER_RECENTS = 5;

/** The recents after opening `symbol`: it moves to the front, the list stays short. */
export function pushRecentTicker(recents: readonly string[], symbol: string): string[] {
  return [symbol, ...recents.filter((s) => s !== symbol)].slice(0, MAX_TICKER_RECENTS);
}
