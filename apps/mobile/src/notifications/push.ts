import * as Device from "expo-device";
import * as Notifications from "expo-notifications";
import { Platform } from "react-native";

import { navigateToAlerts, openTickerChart } from "../navigation/ref";
import { getPreference, setPreference } from "../storage/preferences";
import { createResponseRouter, type NotificationTarget } from "./route";
import { configureForegroundPresentation, registerForPushNotifications } from "./setup";

// Phone alerts (crates/server/src/alerts/push.rs): this phone's Expo push token, registered with
// the server so it can reach the phone while the app is closed. The token is per device and never
// synced; it goes to the server in `push.register` on every connection (idempotent), so a server
// that lost its device list, or a token Expo rotated, heals on the next connect.
//
// Asking for permission is a user action (the Alerts tab's "Turn on phone alerts"), never a
// launch-time prompt; at launch a token is only refreshed when permission was already given.

const KEY = "push.token";
let token: string | null = null;
const listeners = new Set<(t: string | null) => void>();

/** The token this phone registers, once known (synchronous, for the feeds' welcome). */
export function pushToken(): string | null {
  return token;
}

export function subscribePushToken(fn: (t: string | null) => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

function setToken(t: string | null) {
  token = t;
  void setPreference(KEY, t).catch(() => {});
  listeners.forEach((l) => l(t));
}

/** What `push.register` carries besides the token: a name for the server's log and list. */
export function deviceInfo(): { platform: string; name: string } {
  return { platform: Platform.OS, name: Device.modelName ?? Device.deviceName ?? Platform.OS };
}

/** Where a tap goes: a ticker's notification opens THAT ticker's chart (route.ts), anything else the Alerts tab. */
export function goToTarget(target: NotificationTarget): void {
  if (target.to === "chart") openTickerChart(target.symbol);
  else navigateToAlerts();
}

// One router for the app's life: the tap that launched the app comes both from
// getLastNotificationResponse and from the listener, and is routed once; after each tap the native
// "last response" is cleared, so nothing can replay an old tap over a chart opened since (a JS
// reload, a remounted root running initPush again, then reading the old response).
const openFrom = createResponseRouter(goToTarget, { clearLast: () => Notifications.clearLastNotificationResponse() });
let listening = false;

/**
 * At launch: the stored token at once, then a fresh one when permission is already granted (no
 * prompt). Also routes a tapped notification (src/notifications/route.ts), including the one
 * that opened the app from closed.
 */
export async function initPush(): Promise<void> {
  configureForegroundPresentation();
  // Once: initPush runs again whenever the root remounts (an Activity recreated in the
  // background), and each extra listener would route every later tap again.
  if (!listening) {
    listening = true;
    Notifications.addNotificationResponseReceivedListener(openFrom);
  }
  try {
    openFrom(Notifications.getLastNotificationResponse());
  } catch {
    // Not available on this platform/build: a cold-start tap just opens the app.
  }
  token = await getPreference<string>(KEY).catch(() => null);
  try {
    const perm = await Notifications.getPermissionsAsync();
    if (perm.granted) {
      const r = await registerForPushNotifications();
      if (r.status === "granted" && r.expoPushToken !== token) setToken(r.expoPushToken);
    }
  } catch {
    // An emulator or a build without push: in-app alerts still work.
  }
}

/** The Alerts tab's "Turn on phone alerts": asks for permission, then registers. */
export async function enablePush(): Promise<"granted" | "denied" | string> {
  try {
    const r = await registerForPushNotifications();
    if (r.status === "granted") {
      setToken(r.expoPushToken);
      return "granted";
    }
    return r.status === "denied" ? "denied" : r.reason;
  } catch (e) {
    // Android without FCM credentials in the build (google-services.json) cannot get a token:
    // say so instead of failing silently. In-app alerts still work.
    return `Phone alerts are not available in this build (${e instanceof Error ? e.message : String(e)}).`;
  }
}
