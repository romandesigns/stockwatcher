import {
  defaultRoleColor,
  indicatorColorRoles,
  indicatorLabel,
  indicatorSwatches,
  isHexColor,
  MACD_STYLES,
  macdMarkersOn,
  macdStyleOf,
  isPresetSpec,
  resetIndicator,
  roleColor,
  withIndicatorColor,
  type ChartTheme,
  type ColorRole,
  type IndicatorSpec,
} from "@stockwatcher/chart-core";
import { useEffect, useState } from "react";
import { Icon } from "./Icon.tsx";
import { Button } from "./ui/button.tsx";
import { Dialog, DialogClose, DialogContent, DialogTitle } from "./ui/dialog.tsx";
import { Input } from "./ui/input.tsx";
import { Popover, PopoverContent, PopoverTrigger } from "./ui/popover.tsx";
import { ScrollArea } from "./ui/scroll-area.tsx";
import { Switch } from "./ui/switch.tsx";
import { cn } from "./ui/utils.ts";

/**
 * One indicator's settings (owner, 2026-11-07), opened from its row in the Indicators menu:
 * - MACD: the style (computed on the server, `indicators.macd.v2`; offered only when the server
 *   computes them), the crossover markers, and the periods;
 * - every indicator: its line colours by role, each a swatch that opens a picker (the palette's
 *   indicator hues plus a hex field), and "Reset to default", which puts the whole indicator
 *   (style, markers, periods, colours) back to its preset.
 * Every change is a new spec for the chart's list, so it is saved and synced with the indicator
 * like the rest of its settings. Colours are display only (no server round trip).
 */
export function IndicatorSettingsDialog({
  spec,
  colorSlot,
  theme,
  macdStyles,
  onChange,
  onClose,
}: {
  /** The spec being edited; null = closed. */
  spec: IndicatorSpec | null;
  /** Its categorical colour slot on the chart (the default colours follow it). */
  colorSlot: number;
  theme: ChartTheme;
  /** The server computes the MACD styles: show the Style list. */
  macdStyles: boolean;
  onChange: (next: IndicatorSpec) => void;
  onClose: () => void;
}) {
  return (
    <Dialog open={spec !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="inset-x-4 top-[8vh] mx-auto flex max-h-[84vh] w-auto max-w-sm flex-col">
        {spec && <Body spec={spec} colorSlot={colorSlot} theme={theme} macdStyles={macdStyles} onChange={onChange} />}
      </DialogContent>
    </Dialog>
  );
}

function Body({ spec, colorSlot, theme, macdStyles, onChange }: { spec: IndicatorSpec; colorSlot: number; theme: ChartTheme; macdStyles: boolean; onChange: (next: IndicatorSpec) => void }) {
  const isMacd = spec.kind === "macd";
  const style = macdStyleOf(spec);
  const swatches = indicatorSwatches(theme);
  // "Reset to default" puts the whole indicator back to its preset (style, markers, periods,
  // colours): an indicator turned off and on keeps its settings, so this is the way back.
  const custom = !isPresetSpec(spec);
  return (
    <>
      <div className="flex items-center gap-2 px-4 pt-3 pb-2">
        <DialogTitle className="not-sr-only text-sm font-semibold">{indicatorLabel(spec)}</DialogTitle>
        <DialogClose render={<Button variant="ghost" size="sm" aria-label="Close" className="ml-auto size-8 p-0 text-muted-foreground [&_svg]:size-3.5" />}>
          <Icon name="clear" />
        </DialogClose>
      </div>
      <ScrollArea className="flex min-h-0 flex-col" viewportClassName="max-h-[calc(84vh-56px)] px-4 pb-4">
        <div className="flex flex-col gap-4">
          {isMacd && macdStyles && (
            <section aria-labelledby="ind-style" className="flex flex-col gap-1">
              <h3 id="ind-style" className="text-[11px] font-medium text-muted-foreground">
                Style
              </h3>
              <div role="radiogroup" aria-labelledby="ind-style" className="flex flex-col">
                {MACD_STYLES.map((s) => {
                  const on = s.id === style;
                  return (
                    <button
                      key={s.id}
                      type="button"
                      role="radio"
                      aria-checked={on}
                      onClick={() => onChange(s.id === "classic" ? withoutStyle(spec) : { ...spec, style: s.id })}
                      className="flex min-h-11 items-center gap-3 rounded-lg px-2 py-1.5 text-left transition-colors hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                    >
                      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                        <span className={cn("text-[13px] font-medium", on ? "text-foreground" : "text-foreground/90")}>{s.name}</span>
                        <span className="truncate text-[11px] text-text-faint">{s.hint}</span>
                      </span>
                      <span className={cn("text-primary transition-opacity duration-150 [&_svg]:size-3.5", on ? "opacity-100" : "opacity-0")}>
                        <Icon name="check" />
                      </span>
                    </button>
                  );
                })}
              </div>
            </section>
          )}
          {isMacd && (
            <section className="flex flex-col gap-2">
              <label className="flex min-h-11 cursor-pointer items-center gap-3 px-2">
                <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                  <span className="text-[13px] font-medium">Crossover markers</span>
                  <span className="text-[11px] text-text-faint">Triangles at signal crosses, rings at zero crosses</span>
                </span>
                <Switch checked={macdMarkersOn(spec)} onCheckedChange={(on) => onChange({ ...spec, markers: on })} />
              </label>
              <Periods spec={spec} onChange={onChange} />
            </section>
          )}
          <section aria-labelledby="ind-colors" className="flex flex-col gap-1">
            <div className="flex items-center justify-between">
              <h3 id="ind-colors" className="text-[11px] font-medium text-muted-foreground">
                Colors
              </h3>
              <Button variant="ghost" size="sm" disabled={!custom} onClick={() => onChange(resetIndicator(spec))} className="h-7 px-2 text-xs text-muted-foreground">
                Reset to default
              </Button>
            </div>
            {indicatorColorRoles(spec).map(({ role, name }) => (
              <ColorField
                key={role}
                name={name}
                value={roleColor(theme, spec, colorSlot, role)}
                fallback={defaultRoleColor(theme, spec, colorSlot, role)}
                picked={!!spec.colors?.[role]}
                swatches={swatches}
                onPick={(c) => onChange(withIndicatorColor(spec, role as ColorRole, c))}
              />
            ))}
          </section>
        </div>
      </ScrollArea>
    </>
  );
}

function withoutStyle(spec: IndicatorSpec): IndicatorSpec {
  const { style: _, ...rest } = spec;
  return rest;
}

/** MACD's fast, slow and signal periods (1–500, as parseIndicators accepts them); a field applies once it holds a valid one. */
function Periods({ spec, onChange }: { spec: IndicatorSpec; onChange: (next: IndicatorSpec) => void }) {
  const fields = [
    { key: "fast", label: "Fast", value: spec.fast ?? 12 },
    { key: "slow", label: "Slow", value: spec.slow ?? 26 },
    { key: "signal", label: "Signal", value: spec.signal ?? 9 },
  ] as const;
  return (
    <div className="grid grid-cols-3 gap-2 px-2">
      {fields.map((f) => (
        <PeriodField key={f.key} label={f.label} value={f.value} onValue={(v) => onChange({ ...spec, [f.key]: v })} />
      ))}
    </div>
  );
}

function PeriodField({ label, value, onValue }: { label: string; value: number; onValue: (v: number) => void }) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] text-text-faint">{label}</span>
      <Input
        inputMode="numeric"
        value={draft}
        aria-label={`${label} period`}
        className="h-9 font-mono text-[13px]"
        onChange={(e) => {
          const raw = e.currentTarget.value.replace(/\D/g, "").slice(0, 3);
          setDraft(raw);
          const n = Number(raw);
          if (raw && n >= 1 && n <= 500 && n !== value) onValue(n);
        }}
        onBlur={() => setDraft(String(value))}
      />
    </label>
  );
}

