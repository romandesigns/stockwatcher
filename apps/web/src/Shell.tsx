import { useMemo, useState } from "react";
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

const readCollapsed = () => {
  try {
    return localStorage.getItem(RAIL_KEY) === "1";
  } catch {
    return false;
  }
};

/**
 * The app frame: the main navigation around the open page. From 601 px a rail on the left (the
 * same breakpoint the chart page uses for its phone layout), below that a bottom bar. The chart
 * page is the existing app (sign-in, feed, chart); it mounts only while open, so leaving it
 * closes its market subscription.
 */
export function Shell() {
  const page = usePage();
  const phone = useMediaQuery("(max-width: 600px)");
  const [collapsed, setCollapsed] = useState(readCollapsed);
  const toggle = () =>
    setCollapsed((c) => {
      try {
        localStorage.setItem(RAIL_KEY, c ? "0" : "1");
      } catch {
        // Private mode: the choice lasts for this visit only.
      }
      return !c;
    });

  const settingsOpen = useApp((s) => s.settingsOpen);
  // The page headers' NavToggle reads these (far left of each header); stable while unchanged.
  const rail = useMemo(() => ({ collapsed, toggle }), [collapsed]);

  const content = page === "chart" ? <App /> : <DashboardPage />;

  if (phone) {
    // No rail: no NavToggle, and each page header carries its own Settings gear.
    return (
      <div className="min-h-dvh bg-background" style={{ paddingBottom: `calc(${TAB_BAR_HEIGHT} + env(safe-area-inset-bottom))` }}>
        {content}
        <NavTabBar page={page} />
        <AppSettingsDialog />
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
        <Toaster />
      </div>
    </NavRailContext.Provider>
  );
}
