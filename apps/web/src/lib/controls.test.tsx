import { describe, expect, test } from "bun:test";
import { Collapsible, CollapsibleContent, CollapsibleTrigger, Input, OptionSelect } from "@stockwatcher/chart-web/ui";
import { readdirSync, readFileSync, statSync } from "node:fs";
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

// A guard so a native control does not creep back in: every .tsx under the web app and the chart
// package. Bare <input> is allowed only for what has no shadcn equivalent here (none today).
const NATIVE = [/<select[\s>]/, /<option[\s>]/, /<datalist[\s>]/, /<details[\s>]/, /<summary[\s>]/, /<dialog[\s>]/, /<input[\s>]/, /<textarea[\s>]/, /\btype="(?:checkbox|radio|range|date|time|datetime-local|month|week|number|color|file)"/, /\bwindow\.(?:alert|confirm|prompt)\(/, /\stitle=\{/, /\stitle="/];
const walk = (dir: string): string[] => readdirSync(dir).flatMap((f) => {
  const p = join(dir, f);
  return statSync(p).isDirectory() ? walk(p) : /\.tsx$/.test(f) && !/\.test\.tsx$/.test(f) ? [p] : [];
});

test("no browser-native controls, dialogs or title tooltips in the web UI", () => {
  const src = [join(import.meta.dir, ".."), join(import.meta.dir, "../../../../packages/chart-web/src")];
  const hits = src.flatMap(walk).flatMap((file) =>
    readFileSync(file, "utf8").split("\n").flatMap((line, i) => (!/^\s*(?:\/\/|\*|\/\*)/.test(line) && NATIVE.some((re) => re.test(line)) ? [`${file}:${i + 1}`] : [])),
  )
    // The Runners panel is being reworked on another branch (2026-10-22); its one `title` goes with it.
    .filter((h) => !/RocketPanel\.tsx/.test(h));
  expect(hits).toEqual([]);
});
