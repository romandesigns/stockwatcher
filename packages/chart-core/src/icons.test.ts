import { describe, expect, test } from "bun:test";
import { circlePath, ICON_NAMES, ICON_SECONDARY_OPACITY, ICONS, iconPaint, type IconPath } from "./icons.ts";
import { INDICATOR_GLYPHS } from "./indicators.ts";
import { LAYER_IDS, LAYER_SWATCHES } from "./levels.ts";

// Duotone icons (owner request 2026-10-18): every icon in the product, from
// one definition, in exactly two tones.

const sets: [string, readonly IconPath[]][] = [
  ...ICON_NAMES.map((n) => [`icon ${n}`, ICONS[n].paths] as [string, readonly IconPath[]]),
  ...LAYER_IDS.map((id) => [`layer ${id}`, LAYER_SWATCHES[id].paths] as [string, readonly IconPath[]]),
  ...Object.entries(INDICATOR_GLYPHS).map(([k, p]) => [`indicator ${k}`, p] as [string, readonly IconPath[]]),
];

describe("every icon is duotone", () => {
  for (const [name, paths] of sets) {
    test(`${name} has a primary and a secondary tone, as roles, with drawable path data`, () => {
      expect(paths.some((p) => p.role === "primary")).toBe(true);
      expect(paths.some((p) => p.role === "secondary")).toBe(true);
      for (const p of paths) {
        // Roles, never colours: a hard-coded colour would split web and phone again.
        expect(Object.keys(p).every((k) => ["d", "role", "paint", "hue", "dash", "width"].includes(k))).toBe(true);
        expect(p.d).toMatch(/^M[-\d.]/);
        expect(p.d).toMatch(/^[MmLlHhVvCcSsQqTtAaZz\d\s.,-]+$/);
      }
    });
  }
});

test("viewBoxes: 16 × 16 icons, 24 × 16 layer swatches", () => {
  for (const n of ICON_NAMES) expect([ICONS[n].w, ICONS[n].h]).toEqual([16, 16]);
  for (const id of LAYER_IDS) expect([LAYER_SWATCHES[id].w, LAYER_SWATCHES[id].h]).toEqual([24, 16]);
});

test("iconPaint: primary at full strength in the current colour, secondary the same colour softened; a hue replaces the colour, not the role", () => {
  const hues = { good: "#0f0", critical: "#f00", warning: "#fa0" };
  expect(iconPaint({ d: "M0 0", role: "primary" }, "#abc", hues)).toEqual({ color: "#abc", opacity: 1 });
  expect(iconPaint({ d: "M0 0", role: "secondary" }, "#abc", hues)).toEqual({ color: "#abc", opacity: ICON_SECONDARY_OPACITY });
  expect(iconPaint({ d: "M0 0", role: "secondary", hue: "good" }, "#abc", hues)).toEqual({ color: "#0f0", opacity: ICON_SECONDARY_OPACITY });
  expect(ICON_SECONDARY_OPACITY).toBeGreaterThan(0.25);
  expect(ICON_SECONDARY_OPACITY).toBeLessThan(0.6);
});

test("circlePath draws a closed circle of the given radius", () => {
  expect(circlePath(8, 8, 2)).toBe("M6 8a2 2 0 1 0 4 0a2 2 0 1 0 -4 0z");
});
