import {
  applyGesture,
  autoScalePrice,
  buildSlots,
  computeChartModel,
  computeIndicators,
  isPositionKind,
  positionPlan,
  DRAWING_HIT_PX,
  dragDrawing,
  drawingFromPoints,
  hitDrawing,
  hitTrading,
  isWorking,
  lastCandle,
  orderLinePrice,
  priceY,
  snapPrice,
  yPrice,
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
  type Drawing,
  type DrawingDraft,
  type DrawingKind,
  type IndicatorSpec,
  type ObjectHit,
  type OrderModify,
  type OrderPlace,
  type PaperState,
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
import { ChartToolbar } from "./ChartToolbar.tsx";
import { drawDisplayList } from "./draw.ts";
import { OrderTicket } from "./OrderTicket.tsx";
import { PositionPlanCard } from "./PositionPlanCard.tsx";
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
  /** The symbol shown: drawings are kept per symbol, and trading filters orders by it. */
  symbol?: string;
  /** Show the chart's toolbar: drawing tools, indicators, delete, and Trade when trading is on. */
  toolbar?: boolean;
  /** Indicators on the chart. Controlled when onIndicatorsChange is set. */
  indicators?: IndicatorSpec[];
  onIndicatorsChange?: (specs: IndicatorSpec[]) => void;
  /** This symbol's drawings. Controlled when onDrawingsChange is set. */
  drawings?: Drawing[];
  onDrawingsChange?: (drawings: Drawing[]) => void;
  /** Keep uncontrolled indicators and drawings in localStorage under this key. */
  persistKey?: string;
  /** Simulated trading: order and position lines, drag to modify, cancel, the ticket. Absent = off. */
  trading?: ChartTrading | null;
}

/** What the host wires for simulated trading (the chart never talks to a server). */
export interface ChartTrading {
  state: PaperState | null;
  onPlace: (order: Omit<OrderPlace, "type">) => void;
  onModify: (change: Omit<OrderModify, "type">) => void;
  onCancel: (id: string) => void;
}

const loadJson = <T,>(key: string | undefined, fallback: T): T => {
  if (!key) return fallback;
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
};
const saveJson = (key: string | undefined, value: unknown) => {
  if (!key) return;
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Private mode or blocked storage: the chart still works, it just forgets.
  }
};

let idSeq = 0;
const newId = (prefix: string) => `${prefix}-${Date.now().toString(36)}-${(idSeq++).toString(36)}`;

