import { describe, expect, test } from "bun:test";
import { DRAWING_KINDS } from "./drawings.ts";
import {
  armedAfter,
  DEFAULT_DRAWING_SETTINGS,
  isDefaultDrawingSettings,
  isToolboxItemLocked,
  isToolboxItemVisible,
  parseDrawingSettings,
  setToolboxGroupVisible,
  setToolboxItemVisible,
  TOOLBOX_EDIT,
  TOOLBOX_ITEMS,
  TOOLBOX_ITEM_IDS,
  TOOLBOX_TOOLS,
  toolAllowed,
  toolboxLayout,
  type DrawingSettings,
} from "./drawingSettings.ts";
import { ICONS } from "./icons.ts";

// The Drawing tab's model (owner request 2026-10-18): which buttons the
// floating drawing toolbox shows, on the web and the phone alike.

const hide = (...ids: Parameters<typeof setToolboxItemVisible>[1][]): DrawingSettings => ids.reduce((s, id) => setToolboxItemVisible(s, id, false), DEFAULT_DRAWING_SETTINGS);

describe("the item list is the single source of truth", () => {
  test("every tool and action, once, tools first, each with a real icon and words", () => {
    expect(new Set(TOOLBOX_ITEM_IDS).size).toBe(TOOLBOX_ITEMS.length);
    expect(TOOLBOX_ITEMS.filter((i) => i.group === "tools").map((i) => i.id)).toEqual([...TOOLBOX_TOOLS]);
    expect(TOOLBOX_TOOLS).toEqual(["cursor", ...DRAWING_KINDS, "measure"]);
    expect(TOOLBOX_ITEMS.filter((i) => i.group === "actions").map((i) => i.id)).toEqual(["color", ...TOOLBOX_EDIT, "grip"]);
    const firstAction = TOOLBOX_ITEMS.findIndex((i) => i.group === "actions");
    expect(TOOLBOX_ITEMS.slice(firstAction).every((i) => i.group === "actions")).toBe(true);
    for (const i of TOOLBOX_ITEMS) {
      expect(ICONS[i.icon]).toBeDefined();
      expect(i.name.length).toBeGreaterThan(0);
      expect(i.description.length).toBeGreaterThan(0);
    }
  });

  test("only the cursor is locked, and it says why", () => {
    expect(TOOLBOX_ITEMS.filter((i) => i.locked).map((i) => i.id)).toEqual(["cursor"]);
    expect(isToolboxItemLocked("cursor")).toBe(true);
    expect(TOOLBOX_ITEMS[0]!.locked).toMatch(/leave a tool/);
  });
});

describe("defaults", () => {
  test("everything shows: today's toolbox", () => {
    expect(DEFAULT_DRAWING_SETTINGS).toEqual({ version: 1, hidden: [] });
    expect(isDefaultDrawingSettings(DEFAULT_DRAWING_SETTINGS)).toBe(true);
    for (const id of TOOLBOX_ITEM_IDS) expect(isToolboxItemVisible(DEFAULT_DRAWING_SETTINGS, id)).toBe(true);
    expect(toolboxLayout(DEFAULT_DRAWING_SETTINGS)).toEqual({ grip: true, tools: [...TOOLBOX_TOOLS], color: true, edit: [...TOOLBOX_EDIT], separator: true, buttons: TOOLBOX_TOOLS.length + 1 + TOOLBOX_EDIT.length });
  });
});

describe("show and hide", () => {
  test("hiding and showing round-trip, kept in toolbox order without repeats", () => {
    const s = hide("redo", "trend", "trend", "grip");
    expect(s.hidden).toEqual(["trend", "redo", "grip"]);
    expect(isToolboxItemVisible(s, "trend")).toBe(false);
    expect(setToolboxItemVisible(s, "trend", true).hidden).toEqual(["redo", "grip"]);
    expect(isDefaultDrawingSettings(s)).toBe(false);
  });

  test("the cursor cannot be hidden, by item or by group", () => {
    expect(hide("cursor")).toBe(DEFAULT_DRAWING_SETTINGS);
    const noTools = setToolboxGroupVisible(DEFAULT_DRAWING_SETTINGS, "tools", false);
    expect(isToolboxItemVisible(noTools, "cursor")).toBe(true);
    expect(toolboxLayout(noTools).tools).toEqual(["cursor"]);
    expect(setToolboxGroupVisible(noTools, "tools", true).hidden).toEqual([]);
  });

  test("a group switch leaves the other group alone", () => {
    const s = setToolboxGroupVisible(hide("trend"), "actions", false);
    expect(s.hidden).toEqual(["trend", "color", "delete", "undo", "redo", "rotate", "grip"]);
    expect(setToolboxGroupVisible(s, "actions", true).hidden).toEqual(["trend"]);
  });
});

