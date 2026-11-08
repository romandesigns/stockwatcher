import { describe, expect, test } from "bun:test";
import { indicatorSwatches } from "./indicators.ts";
import { familyColor, familyGradient, hexToOklch, hueOf, oklchInGamut, oklchToHex, swatchFamily } from "./oklch.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";

const hex = (n: number) => `#${n.toString(16).padStart(6, "0")}`;

describe("OKLCH conversion", () => {
  test("known values (CSS Color 4 / Ottosson reference)", () => {
    const white = hexToOklch("#ffffff")!;
    expect(white.l).toBeCloseTo(1, 4);
    expect(white.c).toBeLessThan(1e-4);
    expect(hexToOklch("#000000")!.l).toBeCloseTo(0, 6);
    const red = hexToOklch("#ff0000")!;
    expect(red.l).toBeCloseTo(0.628, 3);
    expect(red.c).toBeCloseTo(0.2577, 3);
    expect(red.h).toBeCloseTo(29.23, 1);
    expect(hexToOklch("red")).toBeNull();
  });

  test("sRGB → OKLCH → sRGB round-trips exactly on a sweep of colours", () => {
    for (let n = 0; n < 0x1000000; n += 0x010203 * 7 + 13) {
      const h = hex(n);
      expect(oklchToHex(hexToOklch(h)!)).toBe(h);
    }
    for (const h of indicatorSwatches(DEFAULT_CHART_THEME)) expect(oklchToHex(hexToOklch(h)!)).toBe(h);
  });

  test("out of gamut: chroma comes down, lightness and hue stay", () => {
    const wanted = { l: 0.7, c: 0.35, h: 200 };
    expect(oklchInGamut(wanted)).toBe(false);
    const got = hexToOklch(oklchToHex(wanted))!;
    expect(got.l).toBeCloseTo(0.7, 2);
    expect(Math.abs(got.h - 200)).toBeLessThan(2);
    expect(got.c).toBeLessThan(0.35);
  });
});

describe("the hue picker's tonal family", () => {
  const swatches = indicatorSwatches(DEFAULT_CHART_THEME);
  const family = swatchFamily(swatches)!;

  test("the median L and C of the chromatic swatches, neutrals left out", () => {
    const tones = swatches.map((s) => hexToOklch(s)!).filter((t) => t.c >= 0.03);
    expect(tones.length).toBe(7);
    expect(family.l).toBeGreaterThan(0.62);
    expect(family.l).toBeLessThan(0.76);
    expect(family.c).toBeGreaterThan(0.13);
    expect(family.c).toBeLessThan(0.18);
    expect(swatchFamily(["#98a2ae", "#e4e8ed"])).toBeNull();
    // A median: one far-off swatch does not drag the family.
    const withOutlier = swatchFamily([...swatches, "#ffff00"])!;
    expect(withOutlier.l).toBeCloseTo(family.l, 1);
  });

  test("every hue the strip offers sits at the family's L and C (C lower only where sRGB runs out)", () => {
    for (let h = 0; h < 360; h += 1) {
      const out = hexToOklch(familyColor(family, h))!;
      // 8-bit rounding moves L by well under 0.01.
      expect(Math.abs(out.l - family.l)).toBeLessThan(0.01);
      if (oklchInGamut({ ...family, h })) {
        expect(Math.abs(out.c - family.c)).toBeLessThan(0.01);
        // Hue holds within rounding.
        const dh = Math.abs(((out.h - h + 540) % 360) - 180);
        expect(dh).toBeLessThan(2.5);
      } else {
        expect(out.c).toBeLessThanOrEqual(family.c + 0.005);
        expect(out.c).toBeGreaterThan(family.c * 0.6);
      }
    }
  });

  test("gradient stops span 0 to 360 and the thumb finds a picked hue again", () => {
    const stops = familyGradient(family);
    expect(stops[0]!.offset).toBe(0);
    expect(stops.at(-1)!.offset).toBe(1);
    expect(stops[0]!.color).toBe(stops.at(-1)!.color);
    expect(Math.round(hueOf(familyColor(family, 140))!)).toBeGreaterThanOrEqual(138);
    expect(Math.round(hueOf(familyColor(family, 140))!)).toBeLessThanOrEqual(142);
    expect(hueOf("#98a2ae")).toBeNull();
  });
});
