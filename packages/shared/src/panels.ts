// The dashboard's hide-when-empty rule (owner, 2026-10-22: "If a panel has 0 items then we hide
// the panel", then "hide empty SECTIONS inside a panel too"). One definition of "items" per panel,
// shared by the web and the phone so they never disagree about what is shown.
//
// A panel's items are its last KNOWN rows: while a socket is reconnecting the channels keep the
// last frame (marked not connected), so a panel that had rows stays up, dimmed, instead of
// flashing out and back in. Off, not yet known and empty are all "0 items": nothing is drawn, no
// "Connecting…" or quiet line.

import { groupDetections, type DetectionState } from "./detection.ts";
import { isBurstRow, isShownRocketRow, type RocketState } from "./rocket.ts";
import type { RossState } from "./ross.ts";

/** The HOD Momentum scanner's rows today, unfiltered: the category menu narrows what is listed but never hides the panel. */
export function hodItemCount(s: import("./hod.ts").HodState): number {
  return s.enabled ? s.rows.length : 0;
}

/** The Ross Panel's rows (its five-pillar members). */
export function rossItemCount(s: RossState): number {
  // Near-qualifying rows count too (owner, 2026-11-22): a panel with none at 5/5 but stocks only
  // missing evidence is not empty, it is the case worth seeing.
  return s.enabled === true ? (s.panel?.rows.length ?? 0) + (s.readiness?.near?.length ?? 0) : 0;
}

/** Runners' two sections: buying bursts and the strategy's flags. Each is hidden at 0. */
export function rocketSectionCounts(s: RocketState): { bursts: number; flags: number } {
  if (s.enabled !== true || !s.panel) return { bursts: 0, flags: 0 };
  const rows = s.panel.rows.filter(isShownRocketRow);
  const bursts = rows.filter(isBurstRow).length;
  return { bursts, flags: rows.length - bursts };
}

export function rocketItemCount(s: RocketState): number {
  const c = rocketSectionCounts(s);
  return c.bursts + c.flags;
}

/**
 * Signals' two views, Active (developing) and Recent (resolved), unfiltered: the panel's own
 * default rows. The user's filters narrow what is listed but never hide the panel (its filter
 * controls are how they are cleared).
 */
export function detectionViewCounts(s: DetectionState): { active: number; recent: number } {
  if (s.enabled !== true || !s.snapshot) return { active: 0, recent: 0 };
  return { active: groupDetections(s.snapshot, "all", false).length, recent: groupDetections(s.snapshot, "all", true).length };
}

export function detectionItemCount(s: DetectionState): number {
  const c = detectionViewCounts(s);
  return c.active + c.recent;
}

/**
 * The view Signals shows: the one picked while it has rows, else the other. The Active / Recent
 * toggle is drawn only while both have rows (`showToggle`).
 */
export function detectionView(pickedRecent: boolean, c: { active: number; recent: number }): { recent: boolean; showToggle: boolean } {
  const showToggle = c.active > 0 && c.recent > 0;
  if (showToggle) return { recent: pickedRecent, showToggle };
  return { recent: c.active === 0 && c.recent > 0, showToggle };
}

/**
 * The dashboard's panels (owner, 2026-10-30 wireframe, web and desktop): a right column beside the
 * charts with **Favorites, Signals, Top Gainers** stacked (no tabs), and a full-width bottom row
 * under both with **Ross Panel, Runners, Halt watcher, Hot stocks** side by side. Each group keeps
 * this fixed order; a panel shows only while it has items, and a group with none closes.
 */
export type SidePanel = "favorites" | "signals" | "gainers";
export type BottomPanel = "pending" | "ross" | "hod" | "rocket" | "halt" | "hot";
export type DashboardPanel = SidePanel | BottomPanel;
export const SIDE_PANELS: readonly SidePanel[] = ["favorites", "signals", "gainers"];
export const BOTTOM_PANELS: readonly BottomPanel[] = ["pending", "ross", "hod", "rocket", "halt", "hot"];

/** The panels of one group that have items, in the group's fixed order. */
export function visiblePanels<P extends DashboardPanel>(order: readonly P[], counts: Readonly<Partial<Record<DashboardPanel, number>>>): P[] {
  return order.filter((p) => (counts[p] ?? 0) > 0);
}

/**
 * Whether the item at `index` of `count` spans both columns of a two-column grid: the lone last
 * one of an odd count (a single panel included), so a stacked group never ends on an orphan
 * beside a gap.
 */
export function spansPair(index: number, count: number): boolean {
  return count % 2 === 1 && index === count - 1;
}

/**
 * Whether every channel present has said whether it is on (`enabled` no longer null). Until then
 * the dashboard keeps its previous shape rather than growing a panel per channel as each first
 * snapshot lands (or collapsing and reopening when a feed switch resets the channels).
 */
export function panelsSettled(enabled: readonly (boolean | null | undefined)[]): boolean {
  return enabled.every((e) => e !== null);
}
