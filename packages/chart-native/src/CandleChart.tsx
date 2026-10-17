import { Canvas, Picture } from "@shopify/react-native-skia";
import {
  applyGesture,
  autoScalePrice,
  buildSlots,
  computeChartModel,
  computeIndicators,
  DEFAULT_CHART_THEME,
  defaultViewport,
  describeSlot,
  DRAWING_HIT_PX,
  DRAWING_KINDS,
  DRAWING_LABELS,
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
  type Point,
  type PriceScaleType,
  type ProfileWire,
  type Size,
  type Timeframe,
  type Viewport,
} from "@stockwatcher/chart-core";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { AccessibilityInfo, StyleSheet, Text, View, type LayoutChangeEvent, type StyleProp, type ViewStyle } from "react-native";
import { GestureDetector, GestureStateManager, useManualGesture } from "react-native-gesture-handler";
import { recordPicture } from "./drawSkia.ts";
import { PositionPlanPanel } from "./PositionPlanPanel.tsx";
import { AxisSheet, IndicatorSheet, OrderTicketSheet, SCALE_LABELS } from "./sheets.tsx";
import { playHaptic, ToolButton, ToolIcon, UI } from "./ui.tsx";

export interface ChartSeriesLike {
  candles: readonly Candle[];
  gaps: readonly Gap[];
  profile: ProfileWire | null;
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
  const setDrawings = (d: Drawing[]) => (onDrawingsChange ? onDrawingsChange(d) : setOwnDrawings((all) => ({ ...all, [symbol]: d })));

  const [view, setView] = useState<{ tf: Timeframe; vp: Viewport }>(() => ({ tf: timeframe, vp: defaultViewport(barSpacing) }));
  const vp = view.tf === timeframe ? view.vp : defaultViewport(barSpacing);
  const setVp = useCallback((f: (vp: Viewport) => Viewport) => setView((v) => ({ tf: timeframe, vp: f(v.tf === timeframe ? v.vp : defaultViewport(barSpacing)) })), [timeframe, barSpacing]);

  const [pointer, setPointer] = useState<Point | null>(null);
  const [tool, setTool] = useState<DrawingKind | null>(null);
  const [selected, setSelected] = useState<ObjectHit | null>(null);
  const [draft, setDraft] = useState<DrawingDraft | null>(null);
  const [drag, setDrag] = useState<DragState | null>(null);
  const [sheet, setSheet] = useState<{ kind: "none" } | { kind: "indicators" } | { kind: "axis"; price: number | null } | { kind: "ticket"; price: number | null }>({ kind: "none" });
  const [reducedMotion, setReducedMotion] = useState(false);
  useEffect(() => {
    AccessibilityInfo.isReduceMotionEnabled().then(setReducedMotion, () => {});
    const sub = AccessibilityInfo.addEventListener("reduceMotionChanged", setReducedMotion);
    return () => sub.remove();
  }, []);

  const slots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);
  const indicatorResults = useMemo(() => computeIndicators(slots, indicatorSpecs), [slots, indicatorSpecs]);
  const last = useMemo(() => lastCandle(slots)?.c ?? null, [slots]);
  const orders = useMemo(() => (trading?.state?.orders ?? []).filter((o) => o.symbol === symbol && isWorking(o)), [trading?.state, symbol]);
  const position = useMemo(() => trading?.state?.positions.find((p) => p.symbol === symbol) ?? null, [trading?.state, symbol]);
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
      trading: trading ? { orders, position, last, dragging: drag?.order ?? null, placeY: null } : null,
    }),
    [timeframe, series, slots, inspectT, pointer, options, scaleType, indicatorResults, shownDrawings, selectedDrawing, draft, trading, orders, position, last, drag],
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

  const feed = (type: GestureInput["type"], touches: readonly { id: number; x: number; y: number }[]) => {
    const l = latest.current;
    if (!l) return;
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
    setTool(t);
  };
  const deleteSelected = () => {
    if (selected?.kind !== "drawing") return;
    setDrawings(drawings.filter((d) => d.id !== selected.id));
    setSelected(null);
  };

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
          <View style={s.track} accessibilityRole="radiogroup" accessibilityLabel="Drawing tool">
            {(["cursor", ...DRAWING_KINDS] as const).map((k) => {
              const on = (tool ?? "cursor") === k;
              return (
                <ToolButton key={k} label={k === "cursor" ? "Cursor" : DRAWING_LABELS[k]} onPress={() => changeTool(k === "cursor" ? null : k)} style={[s.tool, on && { backgroundColor: UI.raised }]}>
                  <ToolIcon kind={k} color={on ? UI.text : UI.textDim} />
                </ToolButton>
              );
            })}
          </View>
          <ToolButton label="Delete selected drawing" disabled={selected?.kind !== "drawing"} onPress={deleteSelected}>
            <ToolIcon kind="trash" color={UI.textDim} />
          </ToolButton>
          <ToolButton label="Indicators" onPress={() => setSheet({ kind: "indicators" })}>
            {indicatorSpecs.length ? `Ind ${indicatorSpecs.length}` : "Ind"}
          </ToolButton>
          {trading && (
            <View style={s.trade}>
              {pnl !== null && position && (
                <Text style={s.pos} accessibilityLabel={`Simulated position ${position.qty} at ${formatPrice(position.avgPrice)}`}>
                  {position.qty > 0 ? "+" : ""}
                  {formatSize(position.qty)} <Text style={{ color: pnl >= 0 ? UI.good : UI.critical }}>{`${pnl >= 0 ? "+" : "−"}$${Math.abs(pnl).toFixed(2)}`}</Text>
                </Text>
              )}
              <ToolButton primary label="Simulated order" onPress={() => setSheet({ kind: "ticket", price: null })}>
                Trade
              </ToolButton>
            </View>
          )}
        </View>
      )}
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
      {selectedPosition && (
        // Over the bottom of the chart, never below it: the chart keeps its size, so nothing jumps.
        <PositionPlanPanel
          style={s.plan}
          drawing={selectedPosition}
          onChange={(d) => setDrawings(drawings.map((x) => (x.id === d.id ? d : x)))}
          onPlace={trading ? () => placeBracket(selectedPosition) : undefined}
        />
      )}

      <IndicatorSheet open={sheet.kind === "indicators"} onClose={() => setSheet({ kind: "none" })} indicators={indicatorSpecs} onChange={setIndicators} reducedMotion={reducedMotion} />
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
  track: { flexDirection: "row", backgroundColor: UI.panel, borderRadius: 8, padding: 2, gap: 2 },
  tool: { width: 30, minWidth: 30, paddingHorizontal: 0 },
  trade: { marginLeft: "auto", flexDirection: "row", alignItems: "center", gap: 8 },
  pos: { color: UI.textDim, fontSize: 11, fontVariant: ["tabular-nums"] },
  surface: { flex: 1, minHeight: 120 },
  plan: { position: "absolute", left: 0, right: 0, bottom: 0 },
});
