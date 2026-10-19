import { useEffect, useMemo, useState } from "react";
import { AlertForm } from "./alerts/AlertForm.tsx";
import { AlertsPage } from "./alerts/AlertsPage.tsx";
import { showToast } from "./lib/toastStore.ts";
import App from "./App.tsx";
import { Toaster } from "./lib/toast.tsx";
import { useMediaQuery } from "./lib/useMediaQuery.ts";
import { NavRail, NavTabBar, TAB_BAR_HEIGHT } from "./nav/MainNav.tsx";
import { NavRailContext } from "./nav/navRail.ts";
import { usePage } from "./nav/routes.ts";
import { AppSettingsDialog } from "./nav/settings.tsx";
import { DashboardPage } from "./pages/DashboardPage.tsx";
import { getAppStore, useApp } from "./state/store.ts";

/** Whether the rail is collapsed, remembered on this device (screen layout is never synced). */
const RAIL_KEY = "sw-nav:collapsed";

/** The person's own choice (true = collapsed), or null when they never toggled the rail here. */
const readCollapsed = (): boolean | null => {
  try {
    const v = localStorage.getItem(RAIL_KEY);
    return v === null ? null : v === "1";
  } catch {
    return null;
  }
};

/**
 * Below this width the rail starts collapsed to its icons (owner, 2026-10-19: on a tablet in
 * portrait, 820 px, or an unfolded Fold, the labelled rail took ~200 px from the charts). It is
 * only the default: a choice made with the toggle is stored and wins at every width.
 */
const RAIL_NARROW = "(max-width: 1099px)";

/**
 * The app frame: the main navigation around the open page. From 601 px a rail on the left (the
 * same breakpoint the chart page uses for its phone layout), below that a bottom bar. The chart
 * page is the existing app (sign-in, feed, chart); it mounts only while open, so leaving it
 * closes its market subscription.
 */
export function Shell() {
  const page = usePage();
  const phone = useMediaQuery("(max-width: 600px)");
  const narrow = useMediaQuery(RAIL_NARROW);
  const [choice, setChoice] = useState(readCollapsed);
  // Without a stored choice the rail follows the width, live (rotating a tablet re-decides it).
  const collapsed = choice ?? narrow;
  const toggle = () => {
    const next = !collapsed;
    try {
      localStorage.setItem(RAIL_KEY, next ? "1" : "0");
    } catch {
      // Private mode: the choice lasts for this visit only.
    }
    setChoice(next);
  };

  const settingsOpen = useApp((s) => s.settingsOpen);
  // The page headers' NavToggle reads these (far left of each header); stable while unchanged.
  const rail = useMemo(() => ({ collapsed, toggle }), [collapsed]);

  const content = page === "chart" ? <App /> : page === "alerts" ? <AlertsPage /> : <DashboardPage />;
  useFiredToasts();

  if (phone) {
    // No rail: no NavToggle, and each page header carries its own Settings gear.
    // A screen-tall frame whose page area scrolls by itself, as beside the rail: the page gets a
    // definite height, so the dashboard's charts can fill what is left above the tab bar instead
    // of sitting at their 18rem minimum over empty space (owner, 2026-10-19).
    return (
      <div className="flex h-dvh flex-col bg-background" style={{ paddingBottom: `calc(${TAB_BAR_HEIGHT} + env(safe-area-inset-bottom))` }}>
        <div className="min-h-0 flex-1 overflow-y-auto">{content}</div>
        <NavTabBar page={page} />
        <AppSettingsDialog />
        <AlertForm />
        {/* Above the tab bar. */}
        <Toaster bottom={`calc(${TAB_BAR_HEIGHT} + env(safe-area-inset-bottom) + 0.75rem)`} />
      </div>
    );
  }
  return (
    <NavRailContext.Provider value={rail}>
      <div className="flex h-dvh bg-background">
        <NavRail page={page} collapsed={collapsed} settingsOpen={settingsOpen} onSettings={() => getAppStore().getState().setSettingsOpen(true)} />
        <div className="min-w-0 flex-1 overflow-y-auto">{content}</div>
        <AppSettingsDialog />
        <AlertForm />
        <Toaster />
      </div>
    </NavRailContext.Provider>
  );
}

/**
 * The in-app delivery of a fired alert (approved design: "an alert.fired frame on every open
 * socket; a toast and an Activity row"): a bell toast with View, whatever page is open. Only for
 * fires heard live (a reconnect's Activity snapshot is history, not news) and only when the
 * alert's "In app" chip is on.
 */
function useFiredToasts() {
  useEffect(
    () =>
      getAppStore().subscribe((s, prev) => {
        const e = s.lastFired;
        if (!e || e === prev.lastFired || !e.app) return;
        showToast(`${e.symbol} · ${e.text}`, { icon: "bell", ms: 6000, action: { label: "View", run: () => void (window.location.hash = "#/alerts") } });
      }),
    [],
  );
}
