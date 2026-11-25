import {
  applyGesture,
  alertDragDeletes,
  alertDragPrice,
  alertPriceAt,
  alertTagPositions,
  formatPrice,
  hitAlert,
  stepTick,
  type AlertLayer,
  type ChartAlert,
  type DetectionOverlay,
  type EventMarker,
  type VolumeShelf,
  autoScalePrice,
  buildSlots,
  computeChartModel,
  serverIndicatorResults,
  classicMacd,
  emptyHistory,
  record,
  redo,
  undo,
  type History,
  DEFAULT_LAYERS,
  shownShelves,
  layerIdsFor,
  applyPaneGesture,
  hitTest,
  paneIdsOf,
  serverKeyLevels,
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
  type WatchedDrawing,
  canAlertOnDrawing,
  TIMEFRAME_MS,
  type DrawingDraft,
  type DrawingKind,
  type IndicatorColorActions,
  type IndicatorSpec,
  type IndicatorValuesLike,
  type ChartLevels,
  type AutoZone,
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
  type TimeFocus,
  type Viewport,
  DEFAULT_CHART_OPTIONS,
  DEFAULT_CHART_SETTINGS,
  manualPrice,
  openView,
  parseChartSettings,
  presetViewport,
  sessionSlots,
  type SessionSchedule,
  viewKey,
  easeOut,
  SETTINGS_MOTION_MS,
  type ChartSettings,
  parseDrawingSettings,
  toolAllowed,
  type DrawingSettings,
  brokerOf,
} from "@stockwatcher/chart-core";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode, type MouseEvent, type PointerEvent } from "react";
import { createPortal } from "react-dom";
import { TimeframeSelector } from "./TimeframeSelector.tsx";
import { CandleInspector } from "./CandleInspector.tsx";
import { ChartToolbar } from "./ChartToolbar.tsx";
import { drawDisplayList } from "./draw.ts";
import { DrawingToolbox } from "./DrawingToolbox.tsx";
import { Icon } from "./Icon.tsx";
import { OrderTicket } from "./OrderTicket.tsx";
import { BrokerNotice } from "./AccountBadge.tsx";
import { HoldRing } from "./HoldRing.tsx";
import { QuickAlertCard, type QuickAlertDraft } from "./QuickAlertCard.tsx";
import { PositionPlanCard } from "./PositionPlanCard.tsx";
import { Tip, TooltipProvider } from "./ui/tooltip.tsx";
import { TradeMenu, TRADE_MENU_SIZES } from "./TradeMenu.tsx";
import { PRICE_SCALE_LABELS, PriceAxisMenu } from "./PriceAxisMenu.tsx";
import { cn, TOUCH_HIT } from "./ui/utils.ts";
import { useCanvas, useChartTheme } from "./useCanvas.ts";
import { prefersReducedMotion, useTween } from "./useTween.ts";
import { useDrawingCue } from "./useDrawingCue.ts";

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
  /** The server's session schedule (Series.sessions, `sessions.v1`): regular hours and the shading follow it; absent = the fixed ET schedule. */
  sessions?: SessionSchedule;
}

