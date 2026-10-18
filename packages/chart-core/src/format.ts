// Number formatting shared by the axes, inspector and tape. Stockwatcher
// covers $0.10–$25, so sub-dollar prices get 4 decimals (sub-penny quotes are
// legal below $1) and everything else 2.

export function priceDecimals(p: number): number {
  return Math.abs(p) < 1 ? 4 : 2;
}

export function formatPrice(p: number, decimals = priceDecimals(p)): string {
  return p.toFixed(decimals);
}

/** 41200 → "41.2K", 1_250_000 → "1.25M", 950 → "950". */
export function formatVolume(v: number): string {
  if (v >= 1e6) return `${(v / 1e6).toFixed(2)}M`;
  if (v >= 1e3) return `${(v / 1e3).toFixed(1)}K`;
  return String(Math.round(v));
}

/** 1200 → "1,200" (locale-free, so tests and every client agree). */
export function formatSize(s: number): string {
  return String(Math.round(s)).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/**
 * What the header's change % is measured against: the previous trading day's
 * official close when the server sent one (brokers' "today's change"), else
 * the first candle of the anchor session (e.g. the regular open; before 09:30
 * the premarket's first candle). The ticker search rows use the same rule on
 * the quote board (packages/shared tickers/quotes.ts), so both agree.
 */
export function changeReference(series: { prevClose: number | null; session: { start: number } | null; candles: readonly { t: number; o: number }[] } | null | undefined): { price: number; kind: "prevClose" | "sessionOpen" } | undefined {
  if (!series) return undefined;
  if (series.prevClose !== null && series.prevClose > 0) return { price: series.prevClose, kind: "prevClose" };
  const start = series.session?.start;
  const open = start === undefined ? undefined : series.candles.find((c) => c.t >= start)?.o;
  return open === undefined ? undefined : { price: open, kind: "sessionOpen" };
}

/** Signed percent change, e.g. "+1.23%". */
export function formatChange(from: number, to: number): string {
  if (!from) return "—";
  const pct = (to / from - 1) * 100;
  return `${pct >= 0 ? "+" : ""}${pct.toFixed(2)}%`;
}
