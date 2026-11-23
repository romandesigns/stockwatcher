import { ConnectionGate } from "../ConnectionGate.tsx";
import { Dashboard } from "../dashboard/Dashboard.tsx";

/**
 * The main page (owner request 2026-10-18): the multi-chart panel. It reads ONLY the server's live
 * market (owner, 2026-11-23: the replay simulator must not collide with the dashboard), so its
 * gate is pinned to `live` and nothing here can pick another market. Replay data appears only on
 * the practice page (the Chart nav item, App.tsx).
 */
export const DASHBOARD_SOURCE = "live" as const;

export function DashboardPage() {
  return <ConnectionGate source={DASHBOARD_SOURCE}>{(c) => <Dashboard choice={c.choice} feed={c.feed} />}</ConnectionGate>;
}
