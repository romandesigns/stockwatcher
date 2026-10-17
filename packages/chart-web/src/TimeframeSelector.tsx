import { TIMEFRAMES, type Timeframe } from "@stockwatcher/chart-core";
import { ToggleGroup, ToggleGroupItem } from "./ui/toggle-group.tsx";
import { cn } from "./ui/utils.ts";

export interface TimeframeSelectorProps {
  value: Timeframe;
  onValueChange: (tf: Timeframe) => void;
  options?: readonly Timeframe[];
  /** "inline" (a compact segmented control) or "grid" (full width, 36 px targets). */
  variant?: "inline" | "grid";
  className?: string;
}

const NAMES: Record<Timeframe, string> = { "15s": "15 seconds", "30s": "30 seconds", "1m": "1 minute", "5m": "5 minutes" };

/** Single-select 15s/30s/1m/5m switch (shadcn ToggleGroup on Base UI). */
export function TimeframeSelector({ value, onValueChange, options = TIMEFRAMES, variant = "inline", className }: TimeframeSelectorProps) {
  return (
    <ToggleGroup
      aria-label="Timeframe"
      size="sm"
      spacing={0.5}
      variant="segmented"
      value={[value]}
      // Single select: pressing the active item would empty the group; ignore that.
      onValueChange={(v) => v[0] && onValueChange(v[0] as Timeframe)}
      className={cn(variant === "grid" && "grid w-full grid-cols-4", className)}
    >
      {options.map((tf) => (
        <ToggleGroupItem
          key={tf}
          value={tf}
          aria-label={NAMES[tf]}
          className={cn(
            "font-mono text-[11px]",
            variant === "grid" ? "h-9 text-xs" : "min-w-9 px-2",
          )}
        >
          {tf}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
}
