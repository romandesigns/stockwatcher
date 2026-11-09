// Drawing alerts' tones in the browser (owner decision 2026-11-09: a sound once per event,
// distinct for up-touch / down-touch / stop, a per-alert toggle, a global mute and volume, quiet
// hours respected). Generated with WebAudio from app-state's ALERT_TONES score: no audio files,
// nothing fetched. Which fires sound is app-state's alertSoundFor; this file only plays.
//
// Browsers start audio suspended until the person interacts with the page. The first pointer or
// key press anywhere unlocks it (installAlertSoundUnlock, from the Shell), and so does turning
// sounds on or pressing a Test button in the Alerts page's sound settings. A fire that arrives
// while still locked cannot sound; its toast says how to enable it.

import { ALERT_TONES } from "@stockwatcher/app-state";
import type { DrawingTone } from "@stockwatcher/chart-core";

type Ctx = AudioContext;
let ctx: Ctx | null = null;

function context(): Ctx | null {
  if (ctx) return ctx;
  const C = (globalThis as { AudioContext?: typeof AudioContext; webkitAudioContext?: typeof AudioContext }).AudioContext ?? (globalThis as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
  if (!C) return null;
  try {
    ctx = new C();
  } catch {
    return null;
  }
  return ctx;
}

/** Whether a tone can play now (the page has had its user gesture). */
export function alertSoundUnlocked(): boolean {
  return ctx?.state === "running";
}

/** Unlocks audio; call from a user gesture. */
export function unlockAlertSound(): void {
  const c = context();
  if (c && c.state !== "running") void c.resume().catch(() => {});
}

/** Unlocks on the first press or key anywhere (once). Returns the remover. */
export function installAlertSoundUnlock(): () => void {
  const on = () => {
    unlockAlertSound();
    if (alertSoundUnlocked()) off();
  };
  const off = () => {
    window.removeEventListener("pointerdown", on, true);
    window.removeEventListener("keydown", on, true);
  };
  window.addEventListener("pointerdown", on, true);
  window.addEventListener("keydown", on, true);
  return off;
}

/** Envelope edges, s: soft enough not to click, short enough to stay crisp. */
const ATTACK = 0.008;
const RELEASE = 0.04;

/** Plays a tone at `volume` (0–1). False when audio is locked or unavailable. */
export function playAlertTone(tone: DrawingTone, volume: number): boolean {
  const c = context();
  if (!c || c.state !== "running" || volume <= 0) return false;
  const out = c.createGain();
  // A perceptual curve: half the slider sounds about half as loud.
  out.gain.value = Math.min(1, volume) ** 2 * 0.5;
  out.connect(c.destination);
  let t = c.currentTime + 0.01;
  for (const note of ALERT_TONES[tone]) {
    const len = note.ms / 1000;
    if (note.hz > 0) {
      const osc = c.createOscillator();
      const env = c.createGain();
      osc.type = "triangle";
      osc.frequency.value = note.hz;
      env.gain.setValueAtTime(0, t);
      env.gain.linearRampToValueAtTime(1, t + ATTACK);
      env.gain.setValueAtTime(1, Math.max(t + ATTACK, t + len - RELEASE));
      env.gain.linearRampToValueAtTime(0, t + len);
      osc.connect(env).connect(out);
      osc.start(t);
      osc.stop(t + len + 0.01);
    }
    t += len;
  }
  setTimeout(() => out.disconnect(), (t - c.currentTime + 0.2) * 1000);
  return true;
}
