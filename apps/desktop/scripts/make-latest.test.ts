import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { BASE_URL, FILE_NAME_RE, buildManifest, findInstaller, stage, validateManifest } from "./make-latest.ts";
import { assertInstallerVersion, setVersion } from "./set-version.ts";

// A .sig as the Tauri CLI writes it: base64 of a minisign signature file.
// The signature lines are dummies; only the comment layout matters here.
const fakeSig = (file: string, version: string) =>
  Buffer.from(
    `untrusted comment: signature from tauri secret key\nRUQAAAAA\ntrusted comment: timestamp:1\tfile:${file}\tversion:${version}\nAAAA\n`,
  ).toString("base64");

function fakeBundle(version: string, sigVersion = version) {
  const root = mkdtempSync(path.join(tmpdir(), "sw-bundle-"));
  const files = {
    nsis: `stockwatcher_${version}_x64-setup.exe`,
    msi: `stockwatcher_${version}_x64_en-US.msi`,
  };
  for (const [kind, name] of Object.entries(files)) {
    mkdirSync(path.join(root, kind));
    writeFileSync(path.join(root, kind, name), "installer");
    writeFileSync(path.join(root, kind, `${name}.sig`), fakeSig(name, sigVersion));
  }
  // A leftover from an earlier local build must not be picked up.
  writeFileSync(path.join(root, "nsis", "stockwatcher_0.0.9_x64-setup.exe"), "old");
  return root;
}

describe("make-latest", () => {
  test("builds and stages a manifest the VPS accepts", () => {
    const bundle = fakeBundle("0.1.42");
    const nsis = findInstaller(bundle, "nsis", "0.1.42");
    const msi = findInstaller(bundle, "msi", "0.1.42");
    const m = buildManifest({ version: "0.1.42", notes: "abc1234", pubDate: new Date(0), nsis, msi });

    expect(m.pub_date).toBe("1970-01-01T00:00:00Z");
    expect(Object.keys(m.platforms).sort()).toEqual(["windows-x86_64", "windows-x86_64-msi", "windows-x86_64-nsis"]);
    expect(m.platforms["windows-x86_64"]).toEqual(m.platforms["windows-x86_64-nsis"]!);
    expect(m.platforms["windows-x86_64-msi"]!.url).toBe(`${BASE_URL}/stockwatcher_0.1.42_x64_en-US.msi`);
    validateManifest(m);

    const out = mkdtempSync(path.join(tmpdir(), "sw-out-"));
    const files = stage(out, m, [nsis, msi]);
    expect(readdirSync(path.join(out, "desktop")).sort()).toEqual([...files].sort());
    for (const f of files) expect(FILE_NAME_RE.test(f)).toBe(true);
    expect(JSON.parse(readFileSync(path.join(out, "desktop", "latest.json"), "utf8"))).toEqual(m);
  });

  test("rejects a signature made for another version", () => {
    const bundle = fakeBundle("0.1.42", "0.1.4");
    const nsis = findInstaller(bundle, "nsis", "0.1.42");
    const msi = findInstaller(bundle, "msi", "0.1.42");
    const m = buildManifest({ version: "0.1.42", notes: "", pubDate: new Date(), nsis, msi });
    expect(() => validateManifest(m)).toThrow(/not for version/);
  });

  test("requires exactly one installer per kind", () => {
    expect(() => findInstaller(fakeBundle("0.1.1"), "nsis", "0.1.2")).toThrow(/found 0/);
  });

  test("file names follow publish-updates.sh", () => {
    expect(FILE_NAME_RE.test("stockwatcher_0.1.1_x64-setup.exe.sig")).toBe(true);
    expect(FILE_NAME_RE.test("stock watcher.exe")).toBe(false);
    expect(FILE_NAME_RE.test(".hidden.json")).toBe(false);
    expect(FILE_NAME_RE.test("app.zip")).toBe(false);
  });
});

describe("set-version", () => {
  const conf = readFileSync(path.join(import.meta.dirname, "..", "src-tauri", "tauri.conf.json"), "utf8");

  test("rewrites only the top-level version", () => {
    const next = setVersion(conf, "0.1.123");
    expect(JSON.parse(next).version).toBe("0.1.123");
    expect(next.replace("0.1.123", JSON.parse(conf).version)).toBe(conf);
  });

  test("enforces the MSI version limits", () => {
    expect(() => assertInstallerVersion("0.1.65535")).not.toThrow();
    expect(() => assertInstallerVersion("0.1.65536")).toThrow(/MSI/);
    expect(() => assertInstallerVersion("v0.1.2")).toThrow();
  });
});
