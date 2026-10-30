// The dashboard's movers (owner, 2026-10-30): Top Gainers, and Hot stocks as two sections, Bull
// and Bear, each sortable. The client side of the server's `movers.*` channel
// (crates/server/src/movers.rs): the server ranks its quote board once a tick for every client
// and pushes the three lists with one row per member, so no dashboard holds the whole quote board
// any more. This module folds the frames (merging the day-line slices, as mergeRocketDayLines
// does), sorts a list the way the owner picked, and draws a row's line.

import type { MoverRowWire, MoversMessage } from "./protocol.ts";
import { favoriteLine } from "./favorites.ts";
import type { RossLineView } from "./ross.ts";

export type MoversList = "gainers" | "bull" | "bear";
export const MOVERS_LISTS: readonly MoversList[] = ["gainers", "bull", "bear"];

export interface MoversState {
  /** Symbols in the server's rank order. */
  gainers: readonly string[];
  bull: readonly string[];
  bear: readonly string[];
  /** One row per member; null until the first frame. A row unchanged by a frame keeps its object (memo-friendly). */
  rows: ReadonlyMap<string, MoverRowWire> | null;
  /** The market clock of the last frame (epoch ms). */
  t: number | null;
  /** A frame has arrived on the connection open now (as `RossState.connected`). */
  connected: boolean;
}

export const EMPTY_MOVERS: MoversState = { gainers: [], bull: [], bear: [], rows: null, t: null, connected: false };

export interface MoversChannel {
  get(): MoversState;
  subscribe(fn: () => void): () => void;
}

const strings = (v: unknown): string[] | undefined => (Array.isArray(v) ? v.filter((s): s is string => typeof s === "string") : undefined);

/**
 * One frame folded into the state. A snapshot replaces everything; an update replaces the lists
 * it carries, upserts its rows (a slice from index `from` keeps the first `from` points held for
 * the same symbol and start) and drops `gone`. A slice this client cannot place drops that line
 * and asks for a fresh snapshot (`resync`).
 */
export function applyMoversMessage(state: MoversState, m: MoversMessage): { state: MoversState; resync: boolean } {
  const incoming = Array.isArray(m.rows) ? m.rows.filter((r) => r && typeof r.symbol === "string") : [];
  const t = typeof m.t === "number" ? m.t : state.t;
  if (m.type === "movers.snapshot") {
    return {
      state: { gainers: strings(m.gainers) ?? [], bull: strings(m.bull) ?? [], bear: strings(m.bear) ?? [], rows: new Map(incoming.map((r) => [r.symbol, r])), t, connected: true },
      resync: false,
    };
  }
  let resync = false;
  const rows = new Map(state.rows ?? []);
  for (const s of strings(m.gone) ?? []) rows.delete(s);
  for (const r of incoming) {
    const from = r.day_line_from ?? 0;
    if (!r.day_line || from === 0) {
      rows.set(r.symbol, r);
      continue;
    }
    const old = rows.get(r.symbol);
    if (old?.day_line && old.day_line_start === r.day_line_start && old.day_line.length >= from) {
      rows.set(r.symbol, { ...r, day_line: old.day_line.slice(0, from).concat(r.day_line), day_line_from: 0 });
      continue;
    }
    resync = true;
    const { day_line: _line, day_line_from: _from, day_line_start: _start, ...rest } = r;
    rows.set(r.symbol, rest);
  }
  return {
    state: { gainers: strings(m.gainers) ?? state.gainers, bull: strings(m.bull) ?? state.bull, bear: strings(m.bear) ?? state.bear, rows, t, connected: true },
    resync,
  };
}

