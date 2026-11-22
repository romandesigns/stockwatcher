// The runtime detector switches (crates/server/src/alerts/detectors.rs; DEPLOY-PLAN 2026-11-21
// "Kill rules"): one synced settings document, `detectors`, keyed by detector id, that the server
// re-reads the moment it changes. A detector switched off there keeps running and its alerts are
// held `switched_off` on every device (still in the notification log), with no restart: the
// owner can stop a misfiring research-v2 detector from the phone during market hours. Unlike the
// per-device switches (`ALERT_SWITCH_SECTIONS`), it applies to everybody. Written as an entry
// patch (one key per detector), so two devices flipping two detectors never erase each other.

/** The settings namespace the server reads (client-writable). */
export const DETECTORS_NS = "detectors";

export interface DetectorSwitch {
  /** The detector id the server and the ledger use (the research finalist's id). */
  id: string;
  name: string;
  description: string;
}

/** The detectors shipped behind a runtime switch, in the order Settings lists them. */
export const DETECTOR_SWITCHES: readonly DetectorSwitch[] = [
  {
    id: "x-float-1",
    name: "Float rotation",
    description: "Research v2's x-float-1. Off stops it for every device at once; it keeps running and the log keeps what it would have sent.",
  },
];

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/**
 * Each listed detector's state from the stored document: on unless its entry says `on: false`
 * (or is the bare `false`), exactly the server's rule (`Switches::parse`), so a client never
 * shows "on" for a detector the server holds.
 */
export function parseDetectorSwitches(raw: unknown): Record<string, boolean> {
  const doc = isObj(raw) ? raw : {};
  const out: Record<string, boolean> = {};
  for (const d of DETECTOR_SWITCHES) {
    const v = doc[d.id];
    const on = typeof v === "boolean" ? v : isObj(v) && typeof v.on === "boolean" ? v.on : undefined;
    out[d.id] = on !== false;
  }
  return out;
}

/** The entry patch that switches one detector. */
export function detectorPatch(id: string, on: boolean): Record<string, { on: boolean }> {
  return { [id]: { on } };
}
