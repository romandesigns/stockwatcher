import { ConnectionGate } from "../ConnectionGate.tsx";
import { Dashboard } from "../dashboard/Dashboard.tsx";

/**
 * The main page (owner request 2026-10-18): the multi-chart panel and the live ticker list. It
 * will consume only live data (docs/CONVENTIONS.md "Product direction"); it shows whatever the
 * server streams, which is still replay while production runs replay.
 */
export function DashboardPage() {
  return <ConnectionGate>{(c) => <Dashboard choice={c.choice} feed={c.feed} />}</ConnectionGate>;
}
