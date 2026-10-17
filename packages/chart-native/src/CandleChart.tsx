import { Canvas, Picture } from "@shopify/react-native-skia";
import {
  applyGesture,
  emptyHistory,
  record,
  redo,
  undo,
  type History,
  DEFAULT_LAYERS,
  layerIdsFor,
  applyPaneGesture,
  paneIdsOf,
  keyLevels,
  type ChartLayers,
  type Halt,
  type LuldPoint,
  DEFAULT_TOOLBOX_PHONE,
  isColorable,
  backToLatestMs,
  backToLatestStep,
  followLatest,
  isPannedBack,
  panSlots,
  slotsBehindLatest,
  autoScalePrice,
  buildSlots,
  computeChartModel,
  computeIndicators,
  DEFAULT_CHART_THEME,
  defaultViewport,
  describeSlot,
  DRAWING_HIT_PX,
  dragDrawing,
  drawingFromPoints,
  GestureController,
  hitDrawing,
  hitTrading,
  isAutoPrice,
  isPositionKind,
  positionPlan,
  isWorking,
  lastCandle,
  layoutChart,
  orderLinePrice,
  priceY,
  slotIndexOf,
  snapPrice,
  newPosition,
  stepInspection,
  unrealizedPnl,
  yPrice,
  formatPrice,
  formatSize,
  type Candle,
  type ChartModel,
  type ChartOptions,
  type ChartState,
  type ChartTheme,
  type Drawing,
  type DrawingDraft,
  type DrawingKind,
  type Gap,
  type GestureAction,
  type GestureContext,
  type GestureInput,
  type IndicatorSpec,
  type ObjectHit,
  type OrderModify,
  type OrderPlace,
  type PaperState,
  type ToolboxPlacement,
  type Point,
  type PriceScaleType,
  type ProfileWire,
  type Size,
  type Timeframe,
  type Viewport,
} from "@stockwatcher/chart-core";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { AccessibilityInfo, Pressable, StyleSheet, Text, View, type LayoutChangeEvent, type StyleProp, type ViewStyle } from "react-native";
import { GestureDetector, GestureStateManager, useManualGesture } from "react-native-gesture-handler";
import Animated, { FadeIn, FadeOut, ReduceMotion } from "react-native-reanimated";
import Svg, { Path } from "react-native-svg";
import { recordPicture } from "./drawSkia.ts";
import { DrawingToolbox } from "./DrawingToolbox.tsx";
import { PositionPlanPanel } from "./PositionPlanPanel.tsx";
import { AxisSheet, IndicatorSheet, LayersSheet, OrderTicketSheet, QUICK_TRADE_SIZES, QuickTradeSheet, SCALE_LABELS } from "./sheets.tsx";
import { TimeframeWheel } from "./TimeframeWheel.tsx";
import { playHaptic, ToolButton, ToolIcon, UI } from "./ui.tsx";

export interface ChartSeriesLike {
  candles: readonly Candle[];
  gaps: readonly Gap[];
  profile: ProfileWire | null;
  halts?: readonly Halt[];
  luld?: readonly LuldPoint[];
}

/** What the host wires for simulated trading (the chart never talks to a server). */
export interface ChartTrading {
  state: PaperState | null;
  onPlace: (order: Omit<OrderPlace, "type">) => void;
  onModify: (change: Omit<OrderModify, "type">) => void;
  onCancel: (id: string) => void;
}

