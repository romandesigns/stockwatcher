import {
  diffDrawings,
  drawingsFromDoc,
  drawingsNs,
  parseChartSettings,
  parseDrawingSettings,
  parseIndicators,
  parseLayers,
  SYNC_NS,
  type ChartLayers,
  type ChartSettings,
  type Drawing,
  type DrawingSettings,
  type IndicatorSpec,
  type Timeframe,
  type ToolboxPlacement,
} from "@stockwatcher/chart-core";
import { pushRecentTicker, type SettingsSync } from "@stockwatcher/shared";
import { createStore, type StoreApi } from "zustand/vanilla";
import {
  APP_NS,
  MAX_DASHBOARD_CHARTS,
  parseDashboard,
  parseSearchRecents,
  parseSymbol,
  parseTradeQty,
  parseView,
  type DashboardSlot,
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
  indicators: IndicatorSpec[];
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
  /** The shown symbol's drawings. */
  drawings: Drawing[];
  /** The dashboard's multi-chart panel: chart count and each slot's ticker and timeframe. */
  dashboard: DashboardState;

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

  // Synced setters.
  setIndicators(next: IndicatorSpec[]): void;
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
  /** Loads a ticker into one dashboard chart (and the search recents). */
  setDashboardSymbol(slot: number, symbol: string): void;
  setDashboardTimeframe(slot: number, tf: Timeframe): void;
  /** Loads a ticker into the selected dashboard chart. */
  loadIntoSelected(symbol: string): void;

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
  legacy?: { searchRecents?: unknown; tradeQty?: unknown };
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
  return {
    indicators: parseIndicators(sync.get(SYNC_NS.indicators)),
    layers: parseLayers(sync.get(SYNC_NS.layers)),
    chartSettings: parseChartSettings(sync.get(SYNC_NS.chartSettings)),
    chartSettingsFromRemote: sync.origin(SYNC_NS.chartSettings) === "remote",
    drawingSettings: parseDrawingSettings(sync.get(SYNC_NS.drawingSettings)),
    symbol: view.symbol,
    timeframe: view.timeframe,
    bin: view.bin,
    tradeQty: parseTradeQty(sync.get(APP_NS.tradeQty)),
    searchRecents: parseSearchRecents(sync.get(APP_NS.searchRecents)),
    drawings: shownSymbol ? drawingsFromDoc(sync.get(drawingsNs(shownSymbol))) : [],
    dashboard: parseDashboard(sync.get(APP_NS.dashboard)),
  };
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
  }

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

    setIndicators: (next) => sync.set(SYNC_NS.indicators, next),
    setLayers: (next) => sync.set(SYNC_NS.layers, next),
    setChartSettings: (next) => sync.set(SYNC_NS.chartSettings, next),
    setDrawingSettings: (next) => sync.set(SYNC_NS.drawingSettings, next),
    openTicker: (raw) => {
      const symbol = parseSymbol(raw);
      if (!symbol) return;
      sync.set(APP_NS.searchRecents, pushRecentTicker(get().searchRecents, symbol));
      writeView({ symbol });
      set({ inspectT: null });
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
    setDashboardSymbol: (slot, raw) => {
      const symbol = parseSymbol(raw);
      if (!symbol) return;
      sync.set(APP_NS.searchRecents, pushRecentTicker(get().searchRecents, symbol));
      writeSlot(slot, { symbol });
    },
    setDashboardTimeframe: (slot, timeframe) => writeSlot(slot, { timeframe }),
    loadIntoSelected: (symbol) => get().setDashboardSymbol(get().dashboardSelected, symbol),

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
  const unsubscribe = sync.subscribe((ns) => {
    const s = store.getState();
    switch (ns) {
      case SYNC_NS.indicators:
        return store.setState({ indicators: parseIndicators(sync.get(ns)) });
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
      case APP_NS.dashboard: {
        const dashboard = parseDashboard(sync.get(ns));
        // Fewer charts (from any device): the selection stays on a chart that is still shown.
        return store.setState({ dashboard, dashboardSelected: Math.min(s.dashboardSelected, dashboard.count - 1) });
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
