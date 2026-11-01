// The ticker search ranked on the server (`quotes.search.v1`; crates/server/src/quote_search.rs).
// The clients used to download the whole quote board (thousands of rows on a live day, then every
// changed row once a second) and filter and sort it themselves; the server now filters and ranks
// its own board and pushes only the top N, with `chg` (change %) decided there.
//
// What stays here: fuzzy text matching, over a static list of the candidates' symbols and names
// (`quotes.directory`, fetched once and again only when a candidate is missing from it). The
// query's best matches go to the server as `pins`, so the rows drawn for them are live too.
// Without the feature (an older server) the apps keep `quotes.subscribe` and searchTickers.

import { rankTickers } from "./fuzzy.ts";
import type { TickerInfo } from "./mockTickers.ts";
import { activeTickerFilters, MAX_TICKER_RECENTS, passesTickerFilters, RECENTS_COLLAPSED, withTickerSections, type TickerFilterId, type TickerRow } from "./search.ts";

export const QUOTE_SEARCH_FEATURE = "quotes.search.v1";
/** The server's MAX_PINS and MAX_LIMIT. */
export const MAX_SEARCH_PINS = 200;
export const MAX_SEARCH_LIMIT = 1000;

export type QuoteSearchSort = "activity" | "move";

export interface QuoteSearchQuery {
  filters: TickerFilterId[];
  sort: QuoteSearchSort;
  limit: number;
  pins: string[];
}

export interface QuoteSearchRowWire {
  s: string;
  /** Last price, dollars. */
  p: number;
  /** Change %, the server's (vs the previous close when known, else the open). */
  chg: number;
  v: number;
  /** Only for a row this search had not sent before. */
  name?: string;
  x?: string;
}

export interface QuoteSearchSnapshotMessage {
  type: "quotes.search.snapshot";
  id: string;
  t: number | null;
  /** `t` in exchange-time ns (newer servers; `parseServerMessage` has already taken `t` from it, wireTime.ts). */
  tNs?: number | null;
  total: number;
  rank: string[];
  rows: QuoteSearchRowWire[];
}

export interface QuoteSearchUpdateMessage {
  type: "quotes.search.update";
  id: string;
  t: number | null;
  /** `t` in exchange-time ns (newer servers; `parseServerMessage` has already taken `t` from it, wireTime.ts). */
  tNs?: number | null;
  rows: QuoteSearchRowWire[];
  total?: number;
  rank?: string[];
  gone?: string[];
}

export interface QuoteDirectoryMessage {
  type: "quotes.directory";
  /** `[symbol, name?, exchange?]` per candidate. */
  rows: string[][];
}

export type QuoteSearchMessage = QuoteSearchSnapshotMessage | QuoteSearchUpdateMessage | QuoteDirectoryMessage;

export type QuoteSearchClientMessage = ({ type: "quotes.search"; id: string } & QuoteSearchQuery) | { type: "quotes.search.stop"; id: string } | { type: "quotes.directory" };

export interface SearchQuote {
  price: number;
  changePct: number;
  volume: number;
  name?: string;
  exchange?: string;
}

export interface QuoteSearchState {
  t: number | null;
  /** Rows passing the filters (the rank holds the top `limit` of them). */
  total: number;
  rank: readonly string[];
  rows: ReadonlyMap<string, SearchQuote>;
  /** Snapshots folded so far: a new one answers a new query, so the list is ranked again. */
  snapshots: number;
}

export const EMPTY_SEARCH_STATE: QuoteSearchState = { t: null, total: 0, rank: [], rows: new Map(), snapshots: 0 };

function quoteOf(r: QuoteSearchRowWire, prev: SearchQuote | undefined): SearchQuote {
  return { price: r.p, changePct: r.chg, volume: r.v, name: r.name ?? prev?.name, exchange: r.x ?? prev?.exchange };
}

/** One search frame folded: a snapshot replaces, an update patches. */
export function applySearchMessage(state: QuoteSearchState, m: QuoteSearchSnapshotMessage | QuoteSearchUpdateMessage): QuoteSearchState {
  if (m.type === "quotes.search.snapshot") {
    return { t: m.t, total: m.total, rank: m.rank, rows: new Map(m.rows.map((r) => [r.s, quoteOf(r, state.rows.get(r.s))])), snapshots: state.snapshots + 1 };
  }
  const rows = new Map(state.rows);
  for (const r of m.rows) rows.set(r.s, quoteOf(r, rows.get(r.s)));
  for (const s of m.gone ?? []) rows.delete(s);
  return { t: m.t, total: m.total ?? state.total, rank: m.rank ?? state.rank, rows, snapshots: state.snapshots };
}

export interface TickerDirectory {
  names: ReadonlyMap<string, { name: string; exchange: string }>;
  /** Every symbol the directory answered for, named or not. */
  covered: ReadonlySet<string>;
}

