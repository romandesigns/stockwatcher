import {
  CHART_TYPE_ICONS,
  CHART_TYPE_INFO,
  CHART_TYPES,
  DEFAULT_CHART_SETTINGS,
  ZOOM_PRESETS,
  type ChartSettings,
  type ChartType,
  type ChartVisibility,
  type PriceScaleType,
  type ZoomPreset,
} from "@stockwatcher/chart-core";
import type { ReactNode } from "react";
import { Icon } from "./Icon.tsx";
import { Button } from "./ui/button.tsx";
import { Popover, PopoverContent, PopoverTrigger } from "./ui/popover.tsx";
import { ToggleGroup, ToggleGroupItem } from "./ui/toggle-group.tsx";
import { Tip } from "./ui/tooltip.tsx";
import { cn } from "./ui/utils.ts";

/** Short labels for the type picker's tiles (the full names are the tooltips and accessible names). */
const SHORT: Readonly<Record<ChartType, string>> = { candles: "Candles", hollow: "Hollow", bars: "Bars", heikinAshi: "Heikin-Ashi", line: "Line", area: "Area", baseline: "Baseline" };

const SCALES: readonly { value: PriceScaleType; label: string; name: string }[] = [
  { value: "linear", label: "Price", name: "Linear (price)" },
  { value: "percent", label: "%", name: "Linear (%)" },
  { value: "log", label: "Log", name: "Logarithmic" },
];

type Switch = Exclude<keyof ChartSettings, "version" | "chartType" | "priceScale" | "zoom" | "visibility" | "autoStretch">;
const SHOW: readonly { key: Switch; name: string; description: string }[] = [
  { key: "extendedHours", name: "Extended hours", description: "Pre-market, after-hours and overnight candles" },
  { key: "volume", name: "Volume", description: "Volume bars under the price" },
  { key: "grid", name: "Grid lines", description: "Horizontal lines at the price labels" },
  { key: "sessionShading", name: "Highlight extended hours", description: "Shade candles outside 09:30–16:00" },
  { key: "sessionBreaks", name: "Session breaks", description: "A dashed line where each day starts" },
  { key: "highLow", name: "High / low labels", description: "Mark the highest and lowest price on screen" },
];
const VISIBILITY: readonly { key: keyof ChartVisibility; name: string }[] = [
  { key: "indicators", name: "Indicators" },
  { key: "drawings", name: "Drawings" },
  { key: "trading", name: "Orders and positions" },
];

/** Whether anything differs from the defaults (the Reset action shows only then). */
function isDefault(s: ChartSettings): boolean {
  return JSON.stringify(s) === JSON.stringify(DEFAULT_CHART_SETTINGS);
}

export interface ChartSettingsMenuProps {
  settings: ChartSettings;
  onChange: (s: ChartSettings) => void;
}

/**
 * The chart settings panel behind the toolbar's Chart button (owner request
 * 2026-10-18: Robinhood's chart settings, plus auto stretch and the
 * 25/50/100 % zoom). A Base UI popover in the menus' style: bg-card with a
 * shadow, sections separated by space and quiet labels, never by lines.
 * Choices that are one of several (type, zoom, scale) are segmented
 * controls whose selected option is RAISED, not accented; on/off rows end
 * with the accent check the Indicators and Levels menus use, which is the
 * one accent mark per checked row. Every change applies at once (the chart
 * slides to a new zoom and the volume pane slides in or out); nothing waits
 * for an Apply button.
 */