/**
 * One colour role: a swatch row that opens the picker (the palette's indicator hues as a swatch
 * row, plus a hex field). The swatch itself is the data colour; the picked one is ringed.
 */
function ColorField({ name, value, fallback, picked, swatches, onPick }: { name: string; value: string; fallback: string; picked: boolean; swatches: readonly string[]; onPick: (c: string | null) => void }) {
  const [open, setOpen] = useState(false);
  const [hex, setHex] = useState(value);
  useEffect(() => setHex(value), [value, open]);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        render={
          <button
            type="button"
            aria-label={`${name} color, ${picked ? value : "default"}`}
            className="flex min-h-11 w-full items-center gap-3 rounded-lg px-2 py-1.5 text-left transition-colors hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
          />
        }
      >
        <span className="size-5 shrink-0 rounded-md" style={{ background: value }} aria-hidden />
        <span className="flex-1 text-[13px]">{name}</span>
        <span className="font-mono text-[11px] text-text-faint">{picked ? value : "Default"}</span>
      </PopoverTrigger>
      <PopoverContent side="bottom" align="start" className="w-64" positionerClassName="z-[60]">
        <div className="flex flex-col gap-3">
          <div role="group" aria-label={`${name} presets`} className="grid grid-cols-5 gap-2">
            {swatches.map((c) => {
              const on = c.toLowerCase() === value.toLowerCase();
              return (
                <button
                  key={c}
                  type="button"
                  aria-label={c}
                  aria-pressed={on}
                  onClick={() => onPick(c.toLowerCase() === fallback.toLowerCase() ? null : c)}
                  className={cn("size-9 rounded-lg transition-[box-shadow,scale] duration-150 focus-visible:outline-none active:scale-95", on ? "shadow-[0_0_0_2px_var(--color-card),0_0_0_4px_var(--color-foreground)]" : "focus-visible:shadow-[0_0_0_2px_var(--color-ring)]")}
                  style={{ background: c }}
                />
              );
            })}
          </div>
          <div className="flex items-center gap-2">
            <span className="size-8 shrink-0 rounded-lg" style={{ background: isHexColor(hex) ? hex : value }} aria-hidden />
            <Input
              value={hex}
              aria-label={`${name} hex color`}
              placeholder="#rrggbb"
              spellCheck={false}
              className="h-8 font-mono text-[13px]"
              onChange={(e) => {
                const raw = e.currentTarget.value.trim();
                const next = raw.startsWith("#") ? raw : `#${raw}`;
                setHex(raw);
                if (isHexColor(next)) onPick(next.toLowerCase() === fallback.toLowerCase() ? null : next);
              }}
            />
          </div>
          {picked && (
            <Button variant="ghost" size="sm" onClick={() => onPick(null)} className="h-7 self-start px-2 text-xs text-muted-foreground">
              Use default
            </Button>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}
