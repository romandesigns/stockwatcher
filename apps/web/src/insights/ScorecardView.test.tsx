import { describe, expect, test } from "bun:test";
import { parseScorecard } from "@stockwatcher/shared";
import { renderToStaticMarkup } from "react-dom/server";
import { TooltipProvider } from "@stockwatcher/chart-web/ui";
import { ScorecardBody, ScorecardView } from "./ScorecardView.tsx";

const tally = (fired: number, scored: number, follow: number) => ({ fired, scored, follow_through: follow, stopped: scored - follow });
const days = parseScorecard({
  days: [
    {
      date: "2026-10-26",
      rule: { target_pct: 10, stop_pct: 5, window_min: 30 },
      kinds: [
        { kind: "runners.fast", ...tally(12, 11, 2), rate: 2 / 11, phone: { sent: 9, symbol_limit: 3 }, desk: {}, median_gain_pct: 4.1, median_drawdown_pct: -6.2, median_latency_ms: 1200, median_share_before: 0.77, segments: { open_print: tally(3, 3, 0), rth: tally(9, 8, 2) }, batch: tally(3, 3, 0) },
      ],
    },
    {
      date: "2026-10-23",
      kinds: [
        { kind: "runners.burst", ...tally(5, 5, 2), rate: 0.4, phone: {}, desk: {}, segments: {}, batch: {} },
        { kind: "runners.fast", ...tally(10, 10, 3), rate: 0.3, phone: {}, desk: {}, segments: {}, batch: {} },
      ],
    },
  ],
});
const html = (node: React.ReactNode) => renderToStaticMarkup(<TooltipProvider>{node}</TooltipProvider>);

describe("Insights › Scorecard", () => {
  test("a card per kind with the latest rate, the trend, held counts and the report-batch warning", () => {
    const out = html(<ScorecardBody days={days} />);
    expect(out).toContain("Fast move");
    expect(out).toContain("Buying burst");
    expect(out).toContain("18%");
    expect(out).toContain("9 sent · 3 held");
    expect(out).toContain("3 on report batch");
    expect(out).toContain("text-warning");
    expect(out).toContain("Did not fire on the latest day");
    expect(out).toContain("<polyline");
    // House rules: no borders, no native form controls.
    expect(out).not.toMatch(/class="[^"]*\bborder\b/);
    expect(out).not.toMatch(/<select|<dialog|<details/);
  });

  test("an honest empty state before the first night is scored, and a loading line first", () => {
    expect(html(<ScorecardBody days={[]} />)).toContain("scored after the nightly import");
    expect(html(<ScorecardView />)).toContain("Loading the scorecard");
  });
});
