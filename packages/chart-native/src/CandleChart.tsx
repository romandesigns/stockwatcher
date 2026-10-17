import { Canvas, Picture } from "@shopify/react-native-skia";
import {
  applyGesture,
  emptyHistory,
  record,
  redo,
  undo,
  type History,
  DEFAULT_LAYERS,
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
import { AxisSheet, IndicatorSheet, LayersSheet, OrderTicketSheet, SCALE_LABELS, TradesSheet } from "./sheets.tsx";
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
  /** Which context layers show (halts, LULD, key levels, fills). Controlled when onLayersChange is set (the host persists it). */
  layers?: ChartLayers;
  onLayersChange?: (layers: ChartLayers) => void;
  /** The feed clock (ms), for an ongoing halt's length. */
  now?: number | null;
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
  // Renamed: this module's now() is the monotonic clock the gestures use.
  now: clockNow = null,
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
  const [sheet, setSheet] = useState<{ kind: "none" } | { kind: "indicators" } | { kind: "layers" } | { kind: "trades" } | { kind: "axis"; price: number | null } | { kind: "ticket"; price: number | null }>({ kind: "none" });
  const [reducedMotion, setReducedMotion] = useState(false);
  useEffect(() => {
    AccessibilityInfo.isReduceMotionEnabled().then(setReducedMotion, () => {});
    const sub = AccessibilityInfo.addEventListener("reduceMotionChanged", setReducedMotion);
    return () => sub.remove();
  }, []);

  const slots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);
  const indicatorResults = useMemo(() => computeIndicators(slots, indicatorSpecs), [slots, indicatorSpecs]);
  const last = useMemo(() => lastCandle(slots)?.c ?? null, [slots]);
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
      options,
      priceScaleType: scaleType,
      indicators: indicatorResults,
      drawings: shownDrawings,
      selectedDrawing,
      draft,
      trading: trading ? { orders, position, last, dragging: drag?.order ?? null, placeY: null } : null,
      context,
    }),
    [timeframe, series, slots, inspectT, pointer, options, scaleType, indicatorResults, shownDrawings, selectedDrawing, draft, trading, orders, position, last, drag, context],
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
    }
    for (const t of touches) l.apply(gestures.handle({ type, pointerId: t.id, pointerType: "touch", x: t.x, y: t.y, t: now() }, l.ctx), l.ctx.model);
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
          <ToolButton label="Levels" onPress={() => setSheet({ kind: "layers" })}>
            <ToolIcon kind="levels" color={UI.textDim} />
          </ToolButton>
          <ToolButton label={indicatorSpecs.length ? `Indicators, ${indicatorSpecs.length} on` : "Indicators"} onPress={() => setSheet({ kind: "indicators" })} style={s.indicators}>
            <ToolIcon kind="indicators" color={UI.textDim} />
            {indicatorSpecs.length ? <Text style={s.count}>{indicatorSpecs.length}</Text> : null}
          </ToolButton>
          {trading && (
            <View style={s.trade}>
              {pnl !== null && position && (
                <Text style={s.pos} accessibilityLabel={`Simulated position ${position.qty} at ${formatPrice(position.avgPrice)}`}>
                  {position.qty > 0 ? "+" : ""}
                  {formatSize(position.qty)} <Text style={{ color: pnl >= 0 ? UI.good : UI.critical }}>{`${pnl >= 0 ? "+" : "−"}$${Math.abs(pnl).toFixed(2)}`}</Text>
                </Text>
              )}
              <ToolButton label={`Trades, ${fills.length}`} onPress={() => setSheet({ kind: "trades" })} style={s.indicators}>
                <ToolIcon kind="trades" color={UI.textDim} />
                {fills.length ? <Text style={s.count}>{fills.length}</Text> : null}
              </ToolButton>
              <ToolButton primary label="Simulated order" onPress={() => setSheet({ kind: "ticket", price: null })}>
                Trade
              </ToolButton>
            </View>
          )}
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
        {toolbar && (
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

      <LayersSheet open={sheet.kind === "layers"} onClose={() => setSheet({ kind: "none" })} layers={layers} onChange={setLayers} reducedMotion={reducedMotion} />
      <TradesSheet open={sheet.kind === "trades"} onClose={() => setSheet({ kind: "none" })} fills={fills} realized={trading?.state?.account.realizedPnl ?? 0} reducedMotion={reducedMotion} />
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
        <OrderTicketSheet
          open={sheet.kind === "ticket"}
          onClose={() => setSheet({ kind: "none" })}
          symbol={symbol}
          last={last}
          price={sheet.kind === "ticket" ? sheet.price : null}
          onSubmit={(o) => trading.onPlace({ ...o, clientOrderId: newId("c") })}
          reducedMotion={reducedMotion}
        />
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
  indicators: { flexDirection: "row", gap: 3 },
  count: { color: UI.text, fontSize: 11, fontVariant: ["tabular-nums"] },
  latestButton: { width: 40, height: 40, borderRadius: 20, backgroundColor: UI.panel, alignItems: "center", justifyContent: "center", shadowColor: "#000", shadowOpacity: 0.35, shadowRadius: 8, shadowOffset: { width: 0, height: 2 }, elevation: 6 },
  plan: { position: "absolute", left: 0, right: 0, bottom: 0 },
});
