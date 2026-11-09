import {
  applyIndicatorColors,
  diffDrawings,
  drawingsFromDoc,
  drawingsNs,
  indicatorColorPatch,
  migrateSpecColors,
  parseChartSettings,
  parseDrawingSettings,
  parseIndicatorColors,
  parseIndicatorMemory,
  parseIndicators,
  parseLayers,
  rememberIndicators,
  resetIndicatorColorsPatch,
  stripIndicatorColors,
  SYNC_NS,
  type ChartLayers,
  type ChartSettings,
  type ColorRole,
  type Drawing,
  type DrawingSettings,
  type IndicatorColorActions,
  type IndicatorColors,
  type IndicatorSpec,
  type Timeframe,
  type ToolboxPlacement,
} from "@stockwatcher/chart-core";
import { parseNoticeChartContext, pushRecentTicker, type NoticeChartContext, type AlertEventWire, type AlertsServerMessage, type AlertsStatusWire, type SettingsSync } from "@stockwatcher/shared";
import { createStore, type StoreApi } from "zustand/vanilla";
import { parseBrokerStatus, parseTradingAccount, TRADING_NS, type BrokerStatus, type LiveAccount, type TradingAccount } from "./trading.ts";
import { ALERTS_NS, editedAlert, mergeActivity, parseAlertPrefs, parseAlertRules, parseAlertStates, type AlertPrefs, type AlertRule, type AlertStateWire } from "./alerts.ts";
import {
  APP_NS,
  MAX_DASHBOARD_CHARTS,
  movedFavorite,
  parseDashboard,
  parseFavorites,
  parseSearchRecents,
  toggledFavorites,
  parseSymbol,
  parseTradeQty,
  parseView,
  type DashboardSlot,
  type DashboardSource,
  type DashboardState,
  type ViewState,
} from "./namespaces.ts";

// Every client's one store (owner, 2026-10-18: "global state in all clients"; Zustand chosen over
// Redux Toolkit). The web app, the desktop shell (the same web app) and the phone each create
// one with createAppStore and read it through a one-line `useApp` hook (zustand's useStore), so
// the state's shape and its rules live here once instead of in App.tsx and ChartScreen.tsx.
// docs/architecture/app-state.md.
//
// Three kinds of state, each with one owner:
// - SYNCED (the person's, on every device): mirrored from the settings sync engine
//   (@stockwatcher/shared settingsSync). A setter writes to the engine and the store follows the
//   engine's change event, so a local edit and another device's edit take the same path and the
//   store can never disagree with what will be sent.
// - DEVICE (this screen's layout): toolbox place, pane heights, tools shown. Persisted by the host
//   through `device.save`; the web keeps these inside its CandleChart instead (persistKey).
// - SESSION (this run only): open sheets and dialogs, the inspected candle, the shown symbol.
//
// Market data (series, tape, clock, quotes, paper state) is NOT here: chart-core's ChartStore and
// the feed hooks already hold it as external stores, and the replay clock is the server's, shared
// by every client by construction.

export interface DevicePrefs {
  toolbox: ToolboxPlacement;
  paneHeights: Record<string, number>;
  toolsVisible: boolean;
}

