import { describe, expect, test } from "bun:test";
import { Collapsible, CollapsibleContent, CollapsibleTrigger, Input, OptionSelect } from "@stockwatcher/chart-web/ui";
import { mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { renderToStaticMarkup } from "react-dom/server";
import { TimeSelect } from "./TimeSelect.tsx";

const SESSIONS = [
  { value: "all", label: "All sessions" },
  { value: "pre", label: "Premarket" },
];

describe("shadcn controls (owner, 2026-10-22: no browser-native UI)", () => {
  test("OptionSelect is a labelled combobox showing the picked option's label, not a <select>", () => {
    const html = renderToStaticMarkup(<OptionSelect size="sm" aria-label="Detection session" options={SESSIONS} value="pre" onValueChange={() => {}} />);
    expect(html).toContain('role="combobox"');
    expect(html).toContain('aria-label="Detection session"');
    expect(html).toContain("Premarket");
    expect(html).not.toContain("<select");
    expect(html).not.toContain("<option");
  });

  test("OptionSelect with a visible label wires it to the trigger", () => {
    const html = renderToStaticMarkup(<OptionSelect label="Expires" options={SESSIONS} value="all" onValueChange={() => {}} />);
    const labelId = html.match(/<div id="([^"]+)"[^>]*>Expires</)?.[1];
    expect(labelId).toBeTruthy();
    expect(html).toContain(`aria-labelledby="${labelId}"`);
    expect(html).toContain("All sessions");
  });

  test("numeric option values render their label", () => {
    const html = renderToStaticMarkup(<OptionSelect aria-label="Minimum gap" options={[{ value: 5, label: "5 min" }, { value: 15, label: "15 min" }]} value={15} onValueChange={() => {}} />);
    expect(html).toContain("15 min");
  });

  test("TimeSelect: hour and minute selects, an off-grid minute kept", () => {
    const html = renderToStaticMarkup(<TimeSelect label="Quiet from (ET)" value="22:07" onChange={() => {}} />);
    expect(html).toContain('aria-label="Quiet from (ET)"');
    expect(html).toContain('aria-label="Quiet from (ET), hour"');
    expect(html).toContain('aria-label="Quiet from (ET), minute"');
    expect(html).toContain(">22<");
    expect(html).toContain(">07<");
    expect(html).not.toContain('type="time"');
  });

  test("Collapsible replaces <details>: a button with aria-expanded", () => {
    const html = renderToStaticMarkup(
      <Collapsible defaultOpen>
        <CollapsibleTrigger>Context</CollapsibleTrigger>
        <CollapsibleContent>body</CollapsibleContent>
      </Collapsible>,
    );
    expect(html).toContain('aria-expanded="true"');
    expect(html).toContain("body");
    expect(html).not.toContain("<details");
  });

  test("Input is the shadcn field", () => {
    expect(renderToStaticMarkup(<Input aria-label="Ticker" />)).toContain('data-slot="input"');
  });
});

