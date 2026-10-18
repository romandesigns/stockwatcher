import { expect, test } from "bun:test";
import { ICONS } from "./icons.ts";
import {
  resolveSettingsSection,
  SETTINGS_NAV_ROOT,
  SETTINGS_RAIL_FRACTION,
  SETTINGS_RAIL_LABEL_MIN,
  SETTINGS_SECTIONS,
  SETTINGS_SINGLE_COLUMN_BELOW,
  settingsBack,
  settingsLayout,
  settingsNavReduce,
  settingsRailMode,
  type SettingsNav,
} from "./settingsDialog.ts";

// The Settings dialog's shared logic (owner request 2026-10-18: a 15 / 85 two-column dialog;
// the same day's follow-up: single column on phones).

test("sections: unique ids, a label, a real icon, Chart first then Drawing", () => {
  expect(SETTINGS_SECTIONS.map((s) => s.id)).toEqual(["chart", "drawing"]);
  expect(new Set(SETTINGS_SECTIONS.map((s) => s.id)).size).toBe(SETTINGS_SECTIONS.length);
  for (const s of SETTINGS_SECTIONS) {
    expect(s.label.length).toBeGreaterThan(0);
    expect(s.description.length).toBeGreaterThan(0); // the single-column root row shows it
    expect(ICONS[s.icon]).toBeDefined();
  }
});

test("layout: the phone app is always single column; the web only below 640 px", () => {
  for (const w of [320, 390, 768, 1024, 1440]) expect(settingsLayout(w, "phone")).toBe("single-column");
  expect(settingsLayout(390, "web")).toBe("single-column"); // a phone
  expect(settingsLayout(SETTINGS_SINGLE_COLUMN_BELOW - 1, "web")).toBe("single-column");
  expect(settingsLayout(SETTINGS_SINGLE_COLUMN_BELOW, "web")).toBe("two-column");
  expect(settingsLayout(768, "web")).toBe("two-column"); // a tablet keeps the rail (icons)
  expect(settingsLayout(1440, "web")).toBe("two-column"); // a desktop: unchanged
});

test("the two-column rail keeps 15 %: labels on a desktop, icons once 15 % is too narrow for a label", () => {
  expect(SETTINGS_RAIL_FRACTION).toBe(0.15);
  expect(settingsRailMode(880)).toBe("labels"); // the desktop dialog: 132 px rail
  expect(settingsRailMode(SETTINGS_RAIL_LABEL_MIN / SETTINGS_RAIL_FRACTION)).toBe("labels");
  expect(settingsRailMode(700)).toBe("icons"); // 105 px: too narrow for "Drawing" beside its icon
  // The narrowest two-column dialog (92 % of a 640 px window) still has a rail over a 44 px target.
  expect(SETTINGS_SINGLE_COLUMN_BELOW * 0.92 * SETTINGS_RAIL_FRACTION).toBeGreaterThanOrEqual(44);
  expect(settingsRailMode(SETTINGS_SINGLE_COLUMN_BELOW * 0.92)).toBe("icons");
});

test("a stored tab resolves to one that exists, else the first", () => {
  const ids = ["drawing", "chart"] as const;
  expect(resolveSettingsSection("chart", ids)).toBe("chart");
  expect(resolveSettingsSection("gone", ids)).toBe("drawing");
  expect(resolveSettingsSection(42, ids)).toBe("drawing");
  expect(resolveSettingsSection("x", [])).toBeNull();
  // The real list: a save from before Chart existed still opens Drawing; a first open lands on Chart.
  const real = SETTINGS_SECTIONS.map((s) => s.id);
  expect(resolveSettingsSection("drawing", real)).toBe("drawing");
  expect(resolveSettingsSection(null, real)).toBe("chart");
});

test("drill-down: push opens a section, pop returns to the root and names the row to refocus", () => {
  let nav: SettingsNav = SETTINGS_NAV_ROOT;
  expect(nav).toEqual({ section: null, returnTo: null });
  nav = settingsNavReduce(nav, { type: "push", id: "drawing" });
  expect(nav).toEqual({ section: "drawing", returnTo: null });
  nav = settingsNavReduce(nav, { type: "pop" });
  expect(nav).toEqual({ section: null, returnTo: "drawing" }); // focus goes back to the Drawing row
  // Pushing again clears the stale return target.
  nav = settingsNavReduce(nav, { type: "push", id: "chart" });
  expect(nav).toEqual({ section: "chart", returnTo: null });
});

test("drill-down: pop on the root is a no-op (same object, so React skips the render)", () => {
  const root = settingsNavReduce(SETTINGS_NAV_ROOT, { type: "pop" });
  expect(root).toBe(SETTINGS_NAV_ROOT);
  const after = { section: null, returnTo: "chart" };
  expect(settingsNavReduce(after, { type: "pop" })).toBe(after);
});

test("drill-down: reset (every open) lands on the root, wherever it was left", () => {
  expect(settingsNavReduce({ section: "drawing", returnTo: null }, { type: "reset" })).toEqual(SETTINGS_NAV_ROOT);
  expect(settingsNavReduce({ section: null, returnTo: "chart" }, { type: "reset" })).toEqual(SETTINGS_NAV_ROOT);
});

test("back: pops inside a section, closes on the root", () => {
  expect(settingsBack(SETTINGS_NAV_ROOT)).toBe("close");
  expect(settingsBack({ section: null, returnTo: "chart" })).toBe("close");
  expect(settingsBack({ section: "chart", returnTo: null })).toBe("pop");
});
