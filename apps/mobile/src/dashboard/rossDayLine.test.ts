// The Ross row's day line (owner, 2026-10-21). The repo has no React Native renderer in its tests,
// so this checks the source for the rules a render would break silently: the chart stays a
// compact box beside the text, it animates through opacity only, and the pressable row never
// gets a layout transition (Android stale taps: house rule).
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { rossDemoPanel, rossLinePaths, rossRowView } from "@stockwatcher/shared";

const src = readFileSync(join(import.meta.dir, "RossPanel.tsx"), "utf8");
const num = (name: string) => Number(src.match(new RegExp(`export const ${name} = (\\d+);`))?.[1]);

test("the chart is a compact box that fits beside the row on the Fold's cover screen", () => {
  expect(num("DAY_LINE_W")).toBeGreaterThan(0);
  expect(num("DAY_LINE_W")).toBeLessThanOrEqual(80);
  expect(num("DAY_LINE_H")).toBeLessThanOrEqual(28);
  expect(src).toContain("<DayLine line={r.line} reducedMotion={reducedMotion} />");
});

test("the row container gets no layout transition and the chart fades by opacity, honouring reduced motion", () => {
  const row = src.slice(src.indexOf("<Pressable"), src.indexOf(">", src.indexOf("<Pressable")));
  expect(row).not.toContain("layout=");
  expect(src).not.toContain("LinearTransition");
  const day = src.slice(src.indexOf("export function DayLine"), src.indexOf("function Meter"));
  expect(day).toContain("useNativeDriver: true");
  expect(day).toContain("if (reducedMotion)");
});

test("every demo row draws a day line inside the box", () => {
  const panel = rossDemoPanel(1_000_000_000_000);
  for (const r of panel.rows) {
    const line = rossRowView(r, panel.cutoff, () => "").line;
    expect(line?.kind).toBe("day");
    const p = rossLinePaths(line!, num("DAY_LINE_W"), num("DAY_LINE_H"));
    const ys = p.line.slice(1).split(" L").map((pt) => Number(pt.split(" ")[1]));
    expect(Math.min(...ys)).toBeGreaterThanOrEqual(0);
    expect(Math.max(...ys)).toBeLessThanOrEqual(num("DAY_LINE_H"));
  }
});
