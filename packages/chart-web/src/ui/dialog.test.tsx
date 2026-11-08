import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { DIALOG_CENTERED } from "./dialog.tsx";
import { cn } from "./utils.ts";

// Every modal is centred in the viewport (owner, 2026-11-07: "modals/dialogs should always appear
// in the center of the screen"). The shared DialogContent centres; this holds every caller to it:
// a caller sets the size only, never a position (top/left/inset/translate/margin or an inline
// left/top), except a bottom sheet marked `bottom-sheet-ok` on purpose. The rendered check (the
// popup's box centred at 320–1920 px) is the headless Edge run in the commit that added this.

const ROOTS = [join(import.meta.dir, ".."), join(import.meta.dir, "../../../../apps/web/src")];
const POSITION = /(?:^|\s)-?(?:top|left|right|bottom|inset|inset-x|inset-y|translate-x|translate-y|m|mx|my|mt|mb|ml|mr)-(?!fit\b)[^\s"]+/;

function files(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    if (name === "node_modules" || name.startsWith(".")) continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...files(p));
    else if (p.endsWith(".tsx") && !p.endsWith(".test.tsx")) out.push(p);
  }
  return out;
}

describe("dialogs are centred (owner, 2026-11-07)", () => {
  test("the shared popup centres with inset-0 + auto margins, never past the viewport or the safe area", () => {
    for (const c of ["fixed", "inset-0", "m-auto", "h-fit", "max-w-md"]) expect(DIALOG_CENTERED.split(" ")).toContain(c);
    expect(DIALOG_CENTERED).toContain("max-h-[calc(100dvh-2rem-env(safe-area-inset-top)-env(safe-area-inset-bottom))]");
    // A caller's size survives the merge, the centring too.
    const merged = cn(DIALOG_CENTERED, "w-[min(400px,calc(100vw-2rem))] max-w-sm").split(" ");
    expect(merged).toContain("inset-0");
    expect(merged).toContain("m-auto");
    expect(merged).toContain("max-w-sm");
    expect(merged).not.toContain("max-w-md");
  });

  test("no DialogContent in the web app or the chart package positions itself", () => {
    const uses: string[] = [];
    for (const f of ROOTS.flatMap(files)) {
      const src = readFileSync(f, "utf8");
      for (let i = src.indexOf("<DialogContent"); i >= 0; i = src.indexOf("<DialogContent", i + 1)) {
        const end = src.indexOf(">\n", i);
        const tag = src.slice(i, end < 0 ? i + 800 : end);
        uses.push(f);
        if (tag.includes("bottom-sheet-ok")) continue;
        const cls = /className=(?:"([^"]*)"|\{[^}]*?"([^"]*)")/.exec(tag);
        const value = cls?.[1] ?? cls?.[2] ?? "";
        expect({ file: f, offending: value.split(/\s+/).filter((t) => POSITION.test(` ${t}`)) }).toEqual({ file: f, offending: [] });
        expect({ file: f, inlinePosition: /style=\{[^}]*\b(?:left|top|right|bottom|transform)\b/.test(tag) }).toEqual({ file: f, inlinePosition: false });
      }
    }
    // The four dialogs of today: settings (two layouts), ticker search, alert form, indicator settings.
    expect(uses.length).toBeGreaterThanOrEqual(5);
  });
});
