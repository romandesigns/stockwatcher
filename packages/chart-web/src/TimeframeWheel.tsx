import { TIMEFRAMES, type Timeframe } from "@stockwatcher/chart-core";
import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { Tip } from "./ui/tooltip.tsx";
import { cn } from "./ui/utils.ts";

/** Pointer travel for one step: a short drag turns one notch (the phone wheel's STEP_PX). */
const STEP_PX = 18;
/** Wheel travel for one step. A mouse notch is ~100 px (deltaMode 0) and turns one step; a trackpad's small deltas add up to it. */
const WHEEL_STEP = 50;
/** A mouse wheel fires several events per notch on some systems: a step at most this often (ms). */
const WHEEL_GAP_MS = 90;
const LABEL = 18;
const HEIGHT = 28;
const RIDGE = 3.5;

const NAMES: Record<Timeframe, string> = { "15s": "15 seconds", "30s": "30 seconds", "1m": "1 minute", "5m": "5 minutes" };

/**
 * The phone's timeframe scroll wheel (chart-native TimeframeWheel) for the web, where a toolbar
 * has no room for four segments: the dashboard's multi-chart cells (owner, 2026-10-18). Same look,
 * a slim ridged drum fading at its ends beside the current timeframe, and the same direction: up
 * is the next (longer) timeframe. Turned by the mouse wheel or trackpad over it, by dragging it
 * up or down, or with the arrow keys (a slider to screen readers). No surface: it sits on
 * whatever is behind it, and the drum's fade is a mask, not a painted colour, so any background
 * works.
 */
export function TimeframeWheel({ value, onChange, className }: { value: Timeframe; onChange: (tf: Timeframe) => void; className?: string }) {
  const index = Math.max(0, TIMEFRAMES.indexOf(value));
  const last = TIMEFRAMES.length - 1;
  // The wheel's position in steps while dragging (fractional), else null: it rests on `index`.
  const [drag, setDrag] = useState<number | null>(null);
  const dragRef = useRef<{ y: number; from: number } | null>(null);
  const el = useRef<HTMLDivElement>(null);
  // Latest values for the native wheel listener, which is attached once.
  const latest = useRef({ index, onChange });
  latest.current = { index, onChange };

  const step = (to: number) => {
    const target = Math.min(last, Math.max(0, to));
    if (target !== latest.current.index) latest.current.onChange(TIMEFRAMES[target]!);
  };

  // A non-passive listener: React's onWheel is passive, so it could not stop the page (or the
  // chart under the toolbar) from scrolling while the wheel turns.
  useEffect(() => {
    const node = el.current;
    if (!node) return;
    let acc = 0;
    let lastStep = 0;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      acc += e.deltaMode === 1 ? e.deltaY * 40 : e.deltaY;
      if (Math.abs(acc) < WHEEL_STEP) return;
      const dir = acc < 0 ? 1 : -1; // wheel up = the next (longer) timeframe
      acc = 0;
      if (e.timeStamp - lastStep < WHEEL_GAP_MS) return;
      lastStep = e.timeStamp;
      const { index: i, onChange: change } = latest.current;
      const target = Math.min(last, Math.max(0, i + dir));
      if (target !== i) change(TIMEFRAMES[target]!);
    };
    node.addEventListener("wheel", onWheel, { passive: false });
    return () => node.removeEventListener("wheel", onWheel);
  }, [last]);

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    dragRef.current = { y: e.clientY, from: index };
    setDrag(index);
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const d = dragRef.current;
    if (!d) return;
    // Up (negative y) turns to longer timeframes; a little give past either end.
    const raw = d.from - (e.clientY - d.y) / STEP_PX;
    setDrag(Math.min(last + 0.35, Math.max(-0.35, raw)));
  };
  const endDrag = () => {
    if (!dragRef.current) return;
    dragRef.current = null;
    if (drag !== null) step(Math.round(drag));
    setDrag(null);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const to = e.key === "ArrowUp" || e.key === "ArrowRight" ? index + 1 : e.key === "ArrowDown" || e.key === "ArrowLeft" ? index - 1 : e.key === "Home" ? 0 : e.key === "End" ? last : null;
    if (to === null) return;
    e.preventDefault();
    step(to);
  };

  const pos = drag ?? index;
  // Ridges roll with the turn, wrapping every ridge spacing, so the drum looks like it turns.
  const phase = (((-pos * STEP_PX * 0.5) % RIDGE) + RIDGE) % RIDGE;

  return (
    <Tip label="Timeframe · scroll or drag">
      <div
        ref={el}
        role="slider"
        tabIndex={0}
        aria-label="Timeframe"
        aria-valuemin={0}
        aria-valuemax={last}
        aria-valuenow={index}
        aria-valuetext={NAMES[value]}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onKeyDown={onKeyDown}
        style={{ height: HEIGHT }}
        className={cn("flex shrink-0 cursor-ns-resize touch-none items-center gap-1.5 rounded-md px-1 outline-none select-none focus-visible:ring-2 focus-visible:ring-ring", className)}
      >
        {/* Fixed width, so the drum does not shift as "1m" becomes "15s". Only the current label shows; a neighbour fades in as the wheel turns towards it. */}
        <div className="relative w-6 overflow-hidden" style={{ height: HEIGHT }}>
          <div
            className={cn("absolute inset-x-0", drag === null && "transition-transform duration-150 ease-out motion-reduce:transition-none")}
            style={{ transform: `translateY(${(HEIGHT - LABEL) / 2 - pos * LABEL}px)` }}
          >
            {TIMEFRAMES.map((tf, i) => (
              <div
                key={tf}
                className={cn("font-mono text-[11px] font-semibold text-foreground tabular-nums", drag === null && "transition-opacity duration-150 motion-reduce:transition-none")}
                style={{ height: LABEL, lineHeight: `${LABEL}px`, opacity: Math.max(0, 1 - Math.abs(pos - i) / 0.6) }}
              >
                {tf}
              </div>
            ))}
          </div>
        </div>
        <div
          aria-hidden="true"
          className="h-6 w-2.5 overflow-hidden rounded-full bg-bg-raised"
          // The curvature: the drum fades out towards its ends, as a wheel turning away from you does.
          style={{ maskImage: "linear-gradient(to bottom, transparent, #000 32%, #000 68%, transparent)" }}
        >
          <div
            className="h-[calc(100%+7px)]"
            style={{
              transform: `translateY(${phase - RIDGE}px)`,
              backgroundImage: `repeating-linear-gradient(to bottom, transparent 0 ${RIDGE - 1}px, color-mix(in oklab, var(--color-foreground) 28%, transparent) ${RIDGE - 1}px ${RIDGE}px)`,
            }}
          />
        </div>
      </div>
    </Tip>
  );
}
