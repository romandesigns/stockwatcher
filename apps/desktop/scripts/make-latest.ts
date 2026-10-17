// Stages a desktop release for infra/vps/publish-updates.sh: copies the
// signed NSIS and MSI installers and their .sig files out of the Tauri
// bundle directory into <out>/desktop/ and writes <out>/desktop/latest.json,
// the manifest the updater fetches from
// https://stockwatcher.wavystyle.io/updates/desktop/latest.json.
//
//   bun scripts/make-latest.ts --bundle ../../target/release/bundle \
//     --out ../../target/updates --version 0.1.42 --notes 1a2b3c4
//
// The rules below mirror what publish-updates.sh accepts, so a bad upload
// fails here, in CI, instead of on the VPS.
import { copyFileSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { parseArgs } from "node:util";

export const BASE_URL = "https://stockwatcher.wavystyle.io/updates/desktop";
/** Same pattern as `name_re` in infra/vps/publish-updates.sh (minus `desktop/`). */
export const FILE_NAME_RE = /^[A-Za-z0-9][A-Za-z0-9._+-]{0,127}\.(json|exe|msi|sig)$/;
const MAX_ENTRIES = 50;
const MAX_UNPACKED_BYTES = 400 * 1024 * 1024;

export type Installer = { kind: "nsis" | "msi"; file: string; sigFile: string; signature: string };
export type PlatformEntry = { signature: string; url: string };
export type Manifest = {
  version: string;
  notes: string;
  pub_date: string;
  platforms: Record<string, PlatformEntry>;
};

/** Finds exactly one installer of `kind` for `version`, with its .sig. */
export function findInstaller(bundleDir: string, kind: "nsis" | "msi", version: string): Installer {
  const dir = path.join(bundleDir, kind);
  const ext = kind === "nsis" ? ".exe" : ".msi";
  const matches = readdirSync(dir).filter((f) => f.endsWith(ext) && f.includes(`_${version}_`));
  if (matches.length !== 1) {
    throw new Error(`expected one ${kind} installer for ${version} in ${dir}, found ${matches.length}`);
  }
  const name = matches[0]!;
  const sigFile = path.join(dir, `${name}.sig`);
  const signature = readFileSync(sigFile, "utf8").trim();
  return { kind, file: path.join(dir, name), sigFile, signature };
}

export function buildManifest(opts: {
  version: string;
  notes: string;
  pubDate: Date;
  nsis: Installer;
  msi: Installer;
  baseUrl?: string;
}): Manifest {
  const base = opts.baseUrl ?? BASE_URL;
  const entry = (i: Installer): PlatformEntry => ({
    signature: i.signature,
    url: `${base}/${path.basename(i.file)}`,
  });
  return {
    version: opts.version,
    notes: opts.notes,
    // RFC 3339, without milliseconds.
    pub_date: opts.pubDate.toISOString().replace(/\.\d{3}Z$/, "Z"),
    platforms: {
      // The updater tries `<os>-<arch>-<installer>` first (matching how the
      // app was installed), then `<os>-<arch>`. NSIS is the default.
      "windows-x86_64": entry(opts.nsis),
      "windows-x86_64-nsis": entry(opts.nsis),
      "windows-x86_64-msi": entry(opts.msi),
    },
  };
}

/** Throws unless `m` is a manifest the updater and the VPS will accept. */
export function validateManifest(m: Manifest, baseUrl = BASE_URL): void {
  if (!/^\d+\.\d+\.\d+$/.test(m.version)) throw new Error(`bad version ${m.version}`);
  if (Number.isNaN(Date.parse(m.pub_date))) throw new Error(`bad pub_date ${m.pub_date}`);
  for (const key of ["windows-x86_64", "windows-x86_64-nsis", "windows-x86_64-msi"]) {
    const p = m.platforms[key];
    if (!p) throw new Error(`platform ${key} missing`);
    if (!p.url.startsWith(`${baseUrl}/`)) throw new Error(`${key}: url outside ${baseUrl}`);
    const name = p.url.slice(baseUrl.length + 1);
    if (!FILE_NAME_RE.test(name)) throw new Error(`${key}: file name ${name} would be rejected`);
    if (!name.includes(`_${m.version}_`)) throw new Error(`${key}: ${name} is not version ${m.version}`);
    // A Tauri .sig is base64 of a minisign signature file; the CLI records
    // the version in the trusted comment, which requireSignedVersion checks.
    const decoded = Buffer.from(p.signature, "base64").toString("utf8");
    if (!decoded.startsWith("untrusted comment:")) throw new Error(`${key}: not a minisign signature`);
    // e.g. "timestamp:1792195761\tfile:stockwatcher_0.1.0_x64-setup.exe\tversion:0.1.0"
    const trusted = /^trusted comment: (.*)$/m.exec(decoded)?.[1] ?? "";
    const fields = new Map(trusted.split("\t").map((f) => f.split(/:(.*)/s, 2) as [string, string]));
    if (fields.get("version") !== m.version) throw new Error(`${key}: signature is not for version ${m.version}`);
    if (fields.get("file") !== name) throw new Error(`${key}: signature is for ${fields.get("file")}, not ${name}`);
  }
}

/** Copies installers + sigs and writes latest.json into `<outDir>/desktop/`. */
export function stage(outDir: string, manifest: Manifest, installers: Installer[]): string[] {
  const dest = path.join(outDir, "desktop");
  mkdirSync(dest, { recursive: true });
  const files: string[] = [];
  for (const i of installers) {
    for (const src of [i.file, i.sigFile]) {
      const name = path.basename(src);
      if (!FILE_NAME_RE.test(name)) throw new Error(`file name ${name} would be rejected`);
      copyFileSync(src, path.join(dest, name));
      files.push(name);
    }
  }
  writeFileSync(path.join(dest, "latest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  files.push("latest.json");

  const total = files.reduce((sum, f) => sum + statSync(path.join(dest, f)).size, 0);
  if (files.length + 1 > MAX_ENTRIES) throw new Error(`too many files (${files.length})`);
  if (total > MAX_UNPACKED_BYTES) throw new Error(`staged size ${total} exceeds ${MAX_UNPACKED_BYTES}`);
  return files;
}

if (import.meta.main) {
  const { values } = parseArgs({
    options: {
      bundle: { type: "string" },
      out: { type: "string" },
      version: { type: "string" },
      notes: { type: "string", default: "" },
      "base-url": { type: "string", default: BASE_URL },
    },
  });
  const { bundle, out, version } = values;
  if (!bundle || !out || !version) {
    console.error("usage: make-latest.ts --bundle <dir> --out <dir> --version <x.y.z> [--notes <text>]");
    process.exit(2);
  }
  const baseUrl = values["base-url"];
  const nsis = findInstaller(bundle, "nsis", version);
  const msi = findInstaller(bundle, "msi", version);
  const manifest = buildManifest({ version, notes: values.notes, pubDate: new Date(), nsis, msi, baseUrl });
  validateManifest(manifest, baseUrl);
  const files = stage(out, manifest, [nsis, msi]);

  const rows = files.map((f) => `| \`${f}\` | ${statSync(path.join(out, "desktop", f)).size} |`);
  const summary = [
    `### Desktop ${version}`,
    "",
    `Manifest: ${baseUrl}/latest.json (notes: \`${values.notes}\`)`,
    "",
    "| File | Bytes |",
    "|---|---|",
    ...rows,
    "",
  ].join("\n");
  console.log(summary);
  if (process.env.GITHUB_STEP_SUMMARY) writeFileSync(process.env.GITHUB_STEP_SUMMARY, summary, { flag: "a" });
}