export function ChartSettingsMenu({ settings, onChange }: ChartSettingsMenuProps) {
  const set = (patch: Partial<ChartSettings>) => onChange({ ...settings, ...patch });
  return (
    <Popover>
      <Tip label="Chart settings">
        <PopoverTrigger aria-label="Chart settings" render={<Button variant="ghost" size="icon" className="size-7 rounded-md text-muted-foreground data-popup-open:bg-bg-raised data-popup-open:text-foreground [&_svg]:size-3.5" />}>
          <Icon name="chart" />
        </PopoverTrigger>
      </Tip>
      <PopoverContent side="bottom" align="end" sideOffset={6} aria-label="Chart settings" className="w-80 max-w-[calc(100vw-16px)]">
        <div className="flex flex-col gap-3">
          <div className="flex items-center justify-between">
            <span className="text-[13px] font-medium">Chart</span>
            {!isDefault(settings) && (
              <button type="button" onClick={() => onChange({ ...DEFAULT_CHART_SETTINGS, visibility: { ...DEFAULT_CHART_SETTINGS.visibility } })} className="rounded px-1 text-[11px] text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring">
                Reset
              </button>
            )}
          </div>

          <Section label="Type">
            <ToggleGroup aria-label="Chart type" value={[settings.chartType]} onValueChange={(v) => v[0] && set({ chartType: v[0] as ChartType })} spacing={1} className="grid w-full grid-cols-4 gap-1 rounded-lg bg-background p-1">
              {CHART_TYPES.map((t) => (
                <Tip key={t} label={CHART_TYPE_INFO[t].description}>
                  <ToggleGroupItem
                    value={t}
                    aria-label={CHART_TYPE_INFO[t].name}
                    className="h-auto min-w-0 flex-col gap-1 rounded-md px-0.5 py-1.5 text-[10px] font-normal text-muted-foreground transition-colors hover:bg-bg-row aria-pressed:bg-bg-raised aria-pressed:text-foreground [&_svg]:size-4"
                  >
                    <Icon name={CHART_TYPE_ICONS[t]} />
                    <span className="max-w-full truncate">{SHORT[t]}</span>
                  </ToggleGroupItem>
                </Tip>
              ))}
            </ToggleGroup>
            {/* Heikin-Ashi prices are averages: say so where it was chosen, once, quietly. */}
            {settings.chartType === "heikinAshi" && <p className="m-0 px-0.5 text-[11px] text-text-faint">Averaged candles. The price label, readout and orders use real prices.</p>}
          </Section>

          <Section label="Zoom">
            <Row
              name="Auto stretch"
              description={settings.autoStretch ? "Price fits the candles on screen" : "Fits once on open, then holds"}
              checked={settings.autoStretch}
              onToggle={() => set({ autoStretch: !settings.autoStretch })}
            />
            <div className="flex items-center justify-between gap-2 px-1.5">
              <span className="text-[11px] text-muted-foreground">Of the trading day</span>
              <Segmented ariaLabel="Horizontal zoom" value={String(settings.zoom)} onChange={(v) => set({ zoom: Number(v) as ZoomPreset })} options={ZOOM_PRESETS.map((z) => ({ value: String(z), label: `${z}%`, name: `${z} percent of the trading day` }))} />
            </div>
          </Section>

          <Section label="Price scale">
            <Segmented ariaLabel="Price scale" fill value={settings.priceScale} onChange={(v) => set({ priceScale: v as PriceScaleType })} options={SCALES.map((s) => ({ value: s.value, label: s.label, name: s.name }))} />
          </Section>

          <Section label="Show">
            {SHOW.map((r) => (
              <Row key={r.key} name={r.name} description={r.description} checked={settings[r.key]} onToggle={() => set({ [r.key]: !settings[r.key] } as Partial<ChartSettings>)} />
            ))}
          </Section>

          <Section label="Visibility">
            {VISIBILITY.map((r) => (
              <Row key={r.key} name={r.name} checked={settings.visibility[r.key]} onToggle={() => set({ visibility: { ...settings.visibility, [r.key]: !settings.visibility[r.key] } })} />
            ))}
          </Section>
        </div>
      </PopoverContent>
    </Popover>
  );
}

function Section({ label, children }: { label: string; children: ReactNode }) {
  return (
    <section aria-label={label} className="flex flex-col gap-1">
      <h3 className="m-0 px-1.5 text-[11px] font-medium text-muted-foreground">{label}</h3>
      {children}
    </section>
  );
}

/** An on/off row: name, an optional one-line description, and the accent check when on (the menus' pattern). */
function Row({ name, description, checked, onToggle }: { name: string; description?: string; checked: boolean; onToggle: () => void }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      onClick={onToggle}
      className="flex min-h-8 w-full items-center gap-2 rounded-md px-1.5 py-1 text-left outline-none transition-colors hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring"
    >
      <span className="flex min-w-0 flex-1 flex-col">
        <span className={cn("text-[13px]", checked ? "text-foreground" : "text-foreground/80")}>{name}</span>
        {description && <span className="truncate text-[11px] text-text-faint">{description}</span>}
      </span>
      <span className={cn("grid size-4 shrink-0 place-items-center text-primary transition-opacity duration-150 [&_svg]:size-3.5", checked ? "opacity-100" : "opacity-0")}>
        <Icon name="check" />
      </span>
    </button>
  );
}

/** One choice of several: a recessed track (the popover is bg-card, so the track sits on the page tone) with the selected option raised. */
function Segmented({ ariaLabel, value, onChange, options, fill }: { ariaLabel: string; value: string; onChange: (v: string) => void; options: readonly { value: string; label: string; name: string }[]; fill?: boolean }) {
  return (
    <ToggleGroup aria-label={ariaLabel} size="sm" spacing={0.5} variant="segmented" value={[value]} onValueChange={(v) => v[0] && onChange(v[0])} className={cn("bg-background", fill && "grid w-full auto-cols-fr grid-flow-col")}>
      {options.map((o) => (
        <ToggleGroupItem key={o.value} value={o.value} aria-label={o.name} className="h-6 px-2 font-mono text-[11px] transition-colors">
          {o.label}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
}
