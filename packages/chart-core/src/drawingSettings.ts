import { DRAWING_KINDS, DRAWING_LABELS, type DrawingKind } from "./drawings.ts";
import type { IconName } from "./icons.ts";

// Which buttons the floating drawing toolbox shows (owner request
// 2026-10-18: "the drawing tab ... should allow us to select the features
// the drawing feature should display when active"). The Settings dialog's
// Drawing tab edits this; chart-web's and chart-native's DrawingToolbox read
// it through `toolboxLayout`, so the web and the phone hide the same things
// from the same list (TOOLBOX_ITEMS is the single source of truth: the
// settings rows, their order and the toolbox's order all come from it).
//
// Rules, each with its reason:
// - The cursor cannot be hidden. It is how you leave an armed tool and how
//   you select a drawing; a palette without it would strand you in a tool.
//   The settings row shows it locked, with that reason (LOCKED_REASON).
// - Hiding a tool that is armed disarms it (`armedAfter`), back to the
//   cursor, so no invisible tool is waiting for the next click. The
//   keyboard shortcuts (L / S on the web) obey the same check
//   (`toolAllowed`): a hidden tool cannot be armed from the keyboard either.
// - Drawings already made with a hidden tool stay on the chart and stay
//   editable: select, drag, recolour (if the colour button shows), delete
//   (Del on the web, or the trash button if it shows). Hiding a TOOL hides
//   the way to make a new one, never the drawings. (Hiding all drawings is
//   the Chart settings' Visibility switch, a different thing.)
// - Shift+drag measure on the web is a modifier gesture, not a toolbox
//   button, so it keeps working with the Measure button hidden.
// - Hiding the grip pins the palette where it is (its placement is kept);
//   hiding the flip button keeps its current orientation.
//
// Stored as the HIDDEN ids, not the visible ones: a button added to the
// toolbox later then shows by default for everyone, instead of being
// silently missing from every existing save. Synced across every client
// beside the chart settings (namespace `drawingSettings`, syncedSettings.ts);
// before sync it lived in `sw-chart:drawing` (web) and the phone's
// `chart.drawing` preference, read once to migrate.

export type ToolboxToolId = "cursor" | DrawingKind;
export type ToolboxActionId = "color" | "delete" | "undo" | "redo" | "rotate" | "grip";
export type ToolboxItemId = ToolboxToolId | ToolboxActionId;
export type ToolboxGroup = "tools" | "actions";

export interface ToolboxItemInfo {
  id: ToolboxItemId;
  group: ToolboxGroup;
  name: string;
  /** One line for the settings row. */
  description: string;
  /** Its duotone icon (icons.ts), the same mark the toolbox draws. */
  icon: IconName;
  /** Present when the item cannot be hidden: the reason, shown on the row. */
  locked?: string;
}

export const LOCKED_REASON = "Always shown: it is how you leave a tool and select drawings";

