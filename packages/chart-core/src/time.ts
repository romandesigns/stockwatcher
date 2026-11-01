import type { SessionKind, SessionRef } from "./types.ts";

// Eastern Time helpers. Everything goes through Intl with an explicit
// America/New_York zone: the machine's zone is never consulted (on the
// owner's Windows box Git Bash even ignores TZ=America/New_York), and the
// tz database handles DST. Sessions here are DISPLAY classification only; the
// server's session ids are authoritative for aggregation and the profile.

export const ET_ZONE = "America/New_York";

const partsFormat = new Intl.DateTimeFormat("en-US", {
  timeZone: ET_ZONE,
  hourCycle: "h23",
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  weekday: "short",
  timeZoneName: "short",
});

const dateFormat = new Intl.DateTimeFormat("en-US", { timeZone: ET_ZONE, month: "short", day: "numeric" });

export interface EtParts {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
  second: number;
  millisecond: number;
  /** "Mon" … "Sun" */
  weekday: string;
  /** "EDT" or "EST" */
  abbr: string;
  /** UTC offset in minutes (−240 for EDT, −300 for EST). */
  offsetMinutes: number;
}

// Labels for the same second are requested many times per frame (axis,
// inspector, clock); a tiny cache keeps formatToParts off the hot path.
const cache = new Map<number, Omit<EtParts, "millisecond">>();

export function etParts(t: number): EtParts {
  const sec = Math.floor(t / 1000);
  let base = cache.get(sec);
  if (!base) {
    const fields: Record<string, string> = {};
    for (const p of partsFormat.formatToParts(new Date(sec * 1000))) fields[p.type] = p.value;
    const year = Number(fields.year);
    const month = Number(fields.month);
    const day = Number(fields.day);
    const hour = Number(fields.hour);
    const minute = Number(fields.minute);
    const second = Number(fields.second);
    const asUtc = Date.UTC(year, month - 1, day, hour, minute, second);
    base = {
      year,
      month,
      day,
      hour,
      minute,
      second,
      weekday: fields.weekday ?? "",
      abbr: fields.timeZoneName ?? "",
      offsetMinutes: Math.round((asUtc - sec * 1000) / 60_000),
    };
    if (cache.size > 4096) cache.clear();
    cache.set(sec, base);
  }
  return { ...base, millisecond: ((t % 1000) + 1000) % 1000 };
}

/**
 * Epoch ms for an ET wall-clock time. Exact for every time that exists once
 * (all session boundaries do: they never fall in the 01:00–03:00 DST shift).
 */
export function etToUtc(year: number, month: number, day: number, hour = 0, minute = 0, second = 0): number {
  const guess = Date.UTC(year, month - 1, day, hour, minute, second);
  let t = guess - etParts(guess).offsetMinutes * 60_000;
  const corrected = guess - etParts(t).offsetMinutes * 60_000;
  if (corrected !== t) t = corrected;
  return t;
}

const pad = (n: number, w = 2) => String(n).padStart(w, "0");

/** "HH:mm" or "HH:mm:ss" in ET. */
export function formatEtTime(t: number, withSeconds = false): string {
  const p = etParts(t);
  return withSeconds ? `${pad(p.hour)}:${pad(p.minute)}:${pad(p.second)}` : `${pad(p.hour)}:${pad(p.minute)}`;
}

/** "HH:mm:ss.SSS" in ET (tape). */
export function formatEtMillis(t: number): string {
  const p = etParts(t);
  return `${pad(p.hour)}:${pad(p.minute)}:${pad(p.second)}.${pad(p.millisecond, 3)}`;
}

/**
 * "9:30 AM" or "9:30:15 AM" in ET, the 12-hour clock the owner reads (2026-10-26: the time axis
 * matches the crosshair tag). No leading zero on the hour, as a wall clock writes it.
 */
