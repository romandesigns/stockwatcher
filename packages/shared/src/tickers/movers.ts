// Formatting for the dashboard's movers rows (Top Gainers, Hot stocks Bull / Bear). The lists
// themselves are ranked on the server now (crates/server/src/movers.rs, the `movers.*` channel;
// client side in ../movers.ts): ranking them here from the whole quote board kept a thousands-row
// `quotes` subscription open on every dashboard (owner, 2026-10-30: "we don't want to create any
// sort of bottleneck").

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
