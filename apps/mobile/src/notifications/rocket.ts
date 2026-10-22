import type { RocketRowWire } from "@stockwatcher/shared";
import * as Notifications from "expo-notifications";
import { Platform } from "react-native";
import { useSyncExternalStore } from "react";

import { getPreference, setPreference } from "../storage/preferences";
import { ALERTS_CHANNEL_ID, configureForegroundPresentation, ensureAlertsChannel } from "./setup";

// Rocket watch's optional local notification (owner, 2026-10-21): a banner with sound for each
// new flag while the dashboard is open. Off by default; this device only (not synced: it is
// about this phone making noise). A local notification, not a push: nothing reaches the server.

const KEY = "rocketWatch.notify";
let on = false;
const listeners = new Set<() => void>();
const emit = () => {
  for (const l of listeners) l();
};
void getPreference<boolean>(KEY).then((v) => {
  on = v === true;
  emit();
});

export function rocketNotifyEnabled(): boolean {
  return on;
}

export function useRocketNotify(): boolean {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => on,
  );
}

/** Turns it on (asking for permission first; refused = stays off) or off. Returns the result. */
export async function setRocketNotify(want: boolean): Promise<boolean> {
  if (want) {
    await ensureAlertsChannel();
    const current = await Notifications.getPermissionsAsync();
    const final = current.granted ? current : await Notifications.requestPermissionsAsync();
    if (!final.granted) want = false;
    else configureForegroundPresentation();
  }
  on = want;
  await setPreference(KEY, want);
  emit();
  return want;
}

/** One banner (with the channel's sound) per new flag. */
export function notifyRocketFlags(rows: RocketRowWire[]): void {
  if (!on) return;
  for (const r of rows.slice(0, 3)) {
    void Notifications.scheduleNotificationAsync({
      content: {
        title: `Rocket watch: ${r.s} ${r.p.toFixed(r.p >= 1 ? 2 : 4)}`,
        body: r.reasons.length ? r.reasons.slice(0, 2).join(" · ") : "Flagged (experimental)",
        sound: true,
      },
      // Now, on the alerts channel (Android: its sound and importance).
      trigger: Platform.OS === "android" ? { channelId: ALERTS_CHANNEL_ID } : null,
    });
  }
}
