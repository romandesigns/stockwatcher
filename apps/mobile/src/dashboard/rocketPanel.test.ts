// Rocket watch on the phone (owner, 2026-10-21). The rows' content is the shared view model's
// (packages/shared/src/rocket.test.ts); these pin the phone-side rules that a render would not
// show in a bun test without a native renderer.
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");

test("the Runners card sits under the Ross card and opens the chart modal on tap", () => {
  const screen = read("../screens/DashboardScreen.tsx");
  const ross = screen.indexOf("<RossPanel ");
  const rocket = screen.indexOf("<RocketPanel ");
  expect(ross).toBeGreaterThan(0);
  expect(rocket).toBeGreaterThan(ross);
  expect(screen.slice(rocket, screen.indexOf("/>", rocket))).toContain("onPick={open}");
});

test("rows: tap opens, tap-then-hold copies; no layout animation on a Pressable (Android stale taps)", () => {
  const src = read("RocketPanel.tsx");
  expect(src).toContain("useTickerPress(r.symbol");
  expect(src).not.toMatch(/LinearTransition|layout=\{/);
  // No borders (house rule): rows are separated by space.
  expect(src).not.toMatch(/border(Width|Color|Top|Bottom|Left|Right)/);
});

test("new flags ring locally only while this phone has no push (the server's push rings otherwise)", () => {
  const src = read("../notifications/rocket.ts");
  // Flags under the "new flags" switch, buying bursts under their own (2026-10-22).
  expect(src).toContain("ringRunnersLocally(isBurstRow(r) ? prefs.pushBurst : isRunningRow(r) ? prefs.pushRunning : prefs.pushNew, token)");
  expect(src).toContain('kind: burst ? "runners.burst" : isRunningRow(r) ? "runners.running" : "runners.new"');
  expect(read("RocketPanel.tsx")).toContain("notifyRocketFlags(fresh)");
  expect(read("../settings/SettingsHost.tsx")).toContain("rocketSection()");
});

test("momentum rows (2026-10-22): edge bar not border, crossfade by opacity honouring reduced motion, compact day line", () => {
  const src = read("RocketPanel.tsx");
  // The state edge is a filled bar and the wash a fill; both crossfade through opacity only.
  expect(src).toContain("styles.edge");
  const fade = src.slice(src.indexOf("function useCrossfade"), src.indexOf("const Row = memo"));
  expect(fade).toContain("useNativeDriver: true");
  expect(fade).toContain("reducedMotion");
  // The day line keeps the Ross row's compact box (the Fold's cover screen).
  const num = (name: string) => Number(src.match(new RegExp(`export const ${name} = (\\d+);`))?.[1]);
  expect(num("DAY_LINE_W")).toBeLessThanOrEqual(80);
  expect(num("DAY_LINE_H")).toBeLessThanOrEqual(28);
  // Rows are ordered by state through the shared view model; green only via the outcome tone map.
  expect(src).toContain("rocketSectionsView(panel, now)");
  expect(src).not.toMatch(/UI\.good[^,]/);
});

test("buying bursts (2026-10-22): their own section above the flags; accent only while live; loss tone mapped", () => {
  const src = read("RocketPanel.tsx");
  const panel = src.slice(src.indexOf("export function RocketPanel"), src.indexOf("function useCrossfade"));
  expect(panel.indexOf("bursts.rows.map(")).toBeGreaterThan(0);
  expect(panel.indexOf("bursts.rows.map(")).toBeLessThan(panel.indexOf("rows.map((r) => (\n                <Row"));
  expect(src).toContain("b.live ? UI.accent : UI.text");
  expect(src).toContain("loss: UI.critical");
  // No new scroller (the card sits in the dashboard's own scroll view).
  expect(src).not.toContain("ScrollView");
});
