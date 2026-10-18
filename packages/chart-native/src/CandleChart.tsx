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
  openingRanges,
  type SessionRef,
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
  DEFAULT_CHART_OPTIONS,
  DEFAULT_CHART_SETTINGS,
  easeOut,
  manualPrice,
  openView,
  presetViewport,
  sessionSlots,
  SETTINGS_MOTION_MS,
  viewKey,
  type ChartSettings,
  DEFAULT_DRAWING_SETTINGS,
  toolAllowed,
  type DrawingSettings,
} from "@stockwatcher/chart-core";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { AccessibilityInfo, Pressable, StyleSheet, Text, View, type LayoutChangeEvent, type StyleProp, type ViewStyle } from "react-native";
import { GestureDetector, GestureStateManager, useManualGesture } from "react-native-gesture-handler";
import Animated, { FadeIn, FadeOut, ReduceMotion } from "react-native-reanimated";
import { recordPicture } from "./drawSkia.ts";
import { CandleInspector } from "./CandleInspector.tsx";
import { DrawingToolbox } from "./DrawingToolbox.tsx";
import { PositionPlanPanel } from "./PositionPlanPanel.tsx";
import { AxisSheet, IndicatorSheet, LayersSheet, OrderTicketSheet, QUICK_TRADE_SIZES, QuickTradeSheet, SCALE_LABELS } from "./sheets.tsx";
import { useTween } from "./useTween.ts";
import { TimeframeWheel } from "./TimeframeWheel.tsx";
import { Icon, playHaptic, ToolButton, ToolIcon, UI } from "./ui.tsx";

export interface ChartSeriesLike {
  candles: readonly Candle[];
  gaps: readonly Gap[];
  profile: ProfileWire | null;
  halts?: readonly Halt[];
  luld?: readonly LuldPoint[];
  /** The server's official previous close (Series.prevClose), for the "Prev close" level. */
  prevClose?: number | null;
  /** The symbol's current session (Series.session): its end is the opening range's right edge on an early close. */
  session?: SessionRef | null;
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
  /** Controlled when onPriceScaleTypeChange is set; otherwise it is the chart settings' scale. */
  priceScaleType?: PriceScaleType;
  onPriceScaleTypeChange?: (t: PriceScaleType) => void;
  /** The chart settings (chart-core chartSettings.ts). Controlled when onSettingsChange is set (the host syncs it across clients, namespace `chartSettings`); else kept for the session. */
  settings?: ChartSettings;
  onSettingsChange?: (s: ChartSettings) => void;
  /**
   * True when the current `settings` came from another device (settings sync,
   * docs/architecture/settings-sync.md). They still apply at once (chart type, grid, volume,
   * sessions, …), but this device's view is not re-zoomed or re-fitted for a zoom-preset,
   * auto-stretch or extended-hours change: each device keeps its own zoom and pan, and the
   * next ticker or timeframe it opens uses the new settings.
   */
  settingsFromRemote?: boolean;
  /** Which buttons the floating drawing toolbox shows (the Settings sheet's Drawing tab, chart-core drawingSettings.ts; the host syncs it, namespace `drawingSettings`). A tool that becomes hidden while armed is disarmed. Default: all. */
  drawingSettings?: DrawingSettings;
  options?: Partial<ChartOptions>;
  theme?: ChartTheme;
  barSpacing?: number;
  /** Drawings are kept per symbol; trading filters orders by it. */
  symbol?: string;
  /** Show the inspected candle's readout (time, O/H/L/C, change, Live) at the chart's top-left, as chart-web's `inspector` does. */
  inspector?: boolean;
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

/** Height the top-left readout takes, which overlay legends start below. */
const READOUT_H = 18;

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
  settings: settingsProp,
  onSettingsChange,
  settingsFromRemote = false,
  drawingSettings = DEFAULT_DRAWING_SETTINGS,
  options,
  theme = DEFAULT_CHART_THEME,
  barSpacing = 9,
  symbol = "",
  toolbar = false,
  inspector = false,
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
  const [ownSettings, setOwnSettings] = useState<ChartSettings>(DEFAULT_CHART_SETTINGS);
  const settings = onSettingsChange ? (settingsProp ?? DEFAULT_CHART_SETTINGS) : ownSettings;
  const setSettings = (s: ChartSettings) => (onSettingsChange ? onSettingsChange(s) : setOwnSettings(s));
  const scaleType = onPriceScaleTypeChange ? (scaleProp ?? "linear") : settings.priceScale;
  const setScaleType = (t: PriceScaleType) => (onPriceScaleTypeChange ? onPriceScaleTypeChange(t) : setSettings({ ...settings, priceScale: t }));

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