export function parseDirectory(m: QuoteDirectoryMessage): TickerDirectory {
  const names = new Map<string, { name: string; exchange: string }>();
  const covered = new Set<string>();
  for (const r of Array.isArray(m.rows) ? m.rows : []) {
    const [s, name, exchange] = r;
    if (typeof s !== "string") continue;
    covered.add(s);
    if (name || exchange) names.set(s, { name: name ?? "", exchange: exchange ?? "" });
  }
  return { names, covered };
}

/** Whether the directory is missing a candidate (one entered the band since it was fetched). */
export function directoryStale(dir: TickerDirectory | null, symbols: readonly string[]): boolean {
  return !dir || symbols.some((s) => !dir.covered.has(s));
}

export interface QuoteSearchHandle {
  get(): QuoteSearchState;
  subscribe(fn: () => void): () => void;
  /** A new query for the same search: the server answers with a fresh snapshot. */
  set(query: QuoteSearchQuery): void;
  close(): void;
}

export interface QuoteSearchMeta {
  /** The connected server ranks searches (`quotes.search.v1`). */
  available: boolean;
  directory: TickerDirectory | null;
}

export interface QuoteSearchChannel {
  meta(): QuoteSearchMeta;
  /** `fn` runs when `meta()` changes. */
  watch(fn: () => void): () => void;
  /** Asks for the directory (answered once; a later ask refreshes it). */
  requestDirectory(): void;
  open(id: string, query: QuoteSearchQuery): QuoteSearchHandle;
}

const sameQuery = (a: QuoteSearchQuery, b: QuoteSearchQuery) =>
  a.sort === b.sort && a.limit === b.limit && a.filters.join(",") === b.filters.join(",") && a.pins.join(",") === b.pins.join(",");

/**
 * The feed's side of the searches. `send` goes to the socket (dropped while it is down); the feed
 * calls `setAvailable` from each welcome, `resend` once the connection is open (the server forgets
 * searches with the connection), and `apply` with every search frame.
 */
export function createQuoteSearchChannel(io: { send(m: QuoteSearchClientMessage): void }): QuoteSearchChannel & { apply(m: QuoteSearchMessage): void; setAvailable(on: boolean): void; resend(): void } {
  let meta: QuoteSearchMeta = { available: false, directory: null };
  const watchers = new Set<() => void>();
  const setMeta = (m: QuoteSearchMeta) => {
    meta = m;
    for (const w of watchers) w();
  };
  interface Open {
    query: QuoteSearchQuery;
    state: QuoteSearchState;
    listeners: Set<() => void>;
  }
  const open = new Map<string, Open>();
  const start = (id: string, o: Open) => {
    if (meta.available) io.send({ type: "quotes.search", id, ...o.query });
  };
  return {
    meta: () => meta,
    watch(fn) {
      watchers.add(fn);
      return () => watchers.delete(fn);
    },
    requestDirectory() {
      if (meta.available) io.send({ type: "quotes.directory" });
    },
    open(id, query) {
      const o: Open = { query, state: EMPTY_SEARCH_STATE, listeners: new Set() };
      open.set(id, o);
      start(id, o);
      return {
        get: () => o.state,
        subscribe(fn) {
          o.listeners.add(fn);
          return () => o.listeners.delete(fn);
        },
        set(q) {
          if (open.get(id) !== o || sameQuery(q, o.query)) return;
          o.query = q;
          start(id, o);
        },
        close() {
          if (open.get(id) !== o) return;
          open.delete(id);
          if (meta.available) io.send({ type: "quotes.search.stop", id });
        },
      };
    },
    apply(m) {
      if (m.type === "quotes.directory") return setMeta({ ...meta, directory: parseDirectory(m) });
      const o = open.get(m.id);
      if (!o) return;
      o.state = applySearchMessage(o.state, m);
      for (const l of o.listeners) l();
    },
    setAvailable(on) {
      if (on !== meta.available) setMeta({ ...meta, available: on });
    },
    resend() {
      for (const [id, o] of open) start(id, o);
    },
  };
}

/**
 * The fuzzy pool without quotes: the candidates (and any symbol only the directory knows), named
 * from the directory, plus the mock universe when asked (fixture sources).
 */
export function directoryUniverse(symbols: readonly string[], dir: TickerDirectory | null, mockRows: readonly TickerInfo[] = []): TickerInfo[] {
  const out: TickerInfo[] = [];
  const taken = new Set<string>();
  const add = (symbol: string) => {
    if (taken.has(symbol)) return;
    taken.add(symbol);
    const n = dir?.names.get(symbol);
    out.push({ symbol, name: n?.name || symbol, exchange: n?.exchange ?? "", available: true, mock: false });
  };
  for (const s of symbols) add(s);
  for (const s of dir?.covered ?? []) add(s);
  for (const m of mockRows) if (!taken.has(m.symbol)) out.push(m);
  return out;
}

