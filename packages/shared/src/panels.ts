// The dashboard's hide-when-empty rule (owner, 2026-10-22: "If a panel has 0 items then we hide
// the panel", then "hide empty SECTIONS inside a panel too"). One definition of "items" per panel,
// shared by the web and the phone so they never disagree about what is shown.
//
// A panel's items are its last KNOWN rows: while a socket is reconnecting the channels keep the
// last frame (marked not connected), so a panel that had rows stays up, dimmed, instead of
// flashing out and back in. Off, not yet known and empty are all "0 items": nothing is drawn, no
// "Connecting…" or quiet line.

import { groupDetections, type DetectionState } from "./detection.ts";
import { isBurstRow, type RocketState } from "./rocket.ts";
import type { RossState } from "./ross.ts";

/** The Ross Panel's rows (its five-pillar members). */
export function rossItemCount(s: RossState): number {
  return s.enabled === true ? (s.panel?.rows.length ?? 0) : 0;
}

/** Runners' two sections: buying bursts and the strategy's flags. Each is hidden at 0. */
export function rocketSectionCounts(s: RocketState): { bursts: number; flags: number } {
  if (s.enabled !== true || !s.panel) return { bursts: 0, flags: 0 };
  const bursts = s.panel.rows.filter(isBurstRow).length;
  return { bursts, flags: s.panel.rows.length - bursts };
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

export type SideTab = "ross" | "rocket" | "signals";
export const SIDE_TABS: readonly SideTab[] = ["ross", "rocket", "signals"];

/** The web side column's tabs with items, in their fixed order. */
export function visibleSideTabs(counts: Readonly<Record<SideTab, number>>): SideTab[] {
  return SIDE_TABS.filter((t) => counts[t] > 0);
}

/** The tab shown: the picked one while it has items, else the first that has (undefined: none). */
export function pickSideTab(visible: readonly SideTab[], picked: SideTab): SideTab | undefined {
  return visible.includes(picked) ? picked : visible[0];
}

/**
 * Whether every channel present has said whether it is on (`enabled` no longer null). Until then
 * the side column keeps its previous shape rather than growing a tab per channel as each first
 * snapshot lands (or collapsing and reopening when a feed switch resets the channels).
 */
export function panelsSettled(enabled: readonly (boolean | null | undefined)[]): boolean {
  return enabled.every((e) => e !== null);
}
