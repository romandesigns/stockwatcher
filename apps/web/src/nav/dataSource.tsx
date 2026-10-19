import type { DashboardSource } from "@stockwatcher/app-state";
import type { SettingsDialogSection } from "@stockwatcher/chart-web";
import { DataSourcePanel } from "./DataSourcePanel.tsx";

/**
 * Settings → Data (owner, 2026-10-18): which of the server's two markets the dashboard reads. The
 * chart page always reads replay. Synced (app-state `dashboard.source`), so every device's dashboard
 * follows it; switching reconnects the dashboard's socket on the chosen market.
 */
export function dataSourceSection(source: DashboardSource, onChange: (s: DashboardSource) => void): SettingsDialogSection {
  return {
    id: "data",
    label: "Data",
    icon: "live",
    description: "Where the dashboard's market data comes from.",
    render: () => <DataSourcePanel source={source} onChange={onChange} />,
  };
}
