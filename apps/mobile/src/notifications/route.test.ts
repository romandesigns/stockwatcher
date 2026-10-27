// Runners phone pushes (owner, 2026-10-22): where a tapped notification goes, when the dashboard
// still rings locally, and the wiring a bun test cannot render (source checks).
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { notificationTarget, ringRunnersLocally } from "./route";
import { breakoutTestFixture } from "../../../../packages/shared/src/breakoutTestFixture.ts";

test("a measured Breakout tap retains producer event identity and unsafe optional evidence falls back to ticker-only", () => {
  const data = { symbol: "CJMB", kind: "runners.breakout", event_id: "runners-breakout-fixture", breakout: breakoutTestFixture };
  expect(notificationTarget(data)).toEqual({ to: "chart", symbol: "CJMB", noticeContext: { event_id: data.event_id, symbol: "CJMB", breakout: breakoutTestFixture } });
  expect(notificationTarget({ ...data, event_id: undefined })).toEqual({ to: "chart", symbol: "CJMB" });
  expect(notificationTarget({ ...data, breakout: { ...breakoutTestFixture, high_price_micros: Infinity } })).toEqual({ to: "chart", symbol: "CJMB" });
});

const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");

test("a notification about a ticker opens THAT ticker's chart; anything else the Alerts tab", () => {
  expect(notificationTarget({ symbol: "CJMB", kind: "runners.new", alert_id: "2026-10-22-CJMB-260" })).toEqual({ to: "chart", symbol: "CJMB" });
  expect(notificationTarget({ symbol: "brk.b", kind: "runners.breakout" })).toEqual({ to: "chart", symbol: "BRK.B" });
  expect(notificationTarget({ symbol: "BRST", kind: "runners.burst", alert_id: "2026-10-22-BRST-b1" })).toEqual({ to: "chart", symbol: "BRST" });
  // A price alert's push (owner, 2026-10-23): its ticker's chart too, no longer the Alerts tab
  // (which left the previously open chart modal on screen).
  expect(notificationTarget({ alertId: "a1", symbol: "XLAB", eventId: "h-1", url: "stockwatcher://alerts" })).toEqual({ to: "chart", symbol: "XLAB" });
  expect(notificationTarget({ kind: "runners.new", symbol: "not a ticker!" })).toEqual({ to: "alerts" });
  expect(notificationTarget({ kind: "runners.new" })).toEqual({ to: "alerts" });
  expect(notificationTarget(null)).toEqual({ to: "alerts" });
  expect(notificationTarget("junk")).toEqual({ to: "alerts" });
});

test("the dashboard rings locally only while the phone has no push token (no double banner)", () => {
  expect(ringRunnersLocally(true, null)).toBe(true);
  expect(ringRunnersLocally(true, "ExponentPushToken[x]")).toBe(false);
  expect(ringRunnersLocally(false, null)).toBe(false);
});

test("taps route through notificationTarget, including the one that launched the app", () => {
  const push = read("push.ts");
  expect(push).toContain("addNotificationResponseReceivedListener(openFrom)");
  expect(push).toContain("openFrom(Notifications.getLastNotificationResponse())");
  expect(push).toContain('if (target.to === "chart") openTickerChart(target.symbol, undefined, undefined, target.noticeContext);');
  expect(push).toContain("clearLast: () => Notifications.clearLastNotificationResponse()");
  // Before the navigation exists, the tap waits for the container's onReady (ref.ts).
  expect(read("../navigation/ref.ts")).toContain("whenNavigationReady(() => navigationRef.dispatch(openChartAction(params)))");
  expect(read("../App.tsx")).toContain("onReady={flushNavigation}");
});

test("Ross, Signals, shelf, follow-up and Halt watcher pushes open their ticker's chart too", () => {
  for (const kind of ["ross.crossing", "signals.crossing", "ross.trigger", "signals.trigger", "runners.shelf", "runners.followup", "halt.watch"]) {
    expect(notificationTarget({ symbol: "YMAT", kind, alert_id: "x" })).toEqual({ to: "chart", symbol: "YMAT" });
  }
});

test("push.register carries the opt-ins, on connect and whenever they or the token change", () => {
  const feed = read("../feed/serverFeed.ts");
  expect(feed).toContain('client.send({ type: "push.register", token: t, ...deviceInfo(), ...runnersRegisterFields() })');
  expect(feed).toContain("subscribeRunnersPush(registerPush)");
  expect(feed).toContain("subscribePushToken(registerPush)");
});

test("Settings → Alerts: every shared switch by panel, compact, no borders", () => {
  const src = read("../settings/rocketWatch.tsx");
  expect(src).toContain('label: "Alerts"');
  expect(src).toContain("ALERT_SWITCH_SECTIONS.map");
  expect(src).toContain("toggle(key)");
  expect(src).not.toMatch(/border(Width|Color|Top|Bottom|Left|Right)/);
  // Turning one on without a token asks for permission and registers (as the Alerts tab does).
  expect(src).toContain("enablePush()");
});