export interface AppState {
  // Synced.
  /**
   * The main chart's indicators. This list, `tickerIndicators` and every dashboard slot's own list
   * are shown with the GLOBAL line colours applied (`indicatorColors`, chart-core
   * applyIndicatorColors), so every chart draws a line in the same colour; their stored per-spec
   * colours are ignored once the global map exists and dropped when the list is next written.
   */
  indicators: IndicatorSpec[];
  /**
   * Every indicator line's colour, by identity and role (SYNC_NS.indicatorColors, chart-core
   * indicatorColors.ts); null until the namespace exists (until then per-spec colours apply).
   */
  indicatorColors: IndicatorColors | null;
  /**
   * The settings of indicators turned off, by id (SYNC_NS.indicatorMemory): turning one back on
   * restores them (rememberIndicators). Shared by the main chart, the dashboard charts and the
   * ticker page: an indicator's settings are the person's, whichever chart turned it off.
   */
  indicatorMemory: Record<string, IndicatorSpec>;
  /** The Insights ticker page chart's own indicators (SYNC_NS.tickerIndicators). */
  tickerIndicators: IndicatorSpec[];
  layers: ChartLayers;
  chartSettings: ChartSettings;
  /** The last chart settings change came from another device: the chart applies it without re-zooming (settings-sync.md). */
  chartSettingsFromRemote: boolean;
  drawingSettings: DrawingSettings;
  /** The ticker last opened on any device (null until one is). The chart may show another while it is outside the price band: see `shownSymbol`. */
  symbol: string | null;
  timeframe: Timeframe;
  bin: number;
  tradeQty: number;
  searchRecents: string[];
  /** The Favorites panel's tickers, in the owner's order (APP_NS.favorites). */
  favorites: string[];
  /** The shown symbol's drawings. */
  drawings: Drawing[];
  /** The dashboard's multi-chart panel: chart count and each slot's ticker and timeframe. */
  dashboard: DashboardState;
  /** Price alerts (alerts.ts), oldest first; their server-written states by id; the phone preferences. */
  alerts: AlertRule[];
  alertStates: Record<string, AlertStateWire>;
  alertPrefs: AlertPrefs;
  /** Which account the live market's orders go to (Settings → Trading), and the server's word on Alpaca paper. */
  tradingAccount: TradingAccount;
  brokerStatus: BrokerStatus;

  // Server-pushed (the alerts service; not synced, not persisted: every connection gets it again).
  /** Activity, newest first (fires and hold-backs). */
  alertActivity: AlertEventWire[];
  /** Phone notifications today, the budget, registered phones; null before the first frame. */
  alertStatus: AlertsStatusWire | null;
  /** The last fire this client heard about live (not from a snapshot): hosts toast it. */
  lastFired: AlertEventWire | null;
  /**
   * Drawing alerts' fires heard live and not yet acknowledged, by alert id: the chart keeps each
   * one marked on its candle (chart-core drawingAlerts.ts) until the person acknowledges it
   * (`acknowledgeDrawingFire`). Session state on purpose: a fire seen on another device or before
   * a reload is in Activity, not re-flashed.
   */
  drawingFires: Record<string, { t: number; price: number; tone: "up" | "down" | "stop"; eventId: string }>;

  // Device.
  device: DevicePrefs | null;

  // Session.
  /** What the chart actually shows (the host decides: the synced symbol, or a fallback candidate); drawings follow it. */
  shownSymbol: string;
  inspectT: number | null;
  settingsOpen: boolean;
  searchOpen: boolean;
  tapeOpen: boolean;
  daysOpen: boolean;
  /** The dashboard chart a picked ticker loads into (0-based; always below `dashboard.count`). Per device. */
  dashboardSelected: number;
  /** One bounded, local event inspection; never synced or persisted with the dashboard. */
  noticeSelection: { index: number; source: DashboardSource; context: NoticeChartContext } | null;
  /**
   * Tickers this device explicitly opened this session (a click, a tap, a search pick, a
   * notification), newest first. A host's "the ticker left the price band, show the first
   * candidate" fallback must never replace one of these (owner, 2026-10-23: the chart must always
   * show the ticker just picked; see `shownTicker`). Not synced: another device's pick still falls
   * back here like before.
   */
  picked: readonly string[];

