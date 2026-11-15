// The "Copy ticker" button on a Runners notification (owner, 2026-10-23). A Fast-move push for
// YMAT said $2.41 within 1.4 s of the move; by the time the ticker was typed into the broker and
// a limit placed, the stock was gone. The button puts the symbol on the clipboard without opening
// this app, so the next app opened is the broker. StockWatcher never places orders.
//
// Pure (bun tests it without a native runtime); the native wiring is copyTicker.ts.
//
// - The server sets `categoryId: "runners"` on every Runners push (crates/server/src/alerts/
//   runners.rs `CATEGORY_ID`, push.rs `expo_message`); the category and its button are
//   registered at startup (index.ts → copyTicker.ts `registerCopyTicker`). Android stores it, so
//   a push that arrives while the app is killed still shows the button. A Galaxy Watch shows the
//   phone notification's actions as buttons of its own, and its press runs on the phone.
// - The button does not bring the app forward (`opensAppToForeground: false`). Who copies:
//   - app in the foreground: the response listener (push.ts `openFrom`), which also shows the
//     "Copied" toast;
//   - app in the background or killed: expo-notifications runs the background task
//     (copyTicker.ts, expo-task-manager; Android only), headless when killed.
//   The listener ignores a copy response unless the app is active and has been listening for a
//   moment: a response the task already handled is also queued for the listener and handed to it
//   when the app next starts, and copying then would overwrite whatever the owner copied since.

import { notificationTarget } from "./route";

/** Must match the server's `runners::CATEGORY_ID`. */
export const RUNNERS_CATEGORY_ID = "runners";
/** The button's action identifier. */
export const COPY_TICKER_ACTION = "copy-ticker";
/** The background task's name (TaskManager.defineTask / Notifications.registerTaskAsync). */
export const COPY_TICKER_TASK = "runners-copy-ticker";
/**
 * How long after the listener starts a copy response is treated as queued from before (the
 * launch delivers queued responses at once; a person cannot press a button that fast).
 */
export const SETTLE_MS = 1500;

/** The category's actions, as setNotificationCategoryAsync takes them. */
export const COPY_TICKER_ACTIONS = [
  {
    identifier: COPY_TICKER_ACTION,
    buttonTitle: "Copy ticker",
    // Copying needs no screen: the point is to switch to the broker, not to this app.
    options: { opensAppToForeground: false },
  },
];

/** The notification's data, whether the listener's object or a task payload's JSON string. */
function contentData(response: unknown): unknown {
  const r = response as { notification?: { request?: { content?: { data?: unknown; dataString?: unknown } } } } | null;
  const content = r?.notification?.request?.content;
  if (!content) return null;
  if (content.data && typeof content.data === "object") {
    const d = content.data as Record<string, unknown>;
    if (typeof d.dataString === "string") return parse(d.dataString);
    return d;
  }
  if (typeof content.data === "string") return parse(content.data);
  if (typeof content.dataString === "string") return parse(content.dataString);
  return null;
}

function parse(s: string): unknown {
  try {
    return JSON.parse(s);
  } catch {
    return null;
  }
}

/** A press of "Copy ticker" (not a tap on the notification itself). */
export function isCopyTickerResponse(response: unknown): boolean {
  return !!response && typeof response === "object" && (response as { actionIdentifier?: unknown }).actionIdentifier === COPY_TICKER_ACTION;
}

/** The ticker a "Copy ticker" press is about: a Runners push's valid symbol, else null. */
export function copyTickerSymbol(response: unknown): string | null {
  if (!isCopyTickerResponse(response)) return null;
  const target = notificationTarget(contentData(response));
  return target.to === "chart" ? target.symbol : null;
}

/**
 * The response listener's half: true when `response` is a "Copy ticker" press (so the caller
 * neither opens a chart nor the Alerts tab), and copies its ticker when the app is active and has
 * been listening for at least SETTLE_MS (module notes).
 */
export function handleCopyTickerResponse(response: unknown, app: { active: boolean; listeningForMs: number }, copy: (symbol: string) => void): boolean {
  if (!isCopyTickerResponse(response)) return false;
  const symbol = copyTickerSymbol(response);
  if (symbol && app.active && app.listeningForMs >= SETTLE_MS) copy(symbol);
  return true;
}
