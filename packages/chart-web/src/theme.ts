import { DEFAULT_CHART_THEME, type ChartTheme } from "@stockwatcher/chart-core";

/** Re-exported from chart-core, the one source every renderer shares. */
export { DEFAULT_CHART_THEME };

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
