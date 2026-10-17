import {
  canAddIndicator,
  DEFAULT_CHART_THEME,
  INDICATOR_GLYPHS,
  indicatorInfo,
  MAX_OVERLAYS,
  MAX_PANES,
  DRAWING_KINDS,
  DRAWING_LABELS,
  formatPrice,
  formatSize,
  INDICATOR_PLACEMENT,
  INDICATOR_PRESETS,
  toggleIndicator,
  unrealizedPnl,
  type DrawingKind,
  type IndicatorSpec,
  type PaperPosition,
  type Timeframe,
  type ChartLayers,
  type FillMark,
} from "@stockwatcher/chart-core";
import type { ReactNode } from "react";
import { LayersMenu } from "./ContextMenus.tsx";
import { TimeframeSelector } from "./TimeframeSelector.tsx";
import { Tip } from "./ui/tooltip.tsx";
import { Button } from "./ui/button.tsx";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "./ui/dropdown-menu.tsx";
import { ToggleGroup, ToggleGroupItem } from "./ui/toggle-group.tsx";
import { cn } from "./ui/utils.ts";

const svg = (children: ReactNode) => (
  <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true">
    {children}
  </svg>
);

/** Inline icons (no icon dependency), drawn on a 16 px grid. */
export const TOOL_ICONS: Readonly<Record<DrawingKind | "cursor", ReactNode>> = {
  cursor: svg(<path d="M4 2.5v10l3-3 2 4.5 1.6-.7-2-4.4H13z" fill="currentColor" stroke="none" />),
  trend: svg(
    <>
      <path d="M3 12.5 13 3.5" />
      <circle cx="3" cy="12.5" r="1.3" fill="currentColor" />
      <circle cx="13" cy="3.5" r="1.3" fill="currentColor" />
    </>,
  ),
  ray: svg(
    <>
      <path d="M3 12.5 15 2" />
      <circle cx="3" cy="12.5" r="1.3" fill="currentColor" />
    </>,
  ),
  hline: svg(<path d="M1.5 8h13" />),
  vline: svg(<path d="M8 1.5v13" />),
  rect: svg(<rect x="3" y="4" width="10" height="8" rx="1" fill="currentColor" fillOpacity="0.35" stroke="none" />),
  // The ruler: a measure is not stored, it reads between two points.
  measure: svg(
    <>
      <path d="M2 11.5 11.5 2l2.5 2.5L4.5 14z" />
      <path d="M5 9.5l1 1M7 7.5l1.5 1.5M9 5.5l1 1" />
    </>,
  ),
  // Position tools: the green zone on the side the trade profits, the red one on the side it loses.
  long: svg(
    <>
      <rect x="3" y="2" width="10" height="6" rx="1" fill="#3fb97a" fillOpacity="0.55" stroke="none" />
      <rect x="3" y="8" width="10" height="5" rx="1" fill="#f0645c" fillOpacity="0.55" stroke="none" />
      <path d="M2 8h12" />
    </>,
  ),
  short: svg(
    <>
      <rect x="3" y="3" width="10" height="5" rx="1" fill="#f0645c" fillOpacity="0.55" stroke="none" />
      <rect x="3" y="8" width="10" height="6" rx="1" fill="#3fb97a" fillOpacity="0.55" stroke="none" />
      <path d="M2 8h12" />
    </>,
  ),
};

/** Indicators: a smooth line riding over price, the usual glyph for studies. */
const IndicatorIcon = svg(
  <>
    <path d="M1.5 12.5 4.5 9l2.5 2 3.5-5 4 3" strokeOpacity="0.45" />
    <path d="M1.5 9.5c2.5-4 4.5-5 6.5-3s4 1.5 6.5-3" />
  </>,
);

const DEFAULT_SERIES = [DEFAULT_CHART_THEME.series1, DEFAULT_CHART_THEME.series2, DEFAULT_CHART_THEME.series3] as const;

/** The drawing tools switch: a pencil. */
const PencilIcon = svg(<path d="M10.5 2.5 13.5 5.5 5.5 13.5H2.5v-3zM9 4l3 3" />);

/** A drawing tool's tooltip: its name, and its key where it has one. */
export function toolTip(k: DrawingKind | "cursor"): string {
  if (k === "cursor") return "Cursor (Esc)";
  if (k === "long") return "Long position (L)";
  if (k === "short") return "Short position (S)";
  if (k === "measure") return "Measure (or Shift+drag)";
  return DRAWING_LABELS[k];
}

