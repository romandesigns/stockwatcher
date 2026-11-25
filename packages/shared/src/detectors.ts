// The runtime detector switches (crates/server/src/alerts/detectors.rs; DEPLOY-PLAN 2026-11-21
// "Kill rules"): one synced settings document, `detectors`, keyed by detector id, that the server
// re-reads the moment it changes. A detector switched off there keeps running and its alerts are
// held `switched_off` on every device (still in the notification log), with no restart: the
// owner can stop a misfiring research-v2 detector from the phone during market hours. Unlike the
// per-device switches (`ALERT_SWITCH_SECTIONS`), it applies to everybody. Written as an entry
// patch (one key per detector), so two devices flipping two detectors never erase each other.

/** The settings namespace the server reads (client-writable). */
export const DETECTORS_NS = "detectors";

/** The `detectors` entry that keeps the Ross Panel's news pillar required (default on). */
export const ROSS_REQUIRE_NEWS = "ross-require-news";

export interface DetectorSwitch {
  /** The detector id the server and the ledger use (the research finalist's id). */
  id: string;
  name: string;
  description: string;
  /** Its state with no entry in the document: the server's `Detector::default_on`. */
  defaultOn: boolean;
}

const KEEPS = "Off stops it for every device at once; it keeps running and the log keeps what it would have sent.";

/**
 * The detectors shipped behind a runtime switch, in the order Settings lists them: exactly the
 * server's registry (`detectors::ALL`), ids and defaults included. `run` is the cut Run +20% kind,
 * off by default (research v2 scorecard): on puts it back for everybody.
 */
export const DETECTOR_SWITCHES: readonly DetectorSwitch[] = [
  { id: "x-gap-1", name: "Pre-market high", description: `Research v2's x-gap-1. ${KEEPS}`, defaultOn: true },
  { id: "x-ctx-1", name: "Day 2 · above yesterday's high", description: `Research v2's x-ctx-1. ${KEEPS}`, defaultOn: true },
  { id: "x-float-1", name: "Float rotation", description: `Research v2's x-float-1. ${KEEPS}`, defaultOn: true },
  { id: "x-halt-1", name: "Resumed up after a run", description: `Research v2's x-halt-1. ${KEEPS}`, defaultOn: true },
  { id: "rw-mp5-1", name: "Signals crossings, gated", description: `Research v2's rw-mp5-1: the MP5 Signals crossings that pass its in-play gate. ${KEEPS}`, defaultOn: true },
  { id: "rw2-vwapr-1", name: "VWAP reclaim", description: `Research v2's rw2-vwapr-1. ${KEEPS}`, defaultOn: true },
  { id: "rw2-level-1", name: "20-day high break", description: `Research v2's rw2-level-1. ${KEEPS}`, defaultOn: true },
  { id: "b-good-2", name: "Model picks", description: `Research v2's b-good-2. ${KEEPS}`, defaultOn: true },
  { id: "b-good-1", name: "High confidence tag", description: "Research v2's b-good-1. Off hides the tag on every device; it is still scored and logged.", defaultOn: true },
  // Not an alert: the Ross Panel's membership rule, followed by the server's context engine
  // (policies ross-panel-v1 = on, ross-panel-v2 = off). On is the original rule, all five pillars.
  { id: ROSS_REQUIRE_NEWS, name: "Ross Panel: require a news catalyst", description: "On by default: a stock joins the Ross Panel only with fresh news (a Benzinga story, an SEC 8-K/6-K, a press release or a T1 news-pending halt). Off admits stocks on the other four pillars and marks them “No news confirmed”, so check the catalyst yourself. Applies on every device at the next 15 s decision.", defaultOn: true },
  { id: "hod-1", name: "HOD Momentum scanner", description: "The HOD panel's rows (not an alert). Off stops new rows for every device at once; the rows already shown stay.", defaultOn: true },
  { id: "run", name: "Run +20% (cut)", description: "Cut by research v2 (no extra movers, lower precision), so off by default and only recorded. On sends it again for every device.", defaultOn: false },
];

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/**
 * Each listed detector's state from the stored document: its entry's `on` (or a bare boolean),
 * else its default, exactly the server's rule (`Switches::parse`), so a client never shows "on"
 * for a detector the server holds.
 */
export function parseDetectorSwitches(raw: unknown): Record<string, boolean> {
  const doc = isObj(raw) ? raw : {};
  const out: Record<string, boolean> = {};
  for (const d of DETECTOR_SWITCHES) {
    const v = doc[d.id];
    const on = typeof v === "boolean" ? v : isObj(v) && typeof v.on === "boolean" ? v.on : undefined;
    out[d.id] = on ?? d.defaultOn;
  }
  return out;
}

/**
 * "Hide repeat alerts" (alerts.prefs `hideRepeats`, default off; crates/server/src/alerts/runners.rs
 * "Limits" 4): the words every client's Settings row uses. Listed with the detectors because it is
 * the other every-device delivery choice the owner makes from the phone.
 */
export const HIDE_REPEATS_SWITCH = {
  name: "Hide repeat alerts",
  description: "Off by default. On holds an alert for a stock already sent in the last 5 min, on every device (still in the log). Fewer alerts, but repeats often land on real runners.",
} as const;

/**
 * "Group repeat alerts" (alerts.prefs `groupRepeats`, default on; crates/server/src/alerts/runners.rs
 * "Limits" 4a): the words every client's Settings row uses.
 */
export const GROUP_REPEATS_SWITCH = {
  name: "Group repeat alerts",
  description: "On by default. A stock rings once per kind of alert in each session part; its later ones only update its card (count and latest, still in the log). The first alert is never delayed.",
} as const;

/** The entry patch that switches one detector. */
export function detectorPatch(id: string, on: boolean): Record<string, { on: boolean }> {
  return { [id]: { on } };
}
