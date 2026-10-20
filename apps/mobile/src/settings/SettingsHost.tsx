import { chartSettingsSection, drawingSettingsSection, SettingsSheet } from "@stockwatcher/chart-native";
import { useState } from "react";

import { useReducedMotion } from "../motion/useReducedMotion";
import { appActions, useApp } from "../state/store";
import { dataSourceSection } from "./dataSource";

/**
 * The app's one Settings sheet, opened by the Settings tab (owner, 2026-10-19: the tab bar was
 * missing it) or the Chart tab's gear; both set the store's `settingsOpen`. It lives on the
 * dashboard, the first tab, which is mounted as soon as the store has loaded and stays mounted,
 * so the sheet opens from any tab (the Chart tab only mounts once visited).
 */
export function SettingsHost() {
  const [app] = useState(appActions);
  const open = useApp((s) => s.settingsOpen);
  const dashboardSource = useApp((s) => s.dashboard.source);
  const chartSettings = useApp((s) => s.chartSettings);
  const drawingSettings = useApp((s) => s.drawingSettings);
  // The shared indicator set, for the per-indicator legend switches (ids are the same on every chart).
  const indicators = useApp((s) => s.indicators);
  const reducedMotion = useReducedMotion();
  return (
    <SettingsSheet
      open={open}
      onClose={() => app.setSettingsOpen(false)}
      sections={[dataSourceSection(dashboardSource, app.setDashboardSource), chartSettingsSection(chartSettings, app.setChartSettings, indicators), drawingSettingsSection(drawingSettings, app.setDrawingSettings, reducedMotion)]}
      reducedMotion={reducedMotion}
    />
  );
}
