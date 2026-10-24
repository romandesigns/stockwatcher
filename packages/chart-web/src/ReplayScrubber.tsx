import {
  alertNear,
  formatEtTime,
  REPLAY_ALERT_COLOR,
  scrubberFraction,
  scrubberKeySeek,
  scrubberTimeAt,
  scrubberTooltip,
  scrubberVolumeAt,
  sessionOf,
  type AlertDot,
  type ScrubberBar,
  type ScrubberSegment,
  type ScrubberSpan,
  type ScrubberTick,
} from "@stockwatcher/chart-core";
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { cn } from "./ui/utils.ts";

export interface ReplayScrubberProps {
  /** 04:00–20:00 ET of the replay day (chart-core scrubberSpan). */
  span: ScrubberSpan;
  /** Pre-market / Regular / After hours at their true proportions (scrubberSegments). */
  segments: readonly ScrubberSegment[];
  /** The volume histogram (scrubberBars). */
  bars: readonly ScrubberBar[];
  ticks: readonly ScrubberTick[];
  /** The shown ticker's alert dots (alertDots). */
  dots: readonly AlertDot[];
  /** The replay clock; null hides the playhead. */
  now: number | null;
  /** Seek there (the server rebuilds the replay). Absent or `disabled`: display only. */
  onSeek?: (t: number) => void;
  disabled?: boolean;
  /** "long": Pre-market / After hours labels and a small handle (desktop, unfolded). "short": Pre / After and a 20 px handle (cover screen). */
  labels?: "long" | "short";
  /** Where the time · volume · alert tooltip shows: on mouse hover and while dragging ("hover"), or only while dragging ("drag", touch layouts). */
  tooltip?: "hover" | "drag";
  className?: string;
}

const SESSION_NAME = { overnight: "overnight", pre: "pre-market", regular: "regular session", after: "after hours" } as const;
/** A seek's target stays drawn until the clock reaches it, so the playhead never jumps back: the
 *  server rebuilds the replay first, which takes up to a minute or two on a whole-market day. */
const HOLD_MS = 180_000;

/**
 * The replay bar's lower tier (approved design "B · Two-tier scrubber"): the
 * trading day 04:00–20:00 as session-shaded bands at their true proportions, a
 * volume histogram, the played portion tinted (the accent's only use besides
 * play), the playhead with a draggable handle, the day's alert dots and the
 * hour ticks. Click or drag to seek (the seek is sent on release: each one
 * rebuilds the replay on the server, so a drag must not send one per pixel).
 * It is a slider for the keyboard and assistive tech: arrows step a minute
 * (Shift 15), Page Up/Down 30 minutes, Home/End the day's ends. Each alert
 * dot is a button that seeks to it, named for its kind, time and price.
 */