  // Synced setters.
  /** Every indicator setter remembers what is turned off and restores what is turned back on (rememberIndicators). */
  setIndicators(next: IndicatorSpec[]): void;
  setTickerIndicators(next: IndicatorSpec[]): void;
  /** One line colour of an indicator, on every chart and device (null = back to the theme default). An entry patch. */
  setIndicatorColor(spec: IndicatorSpec, role: ColorRole, color: string | null): void;
  /** "Reset to default" for an indicator's colours: every stored role of its identity, everywhere. */
  resetIndicatorColors(spec: IndicatorSpec): void;
  /** The two above as one stable object, for every chart's `indicatorColorActions` prop. */
  indicatorColorActions: IndicatorColorActions;
  setLayers(next: ChartLayers): void;
  setChartSettings(next: ChartSettings): void;
  setDrawingSettings(next: DrawingSettings): void;
  /** Open a ticker everywhere: the synced view moves to it, it goes to the front of the recents, the readout resets. */
  openTicker(symbol: string): void;
  setTimeframe(tf: Timeframe): void;
  setBin(bin: number): void;
  setTradeQty(qty: number): void;
  /** Writes the shown symbol's drawings back as a per-drawing patch (never overwrites another device's edit of a different drawing). */
  setDrawings(next: Drawing[]): void;
  /** How many charts the dashboard shows (clamped to 1–4); the selection moves inside the new count. */
  setDashboardCount(count: number): void;
  /** Which market the dashboard reads (Settings → Data). */
  setDashboardSource(source: DashboardSource): void;
  /** Which account the live market's chart orders go to (Settings → Trading). */
  setLiveAccount(account: LiveAccount): void;
  /** Loads a ticker into one dashboard chart (and the search recents). */
  setDashboardSymbol(slot: number, symbol: string): void;
  setDashboardTimeframe(slot: number, tf: Timeframe): void;
  /** One dashboard chart's own indicators / levels (used while studies are not linked). */
  setDashboardIndicators(slot: number, next: IndicatorSpec[]): void;
  setDashboardLayers(slot: number, next: ChartLayers): void;
  /** Loads a ticker into the selected dashboard chart. */
  loadIntoSelected(symbol: string): void;
  selectNotice(context?: NoticeChartContext): void;
  /** Puts a ticker at the top of the search recents only (the phone's chart modal: it opens a ticker without loading it into any chart). */
  rememberTicker(symbol: string): void;
  /** Adds a ticker to the end of the favorites, or removes it when it is there (the star toggles). */
  toggleFavorite(symbol: string): void;
  /** Moves a favorite up (−1) or down (+1) in the list. */
  moveFavorite(symbol: string, delta: number): void;
  /** Adds an alert (or puts a deleted one back: Undo), written as one entry of `alerts`. The sessions picked become the next alert's default. */
  saveAlert(rule: AlertRule): void;
  /** Edits an alert: `editedAt` moves on, so the server re-arms it (a dragged line, the form). */
  updateAlert(id: string, patch: Partial<Omit<AlertRule, "id" | "createdAt" | "editedAt">>): void;
  removeAlert(id: string): void;
  /** Snoozes (until epoch ms) or un-snoozes (null) without re-arming. */
  snoozeAlert(id: string, until: number | null): void;
  setAlertPrefs(next: AlertPrefs): void;
  /** A frame from the alerts service (the feed routes them here, whatever page is open). */
  applyAlertsMessage(m: AlertsServerMessage): void;
  /** Clears a drawing alert's fire marker (the toast's or banner's acknowledge, the alerts list). */
  acknowledgeDrawingFire(alertId: string): void;

  // Device setters (a no-op on a store without a device slice).
  setToolbox(p: ToolboxPlacement): void;
  setPaneHeights(h: Record<string, number>): void;
  setToolsVisible(v: boolean): void;

  // Session setters.
  showSymbol(symbol: string): void;
  setInspectT(t: number | null): void;
  setSettingsOpen(open: boolean): void;
  setSearchOpen(open: boolean): void;
  setTapeOpen(open: boolean): void;
  setDaysOpen(open: boolean): void;
  selectDashboardChart(slot: number): void;
}

/** One symbol's drawings as an external store, for screens that show several symbols at once (the dashboard). */
export interface DrawingsHandle {
  subscribe(onChange: () => void): () => void;
  get(): Drawing[];
  /** Written back as a per-drawing patch, like `setDrawings`. */
  set(next: Drawing[]): void;
}

export interface AppStoreOptions {
  /** The client's one settings engine (cached, attached to the socket by the feed). */
  sync: SettingsSync;
  /** This screen's layout, already loaded, and where to persist a change. Omit to keep layout out of the store (the web's CandleChart owns it). */
  device?: { initial: DevicePrefs; save: <K extends keyof DevicePrefs>(key: K, value: DevicePrefs[K]) => void };
  /**
   * Values this device kept locally before they synced (search recents, the trade size). Seeded
   * `ifAbsent` once, only while the engine knows nothing for that namespace: the first device to
   * connect makes its value the shared one, as chart-core's migrateLegacySettings does.
   */
  legacy?: { searchRecents?: unknown; tradeQty?: unknown; tickerIndicators?: unknown };
}

export type AppStore = StoreApi<AppState> & {
  /** Stops following the engine (tests, a hot reload). */
  dispose(): void;
  /** Any symbol's drawings, synced (one handle per symbol, reused). */
  drawingsFor(symbol: string): DrawingsHandle;
};

