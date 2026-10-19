import { useSyncExternalStore } from "react";
import { getAppStore, useApp } from "../state/store.ts";

// The rail's Alerts badge: fires this device has not seen on the Alerts page yet. "Seen" is a
// per-device convenience (a timestamp in localStorage), never synced: reading Activity on the
// desktop says nothing about the phone.

const KEY = "sw-alerts:seen";
const listeners = new Set<() => void>();

const read = (): number => {
  try {
    return Number(localStorage.getItem(KEY)) || 0;
  } catch {
    return 0;
  }
};

let seen = read();

/** Everything fired so far counts as seen (the Alerts page is open). */
export function markAlertsSeen(): void {
  const newest = getAppStore().getState().alertActivity[0]?.at ?? 0;
  if (newest <= seen) return;
  seen = newest;
  try {
    localStorage.setItem(KEY, String(seen));
  } catch {
    // Private mode: the badge resets on reload, nothing else.
  }
  listeners.forEach((l) => l());
}

/** How many fires arrived since the Alerts page was last open. */
export function useUnseenAlerts(): number {
  const since = useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => seen,
  );
  return useApp((s) => s.alertActivity.filter((e) => e.kind === "fired" && e.at > since).length);
}
