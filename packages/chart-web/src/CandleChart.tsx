import {
  applyGesture,
  autoScalePrice,
  buildSlots,
  computeChartModel,
  computeIndicators,
  emptyHistory,
  record,
  redo,
  undo,
  type History,
  DEFAULT_LAYERS,
  keyLevels,
  parseLayers,
  type ChartLayers,
  type Halt,
  type LuldPoint,
  DEFAULT_TOOLBOX,
  isColorable,
  parseToolboxPlacement,
  isPositionKind,
  newPosition,
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
  backToLatestMs,
  backToLatestStep,
  followLatest,
  isPannedBack,
  panSlots,
  slotsBehindLatest,
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
  type ToolboxPlacement,
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
import { DrawingToolbox } from "./DrawingToolbox.tsx";
import { OrderTicket } from "./OrderTicket.tsx";
import { PositionPlanCard } from "./PositionPlanCard.tsx";
import { Tip, TooltipProvider } from "./ui/tooltip.tsx";
import { TradeMenu, TRADE_MENU_SIZES } from "./TradeMenu.tsx";
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
  /** Halts and LULD band changes (the store keeps them per symbol). */
  halts?: readonly Halt[];
  luld?: readonly LuldPoint[];
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
  /** Where the floating drawing tools sit. Controlled when onToolboxPlacementChange is set; else kept (and persisted with persistKey). */
  toolboxPlacement?: ToolboxPlacement;
  onToolboxPlacementChange?: (p: ToolboxPlacement) => void;
  /** Which context layers show (halts, LULD, key levels, fills). Controlled when onLayersChange is set; else kept and persisted with persistKey. */
  layers?: ChartLayers;
  onLayersChange?: (layers: ChartLayers) => void;
  /** The feed clock (ms), for an ongoing halt's length. */
  now?: number | null;
  /** Show the timeframe picker in the toolbar row; the host switches the subscription. */
  onTimeframeChange?: (tf: Timeframe) => void;
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
  toolboxPlacement: toolboxProp,
  onToolboxPlacementChange,
  layers: layersProp,
  onLayersChange,
  now = null,
  onTimeframeChange,
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
  // Undo / redo per symbol. Every committed change goes through setDrawings and is recorded;
  // drag previews never are, so one undo undoes one thing the user did.
  const [histories, setHistories] = useState<Record<string, History<Drawing[]>>>({});
  const history = histories[symbol] ?? emptyHistory<Drawing[]>();
  const setDrawings = (next: Drawing[]) => {
    setHistories((all) => ({ ...all, [symbol]: record(all[symbol] ?? emptyHistory<Drawing[]>(), drawings) }));
    writeDrawings(next);
  };
  const stepHistory = (dir: "undo" | "redo") => {
    const r = dir === "undo" ? undo(history, drawings) : redo(history, drawings);
    if (!r) return;
    setHistories((all) => ({ ...all, [symbol]: r.history }));
    setSelected(null);
    writeDrawings(r.state);
  };
  const writeDrawings = (next: Drawing[]) => {
    if (onDrawingsChange) onDrawingsChange(next);
    else
      setOwnDrawings((all) => {
        const out = { ...all, [symbol]: next };
        saveJson(persistKey && `${persistKey}:drawings`, out);
        return out;
      });
  };

  // The floating drawing tools' place: controlled, or kept here and persisted on drop.
  const [ownToolbox, setOwnToolbox] = useState<ToolboxPlacement>(() => parseToolboxPlacement(loadJson<unknown>(persistKey && `${persistKey}:toolbox`, null), DEFAULT_TOOLBOX));
  const [dragToolbox, setDragToolbox] = useState<ToolboxPlacement | null>(null);
  const toolbox = dragToolbox ?? (onToolboxPlacementChange ? (toolboxProp ?? DEFAULT_TOOLBOX) : ownToolbox);
  const moveToolbox = (p: ToolboxPlacement, done: boolean) => {
    if (!done) {
      setDragToolbox(p);
      return;
    }
    setDragToolbox(null);
    if (onToolboxPlacementChange) onToolboxPlacementChange(p);
    else {
      setOwnToolbox(p);
      saveJson(persistKey && `${persistKey}:toolbox`, p);
    }
  };

  const [ownLayers, setOwnLayers] = useState<ChartLayers>(() => parseLayers(loadJson<unknown>(persistKey && `${persistKey}:layers`, null)));
  const layers = onLayersChange ? (layersProp ?? DEFAULT_LAYERS) : ownLayers;
  const setLayers = (next: ChartLayers) => {
    if (onLayersChange) onLayersChange(next);
    else {
      setOwnLayers(next);
      saveJson(persistKey && `${persistKey}:layers`, next);
    }
  };

  // The right-click trade menu: where it opened (client px for the menu, chart px for a position
  // plan dropped from it) and the price at that height, on the tick grid.
  const [tradeMenu, setTradeMenu] = useState<{ open: boolean; at: { x: number; y: number } | null; local: { x: number; y: number }; price: number }>({ open: false, at: null, local: { x: 0, y: 0 }, price: 0 });
  const tradeMenuOpen = tradeMenu.open;
  const tradeMenuAt = tradeMenu.local;
  // The floating drawing tools can be hidden from the toolbar's pencil; the choice persists.
  const [toolsVisible, setToolsVisible] = useState<boolean>(() => loadJson<unknown>(persistKey && `${persistKey}:toolsVisible`, true) !== false);
  const changeToolsVisible = (v: boolean) => {
    setToolsVisible(v);
    saveJson(persistKey && `${persistKey}:toolsVisible`, v);
    // Hiding the tools mid-drawing would leave an invisible tool armed: back to the cursor.
    if (!v) changeTool(null);
  };

  const [tool, setTool] = useState<DrawingKind | null>(null);
  // A measurement is shown until the next press, Escape or a tool change; it is never stored.
  const [measurement, setMeasurement] = useState<Drawing | null>(null);
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
  const levels = useMemo(() => keyLevels(slots), [slots]);
  const fills = useMemo(() => (trading?.state?.fills ?? []).filter((f) => f.symbol === symbol), [trading?.state, symbol]);
  const context = useMemo(
    () => ({
      levels: levels.filter((l) => layers[l.layer]),
      halts: layers.halts ? (series?.halts ?? []) : [],
      luld: layers.luld ? (series?.luld ?? []) : [],
      fills: layers.fills ? fills : [],
      now,
    }),
    [levels, layers, series, fills, now],
  );
  const orders = useMemo(() => (trading?.state?.orders ?? []).filter((o) => o.symbol === symbol && isWorking(o)), [trading?.state, symbol]);
  const position = useMemo(() => trading?.state?.positions.find((p) => p.symbol === symbol) ?? null, [trading?.state, symbol]);
  // The "+" place button follows a hovering mouse (touch opens the ticket from the price axis menu or Trade).
  const placeY = trading && !tool && !drag && pointer && gesturesRef.current?.lastPointerType === "mouse" ? pointer.y : null;
  const shownDrawings = useMemo(() => {
    const list = drag?.drawing ? drawings.map((d) => (d.id === drag.drawing!.id ? drag.drawing! : d)) : drawings;
    return measurement ? [...list, measurement] : list;
  }, [drawings, drag, measurement]);
  const selectedDrawing = selected?.kind === "drawing" ? selected.id : null;

  const state: ChartState = useMemo(
    () => ({
      timeframe,
      candles: series?.candles ?? [],
      gaps: series?.gaps ?? [],
      profile: series?.profile ?? null,
      slots,
      inspectT,
      // While the trade menu is open its price stays marked, even though the mouse has left the canvas for the menu.
      pointer: tradeMenuOpen ? tradeMenuAt : pointer,
      options,
      priceScaleType: scaleType,
      indicators: indicatorResults,
      drawings: shownDrawings,
      selectedDrawing,
      draft,
      trading: trading ? { orders, position, last, dragging: drag?.order ? { id: drag.order.id, price: drag.order.price } : null, placeY } : null,
      context,
      now,
    }),
    [timeframe, series, slots, inspectT, pointer, tradeMenuOpen, tradeMenuAt, options, scaleType, indicatorResults, shownDrawings, selectedDrawing, draft, trading, orders, position, last, drag, placeY, context, now],
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

  const openTicket = (price: number | null, clientY: number | null, clientX?: number) => {
    const r = canvasRef.current!.getBoundingClientRect();
    setTicket({ open: true, at: { x: clientX ?? r.right - 4, y: clientY ?? r.top + 40 }, price });
  };

  // The menu's share size, remembered between uses (and across reloads with persistKey).
  const [tradeQty, setTradeQty] = useState<number>(() => {
    const q = loadJson<unknown>(persistKey && `${persistKey}:tradeQty`, 100);
    return typeof q === "number" && (TRADE_MENU_SIZES as readonly number[]).includes(q) ? q : 100;
  });
  const changeTradeQty = (q: number) => {
    setTradeQty(q);
    saveJson(persistKey && `${persistKey}:tradeQty`, q);
  };
  const orderFromMenu = (side: "buy" | "sell", type: "limit" | "stop" | "market") => {
    if (!trading) return;
    const price = tradeMenu.price;
    trading.onPlace({
      clientOrderId: newId("c"),
      symbol,
      side,
      orderType: type,
      qty: tradeQty,
      ...(type === "limit" ? { limitPrice: price } : type === "stop" ? { stopPrice: price } : {}),
    });
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
          if (a.tool === "measure") {
            setMeasurement(drawingFromPoints(m, "measure", "measure", a.a, a.b));
            setTool(null);
            break;
          }
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
        case "openTradeMenu": {
          if (!m.price) break;
          const r = canvasRef.current!.getBoundingClientRect();
          // The crosshair stays on the clicked price while the menu is open, so the level being traded is visible.
          setPointer({ x: a.x, y: a.y });
          setTradeMenu({ open: true, at: { x: r.left + a.x, y: r.top + a.y }, local: { x: a.x, y: a.y }, price: snapPrice(yPrice(m.price, a.y)) });
          break;
        }
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
  const gestureCtx = (m: ChartModel): GestureContext => ({ model: m, inspectT, priceManual: !auto0, tool, hitObject: hitObject(m), tradeMenu: !!trading });
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

  // Back to latest: slides the view home with an ease-out (instant with
  // reduced motion), then follows the latest candle again. Any press on the
  // chart stops the slide where it is, like catching a scroll.
  const slide = useRef<{ frame: number } | null>(null);
  const slotsRef = useRef(slots);
  slotsRef.current = slots;
  const stopSlide = () => {
    if (slide.current) cancelAnimationFrame(slide.current.frame);
    slide.current = null;
  };
  useEffect(() => stopSlide, []);
  const backToLatest = () => {
    stopSlide();
    // A fling still coasting would fight the slide.
    apply(gestures.reset(), model);
    const total = slotsBehindLatest(vp, slots);
    const reduce = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (reduce || total <= 0) {
      setVp(followLatest);
      return;
    }
    const duration = backToLatestMs(total, vp.barSpacing);
    const start = performance.now();
    let panned = 0;
    const run = { frame: 0 };
    const frame = (now: number) => {
      const elapsed = now - start;
      const d = backToLatestStep(total, panned, elapsed, duration);
      panned -= d;
      if (elapsed >= duration) {
        setVp(followLatest);
        slide.current = null;
        return;
      }
      setVp((v) => panSlots(v, slotsRef.current, d));
      run.frame = requestAnimationFrame(frame);
    };
    run.frame = requestAnimationFrame(frame);
    slide.current = run;
  };
  const pannedBack = isPannedBack(vp);

  const onPointer = (type: "down" | "move" | "up" | "cancel" | "leave") => (e: PointerEvent<HTMLCanvasElement>) => {
    const kind = pointerKind(e.pointerType) ?? "mouse";
    if (type === "down") {
      stopSlide();
      setMeasurement(null);
    }
    if (type === "down" && (kind !== "mouse" || e.button === 0)) e.currentTarget.setPointerCapture(e.pointerId);
    // shiftKey: Shift+drag measures.
    feed(type, e, { pointerId: e.pointerId, pointerType: kind, button: e.button, shiftKey: e.shiftKey });
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
    setMeasurement(null);
    setTool(t);
  };

  const deleteSelected = () => {
    if (selected?.kind !== "drawing") return;
    setDrawings(drawings.filter((d) => d.id !== selected.id));
    setSelected(null);
  };

  const selectedDrawingObj = selected?.kind === "drawing" ? drawings.find((d) => d.id === selected.id) : undefined;
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
    if ((e.ctrlKey || e.metaKey) && !e.altKey && (e.key === "z" || e.key === "Z" || e.key === "y" || e.key === "Y")) {
      e.preventDefault();
      stepHistory(e.key.toLowerCase() === "y" || e.shiftKey ? "redo" : "undo");
      return;
    }
    if (e.key === "Escape" && measurement) {
      e.preventDefault();
      setMeasurement(null);
      return;
    }
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
        backToLatest();
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
      {pannedBack && (
        <Tip label="Back to latest (End)" side="left">
        <button
          type="button"
          aria-label="Back to latest"
          onClick={backToLatest}
          // Bottom-right of the price pane, clear of the price axis and the volume bars.
          style={{ left: model.geometry.plot.x + model.geometry.plot.w - 40, top: model.geometry.plot.y + model.geometry.plot.h - 40 }}
          className="absolute z-10 grid size-8 place-items-center rounded-full bg-card text-foreground shadow-lg transition-[opacity,scale,background-color] duration-150 outline-none hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring starting:scale-90 starting:opacity-0"
        >
          <svg aria-hidden="true" width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
            <path d="M2.5 8h9M8 4.5 11.5 8 8 11.5M13.5 3.5v9" />
          </svg>
        </button>
        </Tip>
      )}
      {toolbar && toolsVisible && (
        <DrawingToolbox
          placement={toolbox}
          onPlacementChange={moveToolbox}
          container={size}
          tool={tool}
          onToolChange={changeTool}
          color={selectedDrawingObj?.color ?? null}
          canColor={!!selectedDrawingObj && isColorable(selectedDrawingObj)}
          onColorChange={(color) => selectedDrawingObj && setDrawings(drawings.map((d) => (d.id === selectedDrawingObj.id ? { ...d, color } : d)))}
          canDelete={selected?.kind === "drawing"}
          onDelete={deleteSelected}
          canUndo={history.past.length > 0}
          canRedo={history.future.length > 0}
          onUndo={() => stepHistory("undo")}
          onRedo={() => stepHistory("redo")}
        />
      )}
      {trading && (
        <TradeMenu
          open={tradeMenu.open}
          onOpenChange={(open) => {
            setTradeMenu((t) => ({ ...t, open }));
            if (!open) setPointer(null);
          }}
          anchor={tradeMenu.at}
          symbol={symbol}
          price={tradeMenu.price}
          last={last}
          qty={tradeQty}
          onQtyChange={changeTradeQty}
          onOrder={orderFromMenu}
          onTicket={() => tradeMenu.at && openTicket(tradeMenu.price, tradeMenu.at.y, tradeMenu.at.x)}
          onPosition={(kind) => {
            const d = newPosition(model, newId("d"), kind, tradeMenu.local);
            if (!d) return;
            setDrawings([...drawings, d]);
            setSelected({ kind: "drawing", id: d.id, part: "body" });
          }}
          finalFocus={canvasRef}
        />
      )}
      {selectedPosition && (
        <PositionPlanCard
          // Beside the drawing tools when they are parked on the left as a column, else in the corner.
          className={cn("absolute top-2 z-10", toolbar && toolsVisible && toolbox.orientation === "vertical" && toolbox.x < 0.5 ? "left-14" : "left-2")}
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
          fills={fills}
          realized={trading.state?.account.realizedPnl ?? 0}
        />
      )}
    </div>
  );

  if (!toolbar) return <TooltipProvider>{canvasEl}</TooltipProvider>;
  return (
    <TooltipProvider>
    <div className={cn("flex min-h-0 min-w-0 flex-col gap-1.5", className)}>
      <ChartToolbar
        toolsVisible={toolsVisible}
        onToolsVisibleChange={changeToolsVisible}
        timeframe={timeframe}
        onTimeframeChange={onTimeframeChange}
        tool={tool}
        onToolChange={changeTool}
        indicators={indicatorSpecs}
        onIndicatorsChange={setIndicators}
        seriesColors={[theme.series1, theme.series2, theme.series3]}
        drawingTools={false}
        canDelete={selected?.kind === "drawing"}
        onDelete={deleteSelected}
        trading={trading ? { position, last, onTrade: () => openTicket(null, null), fills, realized: trading.state?.account.realizedPnl ?? 0 } : null}
        layers={layers}
        onLayersChange={setLayers}
      />
      {canvasEl}
    </div>
    </TooltipProvider>
  );
}
