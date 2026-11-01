// The Favorites panel (owner, 2026-10-26: "a favorite panel where I can add stocks to keep an eye
// on ... a live view of the chart price action and any other relevant information so I can be
// fully informed before clicking on it"): the client side of the server's `favorites.*` channel
// (crates/server/src/favorites.rs) and ONE row view model, so the web column and the phone tab
// format a favorite identically. The list itself is the synced `favorites` namespace
// (packages/app-state); this channel only carries the rows the server builds for it.

import { rocketAge } from "./rocket.ts";
import { rossPrice, type RossLineView, type RossTone } from "./ross.ts";

/** Today's Runners context for a favorite: its first and latest path, how many fired, the latest row's state. */
export interface FavoriteRunnersWire {
  kind: "runner" | "burst" | "running" | "run" | "fast" | (string & {});
  /** Epoch ms of the first entry today. */
  at: number;
  latest_kind: string;
  latest_at: number;
  entries: number;
  /** The latest row's momentum state (`building`, `running`, `pullback`, `fading`, `halted`, …). */
  state: string;
}

/** The micro-pullback levels, exactly a Runners row's `levels` (micros). */
export interface FavoriteLevelsWire {
  state?: string | null;
  trigger?: number | null;
  invalidation?: number | null;
  meter_milli?: number | null;
  near_trigger?: boolean | null;
}

export interface FavoritePillarsWire {
  passes: number;
  of: number;
  news?: "pass" | "fail" | "unknown" | null;
  /** The qualifying story's headline class (`financing` = offering / dilution), when any. */
  news_kind?: string | null;
  /** Daily-denominator relative volume (×). */
  rvol?: number | null;
  member: boolean;
}

/** One favorite. Prices are micros; anything the server does not know is absent. */
export interface FavoriteRowWire {
  symbol: string;
  price?: number;
  /** The previous regular close. */
  pc?: number;
  /** The quote board's change reference (regular open, else the day's first price): the change's base without `pc`. */
  ref?: number;
  volume?: number;
  trades?: number;
  /** Epoch ms of the last print. */
  last_at?: number;
  /** No print for 5 minutes of market time. */
  stale?: boolean;
  high?: number;
  low?: number;
  pm_high?: number;
  pm_low?: number;
  runners?: FavoriteRunnersWire;
  levels?: FavoriteLevelsWire;
  pillars?: FavoritePillarsWire;
  halted?: boolean;
  /** The day line, as `ross.*` rows carry it (crates/server/src/day_line.rs). */
  day_line_start?: number;
  day_line_step_s?: number;
  day_line_from?: number;
  day_line?: number[];
  prev_close?: number;
}

export interface FavoritesMessage {
  type: "favorites.snapshot" | "favorites.update";
  /** The market clock, epoch ms. */
  t: number | null;
  /** `t` in exchange-time ns (newer servers; `parseServerMessage` has already taken `t` from it, wireTime.ts). */
  tNs?: number | null;
  symbols: string[];
  rows: FavoriteRowWire[];
}

export type FavoritesClientMessage = { type: "favorites.subscribe" } | { type: "favorites.unsubscribe" };

export interface FavoritesState {
  /** null until the first frame on any connection. */
  rows: FavoriteRowWire[] | null;
  /** The market clock of the last frame (epoch ms). */
  t: number | null;
  /** A frame has arrived on the connection open now (as `RossState.connected`). */
  connected: boolean;
}

export const EMPTY_FAVORITES: FavoritesState = { rows: null, t: null, connected: false };

export interface FavoritesChannel {
  get(): FavoritesState;
  subscribe(fn: () => void): () => void;
}

/**
 * Applies an update's day-line slices to the lines held (the rule of `mergeRossDayLines`): a slice
 * from index `from` keeps the first `from` points held for the same symbol and start; one this
 * client cannot place drops that line and asks for a fresh snapshot (`resync`).
 */