export interface CandleChartProps {
  series: ChartSeriesLike | null | undefined;
  timeframe: Timeframe;
  /** The forming candle's tag in the readout; "Live" by default ("Forming" on the practice page). */
  formingLabel?: string;
  /** Rust-authored episode/level references; display timeframe never changes their meaning. */
  detection?: readonly DetectionOverlay[];
  /** Server-placed markers (the Insights ticker page: numbered sent alerts, missed-setup rings); read-only, absent = none. */
  eventMarkers?: readonly EventMarker[];
  /**
   * The window a fresh view opens on (epoch ms) instead of the zoom preset: the Insights ticker
   * page's server focus, the move rather than the whole day. Read once per view; the owner zooms
   * and pans out of it freely. Remount (a React key) to reopen on a new one.
   */
  initialFocus?: TimeFocus | null;
  /** Volume shelves from the server's latest Runners frame (chart-core layoutVolumeShelves); read-only, absent = none. */
  volumeShelves?: readonly VolumeShelf[];
  /** t of the inspected slot; null = latest. Controlled when onInspectChange is set. */
  inspectT?: number | null;
  onInspectChange?: (t: number | null) => void;
  /** Price axis scale type. Controlled when onPriceScaleTypeChange is set; otherwise it is the chart settings' scale. */
  priceScaleType?: PriceScaleType;
  onPriceScaleTypeChange?: (type: PriceScaleType) => void;
  /**
   * The chart settings (chart-core chartSettings.ts): type, scale, sessions,
   * volume, grid, zoom. Controlled when onSettingsChange is set; else kept
   * here and persisted with persistKey under `${persistKey}:settings`.
   */
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
  /**
   * Which buttons the floating drawing toolbox shows (the Settings dialog's
   * Drawing tab; chart-core drawingSettings.ts). The host owns it (the
   * dialog lives in the app header, not in the chart); without it the chart
   * reads `${persistKey}:drawing` once, so a saved choice still applies.
   * A tool that becomes hidden while armed is disarmed.
   */
  drawingSettings?: DrawingSettings;
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
  /**
   * Indicator line colours are global (owner, 2026-11-07): the app store's indicatorColorActions,
   * so a colour picked here recolours that line on every chart and device. Absent: a colour is
   * written onto this chart's own spec, as before.
   */
  indicatorColorActions?: IndicatorColorActions;
  /**
   * The server's values for `indicators` (`indicators.v1`: the feed's `indicators`, the store's
   * set for this chart's extended-hours setting), drawn as sent; the chart computes none itself.
   * null = asked, not answered yet (or no welcome yet): no lines. Absent = the server computes
   * none (older than indicators.v1): no lines, and a note that indicators need a server update.
   * Needs controlled `indicators`: the host sends the same list to the server.
   */
  serverIndicators?: IndicatorValuesLike | null;
  /**
   * The server computes the MACD styles (`indicators.macd.v2`; the feed's `macdStyles`): the
   * MACD settings offer them and they draw as chosen. Off, every MACD draws as classic
   * (`classicMacd`), which an older server sends whatever the spec says.
   */
  macdStyles?: boolean;
  /**
   * The server's key levels and opening ranges (`levels.v1`, or a package's own context): the
   * only ones drawn. null or absent (not answered yet) = none.
   */
  serverLevels?: ChartLevels | null;
  /**
   * The server's supply/demand zones for this chart's timeframe and extended-hours setting
   * (`zones.v1`, asked while the zones layer is on): drawn as sent; null or absent = none.
   */
  serverZones?: readonly AutoZone[] | null;
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
  /** Whether the floating drawing tools show before this chart's persisted choice exists (default true). Small charts (the dashboard's panel) start with them hidden; the pencil shows them. */
  defaultToolsVisible?: boolean;
  /** The trade menu's share size (one of TRADE_SIZES). Controlled when onTradeQtyChange is set (the app syncs it); else kept and persisted with persistKey. */
  tradeQty?: number;
  onTradeQtyChange?: (qty: number) => void;
  /** Which context layers show (halts, LULD, key levels, fills). Controlled when onLayersChange is set; else kept and persisted with persistKey. */
  layers?: ChartLayers;
  onLayersChange?: (layers: ChartLayers) => void;
  /** The feed clock (ms), for an ongoing halt's length. */
  now?: number | null;
  /** Show the timeframe picker in the toolbar row; the host switches the subscription. */
  onTimeframeChange?: (tf: Timeframe) => void;
  /** Show the inspected candle's readout (time, O/H/L/C, change, volume): at the chart's top-left, under `heading`. */
  inspector?: boolean;
  /** Shown at the chart's top-left corner (e.g. ticker, price and change), above the readout. */
  heading?: ReactNode;
  /** Render the toolbar into this element (e.g. a slot in the app header) instead of a row above the chart. */
  toolbarContainer?: HTMLElement | null;
  /** Put the whole toolbar (timeframes, divider, drawing tools, indicators, levels, trade) inside the chart, at its top-right. */
  toolbarInChart?: boolean;
  /**
   * One header row above the chart (owner, 2026-10-18, the dashboard cells): `heading` at the
   * toolbar's left and the timeframe picker with the tools on its right, instead of the heading
   * and the timeframes floating in the chart's top corners. Needs the toolbar row (`toolbar`, not
   * `toolbarInChart` nor `toolbarContainer`). The chart gets the corners back.
   */
  headingInToolbar?: boolean;
  /** The timeframe picker's form in the toolbar: four segments (default) or the compact scroll wheel. */
  timeframeControl?: "segments" | "wheel";
  /** Price alerts on this symbol: lines, the bell, hold to create, drag to move. Absent = off. */
  alerts?: ChartAlerts | null;
  /** Drawing alerts on this symbol: the touching cue, fire markers and the toolbox's bell. Absent = off. */
  drawingAlerts?: ChartDrawingAlerts | null;
}

/**
 * What the host wires for drawing alerts (chart-core drawingAlerts.ts; the server evaluates them).
 * The chart draws the cue from `watched` and its own drawings and slots; the toolbox's bell asks
 * the host to open its form for the selected drawing.
 */
export interface ChartDrawingAlerts {
  /** This symbol's drawing alerts, with any fire not yet acknowledged. */
  watched: readonly WatchedDrawing[];
  /** "Alert on this drawing" (the toolbox's bell), with the chart's timeframe (its slots place a sloped line). */
  onAlert: (drawing: Drawing, timeframe: Timeframe) => void;
}

/**
 * What the host wires for price alerts (the chart never talks to a server; the app stores the
 * alerts in its synced settings and the server evaluates them). Docs: chart-core alerts.ts.
 */
export interface ChartAlerts {
  /** This symbol's price alerts still watching (armed, cooling, snoozed), as lines. */
  lines: readonly ChartAlert[];
  /** The quick card's Create. */
  onCreate: (draft: QuickAlertDraft) => void;
  /** The quick card's "More…": the host's full form, prefilled. */
  onMore?: (draft: QuickAlertDraft) => void;
  /** A line dragged (or nudged with ↑/↓ and Enter) from `from` to `price`. */
  onMove: (id: string, price: number, from: number) => void;
  /** Dragged off the right edge, or Delete on a focused tag. */
  onDelete: (id: string) => void;
  /** Ctrl+Z in the chart when its last change was an alert's: the host undoes it (its toast's Undo). */
  onUndo?: () => void;
  /** The defaults a new alert gets, for the quick card ("Once · this week · in app + phone"). */
  defaultsText?: string;
}

