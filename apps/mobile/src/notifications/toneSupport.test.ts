import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { DRAWING_TONES_NATIVE_VERSION, nativeAtLeast, tonesSupportedOn } from "./toneSupport";

const read = (p: string) => readFileSync(new URL(p, import.meta.url), "utf8");

test("the native version gate: only a binary at or above the tones' version, Android only", () => {
  expect(nativeAtLeast("0.2.0", "0.2.0")).toBe(true);
  expect(nativeAtLeast("0.10.0", "0.2.0")).toBe(true);
  expect(nativeAtLeast("1.0", "0.2.0")).toBe(true);
  expect(nativeAtLeast("0.1.0", "0.2.0")).toBe(false);
  expect(nativeAtLeast("0.1.99", "0.2.0")).toBe(false);
  expect(nativeAtLeast(null, "0.2.0")).toBe(false);
  expect(nativeAtLeast("dev", "0.2.0")).toBe(false);
  expect(tonesSupportedOn("android", "0.2.0")).toBe(true);
  expect(tonesSupportedOn("android", "0.1.0")).toBe(false);
  expect(tonesSupportedOn("ios", "0.2.0")).toBe(false);
});

test("the binary that bundles the sounds carries the gate's version, and the channel ids match the server's", () => {
  const config = read("../../app.config.ts");
  const version = /\n\s*version: "([\d.]+)"/.exec(config)?.[1];
  expect(nativeAtLeast(version, DRAWING_TONES_NATIVE_VERSION)).toBe(true);
  for (const t of ["up", "down", "stop"]) expect(config).toContain(`./assets/sounds/drawing_${t}.wav`);
  const server = read("../../../../crates/server/src/alerts/mod.rs");
  const setup = read("./setup.ts");
  for (const id of ["drawing-up-v1", "drawing-down-v1", "drawing-stop-v1"]) {
    expect(server).toContain(`"${id}"`);
    expect(setup).toContain(`"${id}"`);
  }
  // Nothing creates a drawing channel or plays a tone without the gate.
  expect(setup).toMatch(/ensureDrawingChannels\(\): Promise<void> \{\n\s*if \(!drawingTonesSupported\(\)\) return;/);
  expect(setup).toMatch(/const c = DRAWING_CHANNELS\[tone\];\n\s*if \(!drawingTonesSupported\(\)\) return;/);
});
