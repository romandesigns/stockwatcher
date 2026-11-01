import { describe, expect, test } from "bun:test";
import { deployWindow, loadHolidays, newYork, parseHolidays } from "./deploy-window.ts";

const holidays = loadHolidays();
const at = (iso: string) => deployWindow(new Date(iso), holidays);

describe("deploy window", () => {
  test("reads the server's own NYSE holiday table", () => {
    expect(holidays.has("2026-11-26")).toBe(true); // Thanksgiving
    expect(holidays.has("2026-07-03")).toBe(true); // Independence Day (observed)
    expect(holidays.has("2027-12-24")).toBe(true);
    expect(holidays.has("2026-11-27")).toBe(false); // early close, still a trading day
    expect(holidays.size).toBeGreaterThanOrEqual(30);
  });

  test("a table it cannot find or read is an error, never 'no holidays'", () => {
    expect(() => parseHolidays("const EARLY_CLOSES: &[(i32, u32, u32, &str)] = &[];")).toThrow();
    expect(() => parseHolidays("const HOLIDAYS: &[(i32, u32, u32, &str)] = &[\n];")).toThrow();
    expect(parseHolidays('const HOLIDAYS: &[(i32, u32, u32, &str)] = &[\n    (2030, 1, 1, "x"),\n];')).toEqual(
      new Set(["2030-01-01"]),
    );
  });

  test("opens at 04:00 and closes at 20:00 ET on a summer-time trading day", () => {
    // Friday 2026-10-30, EDT (UTC-4): the INO restart was at 16:38 ET.
    expect(at("2026-10-30T06:59:59Z").inside).toBe(false); // 02:59:59
    // From 03:00 a release would restart the server after 04:00 (checks + build): held.
    expect(at("2026-10-30T07:00:00Z")).toEqual({
      inside: true,
      reason: "2026-10-30 03:00 ET: trading day, a release started now would restart the server after 04:00 ET",
    });
    expect(at("2026-10-30T07:59:59Z").inside).toBe(true); // 03:59:59
    expect(at("2026-10-30T08:00:00Z").inside).toBe(true); // 04:00
    expect(at("2026-10-30T20:38:00Z").inside).toBe(true); // 16:38
    expect(at("2026-10-30T23:59:59Z").inside).toBe(true); // 19:59:59
    expect(at("2026-10-31T00:00:00Z").inside).toBe(false); // 20:00
  });

  test("follows the clock change: 04:00 ET is 09:00 UTC in winter", () => {
    // Sunday 2026-11-01 02:00 EDT -> 01:00 EST; Monday 2026-11-02 is EST (UTC-5).
    expect(at("2026-11-02T07:30:00Z").inside).toBe(false); // 02:30 EST
    expect(at("2026-11-02T08:30:00Z").inside).toBe(true); // 03:30 EST, inside the lead
    expect(at("2026-11-02T09:00:00Z").inside).toBe(true); // 04:00 EST
    expect(at("2026-11-03T00:59:00Z").inside).toBe(true); // 19:59 EST
    expect(at("2026-11-03T01:00:00Z").inside).toBe(false); // 20:00 EST
    // Spring forward: Monday 2026-03-09 is EDT again.
    expect(at("2026-03-06T07:30:00Z").inside).toBe(false); // Friday 02:30 EST
    expect(at("2026-03-09T06:30:00Z").inside).toBe(false); // Monday 02:30 EDT
    expect(at("2026-03-09T08:30:00Z").inside).toBe(true); // Monday 04:30 EDT
    expect(newYork(new Date("2026-03-09T08:30:00Z")).minute).toBe(4 * 60 + 30);
  });

  test("weekends and NYSE holidays are open all day; early-close days are not", () => {
    expect(at("2026-10-31T15:00:00Z")).toEqual({ inside: false, reason: "2026-10-31 11:00 ET: weekend" });
    expect(at("2026-11-01T15:00:00Z").inside).toBe(false); // Sunday, the DST change
    expect(at("2026-11-26T15:00:00Z")).toEqual({ inside: false, reason: "2026-11-26 10:00 ET: NYSE holiday" });
    expect(at("2026-11-27T15:00:00Z").inside).toBe(true); // day after Thanksgiving, 13:00 close
    expect(at("2026-12-24T22:00:00Z").inside).toBe(true); // Christmas Eve 17:00
  });

  test("the date is New York's, not UTC's", () => {
    // 21:30 ET Wednesday 2026-11-25 is already Thanksgiving in UTC: still the evening before.
    expect(at("2026-11-26T02:30:00Z").reason).toBe("2026-11-25 21:30 ET: outside 04:00-20:00 ET");
    // 23:00 ET on Thanksgiving is the holiday's own date.
    expect(at("2026-11-27T04:00:00Z").reason).toBe("2026-11-26 23:00 ET: NYSE holiday");
  });

  test("beyond the holiday table every weekday is a trading day, as the server assumes", () => {
    expect(at("2028-07-04T14:00:00Z").inside).toBe(true); // a Tuesday the table does not cover yet
  });
});