export function mergeFavoriteDayLines(prev: readonly FavoriteRowWire[] | null, next: readonly FavoriteRowWire[]): { rows: FavoriteRowWire[]; resync: boolean } {
  let resync = false;
  const held = new Map(prev?.map((r) => [r.symbol, r]) ?? []);
  const rows = next.map((r) => {
    const from = r.day_line_from ?? 0;
    if (!r.day_line || from === 0) return r;
    const old = held.get(r.symbol);
    if (old?.day_line && old.day_line_start === r.day_line_start && old.day_line.length >= from) {
      return { ...r, day_line: old.day_line.slice(0, from).concat(r.day_line), day_line_from: 0 };
    }
    resync = true;
    const { day_line: _line, day_line_from: _from, day_line_start: _start, ...rest } = r;
    return rest;
  });
  return { rows, resync };
}

/** Subscribed on demand like the Ross channel: the first listener opts in, the last opts out. */
export function createFavoritesChannel(io: { subscribe(): void; unsubscribe(): void }): FavoritesChannel & { apply(m: FavoritesMessage): void; setConnected(open: boolean): void } {
  let state: FavoritesState = EMPTY_FAVORITES;
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
        state = EMPTY_FAVORITES;
      };
    },
    apply(m) {
      const rows = Array.isArray(m.rows) ? m.rows.filter((r) => r && typeof r.symbol === "string") : [];
      const merged = m.type === "favorites.update" ? mergeFavoriteDayLines(state.rows, rows) : { rows, resync: false };
      state = { rows: merged.rows, t: typeof m.t === "number" ? m.t : state.t, connected: true };
      if (merged.resync && listeners.size > 0) io.subscribe();
      for (const l of listeners) l();
    },
    setConnected(open) {
      if (open || !state.connected) return;
      state = { ...state, connected: false };
      for (const l of listeners) l();
    },
  };
}

const RUNNERS_LABEL: Record<string, string> = { runner: "Flag", burst: "Burst", running: "Running", run: "Run +20%", fast: "Fast move" };

/** One context chip on a favorite's row: quiet text in a semantic tone, never accent. */
export interface FavoriteChip {
  label: string;
  tone: RossTone;
}

export interface FavoriteRowView {
  symbol: string;
  price: string;
  /** "+9.49%" against the previous close, "" when it is unknown. */
  change: string;
  changeTone: RossTone;
  /** The row chart: the day line with the live price as its end; null with fewer than two points. */
  line: RossLineView | null;
  /** "1.25M" shares today, "" when unknown. */
  volume: string;
  /** "7.4× RVOL", "" when unknown. */
  rvol: string;
  /** "−2.3% from high", "at high", "" when unknown. */
  fromHigh: string;
  fromHighTone: RossTone;
  /** "PM high 3.18", "" when the premarket did not trade. */
  pmHigh: string;
  chips: FavoriteChip[];
  /** "last trade 6m ago" once stale, else "". */
  stale: string;
  halted: boolean;
  /** No price at all yet (a ticker that has not traded today, or a typo). */
  empty: boolean;
  aria: string;
}

function pct(x: number): string {
  const r = Math.round(x * 100) / 100;
  return `${r > 0 ? "+" : r < 0 ? "−" : ""}${Math.abs(r).toFixed(2)}%`;
}

function volume(v: number): string {
  if (v >= 1e9) return `${(v / 1e9).toFixed(2)}B`;
  if (v >= 1e6) return `${(v / 1e6).toFixed(2)}M`;
  if (v >= 1e3) return `${(v / 1e3).toFixed(1)}K`;
  return String(Math.round(v));
}

/** The row chart's line: the day line, then the live price as the end (as a Ross row draws it). */
export function favoriteLine(r: FavoriteRowWire): RossLineView | null {
  const day = r.day_line ?? [];
  if (day.length === 0) return null;
  const values = typeof r.price === "number" ? [...day, r.price] : day.slice();
  if (values.length < 2) return null;
  const end = values[values.length - 1]!;
  // Against the previous close when it is known (any favorite, unlike a Ross member, can be down
  // on the day), else against the day's first price.
  const ref = r.pc ?? r.prev_close ?? values[0]!;
  return { kind: "day", values, prevClose: r.pc ?? r.prev_close ?? null, tone: end >= ref ? "good" : "critical" };
}

/**
 * A favorite as a row (module docs). `t` is the frame's market clock and `formatTime` turns an
 * epoch-ms instant into the app's ET clock ("9:42 AM").
 */
