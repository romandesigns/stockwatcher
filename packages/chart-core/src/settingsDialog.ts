import type { IconName } from "./icons.ts";

// The Settings dialog (owner request 2026-10-18): "a two columns dialog. One
// column width is 15% and the other one 85%. In the 15% column we will have
// tab-like labels that when active, the right column displays the settings
// features pertaining to that section."
//
// Follow-up the same day: "Instead of two columns tab navigation settings
// modal on mobile, I think we should make it a single column instead." So a
// phone gets the iOS / Android system Settings pattern instead: a root list
// of the sections, and a tap drills into one at full width with a back
// button. Tablets and desktops keep the two columns.
//
// The sections are data, so a future tab is one entry here plus its panel
// (chart-web SettingsDialog and chart-native SettingsSheet take
// `{ id, label, icon, render }` lists built from this, and draw each one as
// a rail tab or a root-list row alike). The pure parts both renderers share
// live here: which tab opens, which layout a screen gets, how the 15 % rail
// adapts when 15 % of the dialog is too narrow for a label, and the
// single-column drill-down's navigation state.

export type SettingsSectionId = "chart" | "drawing";

export interface SettingsSectionInfo {
  id: SettingsSectionId;
  label: string;
  icon: IconName;
  /** One line under the panel's heading (and under the row's label on the single-column root list). */
  description: string;
}

/**
 * Every section, in rail order. Chart is first (owner, 2026-10-18: the
 * chart settings moved here from the toolbar's Chart popover, which was
 * removed, so this tab is now their only home). Putting it first does not
 * disturb the remembered tab: resolveSettingsSection keeps whatever id was
 * stored ("drawing" still opens Drawing for someone who left it there), and
 * only a first open, or a stored id that no longer exists, lands on Chart.
 */
export const SETTINGS_SECTIONS: readonly SettingsSectionInfo[] = [
  { id: "chart", label: "Chart", icon: "chart", description: "Chart type, zoom, price scale and what the chart shows." },
  { id: "drawing", label: "Drawing", icon: "pencil", description: "Choose which tools and actions the drawing toolbox shows." },
];

// ---------------------------------------------------------------------------
// Which layout: two columns or one.

export type SettingsLayout = "two-column" | "single-column";

/**
 * The web viewport width below which Settings is single column. 640 px is
 * Tailwind's `sm`, the usual phone / tablet line: every phone in portrait
 * (and most in landscape) is under it; a tablet or a desktop window is over
 * it. Above it the two-column dialog is kept, with the icon-only rail
 * between 640 and ~812 px (settingsRailMode).
 */
export const SETTINGS_SINGLE_COLUMN_BELOW = 640;

/**
 * Which Settings layout to draw. `app` is where it runs: the phone app
 * (chart-native) is ALWAYS single column, whatever its width, because it
 * is a phone UI even when stretched across a tablet and the owner's request
 * was about the phone; the web is single column only below
 * SETTINGS_SINGLE_COLUMN_BELOW, so a desktop or tablet keeps its 15 / 85
 * dialog unchanged.
 */
export function settingsLayout(viewportWidth: number, app: "web" | "phone"): SettingsLayout {
  if (app === "phone") return "single-column";
  return viewportWidth < SETTINGS_SINGLE_COLUMN_BELOW ? "single-column" : "two-column";
}

// ---------------------------------------------------------------------------
// The two-column dialog's rail.

/** The rail's share of the dialog width (owner: 15 / 85). */
export const SETTINGS_RAIL_FRACTION = 0.15;

/**
 * The rail width at which a label still fits beside its icon: 16 px icon +
 * 8 px gap + ~56 px for "Drawing" at 13 px + 2 × 12 px padding + 8 px of
 * rail inset. Below it the rail shows icons only, with the label as the
 * tooltip and accessible name (the panel's heading names the section too,
 * so nothing is lost visually).
 */
export const SETTINGS_RAIL_LABEL_MIN = 112;

export type SettingsRailMode = "labels" | "icons";

/**
 * How the two-column dialog's rail lays out in a dialog `width` px wide:
 * labels on a desktop (880 px dialog, 132 px rail), icons only below that.
 * The narrowest two-column dialog is now 92 % of a 640 px window, 589 px,
 * whose 88 px rail is still double a 44 px touch target. The third mode
 * this used to have (a row of tabs across the top once the rail fell under
 * 44 px) went away with the single-column phone layout: no two-column
 * dialog is ever that narrow any more.
 */
export function settingsRailMode(width: number): SettingsRailMode {
  return width * SETTINGS_RAIL_FRACTION >= SETTINGS_RAIL_LABEL_MIN ? "labels" : "icons";
}

/** A stored tab → a tab that exists (the first one when the save names a removed or unknown section). */
export function resolveSettingsSection<T extends string>(stored: unknown, ids: readonly T[]): T | null {
  if (typeof stored === "string" && (ids as readonly string[]).includes(stored)) return stored as T;
  return ids[0] ?? null;
}

// ---------------------------------------------------------------------------
// The single-column drill-down.

/**
 * Where the single-column Settings is: on the root list (`section` null) or
 * drilled into one section. `returnTo` is the section just left by a pop, so
 * the renderer can hand focus back to the row that opened it (the usual
 * drill-down rule: focus moves to the new screen's heading on a push, and
 * back to the control that opened it on a pop, so a keyboard or screen
 * reader user is never dropped at the top of the list).
 */
export interface SettingsNav {
  section: string | null;
  returnTo: string | null;
}

export type SettingsNavAction = { type: "push"; id: string } | { type: "pop" } | { type: "reset" };

export const SETTINGS_NAV_ROOT: SettingsNav = { section: null, returnTo: null };

/**
 * The drill-down's one state transition. `reset` is what opening Settings
 * does: single column ALWAYS opens on the root list, not on the section
 * used last (decided 2026-10-18: one predictable starting point is the
 * simpler mental model, it is what the system Settings apps do, and every
 * section is one tap from the root anyway). The two-column dialog keeps its
 * remembered tab; that is separate state and this does not touch it.
 */
export function settingsNavReduce(nav: SettingsNav, action: SettingsNavAction): SettingsNav {
  switch (action.type) {
    case "push":
      return { section: action.id, returnTo: null };
    case "pop":
      return nav.section === null ? nav : { section: null, returnTo: nav.section };
    case "reset":
      return SETTINGS_NAV_ROOT;
  }
}

/**
 * What a "back" does (Escape on the web; the Android hardware or gesture
 * back on the phone): inside a section it pops to the root list, on the root
 * it closes Settings. The system Settings apps' rule, so back never throws
 * away more than one level.
 */
export function settingsBack(nav: SettingsNav): "pop" | "close" {
  return nav.section === null ? "close" : "pop";
}
