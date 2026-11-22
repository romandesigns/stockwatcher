// b-good-1's confidence tag on an alert (owner, 2026-11-22: research v2's proven wins go live;
// crates/server/src/bgood). The server scores every B0-kind alert 30 s after it fires with a frozen
// model and tags the top tier "high confidence". On Validation the tagged alerts reached +10 % from
// the ask before −5 % (good-v1) 29.2 % of the time, against 20.5 % for the rest. This module is the
// ONE place the tag's words live, so the web, the desktop (the same web app) and the phone say the
// same thing.
//
// Where the tag arrives:
// - a Runners row: `confidence: "high"` on the row (`rocket.update`) once one of its notices was
//   tagged (sticky for the day);
// - the notification log: the notice's item gains `confidence {tier, score, held, at}` (the ledger's
//   `confidence` line, regrouped by `event_id`);
// - `runners.confidence {eventId, alertId, symbol, notice, tier, score, at}` to every connection, for
//   a client that wants to update something it already shows.
//
// Colour: about 4 alerts in 10 are tagged, so the tag is a quiet neutral chip with a two-tone
// icon, never the accent: an accent on 40 % of the alerts would stop meaning "look here" (60/30/10).
// It never says "buy": it reports the model's tier and what that tier did in research.

/** The only tier a client draws (an untagged alert shows nothing). */
export type ConfidenceTier = "high";

/** What a client draws for a tagged alert. */
export interface ConfidenceView {
  tier: ConfidenceTier;
  /** "High confidence": the chip's text. */
  label: string;
  /** "High": the chip's text in a tight layout. */
  short: string;
  /** The screen reader's and the tooltip's sentence. */
  aria: string;
}

const VIEW: ConfidenceView = Object.freeze({
  tier: "high",
  label: "High confidence",
  short: "High",
  aria: "High confidence: the research model's top tier, scored 30 s after the alert. In testing about 3 in 10 of these reached +10% before −5%, against 2 in 10 of the rest.",
});

/**
 * The view of a wire tier, or null when there is nothing to show: absent (older servers, a row
 * restored after a restart, an alert not scored yet or not tagged), any tier but `high`, or a tag
 * the server held back (`held` set: its runtime switch was off).
 */
export function confidenceView(tier: unknown, held?: unknown): ConfidenceView | null {
  if (tier !== "high") return null;
  if (held !== undefined && held !== null) return null;
  return VIEW;
}

/** server `runners.confidence`: a tagged alert (crates/server/src/alerts/runners.rs `confidence_frame`). */
export interface RunnersConfidenceMessage {
  type: "runners.confidence";
  /** The alert's event id (its `runners.notice` `id`, the log item's `event_id`). */
  eventId: string;
  /** The alert's board row id (`runners.notice` `alertId`). */
  alertId: string;
  symbol: string;
  /** The alert's data kind (`runners.fast`, …). */
  notice: string;
  tier: ConfidenceTier;
  /** The model's probability, display only. */
  score: number;
  /** When it was decided, epoch ms. */
  at: number;
}

/** A frame read defensively: null for anything that is not a well-formed tag. */
export function parseRunnersConfidence(v: unknown): RunnersConfidenceMessage | null {
  if (!v || typeof v !== "object") return null;
  const m = v as Record<string, unknown>;
  if (m.type !== "runners.confidence" || m.tier !== "high") return null;
  const str = (k: string) => (typeof m[k] === "string" ? (m[k] as string) : "");
  if (!str("eventId") || !str("symbol")) return null;
  const num = (k: string) => (typeof m[k] === "number" && Number.isFinite(m[k]) ? (m[k] as number) : 0);
  return { type: "runners.confidence", eventId: str("eventId"), alertId: str("alertId"), symbol: str("symbol"), notice: str("notice"), tier: "high", score: num("score"), at: num("at") };
}