  // One view per symbol + timeframe, opened from the saved settings (zoom preset, auto stretch)
  // once candles and a size exist: see chart-web's CandleChart for the rule, which this mirrors.
  const key = viewKey(symbol, timeframe);
  const [view, setView] = useState<{ key: string; vp: Viewport; opened: boolean }>(() => ({ key, vp: defaultViewport(barSpacing), opened: false }));
  const fresh = useCallback(() => ({ key, vp: defaultViewport(barSpacing), opened: false }), [key, barSpacing]);
  const current = view.key === key ? view : fresh();
  const vp = current.vp;
  const setVp = useCallback((f: (vp: Viewport) => Viewport) => setView((v) => {
    const b = v.key === key ? v : fresh();
    return { ...b, vp: f(b.vp) };
  }), [key, fresh]);

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

  // All slots, and the shown ones (extended hours off: the regular session only); see chart-web.
  const allSlots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);
  const slots = useMemo(() => sessionSlots(allSlots, settings.extendedHours), [allSlots, settings.extendedHours]);
  const visibility = settings.visibility;
  const indicatorResults = useMemo(() => (visibility.indicators ? computeIndicators(slots, indicatorSpecs) : []), [slots, indicatorSpecs, visibility.indicators]);
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
  // The server's official previous close wins over one read off the candles (levels.ts).
  const prevClose = series?.prevClose ?? null;
  const levels = useMemo(() => keyLevels(allSlots, prevClose), [allSlots, prevClose]);
  // Computed only while the layer is on: one pass over the slots per update otherwise wasted.
  const session = series?.session ?? null;
  const ranges = useMemo(() => (layers.openingRange ? openingRanges(allSlots, session) : []), [layers.openingRange, allSlots, session]);
  const baselinePrice = useMemo(() => levels.find((l) => l.id === "prevClose")?.price ?? null, [levels]);
  const fills = useMemo(() => (trading?.state?.fills ?? []).filter((f) => f.symbol === symbol), [trading?.state, symbol]);
  const context = useMemo(
    () => ({
      levels: levels.filter((l) => layers[l.layer]),
      openingRanges: ranges,
      halts: layers.halts ? (series?.halts ?? []) : [],
      luld: layers.luld ? (series?.luld ?? []) : [],
      fills: layers.fills ? fills : [],
      now: clockNow,
    }),
    [levels, ranges, layers, series, fills, clockNow],
  );
  const orders = useMemo(() => (trading?.state?.orders ?? []).filter((o) => o.symbol === symbol && isWorking(o)), [trading?.state, symbol]);
  const position = useMemo(() => trading?.state?.positions.find((p) => p.symbol === symbol) ?? null, [trading?.state, symbol]);
  const shownDrawings = useMemo(() => {
    const own = visibility.drawings ? drawings : [];
    const list = drag?.drawing ? own.map((d) => (d.id === drag.drawing!.id ? drag.drawing! : d)) : own;
    return measurement ? [...list, measurement] : list;
  }, [drawings, drag, measurement, visibility.drawings]);
  const selectedDrawing = selected?.kind === "drawing" ? selected.id : null;
  const showTrading = !!trading && visibility.trading;
  const volumeHeight = Math.round(useTween(settings.volume ? (options?.volumeHeight ?? DEFAULT_CHART_OPTIONS.volumeHeight) : 0, reducedMotion));
  const chartStyle = useMemo(() => ({ grid: settings.grid, sessionShading: settings.sessionShading, sessionBreaks: settings.sessionBreaks, highLow: settings.highLow }), [settings.grid, settings.sessionShading, settings.sessionBreaks, settings.highLow]);

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
      options: {
        ...options,
        volumeHeight,
        // The readout takes the top-left corner; overlay indicator legends start under it.
        ...(inspector ? { legendTop: READOUT_H } : {}),
        ...(options?.showProfile && !layers.profile ? { showProfile: false } : {}),
      },
      priceScaleType: scaleType,
      indicators: indicatorResults,
      drawings: shownDrawings,
      selectedDrawing,
      draft,
      trading: showTrading ? { orders, position, last, dragging: drag?.order ?? null, placeY: null } : null,
      context,
      now: clockNow,
      paneHeights,
      chartType: settings.chartType,
      chartStyle,
      baselinePrice,
      extendedHours: settings.extendedHours,
    }),
    [timeframe, series, slots, inspectT, pointer, options, volumeHeight, layers.profile, scaleType, indicatorResults, shownDrawings, selectedDrawing, draft, showTrading, orders, position, last, drag, context, clockNow, paneHeights, inspector, settings.chartType, chartStyle, baselinePrice, settings.extendedHours],
  );
  const model = useMemo(() => computeChartModel(state, vp, size), [state, vp, size]);

  // Open a fresh view from the saved settings once its candles and the size exist (chart-web does the same).
  useLayoutEffect(() => {
    if (current.opened || !slots.length || size.width <= 0) return;
    setView({ key, vp: openView(state, settings, size, defaultViewport(barSpacing)), opened: true });
  });
  const stateRef = useRef(state);
  stateRef.current = state;
  const sizeRef = useRef(size);
  sizeRef.current = size;
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
      if (!visibility.drawings) return null;
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

  // Where the current scrub is, so lifting the finger can leave a trade chip there (set by moves and by tick()).
  const lastScrubRef = useRef<Point | null>(null);

  // tick() decides long presses and deferred taps and drives momentum: run
  // it every frame only while the controller has a deadline.
  const frame = useRef(0);
  const pump = useCallback(() => {
    if (frame.current || gestures.nextDeadline() === null) return;
    const loop = () => {
      frame.current = 0;
      const l = latest.current;
      if (!l) return;
      const acts = gestures.tick(now(), l.ctx);
      // A scrub that starts from a still finger begins here (the long-press timer), not in a touch move,
      // so record its point too: otherwise lifting a steady finger leaves no trade chip.
      if (gestures.state === "scrub") for (const a of acts) if (a.kind === "crosshair" && a.point) lastScrubRef.current = a.point;
      l.apply(acts, l.ctx.model);
      if (gestures.nextDeadline() !== null) frame.current = requestAnimationFrame(loop);
    };
    frame.current = requestAnimationFrame(loop);
  }, [gestures]);
  useEffect(() => () => cancelAnimationFrame(frame.current), []);

  // Trade at a price, the touch twin of the web's right-click menu (Robinhood's scrub-then-tap):
  // lifting a scrub leaves a "+ price" chip at the scrubbed price for a few seconds; tapping it
  // opens the quick-trade sheet. Any new touch on the chart dismisses it.
  const lastScrub = lastScrubRef;
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

  // A zoom preset chosen, or extended hours shown/hidden: slide the spacing to the preset's while
  // following the latest candle (chart-web's zoomToPreset, same timing and the same fit-once rule
  // when auto stretch is off). Auto stretch toggled: auto, or the current range held.
  const zoomToPreset = (st: ChartSettings) => {
    stopSlide();
    const m = computeChartModel(stateRef.current, vp, sizeRef.current);
    const target = presetViewport(vp, m.slots, timeframe, st.extendedHours, st.zoom, m.geometry.plot.w);
    const finish = () => {
      setVp((v) => {
        const done = { ...v, barSpacing: target.barSpacing, anchorT: null, anchorFrac: 0 };
        if (st.autoStretch) return done;
        const fit = computeChartModel(stateRef.current, autoScalePrice(done), sizeRef.current).price;
        return fit ? manualPrice(done, { lo: fit.lo, hi: fit.hi }) : done;
      });
      slide.current = 0;
    };
    const from = vp.barSpacing;
    if (reducedMotion || Math.abs(from - target.barSpacing) < 1e-3) {
      finish();
      return;
    }
    const start = now();
    const frame = () => {
      const elapsed = now() - start;
      if (elapsed >= SETTINGS_MOTION_MS) {
        finish();
        return;
      }
      const bs = easeOut(from, target.barSpacing, elapsed, SETTINGS_MOTION_MS);
      setVp((v) => ({ ...v, barSpacing: bs, anchorT: null, anchorFrac: 0 }));
      slide.current = requestAnimationFrame(frame);
    };
    slide.current = requestAnimationFrame(frame);
  };
  const prevSettings = useRef(settings);
  const zoomRef = useRef(zoomToPreset);
  zoomRef.current = zoomToPreset;
  const remoteRef = useRef(settingsFromRemote);
  remoteRef.current = settingsFromRemote;
  useEffect(() => {
    const p = prevSettings.current;
    prevSettings.current = settings;
    if (p === settings || remoteRef.current) return;
    if (p.autoStretch !== settings.autoStretch) {
      if (settings.autoStretch) setVp(autoScalePrice);
      else setVp((v) => {
        const m = computeChartModel(stateRef.current, v, sizeRef.current);
        return m.price ? manualPrice(v, { lo: m.price.lo, hi: m.price.hi }) : v;
      });
    }
    if (p.zoom !== settings.zoom || p.extendedHours !== settings.extendedHours) zoomRef.current(settings);
  }, [settings, setVp]);

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
  // Hiding the armed tool in Settings disarms it: no invisible tool waits for the next tap (chart-core armedAfter).
  // changeTool is recreated every render; the check only needs to run when the settings or the tool change.
  useEffect(() => {
    if (!toolAllowed(drawingSettings, tool)) changeTool(null);
  }, [drawingSettings, tool]);
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
                <ToolIcon kind="pencil" color={toolsVisible ? UI.accent : UI.textDim} />
              </ToolButton>
              {/* No Chart button: the chart settings live in the Settings sheet's Chart tab (owner, 2026-10-18), as on the web. */}
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
        {inspector && (
          // Over the plot's top-left on a transparent background, like the web; touches pass through to the chart.
          <View pointerEvents="none" style={[s.readout, { left: model.geometry.plot.x + 6, right: model.options.priceAxisWidth + 6 }]}>
            <CandleInspector slot={inspectT === null ? slots.at(-1) : slots[slotIndexOf(slots, inspectT) ?? -1]} timeframe={timeframe} />
          </View>
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
            haptics={haptics}
            settings={drawingSettings}
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
        {!auto && model.price && (
          // Manual price range: a small "A" at the foot of the price column returns to auto stretch
          // (a double tap on the price axis does too). Shown only while manual.
          <Animated.View
            entering={FadeIn.duration(150).reduceMotion(ReduceMotion.System)}
            exiting={FadeOut.duration(120).reduceMotion(ReduceMotion.System)}
            // Beside the volume pane, where the price column is otherwise empty, so it never covers a price label.
            style={[s.autoWrap, { left: model.geometry.priceAxis.x + model.geometry.priceAxis.w / 2 - 13, top: model.geometry.volume ? model.geometry.volume.y + model.geometry.volume.h / 2 - 11 : model.geometry.plot.y + model.geometry.plot.h - 30 }]}
          >
            <Pressable
              accessibilityRole="button"
              accessibilityLabel="Auto stretch the price axis"
              hitSlop={8}
              onPress={() => {
                setVp(autoScalePrice);
                if (haptics) playHaptic("light");
              }}
              style={({ pressed }) => [s.autoButton, pressed && { backgroundColor: UI.row }]}
            >
              <Text style={s.autoText}>A</Text>
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
              <Icon name="toLatest" color={UI.text} />
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
  readout: { position: "absolute", top: 4 },
  chipWrap: { position: "absolute" },
  chip: { height: 30, paddingHorizontal: 12, borderRadius: 15, backgroundColor: UI.accent, alignItems: "center", justifyContent: "center", shadowColor: "#000", shadowOpacity: 0.35, shadowRadius: 6, shadowOffset: { width: 0, height: 2 }, elevation: 5 },
  chipText: { color: UI.accentFg, fontSize: 13, fontWeight: "700", fontVariant: ["tabular-nums"] },
  indicators: { flexDirection: "row", gap: 3 },
  count: { color: UI.text, fontSize: 11, fontVariant: ["tabular-nums"] },
  autoWrap: { position: "absolute" },
  autoButton: { width: 26, height: 22, borderRadius: 6, backgroundColor: UI.panel, alignItems: "center", justifyContent: "center", shadowColor: "#000", shadowOpacity: 0.3, shadowRadius: 4, shadowOffset: { width: 0, height: 1 }, elevation: 3 },
  autoText: { color: UI.textDim, fontSize: 11, fontWeight: "700", fontVariant: ["tabular-nums"] },
  latestButton: { width: 40, height: 40, borderRadius: 20, backgroundColor: UI.panel, alignItems: "center", justifyContent: "center", shadowColor: "#000", shadowOpacity: 0.35, shadowRadius: 8, shadowOffset: { width: 0, height: 2 }, elevation: 6 },
  plan: { position: "absolute", left: 0, right: 0, bottom: 0 },
});
