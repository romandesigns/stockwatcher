import { describe, expect, test } from "bun:test";
import { normalizeRows, pointOfControl, rebinProfile, valueArea } from "./profile.ts";

const rows = [
  { p: 3.18, v: 100 },
  { p: 3.19, v: 300 },
  { p: 3.2, v: 900 },
  { p: 3.21, v: 400 },
  { p: 3.23, v: 200 },
  { p: 3.24, v: 100 },
];

describe("profile (engine-identical rules, crates/chart-engine/src/profile.rs)", () => {
  test("the engine's own test vectors", () => {
    const r = [
      { p: 1.0, v: 10 },
      { p: 1.01, v: 30 },
      { p: 1.02, v: 40 },
      { p: 1.03, v: 15 },
      { p: 1.04, v: 5 },
    ];
    expect(pointOfControl(r)).toBe(1.02);
    // total 100, target 70: 40 → +30 (below, 30 > 15) = 70.
    expect(valueArea(r, 0.01)).toEqual({ low: 1.01, high: 1.02, v: 70 });
    expect(pointOfControl([{ p: 0.01, v: 5 }, { p: 0.02, v: 5 }])).toBe(0.01);
    expect(valueArea([], 0.01)).toBeNull();
    expect(valueArea([{ p: 0.05, v: 9 }], 0.01)).toEqual({ low: 0.05, high: 0.05, v: 9 });
  });

  test("value area skips empty bins and rounds the 70 % target up", () => {
    // total 2000, target 1400: 900 → +400 (3.21) → +300 (3.19) = 1600.
    expect(valueArea(rows, 0.01)).toEqual({ low: 3.19, high: 3.21, v: 1600 });
    // 3.21 → next non-empty above is 3.23 (3.22 is empty and skipped).
    const r = [{ p: 3.2, v: 10 }, { p: 3.21, v: 1 }, { p: 3.23, v: 5 }, { p: 3.19, v: 1 }];
    expect(valueArea(r, 0.01)).toEqual({ low: 3.2, high: 3.23, v: 16 });
    // target = ceil(3 × 0.7) = 3, not 2.1
    expect(valueArea([{ p: 1, v: 1 }, { p: 1.01, v: 1 }, { p: 1.02, v: 1 }], 0.01)!.v).toBe(3);
  });

  test("normalizeRows merges float-noise duplicates and drops empty bins", () => {
    expect(normalizeRows([{ p: 3.2, v: 1 }, { p: 3.1999999999, v: 2 }, { p: 3.1, v: 0 }], 0.01)).toEqual([{ p: 3.2, v: 3 }]);
  });

  test("rebin only to exact multiples, summing rows into coarser bins", () => {
    expect(rebinProfile(rows, 0.01, 0.05)).toEqual([
      { p: 3.15, v: 400 },
      { p: 3.2, v: 1600 },
    ]);
    expect(rebinProfile(rows, 0.01, 0.025)).toBeNull();
    expect(rebinProfile(rows, 0.01, 0.005)).toBeNull();
    expect(rebinProfile([{ p: 3.1999999999, v: 5 }], 0.01, 0.05)).toEqual([{ p: 3.2, v: 5 }]);
  });
});