const TrashIcon = svg(<path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.5h5.8l.6-8.5" />);

export interface ChartToolbarProps {
  /** Active drawing tool; null is the cursor (pan, inspect, select). */
  tool: DrawingKind | null;
  onToolChange: (tool: DrawingKind | null) => void;
  indicators: readonly IndicatorSpec[];
  onIndicatorsChange: (specs: IndicatorSpec[]) => void;
  /** The chart's indicator colours (theme series1..3), so the menu's icons match the lines. */
  seriesColors?: readonly string[];
  /** Show the drawing tools and delete in this row (default true). CandleChart turns this off and floats them in a DrawingToolbox instead. */
  drawingTools?: boolean;
  /** A drawing is selected, so Delete applies to it. */
  canDelete: boolean;
  onDelete: () => void;
  /** Simulated trading summary, the trade list and the ticket button; absent hides them. */
  trading?: { position: PaperPosition | null; last: number | null; onTrade: () => void; fills?: readonly FillMark[]; realized?: number } | null;
  /** The context layers switch (halts, LULD, key levels, fills); absent hides the Levels menu. */
  layers?: ChartLayers;
  onLayersChange?: (l: ChartLayers) => void;
  /** The floating drawing tools: shown or hidden, and the switch (absent hides the switch). */
  toolsVisible?: boolean;
  onToolsVisibleChange?: (visible: boolean) => void;
  /** Shown at the left of the row (CandleChart puts the inspected candle's readout here). */
  start?: ReactNode;
  /** The candle timeframe, shown at the left of the row (absent hides it). */
  timeframe?: Timeframe;
  onTimeframeChange?: (tf: Timeframe) => void;
  className?: string;
}

/**
 * The chart's own toolbar: drawing tools, the indicator menu, delete, and
 * (with trading) the simulated position and a Trade button. Part of the
 * chart package, so every screen that shows a chart gets the same tools.
 * One compact row; selection is a raised segment, and the accent is spent
 * only on the Trade button (the primary action).
 */
export function ChartToolbar({ tool, onToolChange, indicators, onIndicatorsChange, seriesColors, drawingTools = true, canDelete, onDelete, trading, layers, onLayersChange, toolsVisible, onToolsVisibleChange, timeframe, onTimeframeChange, start, className }: ChartToolbarProps) {
  const pos = trading?.position && trading.position.qty !== 0 ? trading.position : null;
  const pnl = pos && trading?.last !== null && trading?.last !== undefined ? unrealizedPnl(pos, trading.last) : null;
  return (
    <div className={cn("flex flex-wrap items-center gap-1.5", className)}>
      {drawingTools && (
      <>
      <ToggleGroup aria-label="Drawing tool" size="sm" spacing={0.5} variant="segmented" value={[tool ?? "cursor"]} onValueChange={(v) => v[0] && onToolChange(v[0] === "cursor" ? null : (v[0] as DrawingKind))}>
        {(["cursor", ...DRAWING_KINDS] as const).map((k) => (
          <Tip key={k} label={toolTip(k)}>
            <ToggleGroupItem value={k} aria-label={k === "cursor" ? "Cursor" : DRAWING_LABELS[k]} className="w-7 px-0 [&_svg]:size-3.5">
              {TOOL_ICONS[k]}
            </ToggleGroupItem>
          </Tip>
        ))}
      </ToggleGroup>
      <Tip label="Delete selected drawing (Del)">
        <Button variant="ghost" size="icon" className="size-7 rounded-md text-muted-foreground [&_svg]:size-3.5" aria-label="Delete selected drawing" disabled={!canDelete} onClick={onDelete}>
          {TrashIcon}
        </Button>
      </Tip>
      </>
      )}
      {start && <div className="min-w-0 flex-1 overflow-hidden">{start}</div>}
      {/* One group on the right, kept on one line: position, timeframes, drawing tools, indicators, levels, Trade. */}
      <div className="ml-auto flex flex-nowrap items-center gap-1.5">
        {pos && (
          <Tip label="Simulated position and unrealized P&L">
            <span className="font-mono text-[11px] text-muted-foreground tabular-nums">
              {pos.qty > 0 ? "+" : ""}
              {formatSize(pos.qty)} @ {formatPrice(pos.avgPrice)}
              {pnl !== null && <span className={cn("ml-1.5", pnl >= 0 ? "text-good" : "text-critical")}>{`${pnl >= 0 ? "+" : "−"}$${Math.abs(pnl).toFixed(2)}`}</span>}
            </span>
          </Tip>
        )}
        {timeframe && onTimeframeChange && (
          <>
            <TimeframeSelector value={timeframe} onValueChange={onTimeframeChange} />
            {/* Owner-requested divider between the timeframes and the chart icons: a soft bar in a surface tone, not a hard line. */}
            <span aria-hidden="true" className="mx-1.5 h-5 w-px shrink-0 rounded-full bg-bg-raised" />
          </>
        )}
        {onToolsVisibleChange && (
          <Tip label={toolsVisible ? "Hide drawing tools" : "Show drawing tools"}>
            <Button
              variant="ghost"
              size="icon"
              aria-label="Drawing tools"
              aria-pressed={!!toolsVisible}
              onClick={() => onToolsVisibleChange(!toolsVisible)}
              className={cn("size-7 rounded-md [&_svg]:size-3.5", toolsVisible ? "bg-bg-raised text-foreground hover:bg-bg-raised" : "text-muted-foreground")}
            >
              {PencilIcon}
            </Button>
          </Tip>
        )}
        <IndicatorMenu indicators={indicators} onChange={onIndicatorsChange} seriesColors={seriesColors ?? DEFAULT_SERIES} />
        {layers && onLayersChange && <LayersMenu layers={layers} onChange={onLayersChange} />}
        {trading && (
          <>
            <Tip label="Simulated order (fills against the replay tape)">
              <Button size="sm" className="ml-0.5 h-7 rounded-md px-3 text-xs" onClick={trading.onTrade}>
                Trade
              </Button>
            </Tip>
          </>
        )}
      </div>
    </div>
  );
}

/** An indicator's glyph (shared with the phone through chart-core), tinted with its line colour. */
function Glyph({ kind, color }: { kind: IndicatorSpec["kind"]; color: string }) {
  return (
    <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke={color} strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {INDICATOR_GLYPHS[kind].map((g, i) => (
        <path key={i} d={g.d} fill={g.fill ? color : "none"} fillOpacity={g.fill ? 0.18 : undefined} stroke={g.fill ? "none" : color} strokeOpacity={g.faint ? 0.4 : 1} />
      ))}
    </svg>
  );
}

/**
 * The indicator menu. Each row: the glyph on a tile, tinted with the colour
 * its line has (or will get) on the chart, so the menu doubles as the
 * legend's key; the name with its settings in a muted chip; one line on
 * what it measures; and the accent check when on. Section headers count
 * against the caps (3 on the chart, 2 below), and a full section says so
 * on its rows instead of just greying them out.
 */
function IndicatorMenu({ indicators, onChange, seriesColors }: { indicators: readonly IndicatorSpec[]; onChange: (s: IndicatorSpec[]) => void; seriesColors: readonly string[] }) {
  const on = new Set(indicators.map((s) => s.id));
  const active = (placement: "overlay" | "pane") => indicators.filter((s) => INDICATOR_PLACEMENT[s.kind] === placement);
  const group = (placement: "overlay" | "pane") => INDICATOR_PRESETS.filter((p) => INDICATOR_PLACEMENT[p.kind] === placement);
  // An overlay's colour is its position among the active overlays (the next free slot when off);
  // an oscillator's main line always uses the first slot.
  const colorOf = (p: IndicatorSpec) => {
    if (INDICATOR_PLACEMENT[p.kind] === "pane") return seriesColors[0]!;
    const list = active("overlay");
    const i = list.findIndex((s) => s.id === p.id);
    return seriesColors[(i >= 0 ? i : list.length) % seriesColors.length]!;
  };
  const item = (p: IndicatorSpec) => {
    const checked = on.has(p.id);
    const allowed = canAddIndicator(indicators, p);
    const info = indicatorInfo(p);
    return (
      <DropdownMenuCheckboxItem key={p.id} checked={checked} disabled={!allowed} onCheckedChange={() => onChange(toggleIndicator(indicators, p))} closeOnClick={false} className="gap-2.5 py-1.5 pr-9">
        <span className={cn("grid size-8 shrink-0 place-items-center rounded-lg", checked ? "bg-bg-raised" : "bg-bg-row")}>
          <Glyph kind={p.kind} color={checked ? colorOf(p) : "var(--color-muted-foreground)"} />
        </span>
        <span className="flex min-w-0 flex-col gap-0.5">
          <span className="flex items-center gap-1.5">
            <span className={cn("text-[13px] font-medium", checked ? "text-foreground" : "text-foreground/90")}>{info.name}</span>
            {info.params && <span className="rounded bg-bg-row px-1 font-mono text-[10px] text-muted-foreground">{info.params}</span>}
          </span>
          <span className="truncate text-[11px] text-text-faint">{allowed ? info.description : "Section full: turn one off first"}</span>
        </span>
      </DropdownMenuCheckboxItem>
    );
  };
  const header = (label: string, used: number, cap: number) => (
    <DropdownMenuLabel className="flex items-center justify-between px-1.5 pt-1.5 pb-1 text-[11px] font-medium text-muted-foreground">
      <span>{label}</span>
      <span className={cn("font-mono tabular-nums", used >= cap ? "text-foreground" : "text-text-faint")}>
        {used}/{cap}
      </span>
    </DropdownMenuLabel>
  );
  return (
    <DropdownMenu>
      <Tip label="Indicators">
        <DropdownMenuTrigger
          aria-label={indicators.length ? `Indicators, ${indicators.length} on` : "Indicators"}
          render={<Button variant="ghost" size="sm" className="h-7 min-w-7 gap-1 rounded-md px-1.5 text-xs text-muted-foreground [&_svg]:size-3.5" />}
        >
          {IndicatorIcon}
          {indicators.length ? <span className="font-mono text-[11px] text-foreground">{indicators.length}</span> : null}
        </DropdownMenuTrigger>
      </Tip>
      <DropdownMenuContent align="end" aria-label="Indicators" className="w-80 p-1.5">
        <DropdownMenuGroup>
          {header("On the chart", active("overlay").length, MAX_OVERLAYS)}
          {group("overlay").map(item)}
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuGroup>
          {header("Below the chart", active("pane").length, MAX_PANES)}
          {group("pane").map(item)}
        </DropdownMenuGroup>
        {indicators.length > 0 && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem onClick={() => onChange([])} className="justify-center text-xs text-muted-foreground">
              Turn all off
            </DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
