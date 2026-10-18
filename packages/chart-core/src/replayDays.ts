import { formatEtDateKey, tradingDayOf } from "./time.ts";
import type { Clock } from "./types.ts";

// The replay day menu's data, shared by chart-web (dropdown) and chart-native
// (sheet), so both list the same days the same way.
//
// Source: `days` and `day` in the server's GET /api/replay/state
// (crates/server/src/market.rs `state_json`, crates/server/src/days.rs).
// Choosing one sends `replay.control {action: "day", value: "YYYY-MM-DD"}`.
//
// Day keys are plain calendar dates ("2026-10-16"), not instants, so they are
// formatted in UTC: formatting a date-only value in a local or ET zone can
// slide it to the previous day, the classic off-by-one.

/** One entry of `/api/replay/state`'s `days`. */
export interface ReplayDay {
  /** "YYYY-MM-DD" */
  day: string;
  /** Synthetic fixture data, never market evidence. */
  fixture: boolean;
  /** Symbols archived for the day. */
  symbols: number;
}

/** What the day menu needs from `/api/replay/state`. */
export interface ReplayDayState {
  /** The day being replayed now (null before the provider runs). */
  day: string | null;
  /** Ascending as served; empty in live mode (nothing to switch to). */
  days: ReplayDay[];
}

/** One row of the day menu, ready to render. */
export interface ReplayDayOption {
  /** The value to send: "YYYY-MM-DD". */
  value: string;
  /** "Thu, Oct 16" */
  label: string;
  /** "Thursday, October 16, 2026": the accessible name. */
  longLabel: string;
  fixture: boolean;
  symbols: number;
}

const DAY_KEY = /^(\d{4})-(\d{2})-(\d{2})$/;

const shortFormat = new Intl.DateTimeFormat("en-US", { timeZone: "UTC", weekday: "short", month: "short", day: "numeric" });
const longFormat = new Intl.DateTimeFormat("en-US", { timeZone: "UTC", weekday: "long", month: "long", day: "numeric", year: "numeric" });
const dateFormat = new Intl.DateTimeFormat("en-US", { timeZone: "UTC", month: "short", day: "numeric" });

/** The UTC midnight of a "YYYY-MM-DD" key, or null if it is not a real date. */
function keyToDate(key: string): Date | null {
  const m = DAY_KEY.exec(key);
  if (!m) return null;
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  const t = new Date(Date.UTC(y, mo - 1, d));
  // Rejects "2026-02-30", which Date would roll over to March 2.
  return t.getUTCFullYear() === y && t.getUTCMonth() === mo - 1 && t.getUTCDate() === d ? t : null;
}

/** "Oct 16" for a day key; the key itself if it is not a date. */
export function formatDayKey(key: string): string {
  const d = keyToDate(key);
  return d ? dateFormat.format(d) : key;
}

/**
 * Validates a `/api/replay/state` body. Never throws: anything malformed is
 * dropped (a bad entry, or the whole list), so a server/client version skew
 * shows fewer days rather than breaking the playback bar.
 */
export function parseReplayDayState(body: unknown): ReplayDayState {
  if (typeof body !== "object" || body === null) return { day: null, days: [] };
  const b = body as { day?: unknown; days?: unknown };
  const day = typeof b.day === "string" && keyToDate(b.day) ? b.day : null;
  const days = Array.isArray(b.days)
    ? b.days.flatMap((e): ReplayDay[] => {
        if (typeof e !== "object" || e === null) return [];
        const x = e as { day?: unknown; fixture?: unknown; symbols?: unknown };
        if (typeof x.day !== "string" || !keyToDate(x.day)) return [];
        return [{ day: x.day, fixture: x.fixture === true, symbols: typeof x.symbols === "number" && x.symbols >= 0 ? x.symbols : 0 }];
      })
    : [];
  return { day, days };
}

/** The menu rows: most recent first, one per day (a duplicate key keeps its first entry). */
export function replayDayOptions(days: readonly ReplayDay[]): ReplayDayOption[] {
  const seen = new Set<string>();
  return days
    .filter((d) => !seen.has(d.day) && (seen.add(d.day), true))
    .flatMap((d): ReplayDayOption[] => {
      const t = keyToDate(d.day);
      return t ? [{ value: d.day, label: shortFormat.format(t), longLabel: longFormat.format(t), fixture: d.fixture, symbols: d.symbols }] : [];
    })
    .sort((a, b) => (a.value < b.value ? 1 : a.value > b.value ? -1 : 0));
}

/**
 * The day the playback bar names. The server's `day` is authoritative (it is
 * the archive day being replayed, even at the very end of after-hours, where
 * the clock's trading day would already read as the next day); the clock's
 * trading day stands in until the server has answered.
 */
export function currentReplayDay(state: ReplayDayState | null, clock: Clock | null): string | null {
  if (state?.day) return state.day;
  return clock ? formatEtDateKey(tradingDayOf(clock.now)) : null;
}

/** "4,821 symbols" / "1 symbol". */
export function formatSymbolCount(n: number): string {
  return `${n.toLocaleString("en-US")} symbol${n === 1 ? "" : "s"}`;
}
