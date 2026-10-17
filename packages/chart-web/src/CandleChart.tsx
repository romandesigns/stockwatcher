import {
  buildSlots,
  computeChartModel,
  defaultViewport,
  describeSlot,
  ensureVisible,
  followLatest,
  hitTest,
  layoutChart,
  pan,
  slotIndexOf,
  stepInspection,
  zoom,
  type Candle,
  type ChartModel,
  type ChartOptions,
  type ChartState,
  type ChartTheme,
  type Gap,
  type Point,
  type ProfileWire,
  type Size,
  type Timeframe,
  type Viewport,
} from "@stockwatcher/chart-core";
import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { drawDisplayList } from "./draw.ts";
import { cn } from "./ui/utils.ts";
import { useCanvas, useChartTheme } from "./useCanvas.ts";

/**
 * What the chart draws; chart-core's Series satisfies it. Pass a NEW object
 * whenever anything in it changes (the store does): the chart memoizes on the
 * object, because the store mutates its arrays in place for O(1) appends.
 */
export interface ChartSeriesLike {
  candles: readonly Candle[];
  gaps: readonly Gap[];
  profile: ProfileWire | null;
}

export interface CandleChartProps {
  series: ChartSeriesLike | null | undefined;
  timeframe: Timeframe;
  /** t of the inspected slot; null = latest. Controlled when onInspectChange is set. */
  inspectT?: number | null;
  onInspectChange?: (t: number | null) => void;
  options?: Partial<ChartOptions>;
  theme?: ChartTheme;
  /** Initial pixels per candle. */
  barSpacing?: number;
  className?: string;
  /** Accessible name, e.g. "SMPL 1 minute candles". */
  label?: string;
}

const DRAG_THRESHOLD = 4;

/**
 * Canvas candlestick chart: candles, volume, gaps, axes, last-price tag,
 * optional profile overlay. Mouse hover / touch tap inspects a candle; drag
 * pans; wheel zooms around the pointer; double-click returns to the latest.
 * Keyboard (when focused): ←/→ step (Shift ×10), Home/End, Esc = latest,
 * +/− zoom, 0 = follow latest. Inspections made by keyboard or tap are
 * announced through a polite live region; hover is not, to avoid chatter.
 */
