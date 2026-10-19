/**
 * The trading day the live market shows (`session` in GET /api/symbols?source=live). While the
 * market is closed (a weekend, before the overnight session) the server shows the LAST session
 * from its archive instead of nothing (owner, 2026-10-18): `current` is then false, and the
 * dashboard says which day it is. `provisional`: that day is the server's own live recording,
 * not yet replaced by the nightly import of the complete, corrected day.
 */
export interface LiveSession {
  /** "YYYY-MM-DD", the trading date. */
  day: string;
  current: boolean;
  provisional: boolean;
}

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** Reads `session` from an /api/symbols body; anything malformed is `null`. */
export function parseLiveSession(v: unknown): LiveSession | null {
  if (!v || typeof v !== "object") return null;
  const s = v as Record<string, unknown>;
  if (typeof s.day !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(s.day)) return null;
  return { day: s.day, current: s.current === true, provisional: s.provisional === true };
}

/**
 * "Last session · Fri Oct 16" (plus " · provisional"), or `null` when the session shown is the
 * current one (nothing to label). The date is the trading date as written, never shifted by the
 * viewer's time zone.
 */
export function lastSessionLabel(s: LiveSession | null | undefined): string | null {
  if (!s || s.current) return null;
  const [y, m, d] = s.day.split("-").map(Number) as [number, number, number];
  const weekday = WEEKDAYS[new Date(Date.UTC(y, m - 1, d)).getUTCDay()];
  return `Last session · ${weekday} ${MONTHS[m - 1]} ${d}${s.provisional ? " · provisional" : ""}`;
}
