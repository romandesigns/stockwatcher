import type { IconName } from "./icons.ts";

// The Settings dialog (owner request 2026-10-18): "a two columns dialog. One
// column width is 15% and the other one 85%. In the 15% column we will have
// tab-like labels that when active, the right column displays the settings
// features pertaining to that section."
//
// The sections are data, so a future tab is one entry here plus its panel
// (chart-web SettingsDialog and chart-native SettingsSheet take
// `{ id, label, icon, render }` lists built from this). The pure parts both
// renderers share live here: which tab opens, and how the 15 % rail adapts
// when 15 % of the dialog is too narrow for a label.

export type SettingsSectionId = "drawing";

export interface SettingsSectionInfo {
  id: SettingsSectionId;
  label: string;
  icon: IconName;
  /** One line under the panel's heading. */
  description: string;
}

/** Every section, in rail order. */
export const SETTINGS_SECTIONS: readonly SettingsSectionInfo[] = [
  { id: "drawing", label: "Drawing", icon: "pencil", description: "Choose which tools and actions the drawing toolbox shows." },
];

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

/**
 * The smallest rail that still holds a tappable icon tab: 44 pt (Apple's
 * HIG and WCAG 2.5.5 target size). Below it the 15 / 85 split cannot be
 * kept honestly, so the tabs become a row across the top instead.
 */
export const SETTINGS_RAIL_MIN = 44;

export type SettingsRailMode = "labels" | "icons" | "top";

/**
 * How the tab list lays out in a dialog `width` px wide. The 15 / 85 split
 * is kept whenever the rail can hold a 44 px target: on a desktop (880 px
 * dialog, 132 px rail) with labels, on a phone (390 pt screen, 58 pt rail)
 * as an icon rail. Only below ~293 px does it fall back to a top row.
 */
export function settingsRailMode(width: number): SettingsRailMode {
  const rail = width * SETTINGS_RAIL_FRACTION;
  if (rail >= SETTINGS_RAIL_LABEL_MIN) return "labels";
  if (rail >= SETTINGS_RAIL_MIN) return "icons";
  return "top";
}

/** A stored tab → a tab that exists (the first one when the save names a removed or unknown section). */
export function resolveSettingsSection<T extends string>(stored: unknown, ids: readonly T[]): T | null {
  if (typeof stored === "string" && (ids as readonly string[]).includes(stored)) return stored as T;
  return ids[0] ?? null;
}
