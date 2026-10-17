import { formatEtTime, layoutTimeline, sessionOf, type ChartTheme, type SessionBand, type Size, type VolumeBucket } from "@stockwatcher/chart-core";
import { useCallback } from "react";
import { drawDisplayList } from "./draw.ts";
import { cn } from "./ui/utils.ts";
import { useCanvas, useChartTheme } from "./useCanvas.ts";

export interface SessionTimelineProps {
  /** The day's four session bands (chart-core sessionBands). */
  bands: readonly SessionBand[];
  /** Volume per bucket across the day (chart-core volumeBuckets, or a server series). */
  buckets: readonly VolumeBucket[];
  bucketMs: number;
  /** Replay head / live clock; null hides it. */
  now: number | null;
  /** "full": labelled bands + head time (desktop). "compact": thin bands, labels below (phone). */
  variant?: "full" | "compact";
  theme?: ChartTheme;
  className?: string;
}

const SESSION_NAME = { overnight: "overnight", pre: "premarket", regular: "regular session", after: "after hours" } as const;

/** Full-day session timeline: overnight/pre/regular/after bands, a volume minimap and the replay head. */
export function SessionTimeline({ bands, buckets, bucketMs, now, variant = "full", theme: themeProp, className }: SessionTimelineProps) {
  const theme = useChartTheme(themeProp);
  const paint = useCallback(
    (ctx: CanvasRenderingContext2D, size: Size) => drawDisplayList(ctx, layoutTimeline({ bands, buckets, bucketMs, now, variant }, theme, size)),
    [bands, buckets, bucketMs, now, variant, theme],
  );
  const { containerRef, canvasRef } = useCanvas(paint);
  const label = now === null ? "Session timeline" : `Session timeline: ${formatEtTime(now, true)} Eastern, ${SESSION_NAME[sessionOf(now)]}.`;
  return (
    <div ref={containerRef} className={cn("relative", variant === "full" ? "h-[76px]" : "h-14", className)}>
      <canvas ref={canvasRef} role="img" aria-label={label} className="absolute inset-0 size-full" />
    </div>
  );
}
