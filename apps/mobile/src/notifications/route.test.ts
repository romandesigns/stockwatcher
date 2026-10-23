// Runners phone pushes (owner, 2026-10-22): where a tapped notification goes, when the dashboard
// still rings locally, and the wiring a bun test cannot render (source checks).
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { notificationTarget, ringRunnersLocally } from "./route";

const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");

test("a Runners push opens its ticker's chart; a price alert's (or anything else) the Alerts tab", () => {
  expect(notificationTarget({ symbol: "CJMB", kind: "runners.new", alert_id: "2026-10-22-CJMB-260" })).toEqual({ to: "chart", symbol: "CJMB" });
  expect(notificationTarget({ symbol: "brk.b", kind: "runners.breakout" })).toEqual({ to: "chart", symbol: "BRK.B" });
  expect(notificationTarget({ symbol: "BRST", kind: "runners.burst", alert_id: "2026-10-22-BRST-b1" })).toEqual({ to: "chart", symbol: "BRST" });
  // The price alerts' payload has no kind: the Alerts tab, as before.
  expect(notificationTarget({ alertId: "a1", symbol: "XLAB", eventId: "h-1", url: "stockwatcher://alerts" })).toEqual({ to: "alerts" });
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
  expect(push).toContain('target.to === "chart" ? openTickerChart(target.symbol) : navigateToAlerts()');
  // Before the navigation exists, the tap waits for the container's onReady.
  expect(push).toContain("whenNavigationReady(");
  expect(read("../App.tsx")).toContain("onReady={flushNavigation}");
});

test("push.register carries the Runners opt-ins, on connect and whenever they or the token change", () => {
  const feed = read("../feed/serverFeed.ts");
  expect(feed).toContain('client.send({ type: "push.register", token: t, ...deviceInfo(), ...runnersRegisterFields() })');
  expect(feed).toContain("subscribeRunnersPush(registerPush)");
  expect(feed).toContain("subscribePushToken(registerPush)");
  const prefs = read("runnersPush.ts");
  // The server's defaults: new flags on, breakouts off, buying bursts, running and run entries on
  // (owner, 2026-10-22: not to miss a move while away from the phone).
  expect(prefs).toContain("RUNNERS_PUSH_DEFAULTS: RunnersPushPrefs = { pushNew: true, pushBreakout: false, pushBurst: true, pushRunning: true, pushRun: true }");
  expect(prefs).toContain("runnersNew: prefs.pushNew, runnersBreakout: prefs.pushBreakout, runnersBurst: prefs.pushBurst, runnersRunning: prefs.pushRunning, runnersRun: prefs.pushRun");
  // A preference saved before "Run +20%" existed gets the default (on).
  expect(prefs).toContain('pushRun: typeof v.pushRun === "boolean" ? v.pushRun : RUNNERS_PUSH_DEFAULTS.pushRun');
  // A preference saved before bursts existed keeps the default (on).
  expect(prefs).toContain('pushBurst: typeof v.pushBurst === "boolean" ? v.pushBurst : RUNNERS_PUSH_DEFAULTS.pushBurst');
});

test("Settings → Runners: three compact switches, no borders", () => {
  const src = read("../settings/rocketWatch.tsx");
  expect(src).toContain('toggle("pushNew")');
  expect(src).toContain('toggle("pushBreakout")');
  expect(src).toContain('toggle("pushBurst")');
  // Running and "Run +20%" entries: their own switches (2026-10-22).
  expect(src).toContain('toggle("pushRunning")');
  expect(src).toContain('toggle("pushRun")');
  expect(src).toContain('name="Run +20%"');
  expect(src).toContain('name="Buying bursts"');
  expect(src).not.toMatch(/border(Width|Color|Top|Bottom|Left|Right)/);
  // Turning one on without a token asks for permission and registers (as the Alerts tab does).
  expect(src).toContain("enablePush()");
});
