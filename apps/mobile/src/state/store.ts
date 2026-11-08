// The phone's one app store: @stockwatcher/app-state's store on the app's settings engine, with
// this phone's screen layout as its device slice. docs/architecture/app-state.md.

import { createAppStore, type AppState, type AppStore, type DevicePrefs } from "@stockwatcher/app-state";
import { DEFAULT_TOOLBOX_PHONE, parseToolboxPlacement } from "@stockwatcher/chart-core";
import { useStore } from "zustand";

import { loadSettingsSync } from "../settings/sync";
import { getPreference, setPreference } from "../storage/preferences";

/** Where the device slice is kept (the keys the chart screen used before, so nothing is lost). */
const DEVICE_KEYS: Readonly<Record<keyof DevicePrefs, string>> = {
  toolbox: "chart.toolbox",
  paneHeights: "chart.paneHeights",
  toolsVisible: "chart.toolsVisible",
};
/** The recents before they synced: seeded once into an empty namespace, then left alone. */
const LEGACY_RECENTS = "search.recent";
/** The Insights ticker page chart's indicators before they synced (2026-11-07), this phone's own. */
export const TICKER_INDICATORS_LEGACY_KEY = "insights-ticker:indicators";

const read = (key: string) => getPreference<unknown>(key).catch(() => null);

let loading: Promise<AppStore> | null = null;
let loaded: AppStore | null = null;

/** The app's store, loaded once (sign-in retries reuse it) with the synced cache and this phone's layout. Never rejects. */
export function loadAppStore(): Promise<AppStore> {
  loading ??= (async () => {
    const [sync, toolbox, paneHeights, toolsVisible, recents, tickerIndicators] = await Promise.all([
      loadSettingsSync(),
      read(DEVICE_KEYS.toolbox),
      read(DEVICE_KEYS.paneHeights),
      read(DEVICE_KEYS.toolsVisible),
      read(LEGACY_RECENTS),
      read(TICKER_INDICATORS_LEGACY_KEY),
    ]);
    const initial: DevicePrefs = {
      toolbox: toolbox ? parseToolboxPlacement(toolbox, DEFAULT_TOOLBOX_PHONE) : DEFAULT_TOOLBOX_PHONE,
      paneHeights: paneHeights && typeof paneHeights === "object" ? Object.fromEntries(Object.entries(paneHeights).filter(([, h]) => typeof h === "number")) : {},
      toolsVisible: toolsVisible !== false,
    };
    loaded = createAppStore({
      sync,
      device: { initial, save: (key, value) => void setPreference(DEVICE_KEYS[key], value).catch(() => {}) },
      legacy: { searchRecents: recents, tickerIndicators },
    });
    return loaded;
  })();
  return loading;
}

/**
 * One slice of the app store as React state; only below the screen that awaited loadAppStore.
 * Select the smallest thing a component needs (a new object per call re-renders on every change).
 */
export function useApp<T>(selector: (s: AppState) => T): T {
  if (!loaded) throw new Error("useApp before loadAppStore resolved");
  return useStore(loaded, selector);
}

/** The loaded store itself (its `drawingsFor` handles); only below the screen that awaited loadAppStore. */
export function getLoadedStore(): AppStore {
  if (!loaded) throw new Error("getLoadedStore before loadAppStore resolved");
  return loaded;
}

/** The store's actions (stable functions), for event handlers. */
export function appActions(): AppState {
  if (!loaded) throw new Error("appActions before loadAppStore resolved");
  return loaded.getState();
}
