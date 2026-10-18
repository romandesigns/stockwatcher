import { expect, test } from "bun:test";
import { ICONS } from "./icons.ts";
import { resolveSettingsSection, SETTINGS_RAIL_FRACTION, SETTINGS_RAIL_LABEL_MIN, SETTINGS_RAIL_MIN, SETTINGS_SECTIONS, settingsRailMode } from "./settingsDialog.ts";

// The Settings dialog's shared logic (owner request 2026-10-18: a 15 / 85 two-column dialog).

test("sections: unique ids, a label, a real icon, Drawing first", () => {
  expect(SETTINGS_SECTIONS[0]!.id).toBe("drawing");
  expect(new Set(SETTINGS_SECTIONS.map((s) => s.id)).size).toBe(SETTINGS_SECTIONS.length);
  for (const s of SETTINGS_SECTIONS) {
    expect(s.label.length).toBeGreaterThan(0);
    expect(ICONS[s.icon]).toBeDefined();
  }
});

test("the rail keeps 15 % and adapts its content: labels on a desktop, icons on a phone, a top row only below a 44 px target", () => {
  expect(SETTINGS_RAIL_FRACTION).toBe(0.15);
  expect(settingsRailMode(880)).toBe("labels"); // the desktop dialog: 132 px rail
  expect(settingsRailMode(SETTINGS_RAIL_LABEL_MIN / SETTINGS_RAIL_FRACTION)).toBe("labels");
  expect(settingsRailMode(700)).toBe("icons"); // 105 px: too narrow for "Drawing" beside its icon
  expect(settingsRailMode(390)).toBe("icons"); // a phone: 58.5 pt, still a 44 pt target
  expect(settingsRailMode(SETTINGS_RAIL_MIN / SETTINGS_RAIL_FRACTION)).toBe("icons");
  expect(settingsRailMode(280)).toBe("top"); // 42 px: not a tappable target
});

test("a stored tab resolves to one that exists, else the first", () => {
  const ids = ["drawing", "chart"] as const;
  expect(resolveSettingsSection("chart", ids)).toBe("chart");
  expect(resolveSettingsSection("gone", ids)).toBe("drawing");
  expect(resolveSettingsSection(42, ids)).toBe("drawing");
  expect(resolveSettingsSection("x", [])).toBeNull();
});
