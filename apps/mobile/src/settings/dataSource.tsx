import type { DashboardSource } from "@stockwatcher/app-state";
import type { SettingsSheetSection } from "@stockwatcher/chart-native";

import { DataSourcePanel } from "./DataSourcePanel";

/**
 * Settings → Data (owner, 2026-10-18): which of the server's two markets the dashboard tab reads.
 * The Chart tab always reads replay. Synced (app-state `dashboard.source`), the web's Data tab too.
 */
export function dataSourceSection(source: DashboardSource, onChange: (s: DashboardSource) => void): SettingsSheetSection {
  return {
    id: "data",
    label: "Data",
    icon: "live",
    description: "Where the dashboard's market data comes from.",
    render: () => <DataSourcePanel source={source} onChange={onChange} />,
  };
}
