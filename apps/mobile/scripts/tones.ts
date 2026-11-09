// Renders the drawing alerts' three tones as small WAV files for the phone's notification
// channels (owner decision 2026-11-09: distinct tones for up-touch, down-touch and stop). An
// Android channel's sound is fixed when the channel is created and must be a file bundled in the
// app (the expo-notifications plugin's `sounds`, app.config.ts), so the tone IS the channel.
//
// The notes are app-state's ALERT_TONES, the same score the web plays with WebAudio, so a tone
// sounds the same on every screen. Run from apps/mobile after changing the score, then rebuild
// the app (a channel keeps its first sound: bump the channel ids in setup.ts if a tone changes on
// a phone that already has them):
//
//   bun scripts/tones.ts
//
// 16-bit mono PCM at 22.05 kHz, a triangle wave with an 8 ms attack and 40 ms release (the web's
// envelope), 13–27 KB each.

import { writeFileSync } from "node:fs";
import { ALERT_TONES } from "@stockwatcher/app-state";

export const RATE = 22_050;
const ATTACK = 0.008;
const RELEASE = 0.04;
/** Peak amplitude: loud enough over a notification, never clipping. */
const PEAK = 0.7;

const tri = (phase: number) => 1 - 4 * Math.abs(Math.round(phase - 0.25) - (phase - 0.25));

/** The tone's samples, -1..1. */
export function renderTone(notes: readonly { hz: number; ms: number }[]): Float32Array {
  const total = notes.reduce((n, x) => n + Math.round((x.ms / 1000) * RATE), 0);
  const out = new Float32Array(total + Math.round(0.02 * RATE));
  let at = 0;
  for (const n of notes) {
    const len = Math.round((n.ms / 1000) * RATE);
    if (n.hz > 0) {
      for (let i = 0; i < len; i++) {
        const t = i / RATE;
        const dur = len / RATE;
        const env = Math.min(1, t / ATTACK, (dur - t) / RELEASE);
        out[at + i] = PEAK * Math.max(0, env) * tri((n.hz * t) % 1);
      }
    }
    at += len;
  }
  return out;
}

/** A RIFF/WAVE file of 16-bit mono PCM. */
export function wav(samples: Float32Array, rate = RATE): Uint8Array {
  const data = samples.length * 2;
  const buf = new DataView(new ArrayBuffer(44 + data));
  const str = (o: number, s: string) => [...s].forEach((c, i) => buf.setUint8(o + i, c.charCodeAt(0)));
  str(0, "RIFF");
  buf.setUint32(4, 36 + data, true);
  str(8, "WAVE");
  str(12, "fmt ");
  buf.setUint32(16, 16, true);
  buf.setUint16(20, 1, true);
  buf.setUint16(22, 1, true);
  buf.setUint32(24, rate, true);
  buf.setUint32(28, rate * 2, true);
  buf.setUint16(32, 2, true);
  buf.setUint16(34, 16, true);
  str(36, "data");
  buf.setUint32(40, data, true);
  samples.forEach((v, i) => buf.setInt16(44 + i * 2, Math.round(Math.max(-1, Math.min(1, v)) * 32767), true));
  return new Uint8Array(buf.buffer);
}

if (import.meta.main) {
  for (const tone of ["up", "down", "stop"] as const) {
    const file = new URL(`../assets/sounds/drawing_${tone}.wav`, import.meta.url);
    const bytes = wav(renderTone(ALERT_TONES[tone]));
    writeFileSync(file, bytes);
    console.log(`${file.pathname}: ${bytes.length} bytes`);
  }
}
