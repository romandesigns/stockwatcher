import { describe, expect, test } from "bun:test";
import { TooltipProvider } from "@stockwatcher/chart-web/ui";
import type { NoticeItemWire } from "@stockwatcher/shared";
import { renderToStaticMarkup } from "react-dom/server";
import { NAV_ITEMS, pageOf } from "../nav/routes.ts";
import { InsightsPage } from "./InsightsPage.tsx";
import { NoticeRow } from "./NotificationLog.tsx";
import { breakoutTestFixture } from "../../../../packages/shared/src/breakoutTestFixture.ts";

const row = (item: NoticeItemWire) =>
  renderToStaticMarkup(
    <TooltipProvider>
      <NoticeRow item={item} />
    </TooltipProvider>,
  );

const AT = Date.UTC(2026, 9, 26, 12, 23, 47); // 08:23:47 ET

describe("Insights: notification log", () => {
  test("a breakout row is one plain line; the measured record is behind Details; legacy keeps stored words", () => {
    const item = { event_id: "runners-breakout-fixture", at: AT, symbol: "CJMB", notice: "runners.breakout", title: "immutable title", body: "immutable body", breakout: breakoutTestFixture, entry_context: ["Run +20% at original admission"] };
    const html = row(item);
    expect(html).toContain("15s new high since alert");
    expect(html).toContain("immutable title · immutable body");
    expect(html).toContain("New high $2.04 above alert $2.03 · volume 2.7× normal");
    // The open-the-chart button holds no technical record (owner, 2026-10-30: no debug walls).
    const button = html.slice(html.indexOf("<button"), html.indexOf("</button>"));
    expect(button).not.toContain("15s bar high $2.040001");
    expect(button).not.toContain("+250 ms");
    expect(button).not.toContain("post_flag_exact_high_v1");
    // A Details disclosure (Base UI Collapsible: a real button, aria-expanded) carries it all.
    expect(html).toContain(">Details<");
    expect(html).toContain('aria-expanded="false"');
    expect(html).toMatch(/^<div class="rounded-lg bg-card">/);
    expect(html).not.toMatch(/class="[^"]*\bborder/);
    const legacy = row({ ...item, breakout: undefined, entry_context: undefined });
    expect(legacy).toContain("immutable title · immutable body");
    expect(legacy).not.toContain("15s new high since alert");
    expect(legacy).toContain(">Details<");
  });
  test("Insights is a page in the main nav, routed by its hash", () => {
    expect(NAV_ITEMS.map((i) => i.id)).toContain("insights");
    expect(pageOf("#/insights")).toBe("insights");
    expect(typeof InsightsPage).toBe("function");
  });

  test("a held notice says so in warning tone, with the plain reason, the 12-hour time and both paths", () => {
    const html = row({
      event_id: "a",
      at: AT,
      symbol: "SRTM",
      notice: "runners.pullback",
      title: "SRTM · Pullback setting up",
      desk: { decision: "sent", held: null, devices: 1 },
      phone: { decision: "held", held: "symbol_limit", devices: 1 },
    });
    expect(html).toContain("8:23:47 AM");
    expect(html).toContain("SRTM");
    expect(html).toContain("Pullback");
    expect(html).toContain("text-warning");
    expect(html).toContain("same stock notified moments earlier");
    expect(html).toContain("Phones: held: same stock notified moments earlier");
    expect(html).toContain("Desktop and web: sent to 1 device");
    // House rules: no borders, a real button that opens the chart.
    expect(html).not.toMatch(/class="[^"]*\bborder/);
    expect(html).toMatch(/^<button type="button"/);
  });

  test("a sent notice is quiet; a path that never saw it draws nothing", () => {
    const html = row({ event_id: "b", at: AT, symbol: "PLAG", notice: "runners.burst", title: "PLAG · Buying burst", phone: { decision: "sent", held: null, devices: 2 }, latency: { trade_to_expo_ms: 1044 } });
    expect(html).toContain(">Sent<");
    expect(html).not.toContain("text-warning");
    expect(html).not.toContain("Desktop and web");
    expect(html).toContain("Buying burst");
  });
});
