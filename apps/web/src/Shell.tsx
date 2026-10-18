import { useState } from "react";
import App from "./App.tsx";
import { Toaster } from "./lib/toast.tsx";
import { useMediaQuery } from "./lib/useMediaQuery.ts";
import { NavRail, NavTabBar, TAB_BAR_HEIGHT } from "./nav/MainNav.tsx";
import { usePage } from "./nav/routes.ts";
import { DashboardPage } from "./pages/DashboardPage.tsx";

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

  const content = page === "chart" ? <App /> : <DashboardPage />;

  if (phone) {
    return (
      <div className="min-h-dvh bg-background" style={{ paddingBottom: `calc(${TAB_BAR_HEIGHT} + env(safe-area-inset-bottom))` }}>
        {content}
        <NavTabBar page={page} />
        {/* Above the tab bar. */}
        <Toaster bottom={`calc(${TAB_BAR_HEIGHT} + env(safe-area-inset-bottom) + 0.75rem)`} />
      </div>
    );
  }
  return (
    <div className="flex h-dvh bg-background">
      <NavRail page={page} collapsed={collapsed} onToggle={toggle} />
      <div className="min-w-0 flex-1 overflow-y-auto">{content}</div>
      <Toaster />
    </div>
  );
}
