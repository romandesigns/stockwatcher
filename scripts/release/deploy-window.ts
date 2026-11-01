// The server deploy window (docs/CONVENTIONS.md, "Deploy window"): a release
// never restarts the live server between 04:00 and 20:00 America/New_York on
// an NYSE trading day. A restart mid-session drops every in-memory state the
// restore cannot rebuild exactly; on 2026-10-30 a 16:38 ET deploy silenced two
// winning INO micro-pullback setups. release.yml asks this script whether
// "now" is inside the window; when it is, the server (and the clients that
// depend on its protocol) wait for the after-hours scheduled run.
//
//   bun scripts/release/deploy-window.ts [--now <ISO 8601>]
//
// prints `inside=true|false` and `reason=<words>` (GITHUB_OUTPUT lines).
//
// The holidays are read from the server's own NYSE table
// (crates/market-events/src/session.rs, `HOLIDAYS`), not copied, so the two
// can never disagree. Outside the years that table covers, every weekday is a
// trading day, exactly as `session::day_kind` assumes. Early-close days are
// trading days: their extended hours still run.
//
// A release is held from 03:00 ET (LEAD_MIN), so that nothing it starts
// restarts the server after 04:00.
//
// Time zone arithmetic is Intl's (the IANA database), so DST needs no rules
// of our own: 04:00 ET is 08:00 UTC in summer and 09:00 UTC in winter.

import { readFileSync } from "node:fs";
import path from "node:path";

export const OPENS_MIN = 4 * 60;
export const CLOSES_MIN = 20 * 60;
// The window is about when the server RESTARTS, but this runs at the start of
// the release, before the checks (~10-20 min) and the server image build and
// deploy (job timeout 30 min). A run that started at 03:55 used to restart
// the server around 04:20, inside the premarket it must not touch. So a run
// is held from LEAD_MIN before the open: nothing it starts can restart the
// server after 04:00. The close needs no lead: a run held at 19:59 ships with
// the scheduled run.
export const LEAD_MIN = 60;

const SESSION_RS = path.join(import.meta.dir, "..", "..", "crates", "market-events", "src", "session.rs");

/** `YYYY-MM-DD` of every entry in session.rs's `HOLIDAYS` table. */
export function parseHolidays(sessionRs: string): Set<string> {
  const block = /const HOLIDAYS:[^=]*=\s*&\[([\s\S]*?)\];/.exec(sessionRs);
  if (!block?.[1]) throw new Error("session.rs: no HOLIDAYS table found");
  const out = new Set<string>();
  for (const m of block[1].matchAll(/\((\d{4}),\s*(\d{1,2}),\s*(\d{1,2}),\s*"/g)) {
    out.add(`${m[1]}-${m[2]!.padStart(2, "0")}-${m[3]!.padStart(2, "0")}`);
  }
  if (out.size === 0) throw new Error("session.rs: the HOLIDAYS table is empty");
  return out;
}

export function loadHolidays(file = SESSION_RS): Set<string> {
  return parseHolidays(readFileSync(file, "utf8"));
}

const ET = new Intl.DateTimeFormat("en-US", {
  timeZone: "America/New_York",
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  weekday: "short",
  hourCycle: "h23",
});

/** The New York calendar date, weekday and minute of the day at `now`. */
export function newYork(now: Date): { date: string; weekday: string; minute: number } {
  const p = Object.fromEntries(ET.formatToParts(now).map((x) => [x.type, x.value]));
  return {
    date: `${p.year}-${p.month}-${p.day}`,
    weekday: p.weekday ?? "",
    minute: Number(p.hour) * 60 + Number(p.minute),
  };
}

export interface Window {
  inside: boolean;
  reason: string;
}

/** Whether `now` is inside the server deploy window (module docs). */
export function deployWindow(now: Date, holidays: Set<string>): Window {
  const ny = newYork(now);
  const at = `${ny.date} ${String(Math.floor(ny.minute / 60)).padStart(2, "0")}:${String(ny.minute % 60).padStart(2, "0")} ET`;
  if (ny.weekday === "Sat" || ny.weekday === "Sun") return { inside: false, reason: `${at}: weekend` };
  if (holidays.has(ny.date)) return { inside: false, reason: `${at}: NYSE holiday` };
  if (ny.minute < OPENS_MIN - LEAD_MIN || ny.minute >= CLOSES_MIN) {
    return { inside: false, reason: `${at}: outside 04:00-20:00 ET` };
  }
  if (ny.minute < OPENS_MIN) {
    return { inside: true, reason: `${at}: trading day, a release started now would restart the server after 04:00 ET` };
  }
  return { inside: true, reason: `${at}: trading day, 04:00-20:00 ET` };
}

if (import.meta.main) {
  const i = process.argv.indexOf("--now");
  const now = i > 0 ? new Date(process.argv[i + 1] ?? "") : new Date();
  if (Number.isNaN(now.getTime())) {
    console.error("deploy-window: --now must be an ISO 8601 time");
    process.exit(2);
  }
  const w = deployWindow(now, loadHolidays());
  console.log(`inside=${w.inside}`);
  console.log(`reason=${w.reason}`);
}
