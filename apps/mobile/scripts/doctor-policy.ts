// Decides whether an expo-doctor run is acceptable for CI.
//
// Exactly one failure is tolerated: "no duplicate dependencies", and only
// when every reported duplicate is the SAME version. That case is an
// artifact of Bun's isolated linker, not a real second copy:
// expo-constants, expo-font and expo-keep-awake peer-depend on `expo`,
// which depends on them, and Bun materialises that cycle as two store
// entries of one version. It is harmless because
//   - Metro bundles one copy: SDK 55+ enables autolinking module
//     resolution in monorepos, which pins native-module imports to the
//     path autolinking chose (checked in the exported Android bundle's
//     source map, 2026-10-16), and
//   - autolinking compiles one copy into the binary.
// expo-doctor has no switch for this check, hence the filter. A duplicate
// with two different versions is a real conflict and still fails.

const DUPLICATE_CHECK = "Check that no duplicate dependencies are installed";

// Also tolerated: the SDK version check when EVERY mismatch it lists is a PATCH mismatch
// (owner release 2026-10-21: Expo published patch versions of 8 SDK 58 packages overnight and
// every CI run started failing). Taking those patches changes the native fingerprint, so the
// next release would build a new APK and phones on the current one would stop receiving OTA
// updates until they install it. That is a deliberate step (bump the packages together with a
// planned APK), not something a third-party release should force on an ordinary push. A minor
// or major mismatch, or a mismatch list we cannot parse, still fails.
//
// 2026-11-03: Expo raised react-native-safe-area-context's expected range to ~5.11.1 (we pin ~5.10.1),
// a MINOR mismatch, along with 12 patch ones, and every run failed again, holding a server fix. For
// the same reason (the bump means a new APK), that one package is in package.json
// `expo.install.exclude`, which expo-doctor's own version check honours; take it out when the SDK
// packages are bumped together with the next planned APK. Any other minor or major mismatch still fails.
const SDK_CHECK = "Check that packages match versions required by installed Expo SDK";

export interface DoctorVerdict {
  ok: boolean;
  reasons: string[];
}

export function judgeDoctorOutput(output: string, exitCode: number): DoctorVerdict {
  if (exitCode === 0) return { ok: true, reasons: [] };

  const failed = [...output.matchAll(/^✖ (.+)$/gm)].map((m) => m[1]!.trim());
  if (failed.length === 0) {
    return { ok: false, reasons: [`expo-doctor exited ${exitCode} without a failed check (crash or network error?)`] };
  }

  const reasons: string[] = [];
  for (const check of failed) {
    if (check === DUPLICATE_CHECK) continue;
    if (check === SDK_CHECK) {
      const kinds = [...output.matchAll(/(\w+) version mismatches/gi)].map((m) => m[1]!.toLowerCase());
      if (kinds.length === 0) reasons.push("SDK version check failed but its mismatches could not be parsed");
      else if (kinds.some((k) => k !== "patch")) reasons.push(`failed: ${check} (${[...new Set(kinds)].join(", ")} mismatches)`);
      continue;
    }
    reasons.push(`failed: ${check}`);
  }

  // Each "Found duplicates for <pkg>:" is followed by tree lines naming
  // "<pkg>@<version> (at: ...)".
  const blocks = output.split(/^Found duplicates for /m).slice(1);
  if (failed.includes(DUPLICATE_CHECK) && blocks.length === 0) {
    reasons.push("duplicate check failed but no duplicates could be parsed");
  }
  for (const block of blocks) {
    const name = block.slice(0, block.indexOf(":")).trim();
    const escaped = name.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");
    const versions = new Set(
      [...block.matchAll(new RegExp(`${escaped}@([^\\s]+) \\(at:`, "g"))].map((m) => m[1]),
    );
    if (versions.size !== 1) {
      reasons.push(`${name} is installed in different versions: ${[...versions].join(", ") || "unparsed"}`);
    }
  }

  return { ok: reasons.length === 0, reasons };
}
