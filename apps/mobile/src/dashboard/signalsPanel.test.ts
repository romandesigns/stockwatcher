// Signals on the phone (design canvas "Detection Panel", 2026-10-22). The rows' content is the
// shared view model's (packages/shared/src/detection.test.ts); these pin the phone-side rules
// that a render would not show in a bun test without a native renderer.
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");
const src = read("DetectionPanel.tsx");
const panel = src.slice(src.indexOf("export function DetectionPanel"), src.indexOf("function FilterStrip"));

test("the Signals card sits under the Ross card, above Runners, and opens the chart modal with the episode", () => {
  const screen = read("../screens/DashboardScreen.tsx");
  const ross = screen.indexOf("<RossPanel ");
  const signals = screen.indexOf("<DetectionPanel ");
  const rocket = screen.indexOf("<RocketPanel ");
  expect(signals).toBeGreaterThan(ross);
  expect(rocket).toBeGreaterThan(signals);
  expect(panel).toContain("onPick(group.selected.symbol, group.selected.episode_id, epoch)");
});

test("nothing while the detector is off; the shared row model and honest stages", () => {
  expect(panel).toContain("if (state.enabled !== true) return null;");
  expect(panel).toContain("detectionRowView(group, now, current, hhmm)");
  // Only a crossing and a price at the trigger earn an edge; never green anywhere on the card.
  expect(src).toContain("const EDGE: Partial<Record<DetectionRowView[\"stage\"], string>> = { crossing: UI.accent, near: UI.warning };");
  expect(src).not.toMatch(/UI\.good/);
});

test("rows: tap opens, tap-then-hold copies; crossfade, not layout animation; no borders; scroll views hide bars", () => {
  expect(src).toContain("useTickerPress(group.selected.symbol, onPick)");
  expect(src).not.toMatch(/LinearTransition|layout=\{/);
  expect(src).not.toMatch(/border(Width|Color|Top|Bottom|Left|Right)/);
  const fade = src.slice(src.indexOf("function useCrossfade"), src.indexOf("const SignalRow = memo"));
  expect(fade).toContain("useNativeDriver: true");
  expect(fade).toContain("reducedMotion");
  for (const m of src.matchAll(/<ScrollView[^>]*>/g)) {
    expect(m[0]).toContain("showsVerticalScrollIndicator={false}");
    expect(m[0]).toContain("showsHorizontalScrollIndicator={false}");
  }
});

test("reconnecting: rows dim and the count (last known) is withheld", () => {
  expect(panel).toContain("!state.connected && styles.dimmed");
  expect(panel).toContain("const attention = current ? detectionAttention(snapshot) : 0;");
});