export function CandleChart({ series, timeframe, inspectT: inspectProp, onInspectChange, options, theme: themeProp, barSpacing = 9, className, label }: CandleChartProps) {
  const theme = useChartTheme(themeProp);
  const [ownInspect, setOwnInspect] = useState<number | null>(null);
  const controlled = onInspectChange !== undefined;
  const inspectT = controlled ? (inspectProp ?? null) : ownInspect;
  const setInspect = useCallback(
    (t: number | null) => {
      if (controlled) onInspectChange?.(t);
      else setOwnInspect(t);
    },
    [controlled, onInspectChange],
  );

  // Each timeframe gets a fresh viewport: zoom/pan positions do not carry
  // across timeframes whose slots mean different durations.
  const [view, setView] = useState<{ tf: Timeframe; vp: Viewport }>(() => ({ tf: timeframe, vp: defaultViewport(barSpacing) }));
  const vp = view.tf === timeframe ? view.vp : defaultViewport(barSpacing);
  const setVp = useCallback((f: (vp: Viewport) => Viewport) => setView((v) => ({ tf: timeframe, vp: f(v.tf === timeframe ? v.vp : defaultViewport(barSpacing)) })), [timeframe, barSpacing]);

  const [pointer, setPointer] = useState<Point | null>(null);
  const [announcement, setAnnouncement] = useState("");

  const slots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);

  const state: ChartState = useMemo(
    () => ({ timeframe, candles: series?.candles ?? [], gaps: series?.gaps ?? [], profile: series?.profile ?? null, slots, inspectT, pointer, options }),
    [timeframe, series, slots, inspectT, pointer, options],
  );

  const modelRef = useRef<ChartModel | null>(null);
  const paint = useCallback(
    (ctx: CanvasRenderingContext2D, size: Size) => {
      const model = computeChartModel(state, vp, size);
      modelRef.current = model;
      drawDisplayList(ctx, layoutChart(model, state, theme));
    },
    [state, vp, theme],
  );
  const { containerRef, canvasRef, size } = useCanvas(paint);
  const model = useMemo(() => computeChartModel(state, vp, size), [state, vp, size]);

  // If the inspected slot disappears (trimmed, or a new snapshot), fall back to latest.
  useEffect(() => {
    if (inspectT !== null && slotIndexOf(slots, inspectT) === null) setInspect(null);
  }, [slots, inspectT, setInspect]);

  const drag = useRef<{ id: number; x: number; lastX: number; moved: boolean; type: string } | null>(null);

  const local = (e: { clientX: number; clientY: number }) => {
    const r = canvasRef.current!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };

  const announce = (t: number | null) => {
    const idx = t === null ? slots.length - 1 : slotIndexOf(slots, t);
    const slot = idx === null || idx < 0 ? null : slots[idx];
    setAnnouncement(slot ? describeSlot(slot, timeframe).summary : "");
  };

  const onPointerDown = (e: PointerEvent<HTMLCanvasElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    const p = local(e);
    drag.current = { id: e.pointerId, x: p.x, lastX: p.x, moved: false, type: e.pointerType };
  };

  const onPointerMove = (e: PointerEvent<HTMLCanvasElement>) => {
    const p = local(e);
    const d = drag.current;
    if (d && d.id === e.pointerId) {
      if (!d.moved && Math.abs(p.x - d.x) > DRAG_THRESHOLD) d.moved = true;
      if (d.moved) {
        const dx = p.x - d.lastX;
        d.lastX = p.x;
        setVp((v) => pan(v, slots, dx));
        return;
      }
    }
    if (e.pointerType === "mouse") {
      setPointer(p);
      const hit = hitTest(model, p.x, p.y);
      if (hit.slot && hit.slot.t !== inspectT) setInspect(hit.slot.t);
    }
  };

  const onPointerUp = (e: PointerEvent<HTMLCanvasElement>) => {
    const d = drag.current;
    drag.current = null;
    if (!d || d.moved || e.pointerType === "mouse") return;
    // Tap (touch/pen): pin the tapped candle; tapping it again returns to latest.
    const p = local(e);
    const hit = hitTest(model, p.x, p.y);
    const next = hit.slot && hit.slot.t !== inspectT ? hit.slot.t : null;
    setInspect(next);
    announce(next);
  };

  const onPointerLeave = (e: PointerEvent<HTMLCanvasElement>) => {
    if (e.pointerType !== "mouse" || drag.current) return;
    setPointer(null);
    setInspect(null);
  };

  // Wheel must be a non-passive native listener to preventDefault page scroll.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const r = canvas.getBoundingClientRect();
      const plotW = modelRef.current?.geometry.plot.w ?? r.width;
      if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
        const dx = e.shiftKey ? e.deltaY : e.deltaX;
        setVp((v) => pan(v, slots, -dx));
      } else {
        setVp((v) => zoom(v, slots, Math.exp(-e.deltaY * 0.0015), e.clientX - r.left, plotW));
      }
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", onWheel);
  }, [canvasRef, slots, setVp]);

  const onKeyDown = (e: KeyboardEvent<HTMLCanvasElement>) => {
    const plotW = model.geometry.plot.w;
    let next: number | null | undefined;
    switch (e.key) {
      case "ArrowLeft":
        next = stepInspection(slots, inspectT, e.shiftKey ? -10 : -1);
        break;
      case "ArrowRight":
        next = stepInspection(slots, inspectT, e.shiftKey ? 10 : 1);
        break;
      case "Home":
        next = slots[0]?.t ?? null;
        break;
      case "End":
      case "Escape":
        next = null;
        break;
      case "+":
      case "=":
        setVp((v) => zoom(v, slots, 1.25, plotW, plotW));
        break;
      case "-":
      case "_":
        setVp((v) => zoom(v, slots, 0.8, plotW, plotW));
        break;
      case "0":
        setVp(followLatest);
        break;
      default:
        return;
    }
    e.preventDefault();
    if (next === undefined) return;
    setInspect(next);
    announce(next);
    const idx = next === null ? slots.length - 1 : slotIndexOf(slots, next);
    if (idx !== null && idx >= 0) setVp((v) => ensureVisible(v, slots, idx, plotW));
  };

  return (
    <div ref={containerRef} className={cn("relative min-h-0 min-w-0", className)}>
      <canvas
        ref={canvasRef}
        tabIndex={0}
        role="application"
        aria-roledescription="candlestick chart"
        aria-label={`${label ?? "Candlestick chart"}. Arrow keys inspect candles; plus and minus zoom.`}
        className="absolute inset-0 size-full touch-pan-y rounded-md outline-none select-none focus-visible:ring-2 focus-visible:ring-ring"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={() => (drag.current = null)}
        onPointerLeave={onPointerLeave}
        onDoubleClick={() => setVp(followLatest)}
        onKeyDown={onKeyDown}
      />
      <div aria-live="polite" className="sr-only">
        {announcement}
      </div>
    </div>
  );
}