/** The synced slices, read from the engine. */
function syncedFrom(sync: SettingsSync, shownSymbol: string) {
  const view = parseView(sync.get(APP_NS.view));
  const colors = parseIndicatorColors(sync.get(SYNC_NS.indicatorColors));
  return {
    indicators: applyIndicatorColors(parseIndicators(sync.get(SYNC_NS.indicators)), colors),
    indicatorColors: colors,
    indicatorMemory: parseIndicatorMemory(sync.get(SYNC_NS.indicatorMemory)),
    tickerIndicators: applyIndicatorColors(parseIndicators(sync.get(SYNC_NS.tickerIndicators)), colors),
    layers: parseLayers(sync.get(SYNC_NS.layers)),
    chartSettings: parseChartSettings(sync.get(SYNC_NS.chartSettings)),
    chartSettingsFromRemote: sync.origin(SYNC_NS.chartSettings) === "remote",
    drawingSettings: parseDrawingSettings(sync.get(SYNC_NS.drawingSettings)),
    symbol: view.symbol,
    timeframe: view.timeframe,
    bin: view.bin,
    tradeQty: parseTradeQty(sync.get(APP_NS.tradeQty)),
    searchRecents: parseSearchRecents(sync.get(APP_NS.searchRecents)),
    favorites: parseFavorites(sync.get(APP_NS.favorites)),
    drawings: shownSymbol ? drawingsFromDoc(sync.get(drawingsNs(shownSymbol))) : [],
    dashboard: coloredDashboard(parseDashboard(sync.get(APP_NS.dashboard)), colors),
    alerts: parseAlertRules(sync.get(ALERTS_NS.rules)),
    alertStates: parseAlertStates(sync.get(ALERTS_NS.state)),
    alertPrefs: parseAlertPrefs(sync.get(ALERTS_NS.prefs)),
    tradingAccount: parseTradingAccount(sync.get(TRADING_NS.account)),
    brokerStatus: parseBrokerStatus(sync.get(TRADING_NS.status)),
  };
}

/** The dashboard with the global colours applied to each slot's own indicator list. */
function coloredDashboard(d: DashboardState, colors: IndicatorColors | null): DashboardState {
  if (!colors || !d.slots.some((s) => s.indicators?.length)) return d;
  return { ...d, slots: d.slots.map((s) => (s.indicators ? { ...s, indicators: applyIndicatorColors(s.indicators, colors) } : s)) };
}

/**
 * The one-time migration of per-spec colours into `indicatorColors` (chart-core
 * migrateSpecColors): while this device knows no global map, every per-spec colour it holds is
 * queued as an `ifAbsent` seed. The first device to connect makes its colours the global ones; on
 * the others the server's map wins and the seed is dropped. Nothing to migrate, nothing seeded.
 * Run at start, whenever a list arrives before the map does, and before every indicator or colour
 * write (with the list being written, latest of all), so a write that drops per-spec colours can
 * never get ahead of their seed.
 */
function seedIndicatorColors(sync: SettingsSync, incoming: readonly IndicatorSpec[] = []): void {
  if (sync.get(SYNC_NS.indicatorColors) !== undefined) return;
  const slots = parseDashboard(sync.get(APP_NS.dashboard)).slots.map((s) => s.indicators ?? []);
  const memory = Object.values(parseIndicatorMemory(sync.get(SYNC_NS.indicatorMemory)));
  // Oldest first, the main chart last: there are no per-spec timestamps, so the main list, the
  // one most likely edited most recently, wins a conflict (indicatorColors.ts).
  const map = migrateSpecColors([memory, ...slots, parseIndicators(sync.get(SYNC_NS.tickerIndicators)), parseIndicators(sync.get(SYNC_NS.indicators)), incoming]);
  if (Object.keys(map).length) sync.seed(SYNC_NS.indicatorColors, map);
}

/** How many explicit picks are remembered (`AppState.picked`). */
const MAX_PICKED = 32;

function pushPicked(list: readonly string[], symbol: string): readonly string[] {
  if (list[0] === symbol) return list;
  return [symbol, ...list.filter((s) => s !== symbol)].slice(0, MAX_PICKED);
}

/**
 * What a chart shows for the ticker it was given: that ticker, unless it is no longer a candidate
 * (it traded out of the server's price band), in which case the first candidate. A ticker this
 * device explicitly opened (`picked`) is always shown as asked (owner, 2026-10-23): a click on a
 * Runners notification, a panel row or a search result for a ticker the candidate list (polled)
 * did not hold yet showed the first candidate instead, i.e. some other chart.
 */
