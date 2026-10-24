// The phone's replay bar ("B · Two-tier scrubber", owner-approved 2026-10-24): chart-native's
// ReplayBar cannot load under bun (React Native), so its layout choice is checked through the
// shared rule it calls and its touch sizes through its stylesheet source.
import { expect, test } from "bun:test";
import { replayBarLayout } from "@stockwatcher/chart-core";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const bar = readFileSync(join(import.meta.dir, "../../../packages/chart-native/src/ReplayBar.tsx"), "utf8");
const screen = readFileSync(join(import.meta.dir, "screens/ChartScreen.tsx"), "utf8");

test("cover screen (< 600 dp) stacks, unfolded (≥ 600 dp) uses the two-row artboard; a tablet stays unfolded", () => {
  expect(bar).toContain('replayBarLayout(width) === "cover" ? "cover" : "fold"');
  // Galaxy Z Fold: cover ~ 360–390 dp wide, unfolded ~ 700–740 dp.
  expect(replayBarLayout(360)).toBe("cover");
  expect(replayBarLayout(390)).toBe("cover");
  expect(replayBarLayout(720)).toBe("fold");
  // The chart tab hands the bar its real width (the window less the screen's padding), so
  // folding and unfolding the phone switches the artboard.
  expect(screen).toContain("width={windowWidth - 2 * SCREEN_PAD}");
  expect(screen).toContain("useWindowDimensions()");
});

test("touch targets: 44 dp controls, a 52/48 dp play button, chips padded to 44 by hitSlop", () => {
  expect(bar).toMatch(/control: \{ height: 44,/);
  expect(bar).toMatch(/playCover: \{ width: 52, height: 52/);
  expect(bar).toMatch(/playFold: \{ width: 48, height: 48/);
  expect(bar).toContain("hitSlop={(44 - height) / 2}");
  expect(bar).toMatch(/speedCover: \{ flex: 1, height: 38 \}/);
});

test("the scrubber is adjustable for screen readers and every alert dot is a named button", () => {
  expect(bar).toContain('accessibilityRole="adjustable"');
  expect(bar).toContain("accessibilityLabel={`${d.label}, jump there`}");
  expect(bar).toContain('accessibilityRole="radiogroup" accessibilityLabel="Replay speed"');
});
