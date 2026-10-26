import { describe, expect, test } from "bun:test";
import { parseScorecard, pct, scorecardRows } from "./scorecard.ts";

const kind = (k: string, scored: number, follow: number) => ({ kind: k, fired: scored + 1, scored, follow_through: follow, stopped: scored - follow, rate: scored ? follow / scored : null, phone: { sent: 2, symbol_limit: 1 }, desk: {}, segments: { rth: { fired: 1, scored: 1, follow_through: 1, stopped: 0 } }, batch: {} });

describe("scorecard", () => {
  test("parses days newest first and drops malformed ones", () => {
    const days = parseScorecard({
      days: [
        { date: "2026-10-23", computed_at: "x", rule: { target_pct: 10, stop_pct: 5, window_min: 30 }, kinds: [kind("runners.burst", 10, 4)] },
        { date: "2026-10-26", kinds: [kind("runners.fast", 5, 1), { nope: true }] },
        { date: "not a date", kinds: [] },
        "junk",
      ],
    });
    expect(days.map((d) => d.date)).toEqual(["2026-10-26", "2026-10-23"]);
    expect(days[0]!.kinds.length).toBe(1);
    expect(days[0]!.rule).toEqual({ target_pct: 10, stop_pct: 5, window_min: 30 });
    expect(days[1]!.kinds[0]!.phone).toEqual({ sent: 2, symbol_limit: 1 });
    expect(parseScorecard(null)).toEqual([]);
    expect(parseScorecard({ days: "x" })).toEqual([]);
  });

  test("rows follow the kind order, carry the latest day, the trend and the pooled rate", () => {
    const days = parseScorecard({
      days: [
        { date: "2026-10-26", kinds: [kind("runners.fast", 4, 1)] },
        { date: "2026-10-23", kinds: [kind("runners.burst", 10, 4), kind("runners.fast", 6, 2)] },
      ],
    });
    const rows = scorecardRows(days);
    expect(rows.map((r) => r.label)).toEqual(["Buying burst", "Fast move"]);
    const fast = rows[1]!;
    expect(fast.latest?.scored).toBe(4);
    expect(fast.trend).toEqual([2 / 6, 1 / 4]);
    expect(fast.pooled).toEqual({ scored: 10, follow_through: 3, rate: 0.3 });
    // Burst did not fire on the latest day: no latest line, a gap at the end of its trend.
    expect(rows[0]!.latest).toBeNull();
    expect(rows[0]!.trend).toEqual([0.4, null]);
    expect(pct(0.425)).toBe("43%");
    expect(pct(null)).toBe("—");
  });
});
