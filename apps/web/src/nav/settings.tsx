import { chartSettingsSection, drawingSettingsSection, Icon, SettingsDialog } from "@stockwatcher/chart-web";
import { Tip, TOUCH_HIT, cn } from "@stockwatcher/chart-web/ui";
import { useState } from "react";
import { getAppStore, useApp } from "../state/store.ts";
import { dataSourceSection } from "./dataSource.tsx";

/**
 * The app's one Settings dialog (Data, Chart and Drawing tabs), rendered once by the Shell so every page
 * shares it. Opened from the rail's footer on wider screens, from a page header's gear on a phone
 * layout (no rail there). Open state is the store's `settingsOpen`.
 */
export function AppSettingsDialog() {
  const [app] = useState(() => getAppStore().getState());
  const open = useApp((s) => s.settingsOpen);
  const chartSettings = useApp((s) => s.chartSettings);
  const drawingSettings = useApp((s) => s.drawingSettings);
  const source = useApp((s) => s.dashboard.source);
  return <SettingsDialog open={open} onOpenChange={app.setSettingsOpen} persistKey="sw-settings" sections={[dataSourceSection(source, app.setDashboardSource), chartSettingsSection(chartSettings, app.setChartSettings), drawingSettingsSection(drawingSettings, app.setDrawingSettings)]} />;
}

/**
 * The Settings gear for a page header, used only where there is no rail (the phone layout): a quiet
 * round 28 px chip, raised and accent duotone while its dialog is open; 40 px with a 44 px touch
 * area on a coarse pointer.
 */
export function SettingsButton() {
  const [app] = useState(() => getAppStore().getState());
  const open = useApp((s) => s.settingsOpen);
  return (
    <Tip label="Settings">
      <button
        type="button"
        aria-label="Settings"
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => app.setSettingsOpen(true)}
        className={cn(
          "relative inline-grid size-7 shrink-0 place-items-center rounded-full text-muted-foreground outline-none transition-colors hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring pointer-coarse:size-10 [&_svg]:size-3.5",
          TOUCH_HIT,
          open ? "bg-bg-raised text-primary" : "bg-card",
        )}
      >
        <Icon name="settings" />
      </button>
    </Tip>
  );
}