/** What the host wires for chart trading: the simulator or the Alpaca paper route, whichever the state says (the chart never talks to a server). */
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
  formingLabel,
  inspectT: inspectProp,
  onInspectChange,
  priceScaleType: scaleProp,
  onPriceScaleTypeChange,
  settings: settingsProp,
  onSettingsChange,
  settingsFromRemote = false,
  drawingSettings: drawingSettingsProp,
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
  indicatorColorActions,
  serverIndicators,
  macdStyles = false,
  serverLevels,
  serverZones,
  drawings: drawingsProp,
  onDrawingsChange,
  persistKey,
  trading,
  toolboxPlacement: toolboxProp,
  onToolboxPlacementChange,
  tradeQty: tradeQtyProp,
  onTradeQtyChange,
  defaultToolsVisible = true,
  layers: layersProp,
  onLayersChange,
  now = null,
  onTimeframeChange,
  inspector = false,
  heading,
  toolbarContainer,
  toolbarInChart = false,
  headingInToolbar = false,
  timeframeControl = "segments",
  alerts = null,
  drawingAlerts = null,
  detection,
  eventMarkers,
  initialFocus,
  volumeShelves,
}: CandleChartProps) {
  const theme = useChartTheme(themeProp);
  // Price alerts (chart-core alerts.ts): a line being dragged or nudged from the keyboard (drawn at
  // its new price until the release commits it), the hold ring, the quick card, and which kind of
  // change Ctrl+Z should undo (the chart's drawings, or the host's last alert change).
  const [alertDrag, setAlertDrag] = useState<{ id: string; price: number; from: number; deleting: boolean } | null>(null);
  const [holdRing, setHoldRing] = useState<{ x: number; y: number } | null>(null);
  const [quick, setQuick] = useState<{ open: boolean; at: { x: number; y: number } | null; draft: QuickAlertDraft | null }>({ open: false, at: null, draft: null });
  const lastChange = useRef<"drawing" | "alert">("drawing");

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
    lastChange.current = "drawing";
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

  // Oscillator pane heights by indicator id: drag a pane's top border, double-click to expand (kept with persistKey).
  const [paneHeights, setPaneHeights] = useState<Record<string, number>>(() => {
    const v = loadJson<unknown>(persistKey && `${persistKey}:panes`, null);
    return v && typeof v === "object" ? Object.fromEntries(Object.entries(v).filter(([, h]) => typeof h === "number")) : {};
  });
  // Chart settings: controlled (the app syncs them across clients), or kept here and persisted next to the layers (`${persistKey}:settings`).
  const [ownSettings, setOwnSettings] = useState<ChartSettings>(() => parseChartSettings(loadJson<unknown>(persistKey && `${persistKey}:settings`, null)));
  const settings = onSettingsChange ? (settingsProp ?? DEFAULT_CHART_SETTINGS) : ownSettings;
  const setSettings = (next: ChartSettings) => {
    if (onSettingsChange) onSettingsChange(next);
    else {
      setOwnSettings(next);
      saveJson(persistKey && `${persistKey}:settings`, next);
    }
  };
  // Drawing toolbox contents: the host's (the Settings dialog, synced), else the saved choice (`${persistKey}:drawing`).
  const [savedDrawingSettings] = useState<DrawingSettings>(() => parseDrawingSettings(loadJson<unknown>(persistKey && `${persistKey}:drawing`, null)));
  const drawingSettings = drawingSettingsProp ?? savedDrawingSettings;
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
  const [toolsVisible, setToolsVisible] = useState<boolean>(() => loadJson<unknown>(persistKey && `${persistKey}:toolsVisible`, defaultToolsVisible) !== false);
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

  const scaleControlled = onPriceScaleTypeChange !== undefined;
  const scaleType = scaleControlled ? (scaleProp ?? "linear") : settings.priceScale;

  // Each symbol + timeframe gets a fresh view (chart-core viewKey): zoom/pan
  // positions and a manual price range do not carry to another ticker or to
  // a timeframe whose slots mean different durations. The fresh view is
  // OPENED from the saved settings (openView: the zoom preset, auto stretch)
  // as soon as its candles and the canvas size are known, so a new ticker
  // never needs resizing by hand. Preferences (scale type, chart type) carry.
  const key = viewKey(symbol, timeframe);
  // `fitFor`: the size a focus-opened view was fitted to, until the owner moves it (any setVp).
  // The first measured size is not always the settled one (a container query or a scroll bar
  // lands a frame later), and a resize keeps the spacing and right edge, which would cut the
  // start of the focus window off; so an untouched focus view is refitted to the new size.
  type View = { key: string; vp: Viewport; opened: boolean; fitFor: Size | null };
  const [view, setView] = useState<View>(() => ({ key, vp: defaultViewport(barSpacing), opened: false, fitFor: null }));
  const fresh = useCallback((): View => ({ key, vp: defaultViewport(barSpacing), opened: false, fitFor: null }), [key, barSpacing]);
  const current = view.key === key ? view : fresh();
  const vp = current.vp;
  const setVp = useCallback((f: (vp: Viewport) => Viewport) => setView((v) => {
    const b = v.key === key ? v : fresh();
    return { ...b, vp: f(b.vp), fitFor: null };
  }), [key, fresh]);

  const [pointer, setPointer] = useState<Point | null>(null);
  const [announcement, setAnnouncement] = useState("");
  // The anchor outlives `open` so the menu can animate out where it opened.
  const [menu, setMenu] = useState<{ open: boolean; at: { x: number; y: number } | null }>({ open: false, at: null });

  // Every slot, and the ones shown: with extended hours off only the regular session (no gaps
  // between days). Indicators are computed on the shown list so they line up by index; key levels
  // and the opening range read every slot, so the premarket high/low survives hiding the premarket.
  const allSlots = useMemo(() => (series ? buildSlots(series.candles, series.gaps) : []), [series]);
  const slots = useMemo(() => sessionSlots(allSlots, settings.extendedHours, series?.sessions), [allSlots, settings.extendedHours, series?.sessions]);
  const visibility = settings.visibility;
  // The server's values only (indicators.v1). A server without the feature draws no lines and
  // says why (`indicatorsNeedUpdate`), rather than the chart computing its own.
  const indicatorResults = useMemo(() => (!visibility.indicators || serverIndicators === undefined ? [] : serverIndicatorResults(slots, macdStyles ? indicatorSpecs : classicMacd(indicatorSpecs), serverIndicators)), [slots, indicatorSpecs, visibility.indicators, serverIndicators, macdStyles]);
  const indicatorsNeedUpdate = visibility.indicators && serverIndicators === undefined && indicatorSpecs.length > 0;

  const last = useMemo(() => lastCandle(slots)?.c ?? null, [slots]);
  // Levels, ranges and zones are the server's only (levels.v1, zones.v1, or a package's own
  // context): the web always ships with its server, so it never reads them off its candles (the
  // phone's chart still does, for installed builds on an older server). None sent, none drawn.
  const levels = useMemo(() => (serverLevels ? serverKeyLevels(serverLevels) : []), [serverLevels]);
  const ranges = useMemo(() => (!layers.openingRange ? [] : (serverLevels?.openingRanges ?? [])), [layers.openingRange, serverLevels]);
  const zones = useMemo(() => (!layers.autoZones ? [] : (serverZones ?? [])), [layers.autoZones, serverZones]);
  // The baseline chart's reference: the previous close (the server's, else the one read off the candles).
  const baselinePrice = useMemo(() => levels.find((l) => l.id === "prevClose")?.price ?? null, [levels]);
  const fills = useMemo(() => (trading?.state?.fills ?? []).filter((f) => f.symbol === symbol), [trading?.state, symbol]);
  const context = useMemo(
    () => ({
      levels: levels.filter((l) => layers[l.layer]),
      openingRanges: ranges,
      autoZones: zones,
      halts: layers.halts ? (series?.halts ?? []) : [],
      luld: layers.luld ? (series?.luld ?? []) : [],
      fills: layers.fills ? fills : [],
      shelves: shownShelves(layers, volumeShelves),
      now,
    }),
    [levels, ranges, zones, layers, series, fills, volumeShelves, now],
  );
  const orders = useMemo(() => (trading?.state?.orders ?? []).filter((o) => o.symbol === symbol && isWorking(o)), [trading?.state, symbol]);
  const position = useMemo(() => trading?.state?.positions.find((p) => p.symbol === symbol) ?? null, [trading?.state, symbol]);
  // The "+" place button follows a hovering mouse (touch opens the ticket from the price axis menu or Trade).
  const placeY = trading && !tool && !drag && pointer && gesturesRef.current?.lastPointerType === "mouse" ? pointer.y : null;
  // The bell beside it, at the same y (approved design: "+" opens a trade, the bell an alert).
  const bellY = alerts && !tool && !drag && !alertDrag && pointer && gesturesRef.current?.lastPointerType === "mouse" ? pointer.y : null;
  // Robinhood's visibility switches hide drawings or orders without deleting them; hidden ones are not hit-tested either.
  const shownDrawings = useMemo(() => {
    const own = visibility.drawings ? drawings : [];
    const list = drag?.drawing ? own.map((d) => (d.id === drag.drawing!.id ? drag.drawing! : d)) : own;
    return measurement ? [...list, measurement] : list;
  }, [drawings, drag, measurement, visibility.drawings]);
  const showTrading = !!trading && visibility.trading;
  // Drawing alerts' cue (chart-core drawingAlerts.ts): computed against the drawings as shown, so
  // a hidden drawing has no glow either.
  const drawingCues = useDrawingCue(drawingAlerts?.watched, shownDrawings, slots, TIMEFRAME_MS[timeframe], last);
  const selectedDrawing = selected?.kind === "drawing" ? selected.id : null;
  const alertLines = alerts?.lines;
  const alertLayer: AlertLayer | null = useMemo(
    () =>
      alertLines
        ? { alerts: alertLines, dragging: alertDrag ? { id: alertDrag.id, price: alertDrag.price, deleting: alertDrag.deleting } : null, bellY, besidePlace: showTrading && placeY !== null, last }
        : null,
    [alertLines, alertDrag, bellY, showTrading, placeY, last],
  );

  // The heading and timeframes live in the toolbar row instead of the chart's corners.
  const headerRow = headingInToolbar && toolbar && !toolbarInChart && !toolbarContainer;
  const cornerHeading = headerRow ? null : heading;
  // The top-left overlay (heading + readout) takes the corner; the indicator legend moves below it.
  const overlayTop = (cornerHeading ? 22 : 0) + (inspector ? 20 : 0);
  // The timeframe picker sits at the top-right inside the chart; top-edge tags start below it.
  const topRight = onTimeframeChange && !headerRow ? 34 : 0;
  const inChartProfile = !!options?.showProfile;
  // The volume pane slides in or out when its setting flips (0 drops the pane, and the plot takes the room).
  const volumeTarget = settings.volume ? (options?.volumeHeight ?? DEFAULT_CHART_OPTIONS.volumeHeight) : 0;
  const volumeHeight = Math.round(useTween(volumeTarget));
  const chartOptions = useMemo(
    () => ({
      ...options,
      volumeHeight,
      // The legend also clears the in-chart timeframe picker's band: on a narrow chart the inline legend
      // reaches the right side, and without a heading it would start inside that band, under the picker.
      ...(overlayTop || topRight ? { legendTop: Math.max(overlayTop ? overlayTop + 6 : 0, topRight ? topRight + 4 : 0) } : {}),
      ...(topRight ? { topRightInset: topRight } : {}),
      // The Levels menu's "Volume profile" switch turns the in-chart profile off.
      ...(inChartProfile && !layers.profile ? { showProfile: false } : {}),
      lastPriceLine: layers.lastPrice,
    }),
    [options, volumeHeight, overlayTop, topRight, inChartProfile, layers.profile, layers.lastPrice],
  );
  const chartStyle = useMemo(() => ({ grid: settings.grid, sessionShading: settings.sessionShading, sessionBreaks: settings.sessionBreaks, highLow: settings.highLow, indicatorLegend: settings.indicatorLegend, legendHidden: settings.legendHidden }), [settings.grid, settings.sessionShading, settings.sessionBreaks, settings.highLow, settings.indicatorLegend, settings.legendHidden]);
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
      options: chartOptions,
      priceScaleType: scaleType,
      indicators: indicatorResults,
      drawings: shownDrawings,
      selectedDrawing,
      draft,
      trading: showTrading ? { orders, position, last, dragging: drag?.order ? { id: drag.order.id, price: drag.order.price } : null, placeY } : null,
      alerts: alertLayer,
      drawingCues,
      detection,
      eventMarkers,
      context,
      now,
      paneHeights,
      chartType: settings.chartType,
      chartStyle,
      baselinePrice,
      extendedHours: settings.extendedHours,
      ...(series?.sessions ? { sessions: series.sessions } : {}),
    }),
    [timeframe, series, slots, inspectT, pointer, tradeMenuOpen, tradeMenuAt, chartOptions, scaleType, indicatorResults, shownDrawings, selectedDrawing, draft, showTrading, orders, position, last, drag, placeY, alertLayer, detection, eventMarkers, context, now, paneHeights, settings.chartType, chartStyle, baselinePrice, settings.extendedHours, drawingCues],
  );

  const paint = useCallback((ctx: CanvasRenderingContext2D, size: Size) => drawDisplayList(ctx, layoutChart(computeChartModel(state, vp, size), state, theme)), [state, vp, theme]);
  const { containerRef, canvasRef, size } = useCanvas(paint);
  const model = useMemo(() => computeChartModel(state, vp, size), [state, vp, size]);

  // Open a fresh view (new ticker or timeframe) from the saved settings once its candles and the
  // canvas size exist. Before the snapshot arrives there is nothing to fit, so it waits.
  useLayoutEffect(() => {
    if (!slots.length || size.width <= 0) return;
    const refit = current.opened && initialFocus && current.fitFor && (current.fitFor.width !== size.width || current.fitFor.height !== size.height);
    if (current.opened && !refit) return;
    setView({ key, vp: openView(state, settings, size, defaultViewport(barSpacing), initialFocus), opened: true, fitFor: initialFocus ? { width: size.width, height: size.height } : null });
  });

  // Choosing a zoom preset, or showing/hiding extended hours (which changes what "the day" is),
  // slides the zoom to the preset's spacing, following the latest candle; toggling auto stretch
  // switches the price range between auto and the current range held. Not on first render.
  const stateRef = useRef(state);
  stateRef.current = state;
  const sizeRef = useRef(size);
  sizeRef.current = size;
  const prevSettings = useRef(settings);
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
    if (p.zoom !== settings.zoom || p.extendedHours !== settings.extendedHours) zoomToPreset(settings);
    // zoomToPreset reads refs only; settings is the trigger.
  }, [settings]);

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
    else setSettings({ ...settings, priceScale: t });
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
    // Grab priority (approved design): orders and positions, then alerts, then drawings.
    if (tradingLayer) {
      const t = hitTrading(m, tradingLayer, x, y, tol);
      if (t) return t;
    }
    if (alertLayer) {
      const a = hitAlert(m, alertLayer, x, y, tol, touch);
      if (a) return a;
    }
    if (!visibility.drawings) return null;
    const d = hitDrawing(m, drawings, x, y, tol, selectedDrawing);
    return d ? { kind: "drawing", ...d } : null;
  };

  const openTicket = (price: number | null, clientY: number | null, clientX?: number) => {
    const r = canvasRef.current!.getBoundingClientRect();
    setTicket({ open: true, at: { x: clientX ?? r.right - 4, y: clientY ?? r.top + 40 }, price });
  };

  // The menu's share size: controlled (the app syncs it across devices), or remembered here
  // between uses (and across reloads with persistKey).
  const [ownTradeQty, setOwnTradeQty] = useState<number>(() => {
    const q = loadJson<unknown>(persistKey && `${persistKey}:tradeQty`, 100);
    return typeof q === "number" && (TRADE_MENU_SIZES as readonly number[]).includes(q) ? q : 100;
  });
  const tradeQty = onTradeQtyChange ? (tradeQtyProp ?? TRADE_MENU_SIZES[0]) : ownTradeQty;
  const changeTradeQty = (q: number) => {
    if (onTradeQtyChange) onTradeQtyChange(q);
    else {
      setOwnTradeQty(q);
      saveJson(persistKey && `${persistKey}:tradeQty`, q);
    }
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

  // Opens the quick alert card at a height (chart px), with the direction the price means.
  const openQuickAlert = (y: number, x?: number) => {
    if (!alerts) return;
    const price = alertPriceAt(model, y);
    if (price === null) return;
    const r = canvasRef.current!.getBoundingClientRect();
    setQuick({ open: true, at: { x: r.left + (x ?? model.geometry.plot.x + model.geometry.plot.w - 40), y: r.top + y }, draft: { price, dir: last === null || price >= last ? "above" : "below" } });
  };

  const onObjectDrag = (a: Extract<GestureAction, { kind: "objectDrag" }>, m: ChartModel) => {
    const t = a.target;
    if (t.kind === "alert") {
      const line = alerts?.lines.find((l) => l.id === t.id);
      if (!line || !alerts) return;
      const price = alertDragPrice(m, line.price, a.to.y - a.from.y, a.fine) ?? line.price;
      const deleting = !a.cancelled && alertDragDeletes(m, a.from, a.to);
      if (!a.done) {
        setAlertDrag({ id: t.id, price, from: line.price, deleting });
        return;
      }
      setAlertDrag(null);
      if (a.cancelled) return;
      lastChange.current = "alert";
      if (deleting) alerts.onDelete(t.id);
      else if (price !== line.price) alerts.onMove(t.id, price, line.price);
      return;
    }
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
        case "holdRing":
          setHoldRing(a.point);
          break;
        case "holdAlert":
          setPointer({ x: a.x, y: a.y });
          openQuickAlert(a.y, a.x);
          break;
        case "placeAlert":
          openQuickAlert(a.y);
          break;
        case "liftChip":
          // The web keeps a touch scrub as it was (the phone offers the chip; chart-native).
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
        case "resizePane":
        case "togglePane": {
          const ids = paneIdsOf({ indicators: indicatorResults });
          const persist = a.kind === "togglePane" || a.done;
          setPaneHeights((h) => {
            const next = a.kind === "resizePane" && a.done ? h : applyPaneGesture(h, a, m, ids, size);
            if (persist) saveJson(persistKey && `${persistKey}:panes`, next);
            return next;
          });
          break;
        }
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
  const gestureCtx = (m: ChartModel): GestureContext => ({ model: m, inspectT, priceManual: !auto0, tool, hitObject: hitObject(m), tradeMenu: !!trading, holdAlert: !!alerts });
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
        gestures.state === "paneResize" || (gestures.state === "idle" && hitTest(model, p.x, p.y).region === "paneBorder")
          ? "row-resize"
          : gestures.state === "drag"
          ? "grabbing"
          : tool
            ? "crosshair"
            : hit?.kind === "order" || hit?.kind === "alert"
              ? hit.kind === "order" && hit.part === "cancel"
                ? "pointer"
                : "ns-resize"
              : hit?.kind === "place" || hit?.kind === "placeAlert"
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

  // The preset slide: barSpacing eases to the preset's (220 ms, instant with reduced motion) while
  // following the latest candle; with auto stretch off the range is re-fitted once at the end and
  // held. Shares `slide` with back-to-latest, so a press on the chart stops either.
  const zoomToPreset = (s: ChartSettings) => {
    stopSlide();
    const st = stateRef.current;
    const m = computeChartModel(st, vp, sizeRef.current);
    const target = presetViewport(vp, m.slots, timeframe, s.extendedHours, s.zoom, m.geometry.plot.w);
    const finish = () => {
      setVp((v) => {
        const done = { ...v, barSpacing: target.barSpacing, anchorT: null, anchorFrac: 0 };
        if (s.autoStretch) return done;
        const fit = computeChartModel(stateRef.current, autoScalePrice(done), sizeRef.current).price;
        return fit ? manualPrice(done, { lo: fit.lo, hi: fit.hi }) : done;
      });
      slide.current = null;
    };
    const from = vp.barSpacing;
    if (prefersReducedMotion() || Math.abs(from - target.barSpacing) < 1e-3) {
      finish();
      return;
    }
    const start = performance.now();
    const run = { frame: 0 };
    const frame = (now: number) => {
      const elapsed = now - start;
      if (elapsed >= SETTINGS_MOTION_MS) {
        finish();
        return;
      }
      const bs = easeOut(from, target.barSpacing, elapsed, SETTINGS_MOTION_MS);
      setVp((v) => ({ ...v, barSpacing: bs, anchorT: null, anchorFrac: 0 }));
      run.frame = requestAnimationFrame(frame);
    };
    run.frame = requestAnimationFrame(frame);
    slide.current = run;
  };

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

  // Hiding the armed tool in Settings disarms it: no invisible tool waits for the next click (chart-core armedAfter).
  useEffect(() => {
    if (!toolAllowed(drawingSettings, tool)) changeTool(null);
    // changeTool is recreated every render; the check only needs to run when the settings or the tool change.
  }, [drawingSettings, tool]);

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
    // Escape while dragging an object puts it back (an alert line, an order, a drawing).
    if (e.key === "Escape" && gestures.state === "drag") {
      e.preventDefault();
      apply(gestures.cancelDrag(), model);
      return;
    }
    // Alt+A: a new alert at the crosshair's price (the last price without one).
    if (alerts && e.altKey && !e.ctrlKey && !e.metaKey && (e.code === "KeyA" || e.key === "a" || e.key === "A")) {
      e.preventDefault();
      const lastY = last !== null && model.price ? Math.min(model.geometry.plot.y + model.geometry.plot.h - 1, Math.max(model.geometry.plot.y + 1, priceY(model.price, last))) : null;
      const y = pointer && pointer.y >= model.geometry.plot.y && pointer.y <= model.geometry.plot.y + model.geometry.plot.h ? pointer.y : lastY;
      if (y !== null) openQuickAlert(y, pointer?.x);
      return;
    }
    // Escape unwinds one layer at a time: a drawing being placed, the tool, the selection, then inspection.
    if ((e.ctrlKey || e.metaKey) && !e.altKey && (e.key === "z" || e.key === "Z" || e.key === "y" || e.key === "Y")) {
      e.preventDefault();
      const redoKey = e.key.toLowerCase() === "y" || e.shiftKey;
      // The chart's last change was an alert's (a drag, a delete): the host's Undo takes it back.
      if (!redoKey && lastChange.current === "alert" && alerts?.onUndo) {
        lastChange.current = "drawing";
        alerts.onUndo();
        return;
      }
      stepHistory(redoKey ? "redo" : "undo");
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
      // A tool hidden in Settings cannot be armed from the keyboard either (it would be armed invisibly).
      const k = e.key.toLowerCase() === "l" ? "long" : "short";
      if (toolAllowed(drawingSettings, k)) changeTool(k);
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

  const toolbarEl = (
      <ChartToolbar
        toolsVisible={toolsVisible}
        onToolsVisibleChange={changeToolsVisible}
        timeframe={toolbarInChart || headerRow ? timeframe : undefined}
        onTimeframeChange={toolbarInChart || headerRow ? onTimeframeChange : undefined}
        timeframeControl={timeframeControl}
        // The same 8 px inset as the readout and the legends under it, so the header's ticker lines up with them.
        // No heading (the phone chart page keeps its ticker in the page header): no empty start to take room.
        start={headerRow && heading ? <div className="pl-2">{heading}</div> : undefined}

        tool={tool}
        onToolChange={changeTool}
        indicators={indicatorSpecs}
        onIndicatorsChange={setIndicators}
        indicatorColorActions={indicatorColorActions}
        macdStyles={macdStyles}
        theme={theme}
        seriesColors={[theme.series1, theme.series2, theme.series3]}
        drawingTools={false}
        canDelete={selected?.kind === "drawing"}
        onDelete={deleteSelected}
        trading={trading ? { position, last, onTrade: () => openTicket(null, null), fills, realized: trading.state?.account.realizedPnl ?? 0, broker: brokerOf(trading.state) } : null}
        layers={layers}
        layerIds={layerIdsFor(inChartProfile)}
        onLayersChange={setLayers}
      />
  );

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
          alert={
            alerts && model.price && menu.at
              ? {
                  price: formatPrice(snapPrice(yPrice(model.price, menu.at.y - canvasRef.current!.getBoundingClientRect().top))),
                  onAlertAt: () => openQuickAlert(menu.at!.y - canvasRef.current!.getBoundingClientRect().top),
                }
              : undefined
          }
        />
      )}
      {holdRing && <HoldRing x={holdRing.x} y={holdRing.y} />}
      {alerts && alertLayer && model.price && (
        // Each alert's axis tag as a focusable control (the canvas draws it; this is its keyboard
        // and screen reader twin): ↑/↓ move it one tick, Enter saves, Esc puts it back, Delete
        // removes. Pointer events pass through to the canvas, which owns the drag.
        <div className="pointer-events-none absolute inset-0 z-[4]">
          {alertTagPositions(model, alertLayer).map((t) => {
            const line = alerts.lines.find((l) => l.id === t.id);
            if (!line) return null;
            const g = model.geometry;
            return (
              <button
                key={t.id}
                type="button"
                aria-label={`Alert ${line.label}. Up and down arrows move it one tick, Enter saves, Delete removes.`}
                className="absolute rounded opacity-0 outline-none focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-ring"
                style={{ left: g.priceAxis.x + 2, top: t.y - 9, width: g.priceAxis.w - 2, height: 18 }}
                onBlur={() => setAlertDrag((d) => (d?.id === t.id ? null : d))}
                onKeyDown={(e) => {
                  const cur = alertDrag?.id === t.id ? alertDrag.price : line.price;
                  if (e.key === "ArrowUp" || e.key === "ArrowDown") {
                    e.preventDefault();
                    setAlertDrag({ id: t.id, price: stepTick(cur, e.key === "ArrowUp" ? 1 : -1), from: line.price, deleting: false });
                  } else if (e.key === "Enter") {
                    e.preventDefault();
                    setAlertDrag(null);
                    if (cur !== line.price) {
                      lastChange.current = "alert";
                      alerts.onMove(t.id, cur, line.price);
                    }
                  } else if (e.key === "Escape" && alertDrag?.id === t.id) {
                    e.preventDefault();
                    setAlertDrag(null);
                  } else if (e.key === "Delete" || e.key === "Backspace") {
                    e.preventDefault();
                    setAlertDrag(null);
                    lastChange.current = "alert";
                    alerts.onDelete(t.id);
                    canvasRef.current?.focus();
                  }
                }}
              />
            );
          })}
        </div>
      )}
      {alerts && (
        <QuickAlertCard
          open={quick.open}
          onOpenChange={(open) => {
            setQuick((q) => ({ ...q, open }));
            if (!open) setPointer(null);
          }}
          anchor={quick.at}
          symbol={symbol}
          draft={quick.draft}
          last={last}
          defaultsText={alerts.defaultsText}
          onCreate={alerts.onCreate}
          onMore={alerts.onMore}
          finalFocus={canvasRef}
        />
      )}
      {!auto && model.price && (
        // Manual price range: a small "A" at the foot of the price column returns to auto stretch
        // (TradingView's and Legend's auto button). Shown only while manual, so it costs nothing at rest.
        <Tip label="Auto stretch (A, or double-click the price axis)" side="left">
          <button
            type="button"
            aria-label="Auto stretch the price axis"
            onClick={() => {
              setAnnouncement("Price axis auto-scaled.");
              setVp(autoScalePrice);
            }}
            // Beside the volume pane, where the price column is otherwise empty, so it never covers a price label.
            style={{ left: model.geometry.priceAxis.x + model.geometry.priceAxis.w / 2 - 10, top: model.geometry.volume ? model.geometry.volume.y + model.geometry.volume.h / 2 - 10 : model.geometry.plot.y + model.geometry.plot.h - 24 }}
            className={cn(TOUCH_HIT, "absolute z-10 grid size-5 place-items-center rounded-md bg-card font-mono text-[11px] font-semibold text-muted-foreground shadow-sm transition-[opacity,scale,color,background-color] duration-150 outline-none hover:bg-bg-row hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring starting:scale-90 starting:opacity-0")}
          >
            A
          </button>
        </Tip>
      )}
      {pannedBack && (
        <Tip label="Back to latest (End)" side="left">
        <button
          type="button"
          aria-label="Back to latest"
          onClick={backToLatest}
          // Bottom-right of the price pane, clear of the price axis and the volume bars.
          style={{ left: model.geometry.plot.x + model.geometry.plot.w - 40, top: model.geometry.plot.y + model.geometry.plot.h - 40 }}
          className={cn(TOUCH_HIT, "absolute z-10 grid size-8 place-items-center rounded-full bg-card text-foreground shadow-lg transition-[opacity,scale,background-color] duration-150 outline-none hover:bg-bg-row focus-visible:ring-2 focus-visible:ring-ring starting:scale-90 starting:opacity-0")}
        >
          <Icon name="toLatest" width={14} height={14} />
        </button>
        </Tip>
      )}
      {(cornerHeading || inspector) && (
        // Top-left corner of the chart: the ticker line, then the readout, left-aligned. It does not
        // take pointer events, so the chart under it still pans, scrubs and inspects.
        // Transparent (owner, 2026-10-17): the text sits straight on the chart. Its text starts 8 px
        // in (left-0.5 + px-1.5), the indicator legends' inset (chart-core layout.ts `plot.x + 8`), so
        // the ticker, the readout and the legends share one left edge (owner, 2026-10-18).
        <div className="pointer-events-none absolute top-1 left-0.5 z-[5] flex max-w-[calc(100%-120px)] flex-col gap-0.5 px-1.5 py-1">
          {cornerHeading}
          {/* One line at any width: items never shrink or wrap inside (a phone-width chart wrapped "O" above its value, onto the legend under it). Items that do not fit wrap onto a second line that the one-line height hides, so the row drops whole items from its end (volume, "Live") instead of cutting one in half past the chart's edge (2026-10-19, phones). */}
          {inspector && (
            <CandleInspector className="pointer-events-auto max-h-[1lh] flex-wrap overflow-hidden whitespace-nowrap [&>*]:shrink-0" slot={inspectT === null ? (slots.at(-1) ?? null) : (slots[slotIndexOf(slots, inspectT) ?? -1] ?? null)} timeframe={timeframe} formingLabel={formingLabel} />
          )}
        </div>
      )}
      {(toolbarInChart && toolbar) || (onTimeframeChange && !headerRow) ? (
        // Top-right corner of the chart, just inside the price axis: the whole toolbar when it lives
        // in the chart (timeframes, divider, icons on one line), else the timeframes alone.
        <div className="absolute top-1.5 z-[5]" style={{ right: model.options.priceAxisWidth + 6 }}>
          {toolbarInChart && toolbar ? toolbarEl : onTimeframeChange ? <TimeframeSelector value={timeframe} onValueChange={onTimeframeChange} /> : null}
        </div>
      ) : null}
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
          canAlert={!!drawingAlerts && !!selectedDrawingObj && canAlertOnDrawing(selectedDrawingObj)}
          alerted={!!selectedDrawingObj && !!drawingAlerts?.watched.some((w) => w.drawingId === selectedDrawingObj.id)}
          onAlert={() => selectedDrawingObj && drawingAlerts?.onAlert(selectedDrawingObj, timeframe)}
          canUndo={history.past.length > 0}
          canRedo={history.future.length > 0}
          onUndo={() => stepHistory("undo")}
          onRedo={() => stepHistory("redo")}
          settings={drawingSettings}
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
          account={trading.state}
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
          account={trading?.state}
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
          account={trading.state}
        />
      )}
      {trading && <BrokerNotice state={trading.state} className="absolute bottom-3 left-1/2 z-20 -translate-x-1/2" />}
      {indicatorsNeedUpdate && (
        // Quiet text on the plot's bottom-left, where nothing else sits: a fact about the server,
        // not an action (the web and the server ship together, so only a phone build or a
        // desktop app newer than its server meets it). Does not take pointer events.
        <p role="status" className="pointer-events-none absolute z-[5] max-w-[calc(100%-6rem)] truncate rounded bg-background/80 px-1.5 py-0.5 text-[11px] text-muted-foreground" style={{ left: model.geometry.plot.x + 6, top: model.geometry.plot.y + model.geometry.plot.h - 24 }}>
          Indicators need a server update
        </p>
      )}
    </div>
  );

  if (!toolbar) return <TooltipProvider>{canvasEl}</TooltipProvider>;
  // With a host slot (the app header) the toolbar is portalled there and the chart fills its box.
  // The tree keeps the same shape either way: the slot appears a render later, and moving the
  // canvas to a new parent would remount it under the size observer and leave it at 0×0.
  return (
    <TooltipProvider>
    {toolbarContainer && !toolbarInChart ? createPortal(toolbarEl, toolbarContainer) : null}
    <div className={cn("flex min-h-0 min-w-0 flex-col", !toolbarContainer && !toolbarInChart && "gap-1.5", className)}>
      {toolbarContainer || toolbarInChart ? null : toolbarEl}
      {canvasEl}
    </div>
    </TooltipProvider>
  );
}