/** Subscribed on demand like the Favorites channel: the first listener opts in, the last opts out. */
export function createMoversChannel(io: { subscribe(): void; unsubscribe(): void }): MoversChannel & { apply(m: MoversMessage): void; setConnected(open: boolean): void } {
  let state: MoversState = EMPTY_MOVERS;
  const listeners = new Set<() => void>();
  return {
    get: () => state,
    subscribe(fn) {
      listeners.add(fn);
      if (listeners.size === 1) io.subscribe();
      return () => {
        if (!listeners.delete(fn)) return;
        if (listeners.size > 0) return;
        io.unsubscribe();
        state = EMPTY_MOVERS;
      };
    },
    apply(m) {
      const next = applyMoversMessage(state, m);
      state = next.state;
      if (next.resync && listeners.size > 0) io.subscribe();
      for (const l of listeners) l();
    },
    setConnected(open) {
      if (open || !state.connected) return;
      state = { ...state, connected: false };
      for (const l of listeners) l();
    },
  };
}

/** A list's rows in the server's rank order (a member without a row yet is skipped). */
export function moversRows(state: MoversState, list: MoversList): MoverRowWire[] {
  const rows = state.rows;
  if (!rows) return [];
  const out: MoverRowWire[] = [];
  for (const s of state[list]) {
    const r = rows.get(s);
    if (r) out.push(r);
  }
  return out;
}

/** What a list can be sorted by: dollar volume (the server's Hot stocks order), change, price, ticker. */
export type MoverSortKey = "dv" | "chg" | "price" | "symbol";
export const MOVER_SORT_KEYS: readonly MoverSortKey[] = ["dv", "chg", "price", "symbol"];

/**
 * A sort: `dir` 1 is the key's natural order, -1 reversed. Natural is "strongest first": the most
 * dollars, the highest price, A→Z, and for change the biggest move in the list's direction (up
 * for Gainers and Bull, the most negative first for Bear).
 */
export interface MoverSort {
  key: MoverSortKey;
  dir: 1 | -1;
}

export const DEFAULT_MOVER_SORT: MoverSort = { key: "dv", dir: 1 };

/** A second press on the active key flips it; another key starts in its natural order. */
export function nextMoverSort(cur: MoverSort, key: MoverSortKey): MoverSort {
  return cur.key === key ? { key, dir: cur.dir === 1 ? -1 : 1 } : { key, dir: 1 };
}

/** A stored sort, or the default for anything else (another build's value, junk). */
export function parseMoverSort(raw: unknown): MoverSort {
  if (typeof raw !== "object" || raw === null) return DEFAULT_MOVER_SORT;
  const { key, dir } = raw as { key?: unknown; dir?: unknown };
  return MOVER_SORT_KEYS.includes(key as MoverSortKey) && (dir === 1 || dir === -1) ? { key: key as MoverSortKey, dir } : DEFAULT_MOVER_SORT;
}

/** `rows` sorted for `list` (a new array). Ties keep the server's rank order, so equal rows never swap. */
export function sortMovers(rows: readonly MoverRowWire[], sort: MoverSort, list: MoversList): MoverRowWire[] {
  const sign = sort.key === "chg" && list === "bear" ? -1 : 1;
  const natural = (a: MoverRowWire, b: MoverRowWire): number => {
    switch (sort.key) {
      case "dv":
        return b.dv - a.dv;
      case "chg":
        return sign * (b.chg - a.chg);
      case "price":
        return b.price - a.price;
      case "symbol":
        return a.symbol < b.symbol ? -1 : a.symbol > b.symbol ? 1 : 0;
    }
  };
  const ranked = rows.map((r, i) => ({ r, i }));
  ranked.sort((a, b) => sort.dir * natural(a.r, b.r) || a.i - b.i);
  return ranked.map((x) => x.r);
}

/** The row chart: the day line with the live price as its end, toned against the previous close (as a favorite's). */
export function moverLine(r: MoverRowWire): RossLineView | null {
  return favoriteLine(r);
}

/** A short label for each sort key (the segmented control). */
export const MOVER_SORT_LABEL: Record<MoverSortKey, string> = { dv: "$ Vol", chg: "%", price: "Price", symbol: "Ticker" };

/** The rows a dashboard panel shows, for its count (a panel hides at 0): Top Gainers, or Hot stocks' Bull and Bear together. */
export function moversCount(state: MoversState, panel: "gainers" | "hot"): number {
  return panel === "gainers" ? state.gainers.length : state.bull.length + state.bear.length;
}
