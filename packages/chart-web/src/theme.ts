import type { ChartTheme } from "@stockwatcher/chart-core";

/**
 * Fallback theme: the approved design C values (dark-first, 60/30/10 — the
 * accent only marks the POC, the value area, the inspection line and the
 * replay head). Apps override any token with a `--chart-*` CSS variable
 * (see CSS_VARS) or pass a theme object.
 */
export const DEFAULT_CHART_THEME: ChartTheme = {
  grid: "#1c222a",
  axisText: "#66707c",
  text: "#e4e8ed",
  textDim: "#98a2ae",
  up: "#3fb97a",
  down: "#f0645c",
  upVolume: "rgba(63, 185, 122, 0.45)",
  downVolume: "rgba(240, 100, 92, 0.45)",
  gapHatch: "rgba(152, 162, 174, 0.22)",
  crosshair: "rgba(139, 156, 255, 0.5)",
  crosshairLabelBg: "#262d36",
  crosshairLabelText: "#e4e8ed",
  onTag: "#0f1216",
  profileBar: "rgba(152, 162, 174, 0.22)",
  profileValueArea: "rgba(139, 156, 255, 0.42)",
  profilePoc: "#8b9cff",
  sessionOvernight: "#1c222a",
  sessionPre: "#232a33",
  sessionRegular: "#36405a",
  sessionAfter: "#232a33",
  sessionText: "#98a2ae",
  sessionTextStrong: "#e4e8ed",
  minimapPast: "rgba(152, 162, 174, 0.35)",
  minimapPastRegular: "rgba(139, 156, 255, 0.55)",
  minimapFuture: "rgba(152, 162, 174, 0.12)",
  replayHead: "#8b9cff",
  fontSans: '"Geist Variable", ui-sans-serif, system-ui, sans-serif',
  fontMono: '"Geist Mono Variable", ui-monospace, "Cascadia Code", "SF Mono", monospace',
};

const kebab = (s: string) => s.replace(/[A-Z]/g, (m) => `-${m.toLowerCase()}`);

/** Token → CSS custom property, e.g. upVolume → --chart-up-volume. */
export const CSS_VARS: Readonly<Record<keyof ChartTheme, string>> = Object.fromEntries(
  Object.keys(DEFAULT_CHART_THEME).map((k) => [k, `--chart-${kebab(k)}`]),
) as Record<keyof ChartTheme, string>;

/** Reads `--chart-*` overrides from an element's computed style. */
export function readChartTheme(el: Element | null, base: ChartTheme = DEFAULT_CHART_THEME): ChartTheme {
  if (!el || typeof getComputedStyle !== "function") return base;
  const style = getComputedStyle(el);
  const out = { ...base };
  for (const key of Object.keys(CSS_VARS) as (keyof ChartTheme)[]) {
    const v = style.getPropertyValue(CSS_VARS[key]).trim();
    if (v) out[key] = v;
  }
  return out;
}
