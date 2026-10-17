import type { ChartTheme } from "./displayList.ts";

/**
 * The default theme: the approved design C values (dark-first, 60/30/10: the
 * accent only marks the POC, the value area, the inspection line, the replay
 * head and a selected drawing). It lives in chart-core so every renderer
 * (web canvas, native Skia) starts from the same values; apps override
 * tokens (chart-web reads `--chart-*` CSS variables, see its theme.ts).
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
  // dataviz validator, dark surface #0f1216, --pairs all: worst CVD ΔE 13.2,
  // normal-vision ΔE 19.3, all inside the lightness band, all ≥ 3:1.
  series1: "#3987e5",
  series2: "#c98500",
  series3: "#d55181",
  guide: "rgba(152, 162, 174, 0.28)",
  drawing: "#b8c0ca",
  drawingSelected: "#8b9cff",
  drawingFill: "rgba(139, 156, 255, 0.12)",
  handleFill: "#0f1216",
  orderBuy: "#3fb97a",
  orderSell: "#f0645c",
  onOrder: "#0f1216",
  position: "#98a2ae",
  // Halts and LULD are trading restrictions: the warning (amber) status colour, always with a word.
  halt: "rgba(224, 166, 58, 0.10)",
  haltText: "#e0a63a",
  luld: "rgba(224, 166, 58, 0.6)",
  luldText: "#e0a63a",
  level: "rgba(152, 162, 174, 0.45)",
  levelStrong: "rgba(228, 232, 237, 0.55)",
  fontSans: '"Geist Variable", ui-sans-serif, system-ui, sans-serif',
  fontMono: '"Geist Mono Variable", ui-monospace, "Cascadia Code", "SF Mono", monospace',
};

/** `#rrggbb` (or `#rgb`) at alpha a, as rgba(); any other colour string is returned unchanged. */
export function withAlpha(color: string, a: number): string {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(color);
  if (!m) return color;
  const h = m[1]!.length === 3 ? [...m[1]!].map((c) => c + c).join("") : m[1]!;
  const n = Number.parseInt(h, 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
}
