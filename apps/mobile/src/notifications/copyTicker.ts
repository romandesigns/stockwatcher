import * as Clipboard from "expo-clipboard";
import * as Notifications from "expo-notifications";
import * as TaskManager from "expo-task-manager";
import { Platform } from "react-native";

import { COPY_TICKER_ACTIONS, COPY_TICKER_TASK, copyTickerSymbol, RUNNERS_CATEGORY_ID } from "./copyTickerAction";

// The native half of "Copy ticker" (the decisions and why: copyTickerAction.ts). Imported by
// index.ts for its side effect, before anything else, because expo-task-manager starts the JS
// bundle headless when the button is pressed on a killed app and the task must be defined in
// module scope by then. Kept light: no React, no chart code, so that headless start is quick.

// Runs for a "Copy ticker" press while the app is in the background or killed (Android: expo-
// notifications hands action presses to its tasks only then; in the foreground the response
// listener copies, push.ts). It also runs for every notification received while the app is
// alive, which it ignores.
TaskManager.defineTask<Notifications.NotificationTaskPayload>(COPY_TICKER_TASK, async ({ data }) => {
  const symbol = data && "actionIdentifier" in data ? copyTickerSymbol(data) : null;
  if (symbol) await Clipboard.setStringAsync(symbol).catch(() => {});
});

/** Registers the Runners category (its "Copy ticker" button) and the background task. Idempotent. */
export function registerCopyTicker(): void {
  if (Platform.OS === "web") return;
  // Stored natively: once registered, a push for a killed app still shows the button.
  void Notifications.setNotificationCategoryAsync(RUNNERS_CATEGORY_ID, COPY_TICKER_ACTIONS).catch(() => {});
  void Notifications.registerTaskAsync(COPY_TICKER_TASK).catch(() => {});
}
