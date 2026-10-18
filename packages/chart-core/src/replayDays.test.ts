import { describe, expect, test } from "bun:test";
import { currentReplayDay, formatDayKey, formatSymbolCount, parseReplayDayState, replayDayOptions } from "./replayDays.ts";
import { etToUtc } from "./time.ts";

const served = {
  mode: "replay",
  day: "2026-10-16",
  days: [
    { day: "2026-09-17", fixture: false, symbols: 4821 },
    { day: "2026-10-15", fixture: true, symbols: 4 },
    { day: "2026-10-16", fixture: false, symbols: 1 },
  ],
};

describe("replay day menu", () => {
  test("rows are most recent first, with weekday and date, fixture days marked", () => {
    const rows = replayDayOptions(parseReplayDayState(served).days);
    expect(rows.map((r) => [r.value, r.label, r.fixture])).toEqual([
      ["2026-10-16", "Fri, Oct 16", false],
      ["2026-10-15", "Thu, Oct 15", true],
      ["2026-09-17", "Thu, Sep 17", false],
    ]);
    expect(rows[0]!.longLabel).toBe("Friday, October 16, 2026");
  });

  test("date keys never slide a day, whatever the zone", () => {
    // A date-only key formatted in America/New_York would read Oct 15 (UTC midnight is 20:00 ET).
    expect(formatDayKey("2026-10-16")).toBe("Oct 16");
    expect(formatDayKey("2026-01-01")).toBe("Jan 1");
    expect(formatDayKey("not a day")).toBe("not a day");
  });

  test("malformed bodies and entries are dropped, never thrown", () => {
    expect(parseReplayDayState(null)).toEqual({ day: null, days: [] });
    expect(parseReplayDayState({ day: 3, days: "x" })).toEqual({ day: null, days: [] });
    const p = parseReplayDayState({
      day: "2026-02-30",
      days: [{ day: "2026-02-30" }, { day: "2026-10-14", symbols: -2 }, null, "2026-10-13", { day: "2026-10-13", fixture: "yes", symbols: 7 }],
    });
    expect(p.day).toBeNull();
    expect(p.days).toEqual([
      { day: "2026-10-14", fixture: false, symbols: 0 },
      { day: "2026-10-13", fixture: false, symbols: 7 },
    ]);
  });

  test("a duplicated day is listed once", () => {
    const rows = replayDayOptions([
      { day: "2026-10-14", fixture: false, symbols: 1 },
      { day: "2026-10-14", fixture: true, symbols: 2 },
    ]);
    expect(rows).toHaveLength(1);
    expect(rows[0]!.symbols).toBe(1);
  });

  test("the server's day wins over the clock; the clock's trading day stands in before it answers", () => {
    // 20:30 ET on Oct 15 is already the Oct 16 trading day (its overnight session).
    const clock = { mode: "replay", now: etToUtc(2026, 10, 15, 20, 30), speed: 1, playing: false };
    expect(currentReplayDay(null, clock)).toBe("2026-10-16");
    expect(currentReplayDay({ day: "2026-10-15", days: [] }, clock)).toBe("2026-10-15");
    expect(currentReplayDay(null, null)).toBeNull();
  });

  test("symbol counts", () => {
    expect(formatSymbolCount(4821)).toBe("4,821 symbols");
    expect(formatSymbolCount(1)).toBe("1 symbol");
  });
});
