// Alert sounds in the browser. Drawing alerts' tones (owner decision 2026-11-09: a sound once per
// event, distinct for up-touch / down-touch / stop, a per-alert toggle, a global mute and volume,
// quiet hours respected) and, since 2026-11-16, the halt notices' sounds (app-state HALT_SOUNDS:
// the sound this device picked for Halted / Resumed, notify/runnersNotify.ts). Generated with
// WebAudio from app-state's scores: no audio files, nothing fetched. Which fires sound is decided
// elsewhere (app-state alertSoundFor, the notifier); this file only plays.
//
// Browsers start audio suspended until the person interacts with the page. The first pointer or
// key press anywhere unlocks it (installAlertSoundUnlock, from the Shell), and so does turning
// sounds on or pressing a Test button in the Alerts page's sound settings. A fire that arrives
// while still locked cannot sound; its toast says how to enable it.

import { ALERT_TONES, haltSoundNotes, type HaltSoundKey, type SoundNote } from "@stockwatcher/app-state";
import type { DrawingTone } from "@stockwatcher/chart-core";

type Ctx = AudioContext;
let ctx: Ctx | null = null;
const stateListeners = new Set<() => void>();

function context(): Ctx | null {
  if (ctx) return ctx;
  const C = (globalThis as { AudioContext?: typeof AudioContext; webkitAudioContext?: typeof AudioContext }).AudioContext ?? (globalThis as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
  if (!C) return null;
  try {
    ctx = new C();
  } catch {
    return null;
  }
  ctx.onstatechange = () => stateListeners.forEach((l) => l());
  return ctx;
}

/** Whether a tone can play now (the page has had its user gesture). */
export function alertSoundUnlocked(): boolean {
  return ctx?.state === "running";
}

/** Calls `cb` when audio locks or unlocks (the Settings prompt follows it). Returns the remover. */
export function onAlertSoundState(cb: () => void): () => void {
  stateListeners.add(cb);
  return () => stateListeners.delete(cb);
}

/**
 * Unlocks audio; call from a user gesture. Resolves true once audio can play: resuming takes a
 * moment after the first gesture, so a sound wanted right now (a preview) waits for it.
 */
export function unlockAlertSound(): Promise<boolean> {
  const c = context();
  if (!c) return Promise.resolve(false);
  if (c.state === "running") return Promise.resolve(true);
  return c.resume().then(
    () => c.state === "running",
    () => false,
  );
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
  return playNotes(ALERT_TONES[tone], volume);
}

/** Plays a halt sound at `volume` (0–1). False for "none", or when audio is locked or unavailable. */
export function playHaltSound(key: HaltSoundKey, volume: number): boolean {
  const notes = haltSoundNotes(key);
  return notes ? playNotes(notes, volume) : false;
}

/** A bell note's decay floor: exponential ramps cannot reach 0. */
const BELL_FLOOR = 0.001;
const BELL_ATTACK = 0.005;

/**
 * Plays a score (app-state SoundNote) at `volume`. apps/mobile/scripts/tones.ts renders the same
 * shapes to WAV (envelopes, linear glides, chords as equal partials), so a sound is the same on
 * every screen. False when audio is locked or unavailable.
 */
export function playNotes(notes: readonly SoundNote[], volume: number): boolean {
  const c = context();
  if (!c || c.state !== "running" || volume <= 0) return false;
  const out = c.createGain();
  // A perceptual curve: half the slider sounds about half as loud.
  out.gain.value = Math.min(1, volume) ** 2 * 0.5;
  out.connect(c.destination);
  let t = c.currentTime + 0.01;
  for (const note of notes) {
    const len = note.ms / 1000;
    const freqs = (Array.isArray(note.hz) ? note.hz : [note.hz]).filter((f) => f > 0);
    if (freqs.length) {
      const env = c.createGain();
      const level = (note.gain ?? 1) / freqs.length;
      env.gain.setValueAtTime(0, t);
      if (note.env === "bell") {
        env.gain.linearRampToValueAtTime(level, t + BELL_ATTACK);
        env.gain.exponentialRampToValueAtTime(level * BELL_FLOOR, t + len);
      } else {
        env.gain.linearRampToValueAtTime(level, t + ATTACK);
        env.gain.setValueAtTime(level, Math.max(t + ATTACK, t + len - RELEASE));
        env.gain.linearRampToValueAtTime(0, t + len);
      }
      env.connect(out);
      for (const f of freqs) {
        const osc = c.createOscillator();
        osc.type = note.wave ?? "triangle";
        osc.frequency.setValueAtTime(f, t);
        if (note.to && freqs.length === 1) osc.frequency.linearRampToValueAtTime(note.to, t + len);
        osc.connect(env);
        osc.start(t);
        osc.stop(t + len + 0.01);
      }
    }
    t += len;
  }
  setTimeout(() => out.disconnect(), (t - c.currentTime + 0.2) * 1000);
  return true;
}
