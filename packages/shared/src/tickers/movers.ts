// The dashboard's two quote-board rankings (owner, 2026-10-30): Top Gainers and Hot stocks. Both
// read the server's quote board (quotes.ts), so they cover what the board covers: the price-band
// candidates of the board's trading day (after 20:00 ET that is already the new day's rows).
// Pure, so web and phone rank identically.
//
// Dollar volume is approximated as last price × day volume: the board carries no VWAP or
// turnover, and for a ranking among $0.10–$25 names the last price is close enough.

import type { QuoteBoard, TickerQuote } from "./quotes.ts";

/** The price band both lists keep (the server's screen; the Ross Panel's "$0.10–$25"). */
export const MOVERS_PRICE_MIN = 0.1;
export const MOVERS_PRICE_MAX = 25;
/** Fewer prints than this is not a market yet: one stray print must not top a list. */
export const MOVERS_MIN_PRINTS = 100;
/** Top Gainers also needs real money behind the move: $250K traded today. */
export const GAINERS_MIN_DOLLAR_VOLUME = 250_000;
/** Rows each list shows. */
export const MOVERS_LIMIT = 20;

export interface MoverRow {
  symbol: string;
  price: number;
  /** Percent vs the previous close, else the open (quoteChangePct). */
  changePct: number;
  /** Last price × day volume, dollars. */
  dollarVolume: number;
  trades: number;
}

export function dollarVolume(q: Pick<TickerQuote, "price" | "volume">): number {
  return q.price * q.volume;
}

function eligible(board: QuoteBoard, minDollarVolume: number): MoverRow[] {
  const out: MoverRow[] = [];
  for (const [symbol, q] of board) {
    if (!Number.isFinite(q.price) || q.price < MOVERS_PRICE_MIN || q.price > MOVERS_PRICE_MAX) continue;
    if (!(q.trades >= MOVERS_MIN_PRINTS)) continue;
    const dv = dollarVolume(q);
    if (!(dv >= minDollarVolume)) continue;
    out.push({ symbol, price: q.price, changePct: Number.isFinite(q.changePct) ? q.changePct : 0, dollarVolume: dv, trades: q.trades });
  }
  return out;
}

/**
 * Top Gainers: the highest % change on the day, among $0.10–$25 names with at least
 * {@link MOVERS_MIN_PRINTS} prints and {@link GAINERS_MIN_DOLLAR_VOLUME} traded. Ties go to the
 * bigger dollar volume, then the symbol, so the order never flickers between equal rows.
 */
export function topGainers(board: QuoteBoard, limit = MOVERS_LIMIT): MoverRow[] {
  return eligible(board, GAINERS_MIN_DOLLAR_VOLUME)
    .sort((a, b) => b.changePct - a.changePct || b.dollarVolume - a.dollarVolume || a.symbol.localeCompare(b.symbol))
    .slice(0, limit);
}

/**
 * Hot stocks: the most active by dollar volume today, among $0.10–$25 names with at least
 * {@link MOVERS_MIN_PRINTS} prints. Relative volume (today against a usual day) would say "unusual"
 * better than raw turnover, but the quote board has no average-volume field, so this is the plain
 * most-active list until the server sends one.
 */
export function hotStocks(board: QuoteBoard, limit = MOVERS_LIMIT): MoverRow[] {
  return eligible(board, 0)
    .sort((a, b) => b.dollarVolume - a.dollarVolume || a.symbol.localeCompare(b.symbol))
    .slice(0, limit);
}

/** Compact dollars: 950 → "$950", 41_200 → "$41.2K", 3_240_000 → "$3.2M", 1.5e9 → "$1.5B". */
export function formatDollarVolume(v: number): string {
  if (!Number.isFinite(v) || v <= 0) return "$0";
  const unit = (n: number, s: string) => `$${n >= 100 ? Math.round(n) : Number(n.toFixed(1))}${s}`;
  if (v >= 1e9) return unit(v / 1e9, "B");
  if (v >= 1e6) return unit(v / 1e6, "M");
  if (v >= 1e3) return unit(v / 1e3, "K");
  return `$${Math.round(v)}`;
}

/** Signed percent with one decimal, "+12.4%" / "−3.1%" (a real minus, as the row marks print). */
export function formatChangePct(pct: number): string {
  const r = Math.round(pct * 10) / 10;
  return `${r > 0 ? "+" : r < 0 ? "−" : ""}${Math.abs(r).toFixed(1)}%`;
}