// A guard so native UI does not creep back in, over every .tsx/.ts/.css file under the web app
// and the chart package (the desktop app wraps the web app and has no UI source of its own).
// Bare <input> is allowed only for what has no shadcn equivalent here (none today). A line that
// truly needs a native piece carries `native-ok: <why>` in a comment on that same line; keep
// them rare and specific (none today). Comment lines are skipped, so docs can name what they replace.
const SCROLL_JSX = [/\boverflow(?:-[xy])?-(?:auto|scroll)\b/, /\boverflow[XY]?:\s*["'`](?:auto|scroll)/];
const NATIVE_SCRIPT = [
  // Blocking browser dialogs and the native <dialog> API: use the shadcn Dialog or a toast.
  /(?<![\w$.])(?:alert|confirm|prompt)\(/,
  /\b(?:window|globalThis|self)\.(?:alert|confirm|prompt)\(/,
  /\.showModal\(/,
];
const NATIVE_JSX = [
  // Controls with shadcn/Base UI equivalents (Select, Switch, Collapsible, Dialog, Input).
  /<select[\s>]/, /<option[\s>]/, /<datalist[\s>]/, /<details[\s>]/, /<summary[\s>]/, /<dialog[\s>]/, /<input[\s>]/, /<textarea[\s>]/,
  // Unstyled native gauges: draw them (the Alerts page's phone budget bar is a span).
  /<progress[\s>]/, /<meter[\s>]/,
  // Input types whose UI is the browser's: spinners, pickers, the search field's clear button.
  /\btype=\{?["'`](?:checkbox|radio|range|date|time|datetime-local|month|week|number|color|file|search)["'`]/,
  // Native tooltips.
  /\stitle=\{/, /\stitle="/,
  // Browser scrollbars (owner, 2026-10-23): every scroll region is the shadcn ScrollArea.
  ...SCROLL_JSX,
  ...NATIVE_SCRIPT,
];
// A CSS scroll container: only the ScrollArea's Base UI viewport scrolls (it styles itself).
const NATIVE_CSS = [/\boverflow(?:-[xy]|-block|-inline)?\s*:\s*(?:auto|scroll|overlay)\b/];
// The ScrollArea itself is the one place that may make something scroll.
const ALLOWED = [/[\\/]ui[\\/]scroll-area\.tsx$/];
const walk = (dir: string): string[] => readdirSync(dir).flatMap((f) => {
  const p = join(dir, f);
  if (statSync(p).isDirectory()) return f === "node_modules" ? [] : walk(p);
  return /\.(?:tsx?|css)$/.test(f) && !/\.test\.tsx?$/.test(f) && !/\.d\.ts$/.test(f) ? [p] : [];
});
const ROOTS = [join(import.meta.dir, ".."), join(import.meta.dir, "../../../../packages/chart-web/src")];

/** Each offending `file:line`, outside comments and `native-ok` lines. */
function nativeHits(files: string[]): string[] {
  return files.filter((f) => !ALLOWED.some((re) => re.test(f))).flatMap((file) => {
    const css = file.endsWith(".css");
    const rules = css ? NATIVE_CSS : file.endsWith(".tsx") ? NATIVE_JSX : [...NATIVE_SCRIPT, ...SCROLL_JSX];
    const text = readFileSync(file, "utf8");
    // CSS comments span lines: blank them (keeping the line breaks, so line numbers hold), since
    // a rule quoted inside one is documentation, not a hit.
    const body = css ? text.replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, " ")) : text;
    return body.split("\n").flatMap((line, i) => {
      if (/^\s*(?:\/\/|\*|\/\*)/.test(line) || /native-ok:/.test(line)) return [];
      return rules.some((re) => re.test(line)) ? [`${file}:${i + 1}`] : [];
    });
  });
}

test("no browser-native controls, dialogs, title tooltips or scrollbars in the web UI", () => {
  expect(nativeHits(ROOTS.flatMap(walk))).toEqual([]);
});

test("the guard catches what it is meant to, and only that", () => {
  const banned: [string, string][] = [
    ["tsx", `<div className="min-h-0 overflow-y-auto">`],
    ["tsx", `<div className="overflow-auto p-2">`],
    ["tsx", `<div className="overflow-x-scroll">`],
    ["tsx", `<div style={{ overflowY: "scroll" }}>`],
    ["tsx", `<Input type="number" />`],
    ["tsx", `<Input type={"date"} />`],
    ["tsx", `<details open>`],
    ["tsx", `<dialog open>`],
    ["tsx", `<progress value={1} />`],
    ["tsx", `ref.current?.showModal();`],
    ["ts", `if (confirm("Delete?")) remove();`],
    ["ts", `window.alert("hi");`],
    ["ts", `const row = "flex overflow-y-auto";`],
    ["css", `.list { overflow-y: auto; }`],
    ["css", `.list {\n  overflow: scroll;\n}`],
  ];
  const dir = mkdtempSync(join(tmpdir(), "native-guard-"));
  try {
    banned.forEach(([ext, src], i) => {
      const file = join(dir, `${i}.${ext}`);
      writeFileSync(file, `${src}\n`);
      expect([src, nativeHits([file]).length]).toEqual([src, 1]);
    });
    const ok = join(dir, "ok.tsx");
    writeFileSync(ok, [`// overflow-y-auto named in a comment`, `<div className="overflow-hidden" />`, `<div className="overflow-auto" /> {/* native-ok: a test */}`, `<Input type="password" />`, `onConfirm(x);`].join("\n"));
    expect(nativeHits([ok])).toEqual([]);
    const okCss = join(dir, "ok.css");
    writeFileSync(okCss, `/* a comment that says\n   overflow: auto is banned */\nhtml { overflow: hidden; }\n/* one line, overflow: scroll */\n`);
    expect(nativeHits([okCss])).toEqual([]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
