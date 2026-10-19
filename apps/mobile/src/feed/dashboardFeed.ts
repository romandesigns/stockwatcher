import { useSyncExternalStore } from "react";

import type { MobileFeed } from "./serverFeed";

// The dashboard's live connection, shared with the ticker chart modal. The modal is a root-stack
// screen above the tabs, so it cannot take the feed as a prop; the dashboard tab stays mounted
// while it is open, so its socket (live or replay, per Settings → Data) is already connected and
// has its chart slots free since the dashboard shows no chart of its own. Opening the modal then
// costs a subscription, not a new socket and handshake.

let current: MobileFeed | null = null;
const listeners = new Set<() => void>();

export function setDashboardFeed(feed: MobileFeed | null): void {
  current = feed;
  for (const l of listeners) l();
}

export function useDashboardFeed(): MobileFeed | null {
  return useSyncExternalStore(
    (fn) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    () => current,
  );
}
