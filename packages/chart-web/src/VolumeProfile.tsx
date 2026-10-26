import { formatPrice, formatVolume, layoutProfile, type ChartTheme, type ProfileWire, type Size } from "@stockwatcher/chart-core";
import { useCallback, type ReactNode } from "react";
import { drawDisplayList } from "./draw.ts";
import { Tip } from "./ui/tooltip.tsx";
import { cn } from "./ui/utils.ts";
import { useCanvas, useChartTheme } from "./useCanvas.ts";

export interface VolumeProfileProps {
  profile: ProfileWire | null | undefined;
  /** Align bins to an external price range (e.g. the chart's visible range). */
  range?: { lo: number; hi: number };
  /** Header with bin width, POC and value area. */
  showHeader?: boolean;
  /** Extra header controls, e.g. a bin-width selector (request the new bin from the feed). */
  actions?: ReactNode;
  theme?: ChartTheme;
  className?: string;
}

/**
 * Standalone session volume profile (the chart can also overlay it via
 * options.showProfile). Bars grow from the left; the POC and the 70 % value
 * area (the profile's own: from the server, or recomputed by the store with
 * the engine's rules) use the accent, everything else stays neutral. The
 * value area is shown as bin starts, exactly as the engine reports it.
 */
export function VolumeProfile({ profile, range, showHeader = true, actions, theme: themeProp, className }: VolumeProfileProps) {
  const theme = useChartTheme(themeProp);
  const va = profile?.valueArea ?? null;
  const poc = profile?.poc ?? null;
  const paint = useCallback(
    (ctx: CanvasRenderingContext2D, size: Size) => {
      if (profile) drawDisplayList(ctx, layoutProfile({ profile, range }, theme, size));
    },
    [profile, range, theme],
  );
  const { containerRef, canvasRef } = useCanvas(paint);
  const summary =
    profile && poc !== null && va
      ? `Session profile ${profile.anchor}, bin $${profile.bin}, ${formatVolume(profile.total)} shares: point of control ${formatPrice(poc)}, value area ${formatPrice(va.low)} to ${formatPrice(va.high)}.`
      : "Session profile: no volume yet.";

  return (
    <div className={cn("flex min-h-0 flex-col gap-2", className)}>
      {showHeader && (
        <div className="flex flex-col gap-1">
          <div className="flex items-baseline gap-2">
            <h2 className="panel-title flex-1">Volume profile</h2>
            {poc !== null && (
              <span className="text-[11px] text-text-faint">
                POC <span className="font-mono text-primary">{formatPrice(poc)}</span>
              </span>
            )}
          </div>
          {(actions || va) && (
            <div className="flex items-center gap-2 text-[11px] text-text-faint">
              {actions}
              {va && (
                <Tip label="Value area: the 70% of volume around the POC">
                <span className="ml-auto">
                  VA <span className="font-mono text-foreground">{formatPrice(va.low)}–{formatPrice(va.high)}</span>
                </span>
                </Tip>
              )}
            </div>
          )}
        </div>
      )}
      <div ref={containerRef} className="relative min-h-0 flex-1">
        <canvas ref={canvasRef} role="img" aria-label={summary} className="absolute inset-0 size-full" />
      </div>
    </div>
  );
}
