import type { SessionKind } from "./types.ts";

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
  const h12 = p.hour % 12 || 12;
  const clock = `${h12}:${pad(p.minute)}${withSeconds ? `:${pad(p.second)}` : ""} ${p.hour < 12 ? "AM" : "PM"}`;
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
 * The four bands of one trading day, as epoch ms, each boundary converted
 * from ET separately. US DST shifts happen Sunday 02:00, inside the band of a
 * non-trading "Sunday" day, so weekday bands keep their nominal lengths; the
 * UTC instants still move by an hour across the change (tested).
 */
export function sessionBands(day: EtDate): SessionBand[] {
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
 * Bounds of a session id like "2026-10-15/regular", from the standard ET
 * schedule. APPROXIMATE on early-close days (the engine's NYSE calendar ends
 * regular at 13:00 there); only used when an update rolls into a session the
 * client has no snapshot for yet. Null for an unparseable id.
 */
export function sessionRefFromId(id: string): { id: string; start: number; end: number } | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})\/(overnight|pre|regular|after)$/.exec(id);
  if (!m) return null;
  const band = sessionBands({ year: Number(m[1]), month: Number(m[2]), day: Number(m[3]) }).find((b) => b.kind === m[4]);
  return band ? { id, start: band.start, end: band.end } : null;
}