export interface CandleChartProps {
  series: ChartSeriesLike | null | undefined;
  timeframe: Timeframe;
  inspectT?: number | null;
  onInspectChange?: (t: number | null) => void;
  priceScaleType?: PriceScaleType;
  onPriceScaleTypeChange?: (t: PriceScaleType) => void;
  options?: Partial<ChartOptions>;
  theme?: ChartTheme;
  barSpacing?: number;
  /** Drawings are kept per symbol; trading filters orders by it. */
  symbol?: string;
  /** Show the toolbar: drawing tools, delete, indicators, and Trade when trading is on. */
  toolbar?: boolean;
  indicators?: IndicatorSpec[];
  onIndicatorsChange?: (specs: IndicatorSpec[]) => void;
  drawings?: Drawing[];
  onDrawingsChange?: (drawings: Drawing[]) => void;
  trading?: ChartTrading | null;
  /** Where the floating drawing tools sit. Controlled when onToolboxPlacementChange is set (the host persists it); else kept for the session. */
  toolboxPlacement?: ToolboxPlacement;
  onToolboxPlacementChange?: (p: ToolboxPlacement) => void;
  /** Whether the floating drawing tools show. Controlled when onToolsVisibleChange is set (the host persists it). */
  toolsVisible?: boolean;
  onToolsVisibleChange?: (visible: boolean) => void;
  /** Which context layers show (halts, LULD, key levels, fills). Controlled when onLayersChange is set (the host persists it). */
  layers?: ChartLayers;
  onLayersChange?: (layers: ChartLayers) => void;
  /** The feed clock (ms), for an ongoing halt's length. */
  now?: number | null;
  /** Show the timeframe picker in the toolbar row; the host switches the subscription. */
  onTimeframeChange?: (tf: Timeframe) => void;
  /** Oscillator pane heights by indicator id. Controlled when onPaneHeightsChange is set (the host persists it); else kept for the session. */
  paneHeights?: Readonly<Record<string, number>>;
  onPaneHeightsChange?: (heights: Record<string, number>) => void;
  /** The session volume profile's bin widths; with onProfileBinChange the Levels sheet offers them (the host resubscribes). */
  profileBins?: readonly number[];
  profileBin?: number;
  onProfileBinChange?: (bin: number) => void;
  /** Haptic feedback on scrub, grabs, placements and fills (default true). */
  haptics?: boolean;
  label?: string;
  style?: StyleProp<ViewStyle>;
}

let idSeq = 0;
const newId = (prefix: string) => `${prefix}-${Date.now().toString(36)}-${(idSeq++).toString(36)}`;
const now = () => performance.now();

type DragState = { drawing?: Drawing; order?: { id: string; price: number } };

/**
 * The native candlestick chart: chart-core's layout drawn with Skia, and
 * chart-core's GestureController fed from Gesture Handler touches, so the
 * phone recognizes exactly what the web does (docs/research/
 * robinhood-chart-gestures.md), plus what only a phone can do:
 * - haptics on entering a scrub, a tick per candle while scrubbing, a bump
 *   when grabbing an order or a drawing, and a success tap on each fill;
 * - momentum: a fast horizontal swipe keeps panning and decays;
 * - vertical price panning: this chart's screen does not scroll, so a
 *   mostly vertical swipe pans price (and makes the range manual; a double
 *   tap returns to auto). A mostly horizontal swipe never touches price
 *   while it is auto, so jitter cannot knock it out of auto.
 * Everything else is the web's: tap pins a candle, long press scrubs, pinch
 * zooms time, drag the price axis to scale it, long-press the price axis for
 * its menu (auto-scale, scale type, an order at that price), double-tap to
 * auto-scale; drawings, indicators and simulated orders as on the web.
 */
