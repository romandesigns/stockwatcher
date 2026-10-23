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

test("no card while neither view has rows (off and not yet known included); no quiet state", () => {
  expect(panel).toContain("<Collapse open={counts.active + counts.recent > 0} reducedMotion={reducedMotion} gap={gap}>");
  expect(panel).toContain("const counts = detectionViewCounts(state);");
  expect(panel).not.toContain("Nothing developing right now");
  expect(panel).not.toContain("Nothing resolved yet");
  expect(panel).not.toContain("Waiting for the detector");
});

test("the Active / Recent toggle only while both views have rows; it fades, never a layout animation", () => {
  expect(panel).toContain("const { recent: history, showToggle } = detectionView(pickedRecent, counts);");
  expect(panel).toMatch(/\{showToggle && \(\s*<Reanimated\.View entering=\{FadeIn/);
  expect(panel).toContain("ReduceMotion.Always");
});

test("the shared row model and honest stages", () => {
  expect(panel).toContain("detectionRowView(group, now, current, hhmm)");
  // Only a crossing and a price at the trigger earn an edge; green never marks a stage. Its one
  // use is the day's gain, as on the Ross row.
  expect(src).toContain("const EDGE: Partial<Record<DetectionRowView[\"stage\"], string>> = { crossing: UI.accent, near: UI.warning };");
  expect(src.match(/UI\.good/g)).toHaveLength(1);
  expect(src).toContain("const GAIN: Record<DetectionRowView[\"gainTone\"], string> = { good: UI.good, critical: UI.critical, muted: UI.textDim };");
});

test("the day's gain follows the price on the row, only when the server sent one", () => {
  expect(src).toContain("{r.gain ? <Text style={[styles.gain, { color: GAIN[r.gainTone] }]}>{r.gain}</Text> : null}");
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