describe("the armed tool", () => {
  test("hiding the armed tool disarms it; other tools and the cursor stay", () => {
    const s = hide("ray", "long");
    expect(armedAfter("ray", s)).toBeNull();
    expect(armedAfter("long", s)).toBeNull();
    expect(armedAfter("trend", s)).toBe("trend");
    expect(armedAfter(null, s)).toBeNull();
  });

  test("a hidden tool cannot be armed (the L / S shortcuts check this)", () => {
    const s = hide("short", "measure");
    expect(toolAllowed(s, "short")).toBe(false);
    expect(toolAllowed(s, "measure")).toBe(false);
    expect(toolAllowed(s, "long")).toBe(true);
    expect(toolAllowed(s, null)).toBe(true);
  });
});

describe("toolbox layout for fewer items", () => {
  test("hidden tools drop out in order; the cursor stays first", () => {
    const l = toolboxLayout(hide("trend", "vline", "measure"));
    expect(l.tools).toEqual(["cursor", "ray", "hline", "rect", "supply", "demand", "long", "short"]);
    expect(l.buttons).toBe(8 + 5);
  });

  test("the edit pill holds only what shows, and disappears when empty (no empty dimmed surface)", () => {
    expect(toolboxLayout(hide("undo", "redo")).edit).toEqual(["delete", "rotate"]);
    const l = toolboxLayout(hide("delete", "undo", "redo", "rotate"));
    expect(l.edit).toEqual([]);
    expect(l.color).toBe(true);
    expect(l.separator).toBe(true);
  });

  test("with no actions at all there is no separator: a cursor-and-tools strip (horizontal or vertical)", () => {
    const l = toolboxLayout(setToolboxGroupVisible(DEFAULT_DRAWING_SETTINGS, "actions", false));
    expect(l).toEqual({ grip: false, tools: [...TOOLBOX_TOOLS], color: false, edit: [], separator: false, buttons: TOOLBOX_TOOLS.length });
  });

  test("the smallest toolbox is the cursor alone", () => {
    const s = setToolboxGroupVisible(setToolboxGroupVisible(DEFAULT_DRAWING_SETTINGS, "tools", false), "actions", false);
    expect(toolboxLayout(s)).toEqual({ grip: false, tools: ["cursor"], color: false, edit: [], separator: false, buttons: 1 });
  });
});

describe("parse: validation and migration", () => {
  test("junk is the default", () => {
    for (const raw of [null, undefined, 3, "x", [], { hidden: "trend" }]) expect(parseDrawingSettings(raw)).toEqual(DEFAULT_DRAWING_SETTINGS);
  });

  test("v1 round-trips; unknown ids, repeats and the locked cursor are dropped", () => {
    const s = hide("ray", "color");
    expect(parseDrawingSettings(JSON.parse(JSON.stringify(s)))).toEqual(s);
    expect(parseDrawingSettings({ version: 1, hidden: ["fibonacci", "ray", "ray", 7, "cursor"] })).toEqual({ version: 1, hidden: ["ray"] });
  });

  test("a save without a version is read like v1", () => {
    expect(parseDrawingSettings({ hidden: ["undo"] })).toEqual({ version: 1, hidden: ["undo"] });
  });

  test("a record of booleans hides the false ones; missing keys show", () => {
    expect(parseDrawingSettings({ visible: { trend: false, ray: true, redo: false, cursor: false } })).toEqual({ version: 1, hidden: ["trend", "redo"] });
    expect(parseDrawingSettings({ items: { measure: false } })).toEqual({ version: 1, hidden: ["measure"] });
  });

  test("a `shown` list hides everything else it knows (except the cursor)", () => {
    const s = parseDrawingSettings({ shown: ["trend", "delete", "undo"] });
    expect(s.hidden).toEqual(TOOLBOX_ITEM_IDS.filter((id) => !["cursor", "trend", "delete", "undo"].includes(id)));
  });

  test("old names map to today's ids", () => {
    expect(parseDrawingSettings({ hidden: ["trash", "flip", "colour", "move", "select", "ruler", "range"] }).hidden).toEqual(["rect", "measure", "color", "delete", "rotate", "grip"]);
  });
});
