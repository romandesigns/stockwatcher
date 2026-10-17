import {
  applyGesture,
  autoScalePrice,
  buildSlots,
  computeChartModel,
  defaultViewport,
  describeSlot,
  ensureVisible,
  followLatest,
  GestureController,
  isAutoPrice,
  layoutChart,
  slotIndexOf,
  stepInspection,
  zoom,
  type Candle,
  type ChartModel,
  type ChartOptions,
  type ChartState,
  type ChartTheme,
  type Gap,
  type GestureAction,
  type GestureContext,
  type GestureInput,
  type Point,
  type PointerKind,
  type PriceScaleType,
  type ProfileWire,
  type Size,
  type Timeframe,
  type Viewport,
} from "@stockwatcher/chart-core";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent, type PointerEvent } from "react";
import { drawDisplayList } from "./draw.ts";
import { PRICE_SCALE_LABELS, PriceAxisMenu } from "./PriceAxisMenu.tsx";
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
  /** Price axis scale type. Controlled when onPriceScaleTypeChange is set. */
  priceScaleType?: PriceScaleType;
  onPriceScaleTypeChange?: (type: PriceScaleType) => void;
  /** Initial scale type when uncontrolled (default "linear"). */
  defaultPriceScaleType?: PriceScaleType;
  /** Offer the price axis menu on right-click / long press / Shift+F10 (default true). */
  priceAxisMenu?: boolean;
  options?: Partial<ChartOptions>;
  theme?: ChartTheme;
  /** Initial pixels per candle. */
  barSpacing?: number;
  className?: string;
  /** Accessible name, e.g. "SMPL 1 minute candles". */
  label?: string;
}

const pointerKind = (t: string | undefined): PointerKind | null => (t === "mouse" || t === "touch" || t === "pen" ? t : null);

/**
 * Canvas candlestick chart: candles, volume, gaps, axes, last-price tag,
 * optional profile overlay, with Robinhood-style gestures recognized by
 * chart-core's GestureController (the behaviour list lives there):
 * - touch: tap pins a candle, long press scrubs, horizontal swipe pans,
 *   vertical swipe scrolls the page, pinch zooms time, drag the price axis to
 *   scale price, double-tap auto-scales, long-press the price axis for its menu;
 * - mouse: hover inspects, drag pans, wheel zooms (Ctrl+wheel / trackpad
 *   pinch too), drag or wheel the price axis to scale price, double-click
 *   auto-scales (on the time axis: back to latest), right-click the price
 *   axis for its menu.
 * Keyboard (when focused): ←/→ step (Shift ×10), Home/End, Esc = latest,
 * +/− zoom, 0 = follow latest, A = auto-scale price, Shift+F10 or the Menu
 * key = price axis menu. Taps, keyboard inspection and scale changes are
 * announced through a polite live region; hover and scrub are not, to avoid
 * chatter.
 */