/** Every toolbox button in toolbox order: tools first (cursor, the drawing kinds, measure), then the actions. */
export const TOOLBOX_ITEMS: readonly ToolboxItemInfo[] = [
  { id: "cursor", group: "tools", name: "Cursor", description: "Pan, inspect and select drawings", icon: "cursor", locked: LOCKED_REASON },
  { id: "trend", group: "tools", name: DRAWING_LABELS.trend, description: "A line through two points", icon: "trend" },
  { id: "ray", group: "tools", name: DRAWING_LABELS.ray, description: "A line from a point that runs on to the edge", icon: "ray" },
  { id: "hline", group: "tools", name: DRAWING_LABELS.hline, description: "A price level across the chart", icon: "hline" },
  { id: "vline", group: "tools", name: DRAWING_LABELS.vline, description: "A moment in time, top to bottom", icon: "vline" },
  { id: "rect", group: "tools", name: DRAWING_LABELS.rect, description: "A shaded price and time range", icon: "rect" },
  { id: "long", group: "tools", name: DRAWING_LABELS.long, description: "Entry, target and stop for a buy (L)", icon: "long" },
  { id: "short", group: "tools", name: DRAWING_LABELS.short, description: "Entry, target and stop for a sell (S)", icon: "short" },
  { id: "measure", group: "tools", name: DRAWING_LABELS.measure, description: "Price, percent and time between two points", icon: "measure" },
  { id: "color", group: "actions", name: "Colour", description: "Recolour the selected drawing", icon: "color" },
  { id: "delete", group: "actions", name: "Delete", description: "Remove the selected drawing (Del still works)", icon: "trash" },
  { id: "undo", group: "actions", name: "Undo", description: "Step back one drawing change (Ctrl+Z still works)", icon: "undo" },
  { id: "redo", group: "actions", name: "Redo", description: "Step forward again (Ctrl+Shift+Z still works)", icon: "redo" },
  { id: "rotate", group: "actions", name: "Flip layout", description: "Switch the palette between a column and a row", icon: "rotate" },
  { id: "grip", group: "actions", name: "Move handle", description: "Drag the palette around; hidden, it stays put", icon: "grip" },
];

export const TOOLBOX_ITEM_IDS: readonly ToolboxItemId[] = TOOLBOX_ITEMS.map((i) => i.id);
const KNOWN = new Set<string>(TOOLBOX_ITEM_IDS);
const LOCKED = new Set<ToolboxItemId>(TOOLBOX_ITEMS.filter((i) => i.locked).map((i) => i.id));

/** The tool buttons in toolbox order (cursor, the drawing kinds, measure). */
export const TOOLBOX_TOOLS: readonly ToolboxToolId[] = ["cursor", ...DRAWING_KINDS, "measure"];
/** The edit pill's buttons, in order (the toolbox draws them touching, on one dimmed surface). */
export const TOOLBOX_EDIT: readonly Exclude<ToolboxActionId, "color" | "grip">[] = ["delete", "undo", "redo", "rotate"];

export interface DrawingSettings {
  /** Bumped when the shape changes; parseDrawingSettings migrates older saves. */
  version: 1;
  /** Hidden toolbox items, in TOOLBOX_ITEMS order, never a locked one. */
  hidden: ToolboxItemId[];
}

/** Everything shown: today's toolbox, so nothing changes until the owner opens the tab. */
export const DEFAULT_DRAWING_SETTINGS: DrawingSettings = { version: 1, hidden: [] };

/** Old or alternative names a save may carry, mapped to today's ids. */
const ALIASES: Readonly<Record<string, ToolboxItemId>> = {
  select: "cursor",
  pointer: "cursor",
  trash: "delete",
  remove: "delete",
  colour: "color",
  flip: "rotate",
  orientation: "rotate",
  move: "grip",
  handle: "grip",
  line: "trend",
  range: "rect",
  ruler: "measure",
};

const canon = (id: unknown): ToolboxItemId | null => {
  if (typeof id !== "string") return null;
  const k = ALIASES[id] ?? id;
  return KNOWN.has(k) ? (k as ToolboxItemId) : null;
};

/** A hidden list cleaned up: known ids only, no locked ones, no repeats, in toolbox order. */
function normalizeHidden(ids: Iterable<ToolboxItemId>): ToolboxItemId[] {
  const set = new Set(ids);
  return TOOLBOX_ITEM_IDS.filter((id) => set.has(id) && !LOCKED.has(id));
}

/**
 * A saved value → valid settings. Storage outlives code, so junk never
 * breaks the toolbox: unknown ids are dropped, a locked id (the cursor) is
 * ignored, and anything unreadable is the default (everything shown).
 * Migration of other shapes:
 * - no `version` with a `hidden` array is read as v1;
 * - a `visible` (or `items`) record of booleans, `{ trend: false }`, hides
 *   the false ones (the obvious first shape; nothing missing from it hides);
 * - a `shown` array lists the visible ids: everything else known is hidden;
 * - old names (`select`, `trash`, `flip`, `colour`, …) map through ALIASES.
 */
