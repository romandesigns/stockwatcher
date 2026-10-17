// @stockwatcher/chart-web: React + Canvas 2D execution of chart-core's
// display list, plus the chart's composable React components. Props-driven,
// no app coupling (no fetching, no routing, no global state); works in a
// browser and in the Tauri webview.
//
// Host-app requirements: Tailwind v4 with this package in @source, the
// shadcn token names mapped onto the palette (bg-primary, text-muted-
// foreground, text-text-faint, text-warning, text-critical, bg-bg-row,
// bg-bg-raised),
// and optionally `--chart-*` CSS variables to theme the canvas (theme.ts).
export { CandleChart, type CandleChartProps, type ChartSeriesLike } from "./CandleChart.tsx";
export { PriceAxisMenu, PRICE_SCALE_LABELS, type PriceAxisMenuProps } from "./PriceAxisMenu.tsx";
export { CandleInspector, type CandleInspectorProps } from "./CandleInspector.tsx";
export { ReplayControls, DEFAULT_SPEEDS, speedLabel, type ReplayControlsProps } from "./ReplayControls.tsx";
export { SessionTimeline, type SessionTimelineProps } from "./SessionTimeline.tsx";
export { Tape, type TapeProps } from "./Tape.tsx";
export { TimeframeSelector, type TimeframeSelectorProps } from "./TimeframeSelector.tsx";
export { VolumeProfile, type VolumeProfileProps } from "./VolumeProfile.tsx";
export { drawDisplayList, type Ctx2D } from "./draw.ts";
export { CSS_VARS, DEFAULT_CHART_THEME, readChartTheme } from "./theme.ts";
export { useCanvas, useChartTheme, type Paint } from "./useCanvas.ts";