export function CandleChart({
  series,
  timeframe,
  inspectT: inspectProp,
  onInspectChange,
  priceScaleType: scaleProp,
  onPriceScaleTypeChange,
  defaultPriceScaleType = "linear",
  priceAxisMenu = true,
  options,
  theme: themeProp,
  barSpacing = 9,
  className,
  label,
}: CandleChartProps) {
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

  const [ownScale, setOwnScale] = useState<PriceScaleType>(defaultPriceScaleType);
  const scaleControlled = onPriceScaleTypeChange !== undefined;
  const scaleType = scaleControlled ? (scaleProp ?? "linear") : ownScale;

  // Each timeframe gets a fresh viewport: zoom/pan positions (and a manual
  // price range) do not carry across timeframes whose slots mean different
  // durations. The scale TYPE is a preference and does carry.
  const [view, setView] = useState<{ tf: Timeframe; vp: Viewport }>(() => ({ tf: timeframe, vp: defaultViewport(barSpacing) }));
  const vp = view.tf === timeframe ? view.vp : defaultViewport(barSpacing);
  const setVp = useCallback((f: (vp: Viewport) => Viewport) => setView((v) => ({ tf: timeframe, vp: f(v.tf === timeframe ? v.vp : defaultViewport(barSpacing)) })), [timeframe, barSpacing]);

  const [pointer, setPointer] = useState<Point | null>(null);
  const [announcement, setAnnouncement] = useState("");
  // The anchor outlives `open` so the menu can animate out where it opened.
  const [menu, setMenu] = useState<{ open: boolean; at: { x: number; y: number } | null }>({ open: false, at: null });

  const slots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);

  const state: ChartState = useMemo(
    () => ({ timeframe, candles: series?.candles ?? [], gaps: series?.gaps ?? [], profile: series?.profile ?? null, slots, inspectT, pointer, options, priceScaleType: scaleType }),
    [timeframe, series, slots, inspectT, pointer, options, scaleType],
  );

  const paint = useCallback((ctx: CanvasRenderingContext2D, size: Size) => drawDisplayList(ctx, layoutChart(computeChartModel(state, vp, size), state, theme)), [state, vp, theme]);
  const { containerRef, canvasRef, size } = useCanvas(paint);
  const model = useMemo(() => computeChartModel(state, vp, size), [state, vp, size]);

  // If the inspected slot disappears (trimmed, or a new snapshot), fall back to latest.
  useEffect(() => {
    if (inspectT !== null && slotIndexOf(slots, inspectT) === null) setInspect(null);
  }, [slots, inspectT, setInspect]);

  const announce = (t: number | null) => {
    const idx = t === null ? slots.length - 1 : slotIndexOf(slots, t);
    const slot = idx === null || idx < 0 ? null : slots[idx];
    setAnnouncement(slot ? describeSlot(slot, timeframe).summary : "");
  };

  const setScaleType = (t: PriceScaleType) => {
    if (scaleControlled) onPriceScaleTypeChange?.(t);
    else setOwnScale(t);
  };

  // Announce scale changes (type and auto/manual), never on first render.
  const auto = isAutoPrice(vp);
  const announced = useRef<{ auto: boolean; type: PriceScaleType } | null>(null);
  useEffect(() => {
    const prev = announced.current;
    announced.current = { auto, type: scaleType };
    if (!prev) return;
    if (prev.type !== scaleType) setAnnouncement(`Price axis: ${PRICE_SCALE_LABELS[scaleType]}.`);
    else if (prev.auto && !auto) setAnnouncement("Price axis scaled manually. Press A, double-tap or double-click to auto-scale.");
    else if (!prev.auto && auto) setAnnouncement("Price axis auto-scaled.");
  }, [auto, scaleType]);

  const local = (e: { clientX: number; clientY: number }) => {
    const r = canvasRef.current!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };

  const openMenuAt = (x: number, y: number) => {
    if (!priceAxisMenu) return;
    const r = canvasRef.current!.getBoundingClientRect();
    setPointer(null);
    setMenu({ open: true, at: { x: r.left + x, y: r.top + y } });
  };

  const apply = (actions: readonly GestureAction[], m: ChartModel) => {
    for (const a of actions) {
      switch (a.kind) {
        case "inspect":
          setInspect(a.t);
          if (a.announce) announce(a.t);
          break;
        case "crosshair":
          setPointer(a.point);
          break;
        case "openAxisMenu":
          // A touch long press opened it: the finger is still down, and the
          // compatibility mousedown the browser sends after it lifts would
          // count as an outside press and close the menu at once.
          if (gestures.lastPointerType !== "mouse") suppressCompatMouse.current = true;
          openMenuAt(a.x, a.y);
          break;
        case "release":
          // The browser scrolls the page (touch-action: pan-y); nothing to do.
          break;
        default:
          setVp((v) => applyGesture(v, a, m));
      }
    }
  };

  // The recognizer is pure; this component only feeds it events and runs
  // its actions. `latest` gives native listeners and the tick loop the
  // current render's model, inspection and handlers.
  const [gestures] = useState(() => new GestureController());
  const suppressCompatMouse = useRef(false);
  const latest = useRef<{ ctx: GestureContext; apply: typeof apply } | null>(null);
  useLayoutEffect(() => {
    latest.current = { ctx: { model, inspectT }, apply };
  });

  // Long presses and deferred taps are decided by tick(); run it once per
  // frame only while the controller has a deadline pending.
  const tickFrame = useRef(0);
  const pump = useCallback(() => {
    if (tickFrame.current || gestures.nextDeadline() === null) return;
    const loop = () => {
      tickFrame.current = 0;
      const l = latest.current;
      if (!l) return;
      l.apply(gestures.tick(performance.now(), l.ctx), l.ctx.model);
      if (gestures.nextDeadline() !== null) tickFrame.current = requestAnimationFrame(loop);
    };
    tickFrame.current = requestAnimationFrame(loop);
  }, [gestures]);
  useEffect(
    () => () => {
      cancelAnimationFrame(tickFrame.current);
      tickFrame.current = 0;
    },
    [],
  );

  const feed = (type: GestureInput["type"], e: { clientX: number; clientY: number; timeStamp: number }, extra: Partial<GestureInput> & { pointerType: PointerKind; pointerId?: number }) => {
    const p = local(e);
    const ctx = { model, inspectT };
    apply(gestures.handle({ type, pointerId: 0, x: p.x, y: p.y, t: e.timeStamp, ...extra }, ctx), model);
    pump();
  };

  const onPointer = (type: "down" | "move" | "up" | "cancel" | "leave") => (e: PointerEvent<HTMLCanvasElement>) => {
    const kind = pointerKind(e.pointerType) ?? "mouse";
    if (type === "down" && (kind !== "mouse" || e.button === 0)) e.currentTarget.setPointerCapture(e.pointerId);
    feed(type, e, { pointerId: e.pointerId, pointerType: kind, button: e.button });
  };

  const onDoubleClick = (e: MouseEvent<HTMLCanvasElement>) => feed("dblclick", e, { pointerType: gestures.lastPointerType });

  const onContextMenu = (e: MouseEvent<HTMLCanvasElement>) => {
    // contextmenu is a PointerEvent in current browsers; fall back to the last press's type.
    const kind = pointerKind((e.nativeEvent as { pointerType?: string }).pointerType) ?? gestures.lastPointerType;
    const p = local(e);
    const acts = gestures.handle({ type: "contextmenu", pointerId: 0, pointerType: kind, x: p.x, y: p.y, t: e.timeStamp }, { model, inspectT });
    // Suppress the browser's menu on the price axis (ours opens) and for every
    // touch long press (the chart's own long press is in charge); elsewhere a
    // mouse right-click keeps the browser's menu.
    if (acts.length || kind !== "mouse") e.preventDefault();
    apply(acts, model);
  };

  // Wheel must be a non-passive native listener to preventDefault page
  // scroll (and the browser zoom a Ctrl+wheel / trackpad pinch would do).
  // touchmove likewise: preventDefault only while the chart owns the touch
  // (pan, scrub, pinch, price-axis drag), so a vertical swipe still scrolls.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const onWheel = (e: WheelEvent) => {
      const l = latest.current;
      if (!l) return;
      e.preventDefault();
      const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? l.ctx.model.geometry.plot.h || 400 : 1;
      const r = canvas.getBoundingClientRect();
      const acts = gestures.handle(
        { type: "wheel", pointerId: 0, pointerType: "mouse", x: e.clientX - r.left, y: e.clientY - r.top, t: e.timeStamp, deltaX: e.deltaX * unit, deltaY: e.deltaY * unit, ctrlKey: e.ctrlKey, shiftKey: e.shiftKey },
        l.ctx,
      );
      l.apply(acts, l.ctx.model);
    };
    const onTouchMove = (e: TouchEvent) => {
      if (gestures.capturing && e.cancelable) e.preventDefault();
    };
    // preventDefault on touchend cancels the compatibility mouse events
    // (mousedown/mouseup/click) for that touch; see openAxisMenu above.
    const onTouchEnd = (e: TouchEvent) => {
      if (!suppressCompatMouse.current || e.touches.length) return;
      suppressCompatMouse.current = false;
      if (e.cancelable) e.preventDefault();
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    canvas.addEventListener("touchmove", onTouchMove, { passive: false });
    canvas.addEventListener("touchend", onTouchEnd, { passive: false });
    return () => {
      canvas.removeEventListener("wheel", onWheel);
      canvas.removeEventListener("touchmove", onTouchMove);
      canvas.removeEventListener("touchend", onTouchEnd);
    };
  }, [canvasRef, gestures]);

  const onKeyDown = (e: KeyboardEvent<HTMLCanvasElement>) => {
    const plotW = model.geometry.plot.w;
    let next: number | null | undefined;
    if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
      e.preventDefault();
      const g = model.geometry;
      openMenuAt(g.priceAxis.x + g.priceAxis.w / 2, g.plot.y + g.plot.h / 2);
      return;
    }
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
      case "a":
      case "A":
        if (e.ctrlKey || e.metaKey || e.altKey) return;
        if (auto) setAnnouncement("Price axis auto-scaled.");
        setVp(autoScalePrice);
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
        aria-label={`${label ?? "Candlestick chart"}. Arrow keys inspect candles; plus and minus zoom; A auto-scales price${priceAxisMenu ? "; Shift+F10 opens the price axis menu" : ""}. Price axis: ${PRICE_SCALE_LABELS[scaleType]}${auto ? ", auto-scaled" : ", manual"}.`}
        data-price-scale={scaleType}
        data-price-range={auto ? "auto" : "manual"}
        // touch-pan-y: the browser keeps vertical page scroll and hands us
        // horizontal pans and pinches; no callout or selection on long press.
        className="absolute inset-0 size-full touch-pan-y rounded-md outline-none select-none [-webkit-touch-callout:none] focus-visible:ring-2 focus-visible:ring-ring"
        onPointerDown={onPointer("down")}
        onPointerMove={onPointer("move")}
        onPointerUp={onPointer("up")}
        onPointerCancel={onPointer("cancel")}
        onPointerLeave={onPointer("leave")}
        onDoubleClick={onDoubleClick}
        onContextMenu={onContextMenu}
        onKeyDown={onKeyDown}
      />
      <div aria-live="polite" className="sr-only">
        {announcement}
      </div>
      {priceAxisMenu && (
        <PriceAxisMenu
          open={menu.open}
          onOpenChange={(open) => setMenu((m) => ({ ...m, open }))}
          anchor={menu.at}
          scaleType={scaleType}
          onScaleTypeChange={setScaleType}
          autoScaled={auto}
          onAutoScale={() => {
            if (auto) setAnnouncement("Price axis auto-scaled.");
            setVp(autoScalePrice);
          }}
          finalFocus={canvasRef}
        />
      )}
    </div>
  );
}
