import { TIMEFRAMES, TRADE_SIZES, type Timeframe } from "@stockwatcher/chart-core";
import { MAX_TICKER_RECENTS } from "@stockwatcher/shared";

// The app-level state that syncs across devices on top of chart-core's SYNC_NS (owner,
// 2026-10-18: "sync everything possible"; docs/architecture/app-state.md). Same rules as the
// chart namespaces: the server stores them opaquely, so every value read back, local or remote,
// goes through a parser here and junk from another build falls back to defaults.
//
// - `view`          what the chart shows: ticker, timeframe, profile bin. Changing the ticker
//                   on the phone moves the desktop to it.
// - `tradeQty`      the quick trade menus' share size (TRADE_SIZES).
// - `searchRecents` recently opened tickers, newest first.

export const APP_NS = {
  view: "view",
  tradeQty: "tradeQty",
  searchRecents: "searchRecents",
} as const;

/** The session volume profile's bin widths, the same on every client. */
export const PROFILE_BINS = [0.01, 0.02, 0.05] as const;

export interface ViewState {
  /** The ticker last opened on any device; null until one is (each client then shows its own first candidate). */
  symbol: string | null;
  timeframe: Timeframe;
  bin: number;
}

export const DEFAULT_VIEW: ViewState = { symbol: null, timeframe: "1m", bin: PROFILE_BINS[0] };

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** A ticker as the wire spells it (upper case; letters, digits, `.` and `-`, e.g. BRK.B), else null. */
export function parseSymbol(raw: unknown): string | null {
  if (typeof raw !== "string") return null;
  const s = raw.trim().toUpperCase();
  return /^[A-Z][A-Z0-9.-]{0,11}$/.test(s) ? s : null;
}

export function parseView(raw: unknown): ViewState {
  if (!isObj(raw)) return DEFAULT_VIEW;
  return {
    symbol: parseSymbol(raw.symbol),
    timeframe: (TIMEFRAMES as readonly unknown[]).includes(raw.timeframe) ? (raw.timeframe as Timeframe) : DEFAULT_VIEW.timeframe,
    bin: (PROFILE_BINS as readonly unknown[]).includes(raw.bin) ? (raw.bin as number) : DEFAULT_VIEW.bin,
  };
}

export function parseTradeQty(raw: unknown): number {
  return (TRADE_SIZES as readonly unknown[]).includes(raw) ? (raw as number) : TRADE_SIZES[0];
}

/** Valid, de-duplicated tickers, newest first, capped like pushRecentTicker. */
export function parseSearchRecents(raw: unknown): string[] {
  if (!Array.isArray(raw)) return [];
  const out: string[] = [];
  for (const r of raw) {
    const s = parseSymbol(r);
    if (s && !out.includes(s)) out.push(s);
    if (out.length >= MAX_TICKER_RECENTS) break;
  }
  return out;
}