export function CandleChart({
  series,
  timeframe,
  inspectT: inspectProp,
  onInspectChange,
  priceScaleType: scaleProp,
  onPriceScaleTypeChange,
  options,
  theme = DEFAULT_CHART_THEME,
  barSpacing = 9,
  symbol = "",
  toolbar = false,
  indicators: indicatorsProp,
  onIndicatorsChange,
  drawings: drawingsProp,
  onDrawingsChange,
  trading,
  haptics = true,
  toolboxPlacement: toolboxProp,
  onToolboxPlacementChange,
  layers: layersProp,
  onLayersChange,
  toolsVisible: toolsVisibleProp,
  onToolsVisibleChange,
  // Renamed: this module's now() is the monotonic clock the gestures use.
  now: clockNow = null,
  onTimeframeChange,
  profileBins,
  profileBin,
  onProfileBinChange,
  paneHeights: paneHeightsProp,
  onPaneHeightsChange,
  label,
  style,
}: CandleChartProps) {
  const [size, setSize] = useState<Size>({ width: 0, height: 0 });
  const onLayout = (e: LayoutChangeEvent) => {
    const { width, height } = e.nativeEvent.layout;
    setSize((s) => (s.width === width && s.height === height ? s : { width, height }));
  };

  const [ownInspect, setOwnInspect] = useState<number | null>(null);
  const inspectT = onInspectChange ? (inspectProp ?? null) : ownInspect;
  const setInspect = (t: number | null) => (onInspectChange ? onInspectChange(t) : setOwnInspect(t));
  const [ownScale, setOwnScale] = useState<PriceScaleType>("linear");
  const scaleType = onPriceScaleTypeChange ? (scaleProp ?? "linear") : ownScale;
  const setScaleType = (t: PriceScaleType) => (onPriceScaleTypeChange ? onPriceScaleTypeChange(t) : setOwnScale(t));

  const [ownIndicators, setOwnIndicators] = useState<IndicatorSpec[]>([]);
  const indicatorSpecs = onIndicatorsChange ? (indicatorsProp ?? []) : ownIndicators;
  const setIndicators = (s: IndicatorSpec[]) => (onIndicatorsChange ? onIndicatorsChange(s) : setOwnIndicators(s));
  const [ownDrawings, setOwnDrawings] = useState<Record<string, Drawing[]>>({});
  const drawings = onDrawingsChange ? (drawingsProp ?? []) : (ownDrawings[symbol] ?? []);
  const writeDrawings = (d: Drawing[]) => (onDrawingsChange ? onDrawingsChange(d) : setOwnDrawings((all) => ({ ...all, [symbol]: d })));
  // Undo / redo per symbol (see chart-web): every committed change is recorded, previews never.
  const [histories, setHistories] = useState<Record<string, History<Drawing[]>>>({});
  const history = histories[symbol] ?? emptyHistory<Drawing[]>();
  const setDrawings = (d: Drawing[]) => {
    setHistories((all) => ({ ...all, [symbol]: record(all[symbol] ?? emptyHistory<Drawing[]>(), drawings) }));
    writeDrawings(d);
  };
  const stepHistory = (dir: "undo" | "redo") => {
    const r = dir === "undo" ? undo(history, drawings) : redo(history, drawings);
    if (!r) return;
    setHistories((all) => ({ ...all, [symbol]: r.history }));
    setSelected(null);
    writeDrawings(r.state);
    if (haptics) playHaptic("selection");
  };

  const [view, setView] = useState<{ tf: Timeframe; vp: Viewport }>(() => ({ tf: timeframe, vp: defaultViewport(barSpacing) }));
  const vp = view.tf === timeframe ? view.vp : defaultViewport(barSpacing);
  const setVp = useCallback((f: (vp: Viewport) => Viewport) => setView((v) => ({ tf: timeframe, vp: f(v.tf === timeframe ? v.vp : defaultViewport(barSpacing)) })), [timeframe, barSpacing]);

  const [pointer, setPointer] = useState<Point | null>(null);
  const [ownToolsVisible, setOwnToolsVisible] = useState(true);
  const toolsVisible = onToolsVisibleChange ? (toolsVisibleProp ?? true) : ownToolsVisible;
  const changeToolsVisible = (v: boolean) => {
    if (onToolsVisibleChange) onToolsVisibleChange(v);
    else setOwnToolsVisible(v);
    // Hiding the tools mid-drawing would leave an invisible tool armed: back to the cursor.
    if (!v) changeTool(null);
  };

  const [tool, setTool] = useState<DrawingKind | null>(null);
  // A measurement is shown until the next touch or a tool change; it is never stored.
  const [measurement, setMeasurement] = useState<Drawing | null>(null);
  const [ownToolbox, setOwnToolbox] = useState<ToolboxPlacement>(DEFAULT_TOOLBOX_PHONE);
  const [dragToolbox, setDragToolbox] = useState<ToolboxPlacement | null>(null);
  const toolbox = dragToolbox ?? (onToolboxPlacementChange ? (toolboxProp ?? DEFAULT_TOOLBOX_PHONE) : ownToolbox);
  const moveToolbox = (p: ToolboxPlacement, done: boolean) => {
    if (!done) {
      setDragToolbox(p);
      return;
    }
    setDragToolbox(null);
    if (onToolboxPlacementChange) onToolboxPlacementChange(p);
    else setOwnToolbox(p);
  };
  const [selected, setSelected] = useState<ObjectHit | null>(null);
  const [draft, setDraft] = useState<DrawingDraft | null>(null);
  const [drag, setDrag] = useState<DragState | null>(null);
  const [sheet, setSheet] = useState<{ kind: "none" } | { kind: "indicators" } | { kind: "layers" } | { kind: "quick"; price: number; x: number; y: number } | { kind: "axis"; price: number | null } | { kind: "ticket"; price: number | null }>({ kind: "none" });
  const [reducedMotion, setReducedMotion] = useState(false);
  useEffect(() => {
    AccessibilityInfo.isReduceMotionEnabled().then(setReducedMotion, () => {});
    const sub = AccessibilityInfo.addEventListener("reduceMotionChanged", setReducedMotion);
    return () => sub.remove();
  }, []);

  const slots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);
  const indicatorResults = useMemo(() => computeIndicators(slots, indicatorSpecs), [slots, indicatorSpecs]);
  const last = useMemo(() => lastCandle(slots)?.c ?? null, [slots]);
  // Pane heights: drag a pane's top edge, double-tap a pane to expand it. A drag is shown live and
  // reported to the host once, when the finger lifts.
  const [ownPaneHeights, setOwnPaneHeights] = useState<Record<string, number>>({});
  const [livePaneHeights, setLivePaneHeights] = useState<Record<string, number> | null>(null);
  // Mirrors the live heights, so the release (often in the same batch as the last move) commits the latest.
  const livePaneHeightsRef = useRef<Record<string, number> | null>(null);
  const setLive = (f: (h: Record<string, number> | null) => Record<string, number>) => {
    livePaneHeightsRef.current = f(livePaneHeightsRef.current);
    setLivePaneHeights(livePaneHeightsRef.current);
  };
  const storedPaneHeights = onPaneHeightsChange ? (paneHeightsProp ?? {}) : ownPaneHeights;
  const paneHeights = livePaneHeights ?? storedPaneHeights;
  const commitPaneHeights = (h: Record<string, number>) => {
    livePaneHeightsRef.current = null;
    setLivePaneHeights(null);
    if (onPaneHeightsChange) onPaneHeightsChange(h);
    else setOwnPaneHeights(h);
  };
  const [ownLayers, setOwnLayers] = useState<ChartLayers>(DEFAULT_LAYERS);
  const layers = onLayersChange ? (layersProp ?? DEFAULT_LAYERS) : ownLayers;
  const setLayers = (l: ChartLayers) => (onLayersChange ? onLayersChange(l) : setOwnLayers(l));
  const levels = useMemo(() => keyLevels(slots), [slots]);
  const fills = useMemo(() => (trading?.state?.fills ?? []).filter((f) => f.symbol === symbol), [trading?.state, symbol]);
  const context = useMemo(
    () => ({
      levels: levels.filter((l) => layers[l.layer]),
      halts: layers.halts ? (series?.halts ?? []) : [],
      luld: layers.luld ? (series?.luld ?? []) : [],
      fills: layers.fills ? fills : [],
      now: clockNow,
    }),
    [levels, layers, series, fills, clockNow],
  );
  const orders = useMemo(() => (trading?.state?.orders ?? []).filter((o) => o.symbol === symbol && isWorking(o)), [trading?.state, symbol]);
  const position = useMemo(() => trading?.state?.positions.find((p) => p.symbol === symbol) ?? null, [trading?.state, symbol]);
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
      pointer,
      // The Levels sheet's "Volume profile" switch turns the in-chart profile off.
      options: options?.showProfile && !layers.profile ? { ...options, showProfile: false } : options,
      priceScaleType: scaleType,
      indicators: indicatorResults,
      drawings: shownDrawings,
      selectedDrawing,
      draft,
      trading: trading ? { orders, position, last, dragging: drag?.order ?? null, placeY: null } : null,
      context,
      now: clockNow,
      paneHeights,
    }),
    [timeframe, series, slots, inspectT, pointer, options, layers.profile, scaleType, indicatorResults, shownDrawings, selectedDrawing, draft, trading, orders, position, last, drag, context, clockNow, paneHeights],
  );
  const model = useMemo(() => computeChartModel(state, vp, size), [state, vp, size]);
  const picture = useMemo(() => (size.width > 0 && size.height > 0 ? recordPicture(layoutChart(model, state, theme), size.width, size.height) : null), [model, state, theme, size]);

  // If the inspected slot disappears (trimmed, or a new snapshot), fall back to latest.
  useEffect(() => {
    if (inspectT !== null && slotIndexOf(slots, inspectT) === null) setInspect(null);
  });

  // A success tap when one of this symbol's orders fills.
  const filled = useRef<Set<string> | null>(null);
  useEffect(() => {
    const ids = new Set((trading?.state?.orders ?? []).filter((o) => o.status === "filled" && o.symbol === symbol).map((o) => o.id));
    if (filled.current && haptics) for (const id of ids) if (!filled.current.has(id)) playHaptic("success");
    filled.current = ids;
  }, [trading?.state, symbol, haptics]);

  const hitObject =
    (m: ChartModel) =>
    (x: number, y: number, touch: boolean): ObjectHit | null => {
      const tol = touch ? DRAWING_HIT_PX.touch : DRAWING_HIT_PX.mouse;
      if (state.trading) {
        const t = hitTrading(m, state.trading, x, y, tol);
        if (t) return t;
      }
      const d = hitDrawing(m, drawings, x, y, tol, selectedDrawing);
      return d ? { kind: "drawing", ...d } : null;
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
      } else setDrag({ drawing: moved });
      return;
    }
    if (t.kind !== "order" || !m.price) return;
    const o = orders.find((x) => x.id === t.id);
    const line = o && orderLinePrice(o);
    if (!o || !line) return;
    const price = snapPrice(yPrice(m.price, priceY(m.price, line.price) + (a.to.y - a.from.y)));
    if (!a.done) {
      setDrag({ order: { id: o.id, price } });
      return;
    }
    setDrag(null);
    if (price !== line.price && price > 0) trading?.onModify({ id: o.id, [line.field]: price });
  };

  const apply = (actions: readonly GestureAction[], m: ChartModel) => {
    for (const a of actions) {
      switch (a.kind) {
        case "inspect":
          setInspect(a.t);
          if (a.announce) {
            const idx = a.t === null ? slots.length - 1 : slotIndexOf(slots, a.t);
            const slot = idx === null || idx < 0 ? null : slots[idx];
            if (slot) AccessibilityInfo.announceForAccessibility(describeSlot(slot, timeframe).summary);
          }
          break;
        case "crosshair":
          setPointer(a.point);
          break;
        case "openAxisMenu":
          setSheet({ kind: "axis", price: m.price ? snapPrice(yPrice(m.price, a.y)) : null });
          break;
        case "release":
          break;
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
        case "placeOrder":
          if (m.price) setSheet({ kind: "ticket", price: snapPrice(yPrice(m.price, a.y)) });
          break;
        case "haptic":
          if (haptics) playHaptic(a.style);
          break;
        case "resizePane":
        case "togglePane": {
          const ids = paneIdsOf({ indicators: indicatorResults });
          if (a.kind === "resizePane" && !a.done) setLive((h) => applyPaneGesture(h ?? storedPaneHeights, a, m, ids, size));
          else if (a.kind === "resizePane") commitPaneHeights(livePaneHeightsRef.current ?? storedPaneHeights);
          else {
            commitPaneHeights(applyPaneGesture(storedPaneHeights, a, m, ids, size));
            if (haptics) playHaptic("light");
          }
          break;
        }
        default:
          setVp((v) => applyGesture(v, a, m));
      }
    }
  };

  // The recognizer is the web's, with the phone's options on.
  const [gestures] = useState(() => new GestureController({ momentum: true, touchPricePan: "always", haptics: true }));
  const latest = useRef<{ ctx: GestureContext; apply: typeof apply } | null>(null);
  useLayoutEffect(() => {
    latest.current = { ctx: { model, inspectT, priceManual: !isAutoPrice(vp), tool, hitObject: hitObject(model) }, apply };
  });

  // tick() decides long presses and deferred taps and drives momentum: run
  // it every frame only while the controller has a deadline.
  const frame = useRef(0);
  const pump = useCallback(() => {
    if (frame.current || gestures.nextDeadline() === null) return;
    const loop = () => {
      frame.current = 0;
      const l = latest.current;
      if (!l) return;
      l.apply(gestures.tick(now(), l.ctx), l.ctx.model);
      if (gestures.nextDeadline() !== null) frame.current = requestAnimationFrame(loop);
    };
    frame.current = requestAnimationFrame(loop);
  }, [gestures]);
  useEffect(() => () => cancelAnimationFrame(frame.current), []);

  // Trade at a price, the touch twin of the web's right-click menu (Robinhood's scrub-then-tap):
  // lifting a scrub leaves a "+ price" chip at the scrubbed price for a few seconds; tapping it
  // opens the quick-trade sheet. Any new touch on the chart dismisses it.
  const lastScrub = useRef<Point | null>(null);
  const chipTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [tradeChip, setTradeChip] = useState<{ x: number; y: number; price: number } | null>(null);
  const clearTradeChip = () => {
    if (chipTimer.current) clearTimeout(chipTimer.current);
    chipTimer.current = null;
    setTradeChip(null);
  };
  const showTradeChip = (c: { x: number; y: number; price: number }) => {
    if (chipTimer.current) clearTimeout(chipTimer.current);
    setTradeChip(c);
    chipTimer.current = setTimeout(() => setTradeChip(null), 5000);
  };
  useEffect(() => () => {
    if (chipTimer.current) clearTimeout(chipTimer.current);
  }, []);
  const [quickQty, setQuickQty] = useState<number>(QUICK_TRADE_SIZES[0]);

  // Back to latest (see chart-web): an eased slide home, instant with reduced
  // motion; a touch on the chart stops it where it is.
  const slide = useRef(0);
  const slotsRef = useRef(slots);
  slotsRef.current = slots;
  const stopSlide = () => {
    cancelAnimationFrame(slide.current);
    slide.current = 0;
  };
  useEffect(() => stopSlide, []);
  const backToLatest = () => {
    stopSlide();
    apply(gestures.reset(), model);
    if (haptics) playHaptic("light");
    const total = slotsBehindLatest(vp, slots);
    if (reducedMotion || total <= 0) {
      setVp(followLatest);
      return;
    }
    const duration = backToLatestMs(total, vp.barSpacing);
    const start = now();
    let panned = 0;
    const frame = () => {
      const elapsed = now() - start;
      const d = backToLatestStep(total, panned, elapsed, duration);
      panned -= d;
      if (elapsed >= duration) {
        setVp(followLatest);
        slide.current = 0;
        return;
      }
      setVp((v) => panSlots(v, slotsRef.current, d));
      slide.current = requestAnimationFrame(frame);
    };
    slide.current = requestAnimationFrame(frame);
  };

  const feed = (type: GestureInput["type"], touches: readonly { id: number; x: number; y: number }[]) => {
    const l = latest.current;
    if (!l) return;
    if (type === "down") {
      stopSlide();
      setMeasurement(null);
      clearTradeChip();
    }
    const wasScrub = gestures.state === "scrub";
    for (const t of touches) {
      const acts = gestures.handle({ type, pointerId: t.id, pointerType: "touch", x: t.x, y: t.y, t: now() }, l.ctx);
      // Remember where a scrub is, so lifting the finger can leave a trade chip there.
      if (gestures.state === "scrub") for (const a of acts) if (a.kind === "crosshair" && a.point) lastScrub.current = a.point;
      l.apply(acts, l.ctx.model);
    }
    if (wasScrub && gestures.state !== "scrub" && lastScrub.current && trading && l.ctx.model.price) {
      const pt = lastScrub.current;
      showTradeChip({ x: pt.x, y: pt.y, price: snapPrice(yPrice(l.ctx.model.price, pt.y)) });
    }
    pump();
  };

  // Every touch goes to the controller; the chart owns touches inside it
  // (the screen does not scroll), so the gesture activates at once.
  const gesture = useManualGesture({
    runOnJS: true,
    disableReanimated: true,
    onTouchesDown: (e) => {
      GestureStateManager.activate(e.handlerTag);
      feed("down", e.changedTouches);
    },
    onTouchesMove: (e) => feed("move", e.changedTouches),
    onTouchesUp: (e) => {
      feed("up", e.changedTouches);
      if (e.numberOfTouches === 0) GestureStateManager.deactivate(e.handlerTag);
    },
    onTouchesCancel: (e) => feed("cancel", e.changedTouches),
  });

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
    if (haptics) playHaptic("medium");
    // The plan became orders: their lines replace the zones.
    setDrawings(drawings.filter((x) => x.id !== d.id));
    setSelected(null);
  };

  const auto = isAutoPrice(vp);
  const a11yLabel = `${label ?? "Candlestick chart"}. Price axis ${SCALE_LABELS[scaleType]}${auto ? ", auto-scaled" : ", manual"}. Swipe up or down to inspect candles.`;
  const pnl = position && position.qty !== 0 && last !== null ? unrealizedPnl(position, last) : null;

  return (
    <View style={[s.root, style]}>
      {toolbar && (
        <View style={s.toolbar}>
          {/* The timeframe wheel on the left, in line with the ticker above; the chart's tools on the right. */}
          {onTimeframeChange && <TimeframeWheel value={timeframe} onChange={onTimeframeChange} haptics={haptics} reducedMotion={reducedMotion} />}
          <View style={s.trade}>
              {trading && pnl !== null && position && (
                <Text style={s.pos} accessibilityLabel={`Simulated position ${position.qty} at ${formatPrice(position.avgPrice)}`}>
                  {position.qty > 0 ? "+" : ""}
                  {formatSize(position.qty)} <Text style={{ color: pnl >= 0 ? UI.good : UI.critical }}>{`${pnl >= 0 ? "+" : "−"}$${Math.abs(pnl).toFixed(2)}`}</Text>
                </Text>
              )}
              <ToolButton label={toolsVisible ? "Hide drawing tools" : "Show drawing tools"} onPress={() => changeToolsVisible(!toolsVisible)} style={toolsVisible ? { backgroundColor: UI.raised } : undefined}>
                <ToolIcon kind="pencil" color={toolsVisible ? UI.text : UI.textDim} />
              </ToolButton>
              <ToolButton label={indicatorSpecs.length ? `Indicators, ${indicatorSpecs.length} on` : "Indicators"} onPress={() => setSheet({ kind: "indicators" })} style={s.indicators}>
                <ToolIcon kind="indicators" color={UI.textDim} />
                {indicatorSpecs.length ? <Text style={s.count}>{indicatorSpecs.length}</Text> : null}
              </ToolButton>
              <ToolButton label="Levels" onPress={() => setSheet({ kind: "layers" })}>
                <ToolIcon kind="levels" color={UI.textDim} />
              </ToolButton>
              {trading && (
                <>
                  <ToolButton primary label="Trade (simulated order)" onPress={() => setSheet({ kind: "ticket", price: null })} style={{ width: 34, paddingHorizontal: 0 }}>
                    <ToolIcon kind="trade" color={UI.accentFg} />
                  </ToolButton>
                </>
              )}
          </View>
        </View>
      )}
      {/* The button is a sibling of the gesture surface, not inside it: the chart claims every touch it gets. */}
      <View style={s.surfaceWrap}>
        <GestureDetector gesture={gesture}>
          <View
            style={s.surface}
            onLayout={onLayout}
            accessible
            accessibilityRole="adjustable"
            accessibilityLabel={a11yLabel}
            accessibilityActions={[{ name: "increment" }, { name: "decrement" }, { name: "activate", label: "Auto-scale price" }]}
            onAccessibilityAction={(e) => {
              if (e.nativeEvent.actionName === "activate") {
                setVp(autoScalePrice);
                return;
              }
              const next = stepInspection(slots, inspectT, e.nativeEvent.actionName === "increment" ? 1 : -1);
              setInspect(next);
              const idx = next === null ? slots.length - 1 : slotIndexOf(slots, next);
              const slot = idx === null || idx < 0 ? null : slots[idx];
              if (slot) AccessibilityInfo.announceForAccessibility(describeSlot(slot, timeframe).summary);
            }}
          >
            {picture && (
              <Canvas style={StyleSheet.absoluteFill}>
                <Picture picture={picture} />
              </Canvas>
            )}
          </View>
        </GestureDetector>
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
            haptics={haptics}
          />
        )}
        {tradeChip && (
          <Animated.View
            entering={FadeIn.duration(140).reduceMotion(ReduceMotion.System)}
            exiting={FadeOut.duration(120).reduceMotion(ReduceMotion.System)}
            style={[s.chipWrap, { top: Math.max(model.geometry.plot.y, tradeChip.y - 15), right: model.options.priceAxisWidth + 6 }]}
          >
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={`Trade at ${formatPrice(tradeChip.price)}`}
              hitSlop={8}
              onPress={() => {
                const c = tradeChip;
                clearTradeChip();
                if (haptics) playHaptic("light");
                setSheet({ kind: "quick", price: c.price, x: c.x, y: c.y });
              }}
              style={({ pressed }) => [s.chip, pressed && { opacity: 0.85 }]}
            >
              <Text style={s.chipText}>{`+ ${formatPrice(tradeChip.price)}`}</Text>
            </Pressable>
          </Animated.View>
        )}
        {isPannedBack(vp) && (
          <Animated.View
            entering={FadeIn.duration(150).reduceMotion(ReduceMotion.System)}
            exiting={FadeOut.duration(120).reduceMotion(ReduceMotion.System)}
            // Bottom-right of the price pane, clear of the price axis and the volume bars.
            style={[s.latest, { left: model.geometry.plot.x + model.geometry.plot.w - 46, top: model.geometry.plot.y + model.geometry.plot.h - 46 }]}
          >
            <Pressable accessibilityRole="button" accessibilityLabel="Back to latest" onPress={backToLatest} hitSlop={6} style={({ pressed }) => [s.latestButton, pressed && { backgroundColor: UI.row }]}>
              <Svg width={16} height={16} viewBox="0 0 16 16" fill="none" stroke={UI.text} strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round">
                <Path d="M2.5 8h9M8 4.5 11.5 8 8 11.5M13.5 3.5v9" />
              </Svg>
            </Pressable>
          </Animated.View>
        )}
      </View>
      {selectedPosition && (
        // Over the bottom of the chart, never below it: the chart keeps its size, so nothing jumps.
        <PositionPlanPanel
          style={s.plan}
          drawing={selectedPosition}
          onChange={(d) => setDrawings(drawings.map((x) => (x.id === d.id ? d : x)))}
          onPlace={trading ? () => placeBracket(selectedPosition) : undefined}
        />
      )}

      <LayersSheet open={sheet.kind === "layers"} onClose={() => setSheet({ kind: "none" })} layers={layers}
        onChange={setLayers}
        reducedMotion={reducedMotion}
        profile={series?.profile ?? null}
        layerIds={layerIdsFor(!!options?.showProfile)}
        profileBins={profileBins}
        profileBin={profileBin}
        onProfileBinChange={onProfileBinChange}
      />
      <IndicatorSheet open={sheet.kind === "indicators"} onClose={() => setSheet({ kind: "none" })} indicators={indicatorSpecs} onChange={setIndicators} seriesColors={[theme.series1, theme.series2, theme.series3]} reducedMotion={reducedMotion} />
      <AxisSheet
        open={sheet.kind === "axis"}
        onClose={() => setSheet({ kind: "none" })}
        price={sheet.kind === "axis" ? sheet.price : null}
        autoScaled={auto}
        onAutoScale={() => setVp(autoScalePrice)}
        scaleType={scaleType}
        onScaleType={setScaleType}
        onOrderAt={trading && sheet.kind === "axis" && sheet.price !== null ? () => setSheet({ kind: "ticket", price: sheet.price }) : undefined}
        reducedMotion={reducedMotion}
      />
      {trading && (
        <>
        <QuickTradeSheet
          open={sheet.kind === "quick"}
          onClose={() => setSheet({ kind: "none" })}
          symbol={symbol}
          price={sheet.kind === "quick" ? sheet.price : 0}
          last={last}
          qty={quickQty}
          onQtyChange={setQuickQty}
          onOrder={(side, type) => {
            if (sheet.kind !== "quick") return;
            const price = sheet.price;
            trading.onPlace({ clientOrderId: newId("c"), symbol, side, orderType: type, qty: quickQty, ...(type === "limit" ? { limitPrice: price } : type === "stop" ? { stopPrice: price } : {}) });
            if (haptics) playHaptic("medium");
          }}
          onTicket={() => sheet.kind === "quick" && setTimeout(() => setSheet({ kind: "ticket", price: sheet.price }), 0)}
          onPosition={(kind) => {
            if (sheet.kind !== "quick") return;
            const d = newPosition(model, newId("d"), kind, { x: sheet.x, y: sheet.y });
            if (!d) return;
            setDrawings([...drawings, d]);
            setSelected({ kind: "drawing", id: d.id, part: "body" });
          }}
          reducedMotion={reducedMotion}
        />
        <OrderTicketSheet
          open={sheet.kind === "ticket"}
          onClose={() => setSheet({ kind: "none" })}
          symbol={symbol}
          last={last}
          price={sheet.kind === "ticket" ? sheet.price : null}
          onSubmit={(o) => trading.onPlace({ ...o, clientOrderId: newId("c") })}
          reducedMotion={reducedMotion}
          fills={fills}
          realized={trading.state?.account.realizedPnl ?? 0}
        />
        </>
      )}
    </View>
  );
}

