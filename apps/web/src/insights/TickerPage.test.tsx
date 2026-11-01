import { describe, expect, test } from "bun:test";
import { TooltipProvider } from "@stockwatcher/chart-web/ui";
import { createTickerChannel, type TickerSnapshotMessage } from "@stockwatcher/shared";
import { renderToStaticMarkup } from "react-dom/server";
import { TickerPage } from "./TickerPage.tsx";

// A fictional symbol in the server's package shape (no real market data in the repo).
const T0 = Date.UTC(2026, 9, 30, 20, 32, 30);
const outcome = { ref_price: 0.7264, ref_at: T0, best_price: 2.71, best_at: T0 + 1e6, best_pct: 273.1, plus5_price: 0.76, plus5_at: T0 + 20_000, stop: null, stop_hit_at: null, crossing: null, crossed_after_stop_at: null, through: T0 + 2e6 };
const PKG = {
  type: "insights.ticker.snapshot",
  symbol: "ZZTK",
  day: "2026-10-30",
  available: true,
  name: "Fictional Corp",
  prev_day: { day: "2026-10-29", archived: false },
  next_day: null,
  stats: { prev_close: null, close: 0.6, change: null, last: 2.06, sessions: { pre: null, regular: null, after: null }, volume: 10, dollar_volume: 10, high: null, float: { available: false }, short_interest: { available: false } },
  bars: [{ t: T0 - 60_000, o: 0.7, h: 0.73, l: 0.69, c: 0.72, v: 100, n: 3, final: true }],
  alerts: [{ event_id: "runners-fast-x", n: 1, marker: 1, at: T0, price: 0.7264, notice: "runners.fast", label: "Fast move", title: "ZZTK · Fast move · $0.7264", body: "+17.7% in 15 s", status: "sent", outcome }],
  markers: [{ kind: "alert", n: 1, at: T0, price: 0.7264 }],
  scorecard: { alerts: 1, sent: 1, held: 0, reached_5pct: 1, best_entry: { notice: "runners.fast", marker: 1, best_pct: 273.1 }, setups_armed: 1, missed: 1, by_kind: [] },
  halts: { halts: [], move_session: "after", outside_regular: true, luld_applies: false, closest: { up: null, down: null, regular_prints: 0 } },
  missed: [{ episode_id: "ep:1", armed_at: T0, trigger: 1.36, stop: 1.22, reason: "warmup", reason_text: "muted: warm-up after a server restart", restart_at: null, flagged_at: null, outcome: { ...outcome, ref_price: 1.3, best_pct: 108.5 } }],
} as unknown as TickerSnapshotMessage;

const render = (ch: ReturnType<typeof createTickerChannel>) =>
  renderToStaticMarkup(
    <TooltipProvider>
      <TickerPage channel={ch} want={{ symbol: "ZZTK", day: "2026-10-30" }} onBack={() => {}} />
    </TooltipProvider>,
  );

describe("Insights: ticker page", () => {
  test("renders the server's package: top bar, history with outcome, scorecard, halts, missed setups; no borders", () => {
    const ch = createTickerChannel({ subscribe: () => {}, unsubscribe: () => {} });
    ch.setAvailable(true);
    ch.open({ symbol: "ZZTK", day: "2026-10-30" });
    ch.apply(PKG);
    const html = render(ch);
    expect(html).toContain("Back to the notification log");
    expect(html).toContain("Fictional Corp");
    expect(html).toContain("Previous trading day, 2026-10-29");
    expect(html).toContain("Next trading day (none)");
    expect(html).toContain("Fast move");
    expect(html).toContain("+273.1%");
    expect(html).toContain("1 sent · 0 held");
    expect(html).toContain("LULD bands only apply");
    expect(html).toContain("Muted: warm-up after a server restart");
    expect(html).not.toMatch(/class="[^"]*\bborder(?!-0)/);
  });

  test("while building, and against a server without the feature", () => {
    const ch = createTickerChannel({ subscribe: () => {}, unsubscribe: () => {} });
    ch.open({ symbol: "ZZTK", day: "2026-10-30" });
    expect(render(ch)).toContain("Building ZZTK for 2026-10-30");
    ch.setAvailable(false);
    expect(render(ch)).toContain("does not build ticker pages yet");
  });
});
