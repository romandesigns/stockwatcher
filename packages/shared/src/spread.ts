// The bid-ask spread on every alert (owner, 2026-11-19): what an alert costs to trade, so a costly
// one can be skipped at a glance. The server measures it once, at fire time (crates/server/src/
// spread.rs: the newest market NBBO in the minute up to the fire, `round((ask − bid) / mid ×
// 10 000)` bp, `null` when none qualified), and sends it as `spread_bp` on a Runners row and
// `spreadBp` on a `runners.notice`. This module is the ONE place the cost bands and the label
// live, so the web, the desktop (the same web app) and the phone can never disagree about which
// alert is expensive.
//
// Why these bands: the 2026-11-19 diagnostics measured the alerts' edge at about +0.2 % at the
// mid, against about 2.85 pp per round trip buying the ask and selling the bid (median entry spread
// 157 bp). Up to 100 bp, a round trip costs about 1 pp: normal for these names. Past 300 bp a
// round trip costs several times the edge.
//
// Colour: status colours only, never the accent (the accent marks what to look at; a cost is a
// warning, not an invitation). A normal spread is quiet text, so the chip draws the eye only when
// the cost is the news.

/** At or below this many bp the spread is normal (neutral). */
export const SPREAD_NORMAL_MAX_BP = 100;
/** Above {@link SPREAD_NORMAL_MAX_BP} and at or below this, a warning; above it, danger. */
export const SPREAD_WARNING_MAX_BP = 300;

export type SpreadBand = "normal" | "warning" | "danger";

/** The cost band of a spread in bp: ≤ 100 normal, 100–300 warning, > 300 danger. */
export function spreadBand(bp: number): SpreadBand {
  if (bp <= SPREAD_NORMAL_MAX_BP) return "normal";
  if (bp <= SPREAD_WARNING_MAX_BP) return "warning";
  return "danger";
}

/**
 * A spread in bp as a percentage: two decimals under 1 % ("0.42%"), where the 100 bp edge sits
 * and a hundredth matters, one from 1 % ("2.1%"). The server's push text uses the same rule
 * (crates/server/src/alerts/runners.rs `spread_pct`), so the chip and the notification agree.
 */
export function spreadPct(bp: number): string {
  const p = bp / 100;
  return bp < 100 ? `${p.toFixed(2)}%` : `${p.toFixed(1)}%`;
}

/** The semantic tone of a band (each app maps it to its own status tokens). */
export type SpreadTone = "muted" | "warning" | "critical";

const TONE: Record<SpreadBand, SpreadTone> = { normal: "muted", warning: "warning", danger: "critical" };

const COST: Record<SpreadBand, string> = {
  normal: "normal cost to trade",
  warning: "costly to trade",
  danger: "very costly to trade",
};

/** What a client draws for one spread. */
export interface SpreadView {
  bp: number;
  band: SpreadBand;
  tone: SpreadTone;
  /** "Spread 0.42%": the chip's text. */
  label: string;
  /** "0.42%": the bare figure, for a tight layout. */
  value: string;
  /** "Spread 1.6%, costly to trade": the screen reader's and the tooltip's sentence. */
  aria: string;
}

/**
 * The view of a wire spread, or null when there is none to show: absent (an older server, a
 * replay, a row restored after a restart), `null` (no market quote qualified) or not a sane
 * number. Null shows nothing: a missing cost is never drawn as a cost.
 */
export function spreadView(bp: number | null | undefined): SpreadView | null {
  if (typeof bp !== "number" || !Number.isFinite(bp) || bp < 0) return null;
  const n = Math.round(bp);
  const band = spreadBand(n);
  const value = spreadPct(n);
  return { bp: n, band, tone: TONE[band], label: `Spread ${value}`, value, aria: `Spread ${value}, ${COST[band]}` };
}
