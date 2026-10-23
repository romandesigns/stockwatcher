import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const read = (p: string) => readFileSync(join(import.meta.dir, p), "utf8");

test("both Halt watchers subscribe to status, preserve hide-empty and announce last-known cards", () => {
  const web = read("../../../web/src/dashboard/HaltWatcher.tsx");
  const mobile = read("HaltWatcher.tsx");
  for (const src of [web, mobile]) {
    expect(src).toContain("usePressureStatus(channel)");
    expect(src).toContain("pressureFreshnessLine(status, hhmm)");
    expect(src).toContain("board.length > 0");
    expect(src).toContain("stale={!status.connected}");
    expect(src).toContain("stale ? `${freshness}. Last known: `");
  }
  expect(mobile).toContain('accessibilityLiveRegion="polite"');
  expect(web).toContain('role={freshness ? "status" : undefined}');
});

test("native stale gauges reuse grey and cancel sweeps without calling a stale card halted", () => {
  const src = read("PressureGauge.tsx");
  expect(src).toContain("reducedMotion || stale");
  expect(src).toContain("const neutral = halted || stale;");
  expect(src).toContain("gaugeTone(side, neutral)");
  expect(src).toContain('stale ? "last known risk" : gaugeCaption(side, halted)');
  expect(read("HaltWatcher.tsx")).toContain("stale ? UI.textFaint");
  expect(read("HaltWatcher.tsx")).toContain('key={stale ? "stale" : "live"}');
});