const s = StyleSheet.create({
  root: { minHeight: 0, gap: 6 },
  toolbar: { flexDirection: "row", alignItems: "center", gap: 4, flexWrap: "wrap" },
  trade: { marginLeft: "auto", flexDirection: "row", alignItems: "center", gap: 8 },
  pos: { color: UI.textDim, fontSize: 11, fontVariant: ["tabular-nums"] },
  surfaceWrap: { flex: 1, minHeight: 120 },
  surface: { flex: 1, minHeight: 120 },
  latest: { position: "absolute" },
  chipWrap: { position: "absolute" },
  chip: { height: 30, paddingHorizontal: 12, borderRadius: 15, backgroundColor: UI.accent, alignItems: "center", justifyContent: "center", shadowColor: "#000", shadowOpacity: 0.35, shadowRadius: 6, shadowOffset: { width: 0, height: 2 }, elevation: 5 },
  chipText: { color: UI.accentFg, fontSize: 13, fontWeight: "700", fontVariant: ["tabular-nums"] },
  indicators: { flexDirection: "row", gap: 3 },
  count: { color: UI.text, fontSize: 11, fontVariant: ["tabular-nums"] },
  latestButton: { width: 40, height: 40, borderRadius: 20, backgroundColor: UI.panel, alignItems: "center", justifyContent: "center", shadowColor: "#000", shadowOpacity: 0.35, shadowRadius: 8, shadowOffset: { width: 0, height: 2 }, elevation: 6 },
  plan: { position: "absolute", left: 0, right: 0, bottom: 0 },
});
