// Whether this phone's NATIVE build has the drawing alerts' tones (owner review 2026-11-09: the JS
// can reach a phone before the APK that bundles the sounds does). Android fixes a channel's sound
// when the channel is created; a drawing-* channel created by an APK without the WAV resources
// would keep the wrong sound for good, so nothing may create one, play a tone or tell the server
// "send me tone channels" until the binary itself carries them.
//
// The gate is the native app version (Android versionName, iOS CFBundleShortVersionString), read
// by expo-application from the installed binary, not from the JS bundle or an update's manifest:
// an over-the-air update cannot change it. The APK that adds the sounds (app.config.ts `sounds`)
// is the first with version DRAWING_TONES_NATIVE_VERSION, so any binary at or above it has them.
// (The fingerprint runtime version already keeps this JS off older binaries, since the plugin
// config is part of the fingerprint; this check holds even if that policy changes.)
// Pure, so it is testable without the native modules: setup.ts reads the version and asks here.

/** The first native version that bundles drawing_{up,down,stop}.wav. Bump app.config.ts `version` with it. */
export const DRAWING_TONES_NATIVE_VERSION = "0.2.0";

/** `version` (dotted numbers, e.g. "0.2.0") at or above `min`. Null or junk = no. */
export function nativeAtLeast(version: string | null | undefined, min: string): boolean {
  if (!version) return false;
  const parse = (v: string) => (/^\d+(\.\d+)*$/.test(v.trim()) ? v.trim().split(".").map(Number) : null);
  const a = parse(version);
  const b = parse(min);
  if (!a || !b) return false;
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const x = a[i] ?? 0;
    const y = b[i] ?? 0;
    if (x !== y) return x > y;
  }
  return true;
}

/** Android only: the tones live in notification channels, which iOS does not have. */
export function tonesSupportedOn(os: string, nativeVersion: string | null | undefined): boolean {
  return os === "android" && nativeAtLeast(nativeVersion, DRAWING_TONES_NATIVE_VERSION);
}