export function shownTicker(current: string, candidates: readonly string[], picked: readonly string[] = []): string {
  if (candidates.length === 0 || candidates.includes(current) || picked.includes(current)) return current;
  return candidates[0]!;
}

export function createAppStore({ sync, device, legacy }: AppStoreOptions): AppStore {
  if (legacy) {
    if (sync.get(APP_NS.searchRecents) === undefined && legacy.searchRecents !== undefined && legacy.searchRecents !== null) {
      const recents = parseSearchRecents(legacy.searchRecents);
      if (recents.length) sync.seed(APP_NS.searchRecents, recents);
    }
    if (sync.get(APP_NS.tradeQty) === undefined && legacy.tradeQty !== undefined && legacy.tradeQty !== null) {
      sync.seed(APP_NS.tradeQty, parseTradeQty(legacy.tradeQty));
    }
    if (sync.get(SYNC_NS.tickerIndicators) === undefined && Array.isArray(legacy.tickerIndicators)) {
      const specs = parseIndicators(legacy.tickerIndicators);
      if (specs.length) sync.seed(SYNC_NS.tickerIndicators, specs);
    }
  }

  // An indicator list change, with the turned-off settings remembered and the turned-on ones
  // restored. Read from the engine, not the slices, so two quick toggles both land; the memory is
  // written as an entry patch, so two devices turning off different indicators keep both.
  const withMemory = (prev: readonly IndicatorSpec[], next: readonly IndicatorSpec[]): IndicatorSpec[] => {
    const r = rememberIndicators(prev, parseIndicators(next), parseIndicatorMemory(sync.get(SYNC_NS.indicatorMemory)));
    if (Object.keys(r.set).length || r.remove.length) sync.patch(SYNC_NS.indicatorMemory, r.set, r.remove);
    return r.specs;
  };
  // Lists are written without per-spec colours (they are global now), after the migration seed.
  const writeIndicators = (ns: string, next: readonly IndicatorSpec[]) => {
    seedIndicatorColors(sync, next);
    sync.set(ns, stripIndicatorColors(withMemory(stripIndicatorColors(parseIndicators(sync.get(ns))), stripIndicatorColors(next))));
  };
  seedIndicatorColors(sync);

  const currentView = (): ViewState => parseView(sync.get(APP_NS.view));
  const writeView = (patch: Partial<ViewState>) => sync.set(APP_NS.view, { ...currentView(), ...patch });
  const writeSlot = (slot: number, patch: Partial<DashboardSlot>) => {
    if (!Number.isInteger(slot) || slot < 0 || slot >= MAX_DASHBOARD_CHARTS) return;
    const d = parseDashboard(sync.get(APP_NS.dashboard));
    sync.set(APP_NS.dashboard, { ...d, slots: d.slots.map((s, i) => (i === slot ? { ...s, ...patch } : s)) });
  };

  const store = createStore<AppState>()((set, get) => ({
    ...syncedFrom(sync, ""),
    device: device?.initial ?? null,
    shownSymbol: "",
    inspectT: null,
    settingsOpen: false,
    searchOpen: false,
    tapeOpen: false,
    daysOpen: false,
    dashboardSelected: 0,
    noticeSelection: null,
    picked: [],
    alertActivity: [],
    alertStatus: null,
    lastFired: null,
    drawingFires: {},

    setIndicators: (next) => writeIndicators(SYNC_NS.indicators, next),
    setTickerIndicators: (next) => writeIndicators(SYNC_NS.tickerIndicators, next),
    setIndicatorColor: (spec, role, color) => {
      seedIndicatorColors(sync);
      const p = indicatorColorPatch(spec, role, color);
      sync.patch(SYNC_NS.indicatorColors, p.set, p.remove);
    },
    resetIndicatorColors: (spec) => {
      seedIndicatorColors(sync);
      sync.patch(SYNC_NS.indicatorColors, {}, resetIndicatorColorsPatch(spec, parseIndicatorColors(sync.get(SYNC_NS.indicatorColors))));
    },
    indicatorColorActions: {
      set: (spec, role, color) => get().setIndicatorColor(spec, role, color),
      reset: (spec) => get().resetIndicatorColors(spec),
    },
    setLayers: (next) => sync.set(SYNC_NS.layers, next),
    setChartSettings: (next) => sync.set(SYNC_NS.chartSettings, next),
    setDrawingSettings: (next) => sync.set(SYNC_NS.drawingSettings, next),
    openTicker: (raw) => {
      const symbol = parseSymbol(raw);
      if (!symbol) return;
      sync.set(APP_NS.searchRecents, pushRecentTicker(get().searchRecents, symbol));
      writeView({ symbol });
      set({ inspectT: null, picked: pushPicked(get().picked, symbol) });
    },
    rememberTicker: (raw) => {
      const symbol = parseSymbol(raw);
      if (symbol) sync.set(APP_NS.searchRecents, pushRecentTicker(get().searchRecents, symbol));
    },
    setTimeframe: (timeframe) => {
      writeView({ timeframe });
      set({ inspectT: null });
    },
    setBin: (bin) => writeView({ bin }),
    setTradeQty: (qty) => sync.set(APP_NS.tradeQty, parseTradeQty(qty)),
    setDrawings: (next) => {
      const ns = get().shownSymbol ? drawingsNs(get().shownSymbol) : null;
      if (!ns) return;
      const p = diffDrawings(sync.get(ns), next);
      if (p) sync.patch(ns, p.set, p.remove);
    },
    setDashboardCount: (raw) => {
      const count = Math.min(MAX_DASHBOARD_CHARTS, Math.max(1, Math.round(raw)));
      sync.set(APP_NS.dashboard, { ...parseDashboard(sync.get(APP_NS.dashboard)), count });
    },
    setDashboardSource: (source) => {
      sync.set(APP_NS.dashboard, { ...parseDashboard(sync.get(APP_NS.dashboard)), source: source === "replay" ? "replay" : "live" });
    },
    setLiveAccount: (account) => {
      sync.set(TRADING_NS.account, { live: account === "alpaca_paper" ? "alpaca_paper" : "simulator" });
    },
    setDashboardSymbol: (slot, raw) => {
      const symbol = parseSymbol(raw);
      if (!symbol) return;
      sync.set(APP_NS.searchRecents, pushRecentTicker(get().searchRecents, symbol));
      writeSlot(slot, { symbol });
      set({ picked: pushPicked(get().picked, symbol), ...(get().noticeSelection?.index === slot ? { noticeSelection: null } : {}) });
    },
    // Read from the engine, not the slice, so two quick toggles both land.
    toggleFavorite: (symbol) => sync.set(APP_NS.favorites, toggledFavorites(parseFavorites(sync.get(APP_NS.favorites)), symbol)),
    moveFavorite: (symbol, delta) => sync.set(APP_NS.favorites, movedFavorite(parseFavorites(sync.get(APP_NS.favorites)), symbol, delta)),
    setDashboardTimeframe: (slot, timeframe) => writeSlot(slot, { timeframe }),
    setDashboardIndicators: (slot, next) => {
      // An unlinked chart starts from the shared list (DashboardSlot.indicators null).
      const own = parseDashboard(sync.get(APP_NS.dashboard)).slots[slot]?.indicators;
      seedIndicatorColors(sync, next);
      writeSlot(slot, { indicators: stripIndicatorColors(withMemory(stripIndicatorColors(own ?? parseIndicators(sync.get(SYNC_NS.indicators))), stripIndicatorColors(next))) });
    },
    setDashboardLayers: (slot, next) => writeSlot(slot, { layers: parseLayers(next) }),
    loadIntoSelected: (symbol) => get().setDashboardSymbol(get().dashboardSelected, symbol),
    selectNotice: (raw) => {
      const s = get();
      const context = parseNoticeChartContext(raw, s.dashboard.slots[s.dashboardSelected]?.symbol ?? undefined);
      set({ noticeSelection: context && s.dashboard.source === "live" ? { index: s.dashboardSelected, source: s.dashboard.source, context } : null });
    },
    saveAlert: (rule) => {
      sync.patch(ALERTS_NS.rules, { [rule.id]: rule });
      const prefs = get().alertPrefs;
      if (rule.sessions.length && rule.sessions.join() !== prefs.sessions.join()) sync.set(ALERTS_NS.prefs, { ...prefs, sessions: rule.sessions });
    },
    updateAlert: (id, patch) => {
      const cur = get().alerts.find((a) => a.id === id);
      if (cur) sync.patch(ALERTS_NS.rules, { [id]: editedAlert(cur, patch) });
    },
    removeAlert: (id) => sync.patch(ALERTS_NS.rules, {}, [id]),
    snoozeAlert: (id, until) => {
      const cur = get().alerts.find((a) => a.id === id);
      // Not an edit: snoozing keeps the state (a cooling alert stays cooling under the snooze).
      if (cur) sync.patch(ALERTS_NS.rules, { [id]: { ...cur, snoozeUntil: until } });
    },
    setAlertPrefs: (next) => sync.set(ALERTS_NS.prefs, next),
    applyAlertsMessage: (m) => {
      switch (m.type) {
        case "alerts.history": {
          const { type: _t, events, ...status } = m;
          set({ alertActivity: mergeActivity([], Array.isArray(events) ? events : [], true), alertStatus: status });
          return;
        }
        case "alerts.status": {
          const { type: _t, ...status } = m;
          set({ alertStatus: status });
          return;
        }
        case "alert.fired": {
          const s = get().alertStatus;
          const e = m.event;
          const tone = e.tone === "up" || e.tone === "down" || e.tone === "stop" ? e.tone : null;
          set({
            alertActivity: mergeActivity(get().alertActivity, [e], false),
            lastFired: e,
            ...(s && typeof m.phoneToday === "number" ? { alertStatus: { ...s, phoneToday: m.phoneToday } } : {}),
            ...(tone && typeof e.price === "number" ? { drawingFires: { ...get().drawingFires, [e.alertId]: { t: e.t, price: e.price, tone, eventId: e.id } } } : {}),
          });
          return;
        }
        case "alert.held":
        case "alert.updated":
          set({ alertActivity: mergeActivity(get().alertActivity, [m.event], false) });
          return;
      }
    },

    acknowledgeDrawingFire: (alertId) => {
      const cur = get().drawingFires;
      if (!(alertId in cur)) return;
      const { [alertId]: _gone, ...rest } = cur;
      set({ drawingFires: rest });
    },

    setToolbox: (toolbox) => updateDevice("toolbox", toolbox),
    setPaneHeights: (paneHeights) => updateDevice("paneHeights", paneHeights),
    setToolsVisible: (toolsVisible) => updateDevice("toolsVisible", toolsVisible),

    showSymbol: (shownSymbol) => {
      if (shownSymbol === get().shownSymbol) return;
      set({ shownSymbol, drawings: shownSymbol ? drawingsFromDoc(sync.get(drawingsNs(shownSymbol))) : [] });
    },
    setInspectT: (inspectT) => set({ inspectT }),
    setSettingsOpen: (settingsOpen) => set({ settingsOpen }),
    setSearchOpen: (searchOpen) => set({ searchOpen }),
    setTapeOpen: (tapeOpen) => set({ tapeOpen }),
    setDaysOpen: (daysOpen) => set({ daysOpen }),
    selectDashboardChart: (slot) => {
      if (Number.isInteger(slot) && slot >= 0 && slot < get().dashboard.count) set({ dashboardSelected: slot });
    },
  }));

  function updateDevice<K extends keyof DevicePrefs>(key: K, value: DevicePrefs[K]) {
    const cur = store.getState().device;
    if (!cur || !device) return;
    store.setState({ device: { ...cur, [key]: value } });
    device.save(key, value);
  }

  // Follow the engine: a local edit (optimistic) and another device's edit both arrive here.
  // Only the slice whose namespace changed is re-read, so an unrelated namespace (another
  // symbol's drawings) never re-renders anything.
  const colorsNow = () => store.getState().indicatorColors;
  const unsubscribe = sync.subscribe((ns) => {
    const s = store.getState();
    // A list (or the memory) arriving before any global map may carry per-spec colours to migrate.
    // After a microtask: a snapshot announces its namespaces one by one, and the seed must see all
    // of them (a write in between seeds synchronously itself, so it cannot run ahead of it).
    if (s.indicatorColors === null && (ns === SYNC_NS.indicators || ns === SYNC_NS.tickerIndicators || ns === SYNC_NS.indicatorMemory || ns === APP_NS.dashboard)) queueMicrotask(() => seedIndicatorColors(sync));
    switch (ns) {
      case SYNC_NS.indicators:
        return store.setState({ indicators: applyIndicatorColors(parseIndicators(sync.get(ns)), colorsNow()) });
      case SYNC_NS.indicatorMemory:
        return store.setState({ indicatorMemory: parseIndicatorMemory(sync.get(ns)) });
      case SYNC_NS.tickerIndicators:
        return store.setState({ tickerIndicators: applyIndicatorColors(parseIndicators(sync.get(ns)), colorsNow()) });
      case SYNC_NS.indicatorColors: {
        // A colour changed (on this device or another): every chart's list is re-coloured.
        const colors = parseIndicatorColors(sync.get(ns));
        return store.setState({
          indicatorColors: colors,
          indicators: applyIndicatorColors(parseIndicators(sync.get(SYNC_NS.indicators)), colors),
          tickerIndicators: applyIndicatorColors(parseIndicators(sync.get(SYNC_NS.tickerIndicators)), colors),
          dashboard: coloredDashboard(parseDashboard(sync.get(APP_NS.dashboard)), colors),
        });
      }
      case SYNC_NS.layers:
        return store.setState({ layers: parseLayers(sync.get(ns)) });
      case SYNC_NS.chartSettings:
        return store.setState({ chartSettings: parseChartSettings(sync.get(ns)), chartSettingsFromRemote: sync.origin(ns) === "remote" });
      case SYNC_NS.drawingSettings:
        return store.setState({ drawingSettings: parseDrawingSettings(sync.get(ns)) });
      case APP_NS.tradeQty:
        return store.setState({ tradeQty: parseTradeQty(sync.get(ns)) });
      case APP_NS.searchRecents:
        return store.setState({ searchRecents: parseSearchRecents(sync.get(ns)) });
      case APP_NS.favorites:
        return store.setState({ favorites: parseFavorites(sync.get(ns)) });
      case ALERTS_NS.rules:
        return store.setState({ alerts: parseAlertRules(sync.get(ns)) });
      case ALERTS_NS.state:
        return store.setState({ alertStates: parseAlertStates(sync.get(ns)) });
      case ALERTS_NS.prefs:
        return store.setState({ alertPrefs: parseAlertPrefs(sync.get(ns)) });
      case APP_NS.dashboard: {
        const dashboard = coloredDashboard(parseDashboard(sync.get(ns)), colorsNow());
        // Fewer charts (from any device): the selection stays on a chart that is still shown.
        const n = s.noticeSelection;
        const keepNotice = n && n.index < dashboard.count && n.source === dashboard.source && dashboard.slots[n.index]?.symbol === n.context.symbol;
        return store.setState({ dashboard, dashboardSelected: Math.min(s.dashboardSelected, dashboard.count - 1), ...(n && !keepNotice ? { noticeSelection: null } : {}) });
      }
      case APP_NS.view: {
        const v = parseView(sync.get(ns));
        if (v.symbol === s.symbol && v.timeframe === s.timeframe && v.bin === s.bin) return;
        // Another ticker or timeframe (from any device): the inspected candle no longer exists.
        const moved = v.symbol !== s.symbol || v.timeframe !== s.timeframe;
        return store.setState({ symbol: v.symbol, timeframe: v.timeframe, bin: v.bin, ...(moved ? { inspectT: null } : {}) });
      }
      default:
        if (s.shownSymbol && ns === drawingsNs(s.shownSymbol)) store.setState({ drawings: drawingsFromDoc(sync.get(ns)) });
    }
  });

  // Per-symbol drawing handles for the dashboard's charts, built on the engine's memoized views.
  const handles = new Map<string, DrawingsHandle>();
  const drawingsFor = (raw: string): DrawingsHandle => {
    const symbol = raw.trim().toUpperCase();
    let h = handles.get(symbol);
    if (!h) {
      const ns = drawingsNs(symbol);
      const view = sync.view(ns, drawingsFromDoc);
      h = {
        subscribe: view.subscribe,
        get: view.get,
        set: (next) => {
          if (!symbol) return;
          const p = diffDrawings(sync.get(ns), next);
          if (p) sync.patch(ns, p.set, p.remove);
        },
      };
      handles.set(symbol, h);
    }
    return h;
  };

  return Object.assign(store, { dispose: unsubscribe, drawingsFor });
}
