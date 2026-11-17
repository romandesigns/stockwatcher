// The halt notices' built-in sound library (owner-approved plan "halt sounds", release 1,
// 2026-11-16): about eight short, distinct sounds and "None", the same keys as the server's
// crates/server/src/alerts/halts.rs `SOUNDS` (the push's `data.sound`). Like the drawing alerts'
// ALERT_TONES, each sound is a score, not a recording: the web and the desktop play it with
// WebAudio (apps/web src/alerts/sound.ts) and apps/mobile/scripts/tones.ts renders the same notes
// to WAV for the phone's notification channels (release 2). Synthesized from these numbers alone,
// so they are original and licence-clean, and nothing is fetched at runtime.
//
// The choice of sound is per device (which machine makes which noise), never synced.

/** One note of a sound score. The drawing tones' `{hz, ms}` notes are the plain case. */
export interface SoundNote {
  /** Frequency in Hz; 0 is a rest. Several = a chord, the partials sounding together. */
  hz: number | readonly number[];
  ms: number;
  /** Glides to this frequency by the end of the note (a sweep); single notes only. */
  to?: number;
  /** Oscillator shape; triangle by default (the drawing tones' sound). */
  wave?: "triangle" | "sine";
  /** "flat" (8 ms attack, 40 ms release; the default) or "bell" (5 ms attack, then an exponential decay to silence by the note's end). */
  env?: "flat" | "bell";
  /** Level 0–1 (default 1). */
  gain?: number;
}

/** The library's keys, "none" last (the server's `SOUNDS`; the two must match). */
export const HALT_SOUND_KEYS = ["siren", "klaxon", "triple", "alarm", "chime", "ding", "rise", "blip", "none"] as const;
export type HaltSoundKey = (typeof HALT_SOUND_KEYS)[number];

/** The two events a sound is chosen for. */
export type HaltEvent = "halted" | "resumed";

/** Defaults (the server's `DEFAULT_HALTED_SOUND` / `DEFAULT_RESUMED_SOUND`): urgent for a halt, lighter for a resume. */
export const DEFAULT_HALT_SOUNDS: Readonly<Record<HaltEvent, HaltSoundKey>> = Object.freeze({ halted: "siren", resumed: "chime" });

/** Each sound's name and a few words on its character, in the picker's order. */
export const HALT_SOUND_LABELS: Readonly<Record<HaltSoundKey, { name: string; hint: string }>> = Object.freeze({
  siren: { name: "Siren", hint: "Urgent two-tone sweep" },
  klaxon: { name: "Klaxon", hint: "Low pulsing horn" },
  triple: { name: "Triple beep", hint: "Three sharp beeps" },
  alarm: { name: "Alarm", hint: "Falling repeated alert" },
  chime: { name: "Chime", hint: "Two soft bell notes" },
  ding: { name: "Ding", hint: "One clear bell" },
  rise: { name: "Rise", hint: "Upward sweep" },
  blip: { name: "Blip", hint: "Quiet double blip" },
  none: { name: "None", hint: "Silent" },
});

const S = "sine" as const;
const BELL = "bell" as const;

/**
 * The scores. Each is under a second, so a phone WAV at 22.05 kHz stays under ~45 KB. Urgent
 * sounds (siren, klaxon, triple, alarm) repeat and sweep; light ones (chime, ding, rise, blip)
 * are short, soft and decay.
 */
export const HALT_SOUNDS: Readonly<Record<Exclude<HaltSoundKey, "none">, readonly SoundNote[]>> = Object.freeze({
  siren: [
    { hz: 660, to: 1100, ms: 170 },
    { hz: 1100, to: 660, ms: 170 },
    { hz: 660, to: 1100, ms: 170 },
    { hz: 1100, to: 660, ms: 170 },
  ],
  klaxon: [
    { hz: [392, 494], ms: 150 },
    { hz: 0, ms: 50 },
    { hz: [392, 494], ms: 150 },
    { hz: 0, ms: 50 },
    { hz: [392, 494], ms: 220 },
  ],
  triple: [
    { hz: 1320, ms: 90, wave: S },
    { hz: 0, ms: 60 },
    { hz: 1320, ms: 90, wave: S },
    { hz: 0, ms: 60 },
    { hz: 1320, ms: 90, wave: S },
  ],
  alarm: [
    { hz: 1400, to: 900, ms: 120 },
    { hz: 0, ms: 40 },
    { hz: 1400, to: 900, ms: 120 },
    { hz: 0, ms: 40 },
    { hz: 1400, to: 900, ms: 120 },
  ],
  chime: [
    { hz: 1047, ms: 260, wave: S, env: BELL },
    { hz: 1319, ms: 520, wave: S, env: BELL },
  ],
  ding: [{ hz: [1568, 2349], ms: 700, wave: S, env: BELL, gain: 0.9 }],
  rise: [
    { hz: 440, to: 880, ms: 260, wave: S, gain: 0.8 },
    { hz: 880, ms: 160, wave: S, env: BELL, gain: 0.8 },
  ],
  blip: [
    { hz: 880, ms: 70, wave: S, gain: 0.7 },
    { hz: 0, ms: 50 },
    { hz: 1175, ms: 90, wave: S, gain: 0.7 },
  ],
});

/** A stored or wire value as a sound key, else `fallback`. */
export function haltSoundKey(v: unknown, fallback: HaltSoundKey): HaltSoundKey {
  return typeof v === "string" && (HALT_SOUND_KEYS as readonly string[]).includes(v) ? (v as HaltSoundKey) : fallback;
}

/** A stored value as both events' sounds (anything unreadable takes its default). */
export function haltSounds(v: unknown): Record<HaltEvent, HaltSoundKey> {
  const o = v && typeof v === "object" ? (v as Record<string, unknown>) : {};
  return { halted: haltSoundKey(o.halted, DEFAULT_HALT_SOUNDS.halted), resumed: haltSoundKey(o.resumed, DEFAULT_HALT_SOUNDS.resumed) };
}

/** A sound's notes, or null for "none". */
export function haltSoundNotes(key: HaltSoundKey): readonly SoundNote[] | null {
  return key === "none" ? null : HALT_SOUNDS[key];
}

/** A score's length in ms. */
export function soundMs(notes: readonly SoundNote[]): number {
  return notes.reduce((t, n) => t + n.ms, 0);
}