type DragState = { target: ObjectHit; drawing?: Drawing; order?: { id: string; price: number; original: number; field: "limitPrice" | "stopPrice" } };

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
 * key = price axis menu, L / S = long / short position tool, Delete = remove
 * the selected drawing (Esc first
 * abandons a drawing being placed, then clears the selection).
 *
 * Everything chart-related lives here so any screen reuses it whole: the
 * toolbar (toolbar prop), indicators, drawings (kept per symbol), and
 * simulated trading (trading prop): order and position lines, drag a pill
 * to modify, ✕ to cancel, "+" at the hover price or Trade for the ticket. Taps, keyboard inspection and scale changes are
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
  symbol = "",
  toolbar = false,
  indicators: indicatorsProp,
  onIndicatorsChange,
  drawings: drawingsProp,
  onDrawingsChange,
  persistKey,
  trading,
}: CandleChartProps) {
  const theme = useChartTheme(themeProp);

  // Indicators: controlled, or kept here (and persisted with persistKey).
  const [ownIndicators, setOwnIndicators] = useState<IndicatorSpec[]>(() => loadJson(persistKey && `${persistKey}:indicators`, []));
  const indicatorSpecs = onIndicatorsChange ? (indicatorsProp ?? []) : ownIndicators;
  const setIndicators = (next: IndicatorSpec[]) => {
    if (onIndicatorsChange) onIndicatorsChange(next);
    else {
      setOwnIndicators(next);
      saveJson(persistKey && `${persistKey}:indicators`, next);
    }
  };

  // Drawings: controlled for this symbol, or kept here per symbol.
  const [ownDrawings, setOwnDrawings] = useState<Record<string, Drawing[]>>(() => loadJson(persistKey && `${persistKey}:drawings`, {}));
  const drawings = onDrawingsChange ? (drawingsProp ?? []) : (ownDrawings[symbol] ?? []);
  const setDrawings = (next: Drawing[]) => {
    if (onDrawingsChange) onDrawingsChange(next);
    else
      setOwnDrawings((all) => {
        const out = { ...all, [symbol]: next };
        saveJson(persistKey && `${persistKey}:drawings`, out);
        return out;
      });
  };

  const [tool, setTool] = useState<DrawingKind | null>(null);
  const [selected, setSelected] = useState<ObjectHit | null>(null);
  const [draft, setDraft] = useState<DrawingDraft | null>(null);
  const [drag, setDrag] = useState<DragState | null>(null);
  const [ticket, setTicket] = useState<{ open: boolean; at: { x: number; y: number } | null; price: number | null }>({ open: false, at: null, price: null });
  const [cursor, setCursor] = useState<string | undefined>(undefined);
  const gesturesRef = useRef<GestureController | null>(null);
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
  const indicatorResults = useMemo(() => computeIndicators(slots, indicatorSpecs), [slots, indicatorSpecs]);

  const last = useMemo(() => lastCandle(slots)?.c ?? null, [slots]);
  const orders = useMemo(() => (trading?.state?.orders ?? []).filter((o) => o.symbol === symbol && isWorking(o)), [trading?.state, symbol]);
  const position = useMemo(() => trading?.state?.positions.find((p) => p.symbol === symbol) ?? null, [trading?.state, symbol]);
  // The "+" place button follows a hovering mouse (touch opens the ticket from the price axis menu or Trade).
  const placeY = trading && !tool && !drag && pointer && gesturesRef.current?.lastPointerType === "mouse" ? pointer.y : null;
  const shownDrawings = useMemo(() => (drag?.drawing ? drawings.map((d) => (d.id === drag.drawing!.id ? drag.drawing! : d)) : drawings), [drawings, drag]);
  const selectedDrawing = selected?.kind === "drawing" ? selected.id : null;

  const state: ChartState = useMemo(
    () => ({
      timeframe,
      candles: series?.candles ?? [],
      gaps: series?.gaps ?? [],
      profile: series?.profile ?? null,
      slots,
      inspectT,
      pointer,
      options,
      priceScaleType: scaleType,
      indicators: indicatorResults,
      drawings: shownDrawings,
      selectedDrawing,
      draft,
      trading: trading ? { orders, position, last, dragging: drag?.order ? { id: drag.order.id, price: drag.order.price } : null, placeY } : null,
    }),
    [timeframe, series, slots, inspectT, pointer, options, scaleType, indicatorResults, shownDrawings, selectedDrawing, draft, trading, orders, position, last, drag, placeY],
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

  const tradingLayer = state.trading ?? null;
  const hitObject = (m: ChartModel) => (x: number, y: number, touch: boolean): ObjectHit | null => {
    const tol = touch ? DRAWING_HIT_PX.touch : DRAWING_HIT_PX.mouse;
    if (tradingLayer) {
      const t = hitTrading(m, tradingLayer, x, y, tol);
      if (t) return t;
    }
    const d = hitDrawing(m, drawings, x, y, tol, selectedDrawing);
    return d ? { kind: "drawing", ...d } : null;
  };

  const openTicket = (price: number | null, clientY: number | null) => {
    const r = canvasRef.current!.getBoundingClientRect();
    setTicket({ open: true, at: { x: r.right - 4, y: clientY ?? r.top + 40 }, price });
  };

  const onObjectDrag = (a: Extract<GestureAction, { kind: "objectDrag" }>, m: ChartModel) => {
    const t = a.target;
    if (t.kind === "drawing") {
      const original = drawings.find((d) => d.id === t.id);
      if (!original) return;
      const moved = dragDrawing(m, original, t.part, a.from, a.to);
      if (a.done) {
        setDrag(null);
        setDrawings(drawings.map((d) => (d.id === t.id ? moved : d)));
        setSelected(t);
      } else setDrag({ target: t, drawing: moved });
      return;
    }
    if (t.kind !== "order" || !m.price) return;
    const o = orders.find((x) => x.id === t.id);
    const line = o && orderLinePrice(o);
    if (!o || !line) return;
    const price = snapPrice(yPrice(m.price, priceY(m.price, line.price) + (a.to.y - a.from.y)));
    if (!a.done) {
      setDrag({ target: t, order: { id: o.id, price, original: line.price, field: line.field } });
      return;
    }
    setDrag(null);
    if (price !== line.price && price > 0) trading?.onModify({ id: o.id, [line.field]: price });
  };

  const apply = (actions: readonly GestureAction[], m: ChartModel) => {
    for (const a of actions) {
      switch (a.kind) {
        case "draft":
          setDraft(a.draft);
          break;
        case "createDrawing": {
          const d = drawingFromPoints(m, newId("d"), a.tool, a.a, a.b);
          if (d) {
            setDrawings([...drawings, d]);
            setSelected({ kind: "drawing", id: d.id, part: "body" });
          }
          // Back to the cursor after each drawing, as most charting tools do.
          setTool(null);
          break;
        }
        case "select":
          setSelected(a.target);
          break;
        case "objectDrag":
          onObjectDrag(a, m);
          break;
        case "cancelOrder":
          trading?.onCancel(a.id);
          break;
        case "placeOrder": {
          if (!m.price) break;
          const r = canvasRef.current!.getBoundingClientRect();
          openTicket(snapPrice(yPrice(m.price, a.y)), r.top + a.y);
          break;
        }
        case "haptic":
          // No haptic engine worth using on the web.
          break;
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
  const [gestures] = useState(() => new GestureController({ momentum: true, touchPricePan: "manual" }));
  gesturesRef.current = gestures;
  const suppressCompatMouse = useRef(false);
  const latest = useRef<{ ctx: GestureContext; apply: typeof apply } | null>(null);
  const auto0 = isAutoPrice(vp);
  const gestureCtx = (m: ChartModel): GestureContext => ({ model: m, inspectT, priceManual: !auto0, tool, hitObject: hitObject(m) });
  useLayoutEffect(() => {
    latest.current = { ctx: gestureCtx(model), apply };
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
    const ctx = gestureCtx(model);
    apply(gestures.handle({ type, pointerId: 0, x: p.x, y: p.y, t: e.timeStamp, ...extra }, ctx), model);
    pump();
    // Mouse cursor: what a press here would do.
    if (extra.pointerType === "mouse" && (type === "move" || type === "down" || type === "up")) {
      const hit = ctx.hitObject?.(p.x, p.y, false) ?? null;
      setCursor(
        gestures.state === "drag"
          ? "grabbing"
          : tool
            ? "crosshair"
            : hit?.kind === "order"
              ? hit.part === "cancel"
                ? "pointer"
                : "ns-resize"
              : hit?.kind === "place"
                ? "pointer"
                : hit?.kind === "drawing"
                  ? hit.part === "body"
                    ? "move"
                    : "grab"
                  : undefined,
      );
    }
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
    const acts = gestures.handle({ type: "contextmenu", pointerId: 0, pointerType: kind, x: p.x, y: p.y, t: e.timeStamp }, gestureCtx(model));
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

  const changeTool = (t: DrawingKind | null) => {
    apply(gestures.cancelPlacing(), model);
    setDraft(null);
    setTool(t);
  };

  const deleteSelected = () => {
    if (selected?.kind !== "drawing") return;
    setDrawings(drawings.filter((d) => d.id !== selected.id));
    setSelected(null);
  };

  // The selected position tool, if any: its plan card shows over the chart.
  const selectedPosition = selected?.kind === "drawing" ? drawings.find((d) => d.id === selected.id && isPositionKind(d.kind)) : undefined;
  const placeBracket = (d: Drawing) => {
    const plan = positionPlan(d);
    if (!trading || !plan.valid) return;
    trading.onPlace({
      clientOrderId: newId("c"),
      symbol,
      side: plan.side === "long" ? "buy" : "sell",
      orderType: "limit",
      qty: plan.qty,
      limitPrice: plan.entry,
      bracket: { takeProfit: plan.target, stopLoss: plan.stop },
    });
    // The plan became orders: their lines replace the zones.
    setDrawings(drawings.filter((x) => x.id !== d.id));
    setSelected(null);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLCanvasElement>) => {
    const plotW = model.geometry.plot.w;
    let next: number | null | undefined;
    // Escape unwinds one layer at a time: a drawing being placed, the tool, the selection, then inspection.
    if (e.key === "Escape" && (gestures.isPlacing || tool || selected)) {
      e.preventDefault();
      if (gestures.isPlacing) apply(gestures.cancelPlacing(), model);
      else if (tool) changeTool(null);
      else setSelected(null);
      return;
    }
    // L / S pick the long / short position tool (TradingView's habit of one-key tools).
    if ((e.key === "l" || e.key === "L" || e.key === "s" || e.key === "S") && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      changeTool(e.key.toLowerCase() === "l" ? "long" : "short");
      return;
    }
    if ((e.key === "Delete" || e.key === "Backspace") && selected?.kind === "drawing") {
      e.preventDefault();
      deleteSelected();
      return;
    }
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

  const canvasEl = (
    <div ref={containerRef} className={cn("relative min-h-0 min-w-0", toolbar ? "flex-1" : className)}>
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
        // With a manual price range or a drawing tool, vertical finger moves
        // belong to the chart (price pan, drawing), so touch-action is none.
        className={cn(
          "absolute inset-0 size-full rounded-md outline-none select-none [-webkit-touch-callout:none] focus-visible:ring-2 focus-visible:ring-ring",
          auto && !tool ? "touch-pan-y" : "touch-none",
        )}
        style={cursor ? { cursor } : undefined}
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
          trade={
            trading && model.price && menu.at
              ? {
                  onBuyAt: () => openTicket(snapPrice(yPrice(model.price!, menu.at!.y - canvasRef.current!.getBoundingClientRect().top)), menu.at!.y),
                }
              : undefined
          }
        />
      )}
      {selectedPosition && (
        <PositionPlanCard
          className="absolute top-2 left-2 z-10"
          drawing={selectedPosition}
          onChange={(d) => setDrawings(drawings.map((x) => (x.id === d.id ? d : x)))}
          onPlace={trading ? () => placeBracket(selectedPosition) : undefined}
        />
      )}
      {trading && (
        <OrderTicket
          open={ticket.open}
          onOpenChange={(open) => setTicket((t) => ({ ...t, open }))}
          anchor={ticket.at}
          symbol={symbol}
          last={last}
          price={ticket.price}
          onSubmit={(o) => trading.onPlace({ ...o, clientOrderId: newId("c") })}
          finalFocus={canvasRef}
        />
      )}
    </div>
  );

  if (!toolbar) return canvasEl;
  return (
    <div className={cn("flex min-h-0 min-w-0 flex-col gap-1.5", className)}>
      <ChartToolbar
        tool={tool}
        onToolChange={changeTool}
        indicators={indicatorSpecs}
        onIndicatorsChange={setIndicators}
        canDelete={selected?.kind === "drawing"}
        onDelete={deleteSelected}
        trading={trading ? { position, last, onTrade: () => openTicket(null, null) } : null}
      />
      {canvasEl}
    </div>
  );
}