export function favoriteRowView(r: FavoriteRowWire, t: number | null, formatTime: (epochMs: number) => string): FavoriteRowView {
  const price = typeof r.price === "number" ? r.price : null;
  // Against the previous close, else the reference the search and chart header use (quotes.rs).
  const base = r.pc ?? r.prev_close ?? r.ref ?? null;
  const ch = price !== null && base ? (price / base - 1) * 100 : null;
  const fromHigh = price !== null && r.high ? (price / r.high - 1) * 100 : null;
  const chips: FavoriteChip[] = [];
  if (r.halted) chips.push({ label: "Halted", tone: "warning" });
  if (r.runners) {
    const k = RUNNERS_LABEL[r.runners.kind] ?? "Runners";
    const more = r.runners.entries > 1 ? ` · ${r.runners.entries} entries` : "";
    chips.push({ label: `${k} ${formatTime(r.runners.at)}${more}`, tone: "text" });
  }
  if (r.levels && typeof r.levels.trigger === "number") {
    const stop = typeof r.levels.invalidation === "number" ? ` · stop ${rossPrice(r.levels.invalidation)}` : "";
    chips.push({ label: `Pullback · trigger ${rossPrice(r.levels.trigger)}${stop}`, tone: r.levels.near_trigger ? "warning" : "muted" });
  }
  if (r.pillars) {
    chips.push({ label: r.pillars.member ? `Ross ${r.pillars.passes}/${r.pillars.of}` : `${r.pillars.passes}/${r.pillars.of} pillars`, tone: r.pillars.member ? "text" : "muted" });
    if (r.pillars.news) chips.push({ label: r.pillars.news === "pass" ? "News" : r.pillars.news === "fail" ? "No news" : "News ?", tone: r.pillars.news === "pass" ? "text" : "dim" });
  }
  const staleFor = r.stale && t !== null && typeof r.last_at === "number" ? rocketAge(Math.max(0, t - r.last_at)) : "";
  const view: FavoriteRowView = {
    symbol: r.symbol,
    price: price !== null ? rossPrice(price) : "—",
    change: ch !== null ? pct(ch) : "",
    changeTone: ch === null ? "muted" : ch > 0 ? "good" : ch < 0 ? "critical" : "muted",
    line: favoriteLine(r),
    volume: typeof r.volume === "number" ? volume(r.volume) : "",
    rvol: typeof r.pillars?.rvol === "number" ? `${r.pillars.rvol.toFixed(1)}× RVOL` : "",
    fromHigh: fromHigh === null ? "" : fromHigh > -0.05 ? "at high" : `${pct(fromHigh)} from high`,
    fromHighTone: fromHigh !== null && fromHigh > -1 ? "text" : "muted",
    pmHigh: typeof r.pm_high === "number" ? `PM high ${rossPrice(r.pm_high)}` : "",
    chips,
    stale: staleFor ? `last trade ${staleFor === "now" ? "just now" : `${staleFor} ago`}` : "",
    halted: r.halted === true,
    empty: price === null,
    aria: "",
  };
  view.aria = [r.symbol, price !== null ? `${view.price}` : "no trades yet", view.change, view.volume && `${view.volume} volume`, view.fromHigh, ...chips.map((c) => c.label), view.stale].filter(Boolean).join(", ");
  return view;
}

/**
 * The panel's rows in the owner's order: the synced list leads (a star just pressed shows at
 * once, before the server has built its row) and each entry takes the server's row when it has
 * one, else `null` (the client shows it as loading).
 */
export function favoriteRows(list: readonly string[], state: FavoritesState, formatTime: (epochMs: number) => string): { symbol: string; view: FavoriteRowView | null }[] {
  const bySymbol = new Map(state.rows?.map((r) => [r.symbol, r]) ?? []);
  return list.map((symbol) => {
    const r = bySymbol.get(symbol);
    return { symbol, view: r ? favoriteRowView(r, state.t, formatTime) : null };
  });
}

/** Most favorites a list keeps; the server caps it the same way (crates/server/src/favorites.rs MAX). */
export const MAX_FAVORITES = 40;