/**
 * The pool with the search's live quotes on it (a row the pool lacks, from the rank, is added),
 * the TickerInfo list refreshTickerRows reads values from.
 */
export function withSearchQuotes(universe: readonly TickerInfo[], state: QuoteSearchState): TickerInfo[] {
  const out = universe.map((t) => {
    const q = state.rows.get(t.symbol);
    if (!q || t.mock) return t;
    return { ...t, name: t.name === t.symbol && q.name ? q.name : t.name, exchange: t.exchange || q.exchange || "", price: q.price, changePct: q.changePct, volume: q.volume };
  });
  const have = new Set(universe.map((t) => t.symbol));
  for (const [symbol, q] of state.rows) {
    if (have.has(symbol)) continue;
    out.push({ symbol, name: q.name ?? symbol, exchange: q.exchange ?? "", price: q.price, changePct: q.changePct, volume: q.volume, available: true, mock: false });
  }
  return out;
}

/**
 * The query to send for what the person typed and picked. With text, the best fuzzy matches are
 * the pins (their rows must be live) and the rank is not drawn, so it asks for one row; without,
 * the recents are pinned and the rank is the list, `limit` long, by the size of the move under a
 * mover chip, else by activity.
 */
export function quoteSearchQuery(universe: readonly TickerInfo[], query: string, filters: ReadonlySet<TickerFilterId>, recents: readonly string[], limit: number): QuoteSearchQuery {
  const on = activeTickerFilters(universe, filters);
  const text = query.trim();
  const pins = text ? rankTickers(universe.filter((t) => !t.mock), text).slice(0, MAX_SEARCH_PINS).map((r) => r.item.symbol) : recents.slice(0, MAX_TICKER_RECENTS);
  return {
    filters: [...on].sort(),
    sort: on.has("gainers") || on.has("losers") ? "move" : "activity",
    limit: text ? 1 : Math.max(1, Math.min(MAX_SEARCH_LIMIT, limit)),
    pins,
  };
}

/**
 * The rows searchTickers would give, from the server's ranking: recents first (as there), then
 * the server's rank, then the mock rows that pass the filters (fixture sources only; never ahead
 * of a real one). With text the fuzzy ranking runs over `tickers` (the pool with quotes), and the
 * chips are tested on the pinned matches' live quotes (a match without a quote drops out under a
 * price or mover chip, as before).
 */
export function serverTickerRows(tickers: readonly TickerInfo[], state: QuoteSearchState, query: string, filters: ReadonlySet<TickerFilterId>, recents: readonly string[], opts: { recentsExpanded?: boolean } = {}): TickerRow[] {
  const on = activeTickerFilters(tickers, filters);
  const plain = (item: TickerInfo): TickerRow => ({ item, score: 0, symbolIdx: [], nameIdx: [] });
  if (query.trim()) {
    const pool = on.size ? tickers.filter((t) => passesTickerFilters(t, on)) : tickers;
    return withTickerSections(rankTickers(pool, query).slice(0, MAX_SEARCH_PINS), false);
  }
  const bySym = new Map(tickers.map((t) => [t.symbol, t]));
  const recent = recents
    .map((s) => bySym.get(s))
    .filter((t): t is TickerInfo => !!t)
    .slice(0, MAX_TICKER_RECENTS);
  const shownRecent = opts.recentsExpanded ? recent : recent.slice(0, RECENTS_COLLAPSED);
  const hidden = recent.length - shownRecent.length;
  const taken = new Set(shownRecent.map((t) => t.symbol));
  const ranked = state.rank.filter((s) => !taken.has(s)).map((s) => bySym.get(s)).filter((t): t is TickerInfo => !!t);
  const mock = tickers.filter((t) => t.mock && !taken.has(t.symbol) && (!on.size || passesTickerFilters(t, on)));
  const recentRows: TickerRow[] = shownRecent.map((item, i) => ({ ...plain(item), recent: true, ...(hidden > 0 && i === shownRecent.length - 1 ? { moreRecents: hidden } : {}) }));
  return withTickerSections([...recentRows, ...ranked.map(plain), ...mock.map(plain)], true, on.size > 0);
}

/** The phone dashboard's Most active list from a server search (sort activity, `limit` rows). */
export function searchRankRows(state: QuoteSearchState, dir: TickerDirectory | null): TickerInfo[] {
  const out: TickerInfo[] = [];
  for (const symbol of state.rank) {
    const q = state.rows.get(symbol);
    if (!q) continue;
    const n = dir?.names.get(symbol);
    out.push({ symbol, name: n?.name || q.name || symbol, exchange: n?.exchange || q.exchange || "", price: q.price, changePct: q.changePct, volume: q.volume, available: true, mock: false });
  }
  return out;
}
