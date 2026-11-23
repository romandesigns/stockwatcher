import type { HodState, PendingState } from "@stockwatcher/shared";

/** A panel the live dashboard has that does not run on the replay clock yet. */
export interface OffPanel {
  id: "hod" | "pending";
  label: string;
}

/**
 * The live dashboard's panels that are still off on the replay market, from their own snapshots
 * (`enabled: false` there): HOD Momentum and News pending read evidence on the wall clock today
 * (replay parity plan, Phase 3). Unknown until the snapshot arrives, so nothing is listed early.
 * When a later phase turns one on for replay, it leaves this list by itself.
 */
export function offPanels(hod: Pick<HodState, "connected" | "enabled" | "available">, pending: Pick<PendingState, "snapshot" | "enabled" | "available">): OffPanel[] {
  const out: OffPanel[] = [];
  if (hod.available && hod.connected && !hod.enabled) out.push({ id: "hod", label: "HOD Momentum" });
  if (pending.available && pending.snapshot && !pending.enabled) out.push({ id: "pending", label: "News pending" });
  return out;
}
