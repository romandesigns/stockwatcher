import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { DEFAULT_HALT_SOUNDS, HALT_SOUND_KEYS, HALT_SOUND_LABELS, HALT_SOUNDS, haltSoundKey, haltSoundNotes, haltSounds, soundMs } from "./haltSounds.ts";

test("the library: eight sounds and None, each short and labelled", () => {
  expect(HALT_SOUND_KEYS.length).toBe(9);
  expect(HALT_SOUND_KEYS.at(-1)).toBe("none");
  for (const k of HALT_SOUND_KEYS) {
    expect(HALT_SOUND_LABELS[k].name.length).toBeGreaterThan(0);
    const notes = haltSoundNotes(k);
    if (k === "none") {
      expect(notes).toBeNull();
      continue;
    }
    // Under a second: a 22.05 kHz 16-bit WAV of it stays under ~50 KB.
    expect(soundMs(notes!)).toBeLessThanOrEqual(1000);
    expect(notes!.some((n) => n.hz !== 0)).toBe(true);
  }
  expect(Object.keys(HALT_SOUNDS).sort()).toEqual(HALT_SOUND_KEYS.filter((k) => k !== "none").sort());
});

test("the sounds are distinct", () => {
  const seen = new Set(Object.values(HALT_SOUNDS).map((n) => JSON.stringify(n)));
  expect(seen.size).toBe(8);
});

test("the keys and defaults are the server's", () => {
  const rs = readFileSync(new URL("../../../crates/server/src/alerts/halts.rs", import.meta.url), "utf8");
  const keys = rs.match(/pub const SOUNDS: \[&str; \d+\] = \[([^\]]*)\]/)![1]!.match(/"([a-z]+)"/g)!.map((s) => s.slice(1, -1));
  expect(keys).toEqual([...HALT_SOUND_KEYS]);
  expect(rs).toContain(`pub const DEFAULT_HALTED_SOUND: &str = "${DEFAULT_HALT_SOUNDS.halted}";`);
  expect(rs).toContain(`pub const DEFAULT_RESUMED_SOUND: &str = "${DEFAULT_HALT_SOUNDS.resumed}";`);
});

test("stored values read defensively", () => {
  expect(haltSounds(undefined)).toEqual({ halted: "siren", resumed: "chime" });
  expect(haltSounds({ halted: "ding", resumed: "bogus" })).toEqual({ halted: "ding", resumed: "chime" });
  expect(haltSounds({ halted: "none" })).toEqual({ halted: "none", resumed: "chime" });
  expect(haltSoundKey(3, "blip")).toBe("blip");
});
