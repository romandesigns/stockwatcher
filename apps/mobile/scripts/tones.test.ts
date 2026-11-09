import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { ALERT_TONES } from "@stockwatcher/app-state";
import { RATE, renderTone, wav } from "./tones.ts";

test("the bundled tones are the score, rendered (rerun scripts/tones.ts after changing ALERT_TONES)", () => {
  for (const tone of ["up", "down", "stop"] as const) {
    const want = wav(renderTone(ALERT_TONES[tone]));
    const got = new Uint8Array(readFileSync(new URL(`../assets/sounds/drawing_${tone}.wav`, import.meta.url)));
    expect(got.length).toBe(want.length);
    expect(Buffer.from(got).equals(Buffer.from(want))).toBe(true);
  }
});

test("a valid 16-bit mono WAV, the notes' length, never clipping, the three tones distinct", () => {
  const s = renderTone(ALERT_TONES.stop);
  const ms = ALERT_TONES.stop.reduce((n, x) => n + x.ms, 0);
  expect(s.length).toBe(Math.round((ms / 1000) * RATE) + Math.round(0.02 * RATE));
  expect(Math.max(...s.map(Math.abs))).toBeLessThanOrEqual(0.7 + 1e-6);
  const w = wav(s);
  const head = new TextDecoder().decode(w.slice(0, 4)) + new TextDecoder().decode(w.slice(8, 12));
  expect(head).toBe("RIFFWAVE");
  expect(new DataView(w.buffer).getUint16(22, true)).toBe(1);
  expect(new DataView(w.buffer).getUint32(24, true)).toBe(RATE);
  const up = renderTone(ALERT_TONES.up);
  const down = renderTone(ALERT_TONES.down);
  expect(up.length).toBe(down.length);
  expect(up.some((v, i) => Math.abs(v - down[i]!) > 0.1)).toBe(true);
});
