// "Copy ticker" on a Runners notification (owner, 2026-10-23): which symbol a press copies, when
// the listener copies, and the native wiring a bun test cannot run (source checks).
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { COPY_TICKER_ACTION, COPY_TICKER_ACTIONS, copyTickerSymbol, handleCopyTickerResponse, RUNNERS_CATEGORY_ID, SETTLE_MS } from "./copyTickerAction";

const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");

const response = (actionIdentifier: string, data: unknown) => ({ actionIdentifier, notification: { request: { content: { data } } } });
const runners = { symbol: "YMAT", kind: "runners.fast", alert_id: "2026-10-23-YMAT-1" };
// expo-notifications' DEFAULT_ACTION_IDENTIFIER (a tap on the notification body).
const TAP = "expo.modules.notifications.actions.DEFAULT";

test("a press copies the push's own symbol; a tap or another push copies nothing", () => {
  expect(copyTickerSymbol(response(COPY_TICKER_ACTION, runners))).toBe("YMAT");
  expect(copyTickerSymbol(response(COPY_TICKER_ACTION, { ...runners, symbol: "brk.b" }))).toBe("BRK.B");
  // A background task's payload may carry the data as a JSON string.
  expect(copyTickerSymbol(response(COPY_TICKER_ACTION, { dataString: JSON.stringify(runners) }))).toBe("YMAT");
  expect(copyTickerSymbol(response(COPY_TICKER_ACTION, JSON.stringify(runners)))).toBe("YMAT");
  // The default tap still opens the chart (push.ts), it is not a copy.
  expect(copyTickerSymbol(response(TAP, runners))).toBeNull();
  // A price alert's push, junk, a symbol that is not a ticker: nothing.
  expect(copyTickerSymbol(response(COPY_TICKER_ACTION, { alertId: "a1", symbol: "XLAB" }))).toBeNull();
  expect(copyTickerSymbol(response(COPY_TICKER_ACTION, { ...runners, symbol: "not a ticker!" }))).toBeNull();
  expect(copyTickerSymbol(response(COPY_TICKER_ACTION, null))).toBeNull();
  expect(copyTickerSymbol(null)).toBeNull();
});

test("the listener copies only while active and settled, and never opens anything for a press", () => {
  const copied: string[] = [];
  const copy = (s: string) => copied.push(s);
  const press = response(COPY_TICKER_ACTION, runners);
  expect(handleCopyTickerResponse(press, { active: true, listeningForMs: SETTLE_MS + 1 }, copy)).toBe(true);
  expect(copied).toEqual(["YMAT"]);
  // Backgrounded: the background task copies instead.
  expect(handleCopyTickerResponse(press, { active: false, listeningForMs: 60_000 }, copy)).toBe(true);
  // Queued from before this start (the task already copied it): not again.
  expect(handleCopyTickerResponse(press, { active: true, listeningForMs: 10 }, copy)).toBe(true);
  expect(copied).toEqual(["YMAT"]);
  // A tap is not handled here: push.ts goes on and opens the chart.
  expect(handleCopyTickerResponse(response(TAP, runners), { active: true, listeningForMs: 60_000 }, copy)).toBe(false);
});

test("the category matches the server and its button does not bring the app forward", () => {
  expect(RUNNERS_CATEGORY_ID).toBe("runners");
  expect(read("../../../../crates/server/src/alerts/runners.rs")).toContain('pub const CATEGORY_ID: &str = "runners";');
  expect(COPY_TICKER_ACTIONS).toEqual([{ identifier: COPY_TICKER_ACTION, buttonTitle: "Copy ticker", options: { opensAppToForeground: false } }]);
});

test("wiring: registered first thing at startup, task in module scope, listener checks presses first", () => {
  const index = read("../../index.ts");
  expect(index.indexOf('from "./src/notifications/copyTicker"')).toBeLessThan(index.indexOf('from "./src/App"'));
  expect(index).toContain("registerCopyTicker();");
  const native = read("copyTicker.ts");
  expect(native).toContain("TaskManager.defineTask<Notifications.NotificationTaskPayload>(COPY_TICKER_TASK");
  expect(native).toContain("Clipboard.setStringAsync(symbol)");
  expect(native).toContain("Notifications.setNotificationCategoryAsync(RUNNERS_CATEGORY_ID, COPY_TICKER_ACTIONS)");
  expect(native).toContain("Notifications.registerTaskAsync(COPY_TICKER_TASK)");
  const push = read("push.ts");
  expect(push.indexOf("handleCopyTickerResponse(response")).toBeLessThan(push.indexOf("notificationTarget(response"));
});
