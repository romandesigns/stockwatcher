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
    if (check !== DUPLICATE_CHECK) reasons.push(`failed: ${check}`);
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
