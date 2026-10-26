import { describe, expect, test } from "bun:test";
import { etParts, etToUtc, formatCrosshairTime, formatEtDate, formatEtMillis, formatEtTime, sessionBands, sessionOf, tradingDayOf } from "./time.ts";

const utc = (y: number, mo: number, d: number, h: number, mi = 0, s = 0) => Date.UTC(y, mo - 1, d, h, mi, s);

describe("Eastern Time", () => {
  test("EDT and EST parts and offsets", () => {
    const summer = etParts(utc(2026, 10, 15, 14, 42, 7) + 912);
    expect(summer).toMatchObject({ year: 2026, month: 10, day: 15, hour: 10, minute: 42, second: 7, millisecond: 912, abbr: "EDT", offsetMinutes: -240 });
    const winter = etParts(utc(2026, 12, 15, 14, 42, 7));
    expect(winter).toMatchObject({ hour: 9, abbr: "EST", offsetMinutes: -300 });
  });

  test("09:30 ET maps to the right UTC instant on both sides of each 2026 DST change", () => {
    // DST starts Sun 2026-03-08, ends Sun 2026-11-01.
    expect(etToUtc(2026, 3, 6, 9, 30)).toBe(utc(2026, 3, 6, 14, 30));
    expect(etToUtc(2026, 3, 9, 9, 30)).toBe(utc(2026, 3, 9, 13, 30));
    expect(etToUtc(2026, 10, 30, 9, 30)).toBe(utc(2026, 10, 30, 13, 30));
    expect(etToUtc(2026, 11, 2, 9, 30)).toBe(utc(2026, 11, 2, 14, 30));
  });

  test("session bands for the Monday after DST starts", () => {
    const b = sessionBands({ year: 2026, month: 3, day: 9 });
    expect(b.map((x) => x.kind)).toEqual(["overnight", "pre", "regular", "after"]);
    expect(b[0]!.start).toBe(utc(2026, 3, 9, 0, 0)); // Sun 20:00 EDT
    expect(b[1]!.start).toBe(utc(2026, 3, 9, 8, 0));
    expect(b[2]!.start).toBe(utc(2026, 3, 9, 13, 30));
    expect(b[3]!.start).toBe(utc(2026, 3, 9, 20, 0));
    expect(b[3]!.end).toBe(utc(2026, 3, 10, 0, 0));
  });

  test("session bands for the Monday after DST ends", () => {
    const b = sessionBands({ year: 2026, month: 11, day: 2 });
    expect(b[0]!.start).toBe(utc(2026, 11, 2, 1, 0)); // Sun 20:00 EST
    expect(b[2]!.start).toBe(utc(2026, 11, 2, 14, 30));
    expect(b[2]!.end - b[2]!.start).toBe(6.5 * 3_600_000);
  });

  test("sessionOf classifies by ET wall clock, half-open boundaries", () => {
    expect(sessionOf(etToUtc(2026, 10, 15, 3, 59, 59))).toBe("overnight");
    expect(sessionOf(etToUtc(2026, 10, 15, 4, 0))).toBe("pre");
    expect(sessionOf(etToUtc(2026, 10, 15, 9, 29, 59))).toBe("pre");
    expect(sessionOf(etToUtc(2026, 10, 15, 9, 30))).toBe("regular");
    expect(sessionOf(etToUtc(2026, 10, 15, 16, 0))).toBe("after");
    expect(sessionOf(etToUtc(2026, 10, 15, 20, 0))).toBe("overnight");
    expect(sessionOf(etToUtc(2026, 12, 15, 9, 30))).toBe("regular");
  });

  test("overnight after 20:00 belongs to the next trading day", () => {
    expect(tradingDayOf(etToUtc(2026, 10, 14, 20, 30))).toEqual({ year: 2026, month: 10, day: 15 });
    expect(tradingDayOf(etToUtc(2026, 10, 15, 19, 59))).toEqual({ year: 2026, month: 10, day: 15 });
  });

  test("formatting ignores the process time zone", () => {
    const t = utc(2026, 10, 15, 14, 42, 7) + 5;
    const before = [formatEtTime(t), formatEtTime(t, true), formatEtMillis(t), formatEtDate(t)];
    const saved = process.env.TZ;
    process.env.TZ = "Asia/Tokyo";
    try {
      expect([formatEtTime(t + 1000), formatEtTime(t + 1000, true), formatEtMillis(t + 1000), formatEtDate(t + 1000)]).toEqual(["10:42", "10:42:08", "10:42:08.005", "Oct 15"]);
    } finally {
      process.env.TZ = saved;
    }
    expect(before).toEqual(["10:42", "10:42:07", "10:42:07.005", "Oct 15"]);
  });
});

describe("formatCrosshairTime (12-hour ET, after TradingView's crosshair label)", () => {
  test("full, short-date and time-only forms; seconds only when asked", () => {
    const t = etToUtc(2026, 10, 26, 10, 41, 15); // Monday, EDT
    expect(formatCrosshairTime(t, true)).toBe("Mon 26 Oct '26  10:41:15 AM");
    expect(formatCrosshairTime(t, false)).toBe("Mon 26 Oct '26  10:41 AM");
    expect(formatCrosshairTime(t, true, "date")).toBe("26 Oct  10:41:15 AM");
    expect(formatCrosshairTime(t, false, "time")).toBe("10:41 AM");
  });

  test("noon, midnight and afternoon on the 12-hour clock, in EST too", () => {
    expect(formatCrosshairTime(etToUtc(2026, 10, 26, 12, 0), false, "time")).toBe("12:00 PM");
    expect(formatCrosshairTime(etToUtc(2026, 10, 26, 0, 5), false, "time")).toBe("12:05 AM");
    expect(formatCrosshairTime(etToUtc(2026, 10, 26, 16, 0), false, "time")).toBe("4:00 PM");
    // After the November DST change the same wall-clock time still reads the same.
    expect(formatCrosshairTime(etToUtc(2026, 12, 4, 9, 30), false)).toBe("Fri 4 Dec '26  9:30 AM");
  });
});
