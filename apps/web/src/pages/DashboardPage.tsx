import { ConnectionGate } from "../ConnectionGate.tsx";
import { useApp } from "../state/store.ts";
import { Dashboard } from "../dashboard/Dashboard.tsx";

/**
 * The main page (owner request 2026-10-18): the multi-chart panel. It reads the server's live
 * market unless Settings → Data says replay (app-state `dashboard.source`, synced); changing it
 * reconnects the gate on the other market.
 */
export function DashboardPage() {
  const source = useApp((s) => s.dashboard.source);
  return <ConnectionGate source={source}>{(c) => <Dashboard choice={c.choice} feed={c.feed} />}</ConnectionGate>;
}
