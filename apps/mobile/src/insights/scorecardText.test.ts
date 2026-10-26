import { describe, expect, test } from "bun:test";
import { parseScorecard } from "@stockwatcher/shared";
import { detailLines, summaryLine, trendRuns } from "./scorecardText";

const t = (fired: number, scored: number, follow: number) => ({ fired, scored, follow_through: follow, stopped: scored - follow });
const [day] = parseScorecard({
  days: [
    {
      date: "2026-10-26",
      kinds: [{ kind: "runners.fast", ...t(12, 11, 2), rate: 2 / 11, phone: { sent: 9, symbol_limit: 3 }, desk: {}, median_gain_pct: 4.1, median_drawdown_pct: -6.2, median_latency_ms: 1200, median_share_before: 0.77, segments: { open_print: t(3, 3, 0), rth: t(9, 8, 2) }, batch: t(3, 3, 0) }],
    },
  ],
});

describe("phone scorecard words", () => {
  test("the summary and the expanded detail say what the web says in tooltips", () => {
    const k = day!.kinds[0]!;
    expect(summaryLine(k)).toBe("12 fired · 9 sent · 3 held · gain +4.1% · drop −6.2%");
    const d = detailLines(k);
    expect(d[0]).toBe("2 of 11 scored alerts followed through, 9 stopped out (1 could not be scored)");
    expect(d).toContain("3 fired on the 04:00 trade-report batch, 0 of 3 followed through");
    expect(d).toContain("04:00–04:02: 0% (0 of 3, 3 fired)");
    expect(d.some((l) => l.includes("same stock just notified"))).toBe(true);
  });

  test("the trend splits at days the kind did not fire", () => {
    // x = i/3 × 96 + 2, y = 27 − v × 24.
    expect(trendRuns([0.5, null, 0.2, 0.4], 100, 30)).toEqual(["2.0,15.0", "66.0,22.2 98.0,17.4"]);
  });
});
