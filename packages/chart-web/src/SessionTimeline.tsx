import { formatEtTime, layoutTimeline, sessionOf, timelineTimeAt, type ChartTheme, type SessionBand, type Size, type VolumeBucket } from "@stockwatcher/chart-core";
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
  variant?: "full" | "compact" | "minimal";
  theme?: ChartTheme;
  className?: string;
  /** Click (or arrow keys) to jump the replay there; absent = display only. */
  onSeek?: (t: number) => void;
}

const SESSION_NAME = { overnight: "overnight", pre: "premarket", regular: "regular session", after: "after hours" } as const;

/** Full-day session timeline: overnight/pre/regular/after bands, a volume minimap and the replay head. */
export function SessionTimeline({ bands, buckets, bucketMs, now, variant = "full", theme: themeProp, className, onSeek }: SessionTimelineProps) {
  const theme = useChartTheme(themeProp);
  const paint = useCallback(
    (ctx: CanvasRenderingContext2D, size: Size) => drawDisplayList(ctx, layoutTimeline({ bands, buckets, bucketMs, now, variant }, theme, size)),
    [bands, buckets, bucketMs, now, variant, theme],
  );
  const { containerRef, canvasRef } = useCanvas(paint);
  const label = now === null ? "Session timeline" : `Session timeline: ${formatEtTime(now, true)} Eastern, ${SESSION_NAME[sessionOf(now)]}.`;
  if (!onSeek)
    return (
      <div ref={containerRef} className={cn("relative", variant === "full" ? "h-[76px]" : variant === "minimal" ? "h-8" : "h-14", className)}>
        <canvas ref={canvasRef} role="img" aria-label={label} className="absolute inset-0 size-full" />
      </div>
    );
  const start = bands[0]?.start ?? 0;
  const end = bands[bands.length - 1]?.end ?? start;
  // Seeking (owner, 2026-10-17; the spec's pending "UI seek"): a click jumps there; with focus,
  // arrows step 5 minutes (Shift: 30). It is a slider over the trading day for assistive tech.
  return (
    <div ref={containerRef} className={cn("relative", variant === "full" ? "h-[76px]" : variant === "minimal" ? "h-8" : "h-14", className)}>
      <canvas
        ref={canvasRef}
        role="slider"
        tabIndex={0}
        aria-label="Replay position, click to jump"
        aria-valuemin={start}
        aria-valuemax={end}
        aria-valuenow={now ?? start}
        aria-valuetext={label}
        className="absolute inset-0 size-full cursor-pointer rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring"
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          const t = timelineTimeAt(bands, r.width, e.clientX - r.left);
          if (t !== null) onSeek(t);
        }}
        onKeyDown={(e) => {
          const step = (e.shiftKey ? 30 : 5) * 60_000;
          const d = e.key === "ArrowRight" ? step : e.key === "ArrowLeft" ? -step : 0;
          if (!d) return;
          e.preventDefault();
          onSeek(Math.min(end, Math.max(start, (now ?? start) + d)));
        }}
      />
    </div>
  );
}
