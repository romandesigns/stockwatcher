// Sets the app version in src-tauri/tauri.conf.json. The release job runs
// this in its own checkout only (0.1.<run_number>); the committed version
// stays 0.1.0. Tauri takes the bundle and updater version from this field,
// so Cargo.toml does not need to change.
//
//   bun scripts/set-version.ts 0.1.42
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

// MSI (WiX) caps each part: major and minor at 255, patch at 65535.
export function assertInstallerVersion(version: string): void {
  const m = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
  if (!m) throw new Error(`version must be MAJOR.MINOR.PATCH, got "${version}"`);
  const [major, minor, patch] = m.slice(1).map(Number) as [number, number, number];
  if (major > 255 || minor > 255 || patch > 65535) {
    throw new Error(`version ${version} exceeds the MSI limits (255.255.65535)`);
  }
}

export function setVersion(confText: string, version: string): string {
  assertInstallerVersion(version);
  // Replace only the top-level field and keep the file's layout.
  const next = confText.replace(/^(  "version": )"[^"]*"/m, `$1"${version}"`);
  if (JSON.parse(next).version !== version) throw new Error("could not set the version field");
  return next;
}

if (import.meta.main) {
  const version = process.argv[2] ?? "";
  const conf = path.join(import.meta.dirname, "..", "src-tauri", "tauri.conf.json");
  writeFileSync(conf, setVersion(readFileSync(conf, "utf8"), version));
  console.log(`tauri.conf.json version = ${version}`);
}
