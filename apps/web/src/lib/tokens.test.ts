import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { ICON_SECONDARY_OPACITY } from "@stockwatcher/chart-core";

// CSS cannot import a TypeScript constant, so the duotone secondary tone is
// written twice: here is where the two are held equal (the phone reads the
// constant directly).
const css = readFileSync(new URL("../index.css", import.meta.url), "utf8");

test("--icon-secondary softens currentColor by ICON_SECONDARY_OPACITY, --icon-primary is currentColor", () => {
  expect(css).toContain("--icon-primary: currentColor;");
  const m = /--icon-secondary:\s*color-mix\(in srgb, currentColor (\d+(?:\.\d+)?)%, transparent\);/.exec(css);
  expect(m).not.toBeNull();
  expect(Number(m![1]) / 100).toBe(ICON_SECONDARY_OPACITY);
});