export function parseDrawingSettings(raw: unknown): DrawingSettings {
  if (typeof raw !== "object" || raw === null) return { ...DEFAULT_DRAWING_SETTINGS, hidden: [] };
  const r = raw as Record<string, unknown>;
  let hidden: ToolboxItemId[] = [];
  if (Array.isArray(r.hidden)) {
    hidden = r.hidden.map(canon).filter((x): x is ToolboxItemId => x !== null);
  } else if (Array.isArray(r.shown)) {
    const shown = new Set(r.shown.map(canon));
    hidden = TOOLBOX_ITEM_IDS.filter((id) => !shown.has(id));
  } else {
    const rec = (typeof r.visible === "object" && r.visible !== null ? r.visible : typeof r.items === "object" && r.items !== null ? r.items : null) as Record<string, unknown> | null;
    for (const [k, v] of Object.entries(rec ?? {})) {
      const id = v === false ? canon(k) : null;
      if (id) hidden.push(id);
    }
  }
  return { version: 1, hidden: normalizeHidden(hidden) };
}

export const isToolboxItemLocked = (id: ToolboxItemId): boolean => LOCKED.has(id);

export function isToolboxItemVisible(s: DrawingSettings, id: ToolboxItemId): boolean {
  return LOCKED.has(id) || !s.hidden.includes(id);
}

/** Shows or hides one item (a locked item stays shown). */
export function setToolboxItemVisible(s: DrawingSettings, id: ToolboxItemId, visible: boolean): DrawingSettings {
  if (LOCKED.has(id)) return s;
  const rest = s.hidden.filter((x) => x !== id);
  return { version: 1, hidden: normalizeHidden(visible ? rest : [...rest, id]) };
}

/** Shows or hides a whole group (the "all" switch on a group header); locked items stay shown. */
export function setToolboxGroupVisible(s: DrawingSettings, group: ToolboxGroup, visible: boolean): DrawingSettings {
  const ids = TOOLBOX_ITEMS.filter((i) => i.group === group).map((i) => i.id);
  const rest = s.hidden.filter((x) => !ids.includes(x));
  return { version: 1, hidden: normalizeHidden(visible ? rest : [...rest, ...ids]) };
}

export const isDefaultDrawingSettings = (s: DrawingSettings): boolean => s.hidden.length === 0;

/** Whether a tool may be armed (null, the cursor, always may). Keyboard shortcuts check this too. */
export function toolAllowed(s: DrawingSettings, tool: DrawingKind | null): boolean {
  return tool === null || isToolboxItemVisible(s, tool);
}

/** The armed tool after a settings change: a tool that is now hidden is disarmed (back to the cursor). */
export function armedAfter(tool: DrawingKind | null, s: DrawingSettings): DrawingKind | null {
  return toolAllowed(s, tool) ? tool : null;
}

/** What the toolbox draws, in order, for the current settings. */
export interface ToolboxLayout {
  grip: boolean;
  /** Tool buttons (the cursor is always first). */
  tools: ToolboxToolId[];
  color: boolean;
  /** The edit pill's buttons; empty means no pill at all (never an empty dimmed surface). */
  edit: Exclude<ToolboxActionId, "color" | "grip">[];
  /** The soft bar between tools and actions: only when there are actions on its far side. */
  separator: boolean;
  /** Buttons on the main line(s), grip and separator excluded: what the phone's split rule measures. */
  buttons: number;
}

export function toolboxLayout(s: DrawingSettings): ToolboxLayout {
  const on = (id: ToolboxItemId) => isToolboxItemVisible(s, id);
  const tools = TOOLBOX_TOOLS.filter(on);
  const color = on("color");
  const edit = TOOLBOX_EDIT.filter(on);
  const actions = (color ? 1 : 0) + edit.length;
  return { grip: on("grip"), tools, color, edit, separator: actions > 0, buttons: tools.length + actions };
}
