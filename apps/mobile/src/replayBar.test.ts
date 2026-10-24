// The phone's replay bar: chart-native's ReplayBar cannot load under bun (React Native), so its
// layout choice is checked through the shared rules it calls and its touch sizes through its
// stylesheet source. The two-tier bar ("B", owner-approved 2026-10-24) took about half the cover
// screen ("not practical", owner, 2026-10-24); it now rests as one compact row, the rest in a sheet.
import { expect, test } from "bun:test";
import { replayBarLayout, replayDetailsInline, replayScrubberInline } from "@stockwatcher/chart-core";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const bar = readFileSync(join(import.meta.dir, "../../../packages/chart-native/src/ReplayBar.tsx"), "utf8");
const screen = readFileSync(join(import.meta.dir, "screens/ChartScreen.tsx"), "utf8");

test("cover screen (< 600 dp): the thin scrubber over the row; unfolded (≥ 600 dp): inside it; a tablet stays unfolded", () => {
  expect(bar).toContain('replayBarLayout(width) === "cover" ? "cover" : "fold"');
  expect(bar).toContain("replayScrubberInline(layout)");
  // Galaxy Z Fold: cover ~ 344–390 dp wide, unfolded ~ 690–740 dp.
  expect(replayBarLayout(344)).toBe("cover");
  expect(replayBarLayout(390)).toBe("cover");
  expect(replayBarLayout(720)).toBe("fold");
  expect(replayScrubberInline("cover")).toBe(false);
  expect(replayScrubberInline("fold")).toBe(true);
  // Day and ticker join the row only where the inline scrubber keeps its room.
  expect(replayDetailsInline(720)).toBe(false);
  // The chart tab hands the bar its real width (the window less the screen's padding), so
  // folding and unfolding the phone switches the artboard.
  expect(screen).toContain("width={windowWidth - 2 * SCREEN_PAD}");
  expect(screen).toContain("useWindowDimensions()");
});

test("resting height: 28 + 44 + 8 dp on the cover screen, 44 + 2 × 10 dp unfolded (under 10 % of either screen)", () => {
  expect(bar).toContain("const microH = inline ? 44 : 28;");
  expect(bar).toMatch(/cardCover: \{ paddingHorizontal: 8, paddingBottom: 8,/);
  expect(bar).toMatch(/cardInline: \{ paddingHorizontal: 10, paddingVertical: 10,/);
  // The Fold's screens are ~ 880 dp (cover) and ~ 820 dp (unfolded) tall.
  expect((28 + 44 + 8) / 880).toBeLessThan(0.1);
  expect((44 + 20) / 820).toBeLessThan(0.08);
});

test("touch targets: every control of the row is 44 dp; chips in the sheet padded to 44 by hitSlop", () => {
  expect(bar).toMatch(/play: \{ width: 44, height: 44,/);
  expect(bar).toMatch(/iconButton: \{ width: 44, height: 44,/);
  expect(bar).toMatch(/speedChip: \{ minWidth: 44, height: 44,/);
  expect(bar).toMatch(/control: \{ height: 44,/);
  expect(bar).toContain("hitSlop={(44 - height) / 2}");
});

test("the row keeps Prev/Next alert and a cycling speed chip; more opens the app's sheet, remembered on the phone", () => {
  expect(bar).toContain('icon="alertPrev"');
  expect(bar).toContain('icon="alertNext"');
  expect(bar).toContain("nextReplaySpeed(speed)");
  expect(bar).toContain('a11y="More replay controls"');
  expect(bar).toContain('<Sheet open={expanded} onClose={() => onExpandedChange(false)} title="Replay" reducedMotion={p.reducedMotion}>');
  expect(screen).toContain("getPreference<boolean>(REPLAY_BAR_EXPANDED_KEY)");
  expect(screen).toContain("setPreference(REPLAY_BAR_EXPANDED_KEY, v)");
});

test("the scrubbers are adjustable for screen readers; the sheet's alert dots are named buttons", () => {
  expect(bar).toContain('accessibilityRole: "adjustable"');
  expect(bar).toContain("accessibilityLabel={`${d.label}, jump there`}");
  expect(bar).toContain('accessibilityRole="radiogroup" accessibilityLabel="Replay speed"');
  // The thin scrubber's dots are drawn; a release near one snaps onto it.
  expect(bar).toContain("scrubberReleaseTarget(dots, width, drag.x, drag.t)");
});