export function formatEtTime12(t: number, withSeconds = false): string {
  const p = etParts(t);
  const h12 = p.hour % 12 || 12;
  return `${h12}:${pad(p.minute)}${withSeconds ? `:${pad(p.second)}` : ""} ${p.hour < 12 ? "AM" : "PM"}`;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/**
 * The crosshair's time-axis tag, in ET on a 12-hour clock (owner, 2026-10-25: East Coast, 12-hour).
 * Modelled on TradingView's crosshair label: weekday, date and the candle's start time, with
 * seconds on seconds timeframes. `length` drops the weekday and year ("date") or the whole date
 * ("time") when the axis is too narrow for the full form.
 *   full: "Mon 26 Oct '26  10:41:15 AM"   date: "26 Oct  10:41:15 AM"   time: "10:41:15 AM"
 */
export function formatCrosshairTime(t: number, withSeconds: boolean, length: "full" | "date" | "time" = "full"): string {
  const p = etParts(t);
  const clock = formatEtTime12(t, withSeconds);
  if (length === "time") return clock;
  const date = `${p.day} ${MONTHS[p.month - 1]}`;
  return length === "date" ? `${date}  ${clock}` : `${p.weekday} ${date} '${pad(p.year % 100)}  ${clock}`;
}

/** "Oct 15" in ET. */
export function formatEtDate(t: number): string {
  return dateFormat.format(new Date(t));
}

/** Session minute boundaries in ET (minutes after midnight). */
export const SESSION_MINUTES = {
  preStart: 4 * 60,
  regularStart: 9 * 60 + 30,
  regularEnd: 16 * 60,
  afterEnd: 20 * 60,
} as const;

/** Which session an instant falls in, by ET wall clock (display only). */
export function sessionOf(t: number): SessionKind {
  const p = etParts(t);
  const m = p.hour * 60 + p.minute;
  if (m < SESSION_MINUTES.preStart || m >= SESSION_MINUTES.afterEnd) return "overnight";
  if (m < SESSION_MINUTES.regularStart) return "pre";
  if (m < SESSION_MINUTES.regularEnd) return "regular";
  return "after";
}

export interface EtDate {
  year: number;
  month: number;
  day: number;
}

function addDays(d: EtDate, n: number): EtDate {
  const x = new Date(Date.UTC(d.year, d.month - 1, d.day + n));
  return { year: x.getUTCFullYear(), month: x.getUTCMonth() + 1, day: x.getUTCDate() };
}

/**
 * The trading day an instant belongs to. The overnight session that starts at
 * 20:00 ET belongs to the NEXT day, matching the timeline's band order
 * (overnight → pre → regular → after).
 */
export function tradingDayOf(t: number): EtDate {
  const p = etParts(t);
  const d = { year: p.year, month: p.month, day: p.day };
  return p.hour * 60 + p.minute >= SESSION_MINUTES.afterEnd ? addDays(d, 1) : d;
}

export function formatEtDateKey(d: EtDate): string {
  return `${d.year}-${pad(d.month)}-${pad(d.day)}`;
}

export interface SessionBand {
  kind: SessionKind;
  start: number;
  end: number;
}

/**
 * The bands of one trading day, as epoch ms: the server's `schedule` for it when given (exact on
 * early closes), else the standard four, each boundary converted
 * from ET separately. US DST shifts happen Sunday 02:00, inside the band of a
 * non-trading "Sunday" day, so weekday bands keep their nominal lengths; the
 * UTC instants still move by an hour across the change (tested).
 */
export function sessionBands(day: EtDate, schedule?: SessionSchedule | null): SessionBand[] {
  // The server's schedule when it lists the day (`sessions.v1`: an early close's 13:00/17:00).
  const key = `${formatEtDateKey(day)}/`;
  const listed = schedule?.filter((s) => s.id.startsWith(key)) ?? [];
  if (listed.length) return listed.map((s) => ({ kind: sessionIdKind(s.id) ?? "regular", start: s.start, end: s.end }));
  const prev = addDays(day, -1);
  const overnightStart = etToUtc(prev.year, prev.month, prev.day, 20, 0);
  const pre = etToUtc(day.year, day.month, day.day, 4, 0);
  const regular = etToUtc(day.year, day.month, day.day, 9, 30);
  const after = etToUtc(day.year, day.month, day.day, 16, 0);
  const end = etToUtc(day.year, day.month, day.day, 20, 0);
  return [
    { kind: "overnight", start: overnightStart, end: pre },
    { kind: "pre", start: pre, end: regular },
    { kind: "regular", start: regular, end: after },
    { kind: "after", start: after, end },
  ];
}

/**
 * The server's session schedule (chart.snapshot `sessions`, `sessions.v1`): ascending,
 * non-overlapping sessions from the engine's NYSE calendar. Empty or absent = an older server.
 */
export type SessionSchedule = readonly SessionRef[];

/** The kind of a session id like "2026-11-27/after"; null for anything else. */
export function sessionIdKind(id: string): SessionKind | null {
  const m = /\/(overnight|pre|regular|after)$/.exec(id);
  return m ? (m[1] as SessionKind) : null;
}

/** The schedule's session holding `t` (binary search), or null outside every one of them. */
export function scheduledSession(t: number, schedule: SessionSchedule | null | undefined): SessionRef | null {
  if (!schedule || schedule.length === 0) return null;
  let lo = 0;
  let hi = schedule.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const s = schedule[mid]!;
    if (t < s.start) hi = mid - 1;
    else if (t >= s.end) lo = mid + 1;
    else return s;
  }
  return null;
}

/**
 * Which session an instant falls in: the server's schedule when it covers `t` (an early close
 * ends regular at 13:00 there), else the fixed ET schedule ({@link sessionOf}). Between an early
 * close's 17:00 after-hours end and 20:00 no session is open; the fixed schedule's "after" stands
 * in, as nothing trades there.
 */
export function sessionAt(t: number, schedule?: SessionSchedule | null): SessionKind {
  const s = scheduledSession(t, schedule);
  return (s && sessionIdKind(s.id)) ?? sessionOf(t);
}

/**
 * Bounds of a session id like "2026-10-15/regular": the server's schedule's when it lists the
 * id (exact on early closes), else the standard ET schedule, APPROXIMATE on early-close days (an
 * older server: the engine's calendar ends regular at 13:00 there). Only used when an update
 * rolls into a session the client has no snapshot for yet. Null for an unparseable id.
 */
export function sessionRefFromId(id: string, schedule?: SessionSchedule | null): { id: string; start: number; end: number } | null {
  const listed = schedule?.find((s) => s.id === id);
  if (listed) return { id, start: listed.start, end: listed.end };
  const m = /^(\d{4})-(\d{2})-(\d{2})\/(overnight|pre|regular|after)$/.exec(id);
  if (!m) return null;
  const band = sessionBands({ year: Number(m[1]), month: Number(m[2]), day: Number(m[3]) }).find((b) => b.kind === m[4]);
  return band ? { id, start: band.start, end: band.end } : null;
}
