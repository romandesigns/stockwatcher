// Rocket watch on the phone (owner, 2026-10-21). The rows' content is the shared view model's
// (packages/shared/src/rocket.test.ts); these pin the phone-side rules that a render would not
// show in a bun test without a native renderer.
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");

test("the Rocket watch card sits under the Ross card and opens the chart modal on tap", () => {
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

test("the new-flag notification is off by default and per device", () => {
  const src = read("../notifications/rocket.ts");
  expect(src).toContain("let on = false;");
  expect(src).toContain("on = v === true;");
  expect(src).toContain("if (!on) return;");
  expect(read("../settings/SettingsHost.tsx")).toContain("rocketSection()");
});
