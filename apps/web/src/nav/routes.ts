import type { IconName } from "@stockwatcher/chart-core";
import { useSyncExternalStore } from "react";

// The app's top-level destinations (owner request 2026-10-18: a main nav whose first page is the
// dashboard). The page lives in the URL hash so a reload, a bookmark and the desktop shell (which
// loads the same index.html) all reopen it; the server's SPA fallback is not needed for hashes.
// Which page is open is this device's own state: it is not synced across devices.

export type PageId = "dashboard" | "chart" | "alerts";

export interface NavItem {
  id: PageId;
  label: string;
  icon: IconName;
  href: string;
}

/** In nav order. The dashboard is the main page, so it owns the empty hash. */
export const NAV_ITEMS: readonly NavItem[] = [
  { id: "dashboard", label: "Dashboard", icon: "dashboard", href: "#/" },
  // The replay icon (owner, 2026-10-18): this page is the replay practice feature, the dashboard is live.
  { id: "chart", label: "Chart", icon: "replay", href: "#/chart" },
  // Price alerts (2026-10-19): the manager and Activity; the rail item carries the unseen-fires badge.
  { id: "alerts", label: "Alerts", icon: "bell", href: "#/alerts" },
];

/** The page a hash names; anything unknown is the dashboard. */
export function pageOf(hash: string): PageId {
  const path = hash.replace(/^#\/?/, "").split(/[?/]/)[0] ?? "";
  return NAV_ITEMS.find((i) => i.id === path)?.id ?? "dashboard";
}

const subscribe = (cb: () => void) => {
  window.addEventListener("hashchange", cb);
  return () => window.removeEventListener("hashchange", cb);
};

/** The open page, following the hash (links in the nav change it; back and forward work). */
export function usePage(): PageId {
  return useSyncExternalStore(subscribe, () => pageOf(window.location.hash), () => "dashboard");
}
