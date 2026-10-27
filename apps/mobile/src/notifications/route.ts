// Where a tapped phone notification goes (pure, so bun tests it without a native runtime).
//
// Any notification about a ticker opens THAT ticker's chart modal (owner, 2026-10-23: "I want the
// currently open stock chart to always be replaced by the stock I click or tap on, regardless of
// where the stock is being presented to me from"):
// - a Runners push (crates/server/src/alerts/runners.rs, data `{symbol, kind: "runners.new" |
//   "runners.breakout" | "runners.burst" | "runners.running" | "runners.run" | "runners.fast",
//   alert_id}`), and since 2026-10-23 the same shape for "runners.shelf", "runners.followup",
//   "ross.crossing", "signals.crossing", "ross.trigger", "signals.trigger", "halt.watch" and
//   (2026-10-25) "runners.pullback":
//   any kind with a valid ticker opens its chart, so a new kind needs nothing here;
// - a price alert's push (crates/server/src/alerts/push.rs, data `{alertId, symbol, eventId,
//   url}`). It used to open the Alerts tab, which left whatever chart was open on screen (the
//   modal covers the tabs); the alert's record is still in the Alerts tab's Activity.
// Anything without a valid ticker (an unknown or older payload) opens the Alerts tab.
//
// A Galaxy Watch's "Open on phone" fires the phone notification's own tap, so it arrives here as
// the same response.

import { runnersNoticeChartContext, type NoticeChartContext } from "@stockwatcher/shared";

export type NotificationTarget = { to: "chart"; symbol: string; noticeContext?: NoticeChartContext } | { to: "alerts" };

/** A ticker as the wire spells it (app-state parseSymbol's rule). */
const SYMBOL = /^[A-Z][A-Z0-9.-]{0,11}$/;

export function notificationTarget(data: unknown): NotificationTarget {
  if (data && typeof data === "object") {
    const d = data as Record<string, unknown>;
    const symbol = typeof d.symbol === "string" ? d.symbol.trim().toUpperCase() : "";
    if (SYMBOL.test(symbol)) {
      const noticeContext = runnersNoticeChartContext({ id: typeof d.event_id === "string" ? d.event_id : "", symbol, kind: typeof d.kind === "string" ? d.kind : "", data: d });
      return { to: "chart", symbol, ...(noticeContext ? { noticeContext } : {}) };
    }
  }
  return { to: "alerts" };
}

/** The part of an expo-notifications NotificationResponse routing reads (so tests need no native module). */
export interface ResponseLike {
  actionIdentifier?: string;
  notification: { request: { identifier: string; content: { data?: unknown } } };
}

/**
 * How long one response counts as "the same tap": the response that launched the app arrives
 * twice, from getLastNotificationResponse and from the listener, a few milliseconds apart. Later,
 * the same notification tapped again (the watch, then the phone) is a new request to see that
 * ticker and is routed again; opening the chart is idempotent, so that is safe.
 */
export const SAME_TAP_MS = 5_000;

/**
 * Routes tapped notifications to `go`, once per tap. `clearLast` empties the native "last
 * response" after each one, so a later read of it (a JS reload, a remounted root re-running
 * initPush) never replays an old tap over the chart the person has open since.
 */
export function createResponseRouter(go: (target: NotificationTarget) => void, opts: { clearLast?: () => void; now?: () => number } = {}) {
  const now = opts.now ?? (() => Date.now());
  const recent = new Map<string, number>();
  return (response: ResponseLike | null | undefined): boolean => {
    if (!response) return false;
    try {
      opts.clearLast?.();
    } catch {
      // Not available on this platform/build: the dedupe below still covers the launch tap.
    }
    const id = response.notification.request.identifier;
    const key = `${id}\u0000${response.actionIdentifier ?? ""}`;
    const t = now();
    for (const [k, at] of recent) if (t - at > SAME_TAP_MS) recent.delete(k);
    if (id && recent.has(key)) return false;
    if (id) recent.set(key, t);
    go(notificationTarget(response.notification.request.content.data));
    return true;
  };
}

/**
 * Whether the dashboard should ring its own local banner for a new Runners flag: only while this
 * phone has no push token. With one, the server's push already rings (in the foreground too), and
 * a second banner for the same flag would be noise.
 */
export function ringRunnersLocally(pushNew: boolean, token: string | null): boolean {
  return pushNew && !token;
}
