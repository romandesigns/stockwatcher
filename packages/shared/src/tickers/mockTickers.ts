// MOCK DATA, NOT MARKET DATA. Made-up small-cap tickers (none is meant to
// be a real company) so the quick-open search has a realistic list to
// filter when the data is a fixture (a server replaying a fixture
// archive). Next to a real replayed or live day they
// are noise, so tickerUniverse only appends them on request. Prices sit
// in the $0.10–$25 band the server screens for. Every mock row is
// labelled "mock" in the UI and cannot be opened: there is no data behind it.

import type { QuoteBoard } from "./quotes.ts";

export interface TickerInfo {
  symbol: string;
  name: string;
  /** Listing exchange ("NASDAQ", "NYSE", ...); "" when the server has no metadata for it. */
  exchange: string;
  /** Last price, change % since the open (see TickerQuote) and day volume: live from the
   *  server's quote board for openable rows, made up for mock rows, absent until known. */
  price?: number;
  changePct?: number;
  volume?: number;
  /** Has data in the running simulator, so it can be opened. */
  available: boolean;
  mock: boolean;
}

export const MOCK_TICKERS: readonly TickerInfo[] = [
  ["AQRX", "Aquarix Therapeutics", "NASDAQ", 2.41, 18.7],
  ["BLZN", "Blazon Energy", "NYSE", 4.86, -3.2],
  ["CRVT", "Curvato Robotics", "NASDAQ", 0.734, 42.1],
  ["DLTX", "Deltix Biosciences", "NASDAQ", 1.12, -11.4],
  ["EMBR", "Ember Mobility", "NASDAQ", 7.35, 6.8],
  ["FNTM", "Fontem Media", "NYSE", 3.02, 0.4],
  ["GLYD", "Glyde Aero", "NASDAQ", 12.66, 24.9],
  ["HXOR", "Hexor Semiconductor", "NASDAQ", 5.49, -1.7],
  ["IONQX", "Ionique Batteries", "NASDAQ", 0.218, 65.3],
  ["JUNO", "Junoware Software", "NASDAQ", 9.81, 2.2],
  ["KYTE", "Kyte Logistics", "NYSE", 2.95, -7.9],
  ["LUMN", "Lumina Health", "NASDAQ", 1.87, 31.5],
  ["MDXT", "Meridex Tech", "NASDAQ", 6.12, -4.6],
  ["NOVR", "Novara Mining", "NYSE", 0.452, 12.0],
  ["OPTX", "Optix Vision", "NASDAQ", 3.77, 9.3],
  ["PRSM", "Prismo Pharma", "NASDAQ", 14.2, -15.8],
  ["QNTA", "Quanta Freight", "NYSE", 8.05, 1.1],
  ["RVLT", "Revolta Motors", "NASDAQ", 2.08, 56.4],
  ["SLNX", "Solenix Solar", "NASDAQ", 4.33, -2.5],
  ["TRVX", "Trivex Genomics", "NASDAQ", 0.987, 22.8],
  ["UPLF", "Uplift Foods", "NYSE", 11.4, 3.9],
  ["VRDN", "Verdana Agritech", "NASDAQ", 1.56, -9.1],
  ["WAVR", "Wavera Networks", "NASDAQ", 6.9, 14.2],
  ["XNTH", "Xanth Biotech", "NASDAQ", 0.31, 88.6],
  ["YLDR", "Yielder Finance", "NYSE", 19.75, -0.8],
  ["ZEPH", "Zephyr Drones", "NASDAQ", 3.64, 27.3],
].map(([symbol, name, exchange, price, changePct]) => ({ symbol, name, exchange, price, changePct, available: false, mock: true }) as TickerInfo);

/** Names for the fixture symbols, which have no asset metadata (they are not companies). */
const FIXTURE_NAMES: Record<string, string> = { SMPL: "Sample Labs (fixture)", FXLO: "Fixture Low (fixture)", FXMD: "Fixture Medium (fixture)", FXHI: "Fixture High (fixture)" };

/**
 * The quick-open list: every openable symbol (the server's candidates, plus
 * any the quote board already has, since the two refresh independently),
 * with its live quote and the server's name and exchange; then, only when
 * `mock` is set, the mock universe. A symbol the server has no name for
 * shows its ticker: names are never invented.
 */
export function tickerUniverse(symbols: readonly string[], quotes: QuoteBoard = new Map(), opts: { mock?: boolean } = {}): TickerInfo[] {
  const all = [...symbols];
  const taken = new Set(symbols);
  for (const s of quotes.keys()) {
    if (taken.has(s)) continue;
    all.push(s);
    taken.add(s);
  }
  const live = all.map((symbol): TickerInfo => {
    const q = quotes.get(symbol);
    const t: TickerInfo = { symbol, name: q?.name ?? FIXTURE_NAMES[symbol] ?? symbol, exchange: q?.exchange ?? "", available: true, mock: false };
    if (q) Object.assign(t, { price: q.price, changePct: q.changePct, volume: q.volume });
    return t;
  });
  return opts.mock ? [...live, ...MOCK_TICKERS.filter((t) => !taken.has(t.symbol))] : live;
}