export function ReplayScrubber({ span, segments, bars, ticks, dots, now, onSeek, disabled, labels = "long", tooltip = "hover", className }: ReplayScrubberProps) {
  const trackRef = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<number | null>(null);
  const [hover, setHover] = useState<{ x: number; t: number } | null>(null);
  const [held, setHeld] = useState<{ t: number; from: number | null } | null>(null);
  const interactive = !!onSeek && !disabled;

  // Drop the held target once the rebuilt clock has arrived near it, or after HOLD_MS (a refused
  // seek). Playing on from the old position moves the clock too, but stays about as far away.
  if (held && now !== null && now !== held.from && Math.abs(now - held.t) < Math.max(5_000, Math.abs((held.from ?? now) - held.t) / 2)) setHeld(null);
  useEffect(() => {
    if (!held) return;
    const id = setTimeout(() => setHeld(null), HOLD_MS);
    return () => clearTimeout(id);
  }, [held]);

  const seek = (t: number) => {
    if (!onSeek) return;
    setHeld({ t, from: now });
    onSeek(t);
  };
  const shown = drag ?? held?.t ?? now;
  const frac = shown === null ? null : scrubberFraction(span, shown);
  const pct = (f: number) => `${(f * 100).toFixed(3)}%`;

  const timeAt = (e: ReactPointerEvent) => {
    const r = trackRef.current?.getBoundingClientRect();
    if (!r) return null;
    return { x: e.clientX - r.left, w: r.width, t: scrubberTimeAt(span, r.width, e.clientX - r.left) };
  };
  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (!interactive || e.button !== 0) return;
    const p = timeAt(e);
    if (!p) return;
    e.currentTarget.setPointerCapture?.(e.pointerId);
    setDrag(p.t);
    setHover({ x: p.x, t: p.t });
  };
  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const p = timeAt(e);
    if (!p) return;
    if (drag !== null) setDrag(p.t);
    if (drag !== null || (tooltip === "hover" && e.pointerType === "mouse")) setHover({ x: p.x, t: p.t });
  };
  const onPointerUp = () => {
    if (drag === null) return;
    seek(drag);
    setDrag(null);
    setHover(null);
  };

  const width = trackRef.current?.getBoundingClientRect().width ?? 0;
  const tip = hover && (drag !== null || tooltip === "hover") ? scrubberTooltip(hover.t, scrubberVolumeAt(bars, span, hover.t), alertNear(dots, width, hover.x)?.alert ?? null) : null;
  const tipAlert = hover ? alertNear(dots, width, hover.x)?.alert : undefined;
  const valueText = shown === null ? "Clock not started" : `${formatEtTime(shown, true)} Eastern, ${SESSION_NAME[sessionOf(shown)]}`;
  const short = labels === "short";

  return (
    <div className={cn("relative select-none", className)}>
      {/* The track: bands, histogram, tint and ticks; the slider owns the pointer and the keys. */}
      <div
        ref={trackRef}
        role="slider"
        tabIndex={interactive ? 0 : -1}
        aria-label="Replay position"
        aria-disabled={!interactive || undefined}
        aria-valuemin={span.start}
        aria-valuemax={span.end}
        aria-valuenow={Math.round(shown ?? span.start)}
        aria-valuetext={valueText}
        className={cn("absolute inset-0 touch-none rounded-lg outline-none focus-visible:ring-2 focus-visible:ring-ring", interactive ? "cursor-pointer" : "cursor-default")}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={() => {
          setDrag(null);
          setHover(null);
        }}
        onPointerLeave={() => drag === null && setHover(null)}
        onKeyDown={(e) => {
          if (!interactive) return;
          const t = scrubberKeySeek(e.key, e.shiftKey, shown ?? span.start, span);
          if (t === null) return;
          e.preventDefault();
          seek(t);
        }}
      >
        <div className="absolute inset-x-0 top-0 bottom-[18px] flex overflow-hidden rounded-lg" aria-hidden="true">
          {segments.map((s) => (
            <div key={s.kind} className={cn("relative h-full", s.kind === "regular" ? "bg-bg-row/70" : "bg-background/55")} style={{ width: pct(s.to - s.from) }}>
              <span className={cn("absolute top-1.5 left-2 truncate text-[11px] leading-none", short && "top-[5px] left-1.5 text-[10px]", s.kind === "regular" ? "text-muted-foreground" : "text-text-faint")} style={{ maxWidth: "calc(100% - 12px)" }}>
                {short ? s.short : s.label}
              </span>
            </div>
          ))}
        </div>
        <div className="absolute inset-x-0 bottom-[18px] flex h-[52px] items-end gap-px px-0.5" aria-hidden="true">
          {bars.map((b) => {
            const played = shown !== null && b.t < shown;
            return <div key={b.t} className={cn("min-w-0 flex-1 rounded-t-[2px]", played ? "bg-primary/80" : "bg-text-faint/45")} style={{ height: `${Math.round(b.h * 100)}%` }} />;
          })}
        </div>
        {frac !== null && (
          <div
            aria-hidden="true"
            className={cn("absolute top-0 bottom-[18px] left-0 rounded-l-lg bg-primary/[0.07]", drag === null && "transition-[width] duration-200 ease-linear motion-reduce:transition-none")}
            style={{ width: pct(frac) }}
          />
        )}
        <div className="absolute inset-x-0 bottom-0 h-3.5 font-mono text-[10px] leading-[14px] text-text-faint" aria-hidden="true">
          {ticks.map((t) => (
            <span key={t.t} className="absolute top-0 whitespace-nowrap" style={{ left: pct(t.frac), transform: `translateX(-${(t.frac * 100).toFixed(2)}%)` }}>
              {t.label}
            </span>
          ))}
        </div>
      </div>

      {/* Alert dots: buttons above the track (siblings, so the slider holds no controls). */}
      {dots.map((d) => (
        <button
          key={`${d.alert.symbol}-${d.alert.kind}-${d.alert.t}`}
          type="button"
          aria-label={`${d.label}, jump there`}
          disabled={!interactive}
          onClick={() => seek(d.alert.t)}
          className={cn("absolute top-[16px] grid size-5 -translate-x-1/2 place-items-center rounded-full outline-none focus-visible:ring-2 focus-visible:ring-ring", interactive ? "cursor-pointer" : "pointer-events-none")}
          style={{ left: pct(d.frac) }}
        >
          <span className="size-2 rounded-full transition-transform duration-150 hover:scale-125 motion-reduce:transition-none" style={{ background: REPLAY_ALERT_COLOR[d.alert.kind] }} />
        </button>
      ))}

      {frac !== null && (
        <div
          aria-hidden="true"
          className={cn("pointer-events-none absolute top-0 bottom-[14px] -translate-x-1/2", drag === null && "transition-[left] duration-200 ease-linear motion-reduce:transition-none")}
          style={{ left: pct(frac) }}
        >
          <div className={cn("absolute bottom-0 left-1/2 w-0.5 -translate-x-1/2 rounded-full bg-foreground", short ? "-top-1" : "-top-0.5")} />
          <div className={cn("absolute left-1/2 -translate-x-1/2 rounded-full bg-foreground", short ? "-top-2.5 size-5 shadow-[0_2px_8px_rgb(0_0_0/0.4)]" : "-top-0.5 size-3")} />
        </div>
      )}

      {tip && hover && (
        <div
          role="status"
          className="pointer-events-none absolute top-[18px] z-10 flex gap-2 rounded-lg bg-bg-raised px-2 py-1.5 text-[11px] whitespace-nowrap text-foreground shadow-[0_6px_18px_rgb(0_0_0/0.35)]"
          style={{ left: `clamp(0px, calc(${hover.x}px - 60px), calc(100% - 200px))` }}
        >
          <span className="font-mono">{tip.time}</span>
          {tip.volume && <span className="text-muted-foreground">{tip.volume}</span>}
          {tip.alert && tipAlert && <span style={{ color: REPLAY_ALERT_COLOR[tipAlert.kind] }}>{tip.alert}</span>}
        </div>
      )}
    </div>
  );
}
