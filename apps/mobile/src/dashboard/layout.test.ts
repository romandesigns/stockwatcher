import { describe, expect, test } from "bun:test";

import { maxChartsFor } from "./layout";

describe("maxChartsFor", () => {
  test("a phone-size screen shows one chart, in either orientation", () => {
    expect(maxChartsFor(412, 915)).toBe(1); // Fold cover screen, portrait
    expect(maxChartsFor(915, 412)).toBe(1); // ...and landscape
  });
  test("a tablet-size screen (sw600dp) shows two, in either orientation", () => {
    expect(maxChartsFor(884, 1104)).toBe(2); // Fold unfolded
    expect(maxChartsFor(1104, 884)).toBe(2);
    expect(maxChartsFor(600, 960)).toBe(2);
    expect(maxChartsFor(599, 960)).toBe(1);
  });
});
