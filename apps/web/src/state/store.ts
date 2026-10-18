// The web app's (and the desktop shell's) one app store: @stockwatcher/app-state's store on the
// page's settings engine. docs/architecture/app-state.md.

import { createAppStore, type AppState, type AppStore } from "@stockwatcher/app-state";
import { useStore } from "zustand";
import { getSettingsSync } from "../settings/sync.ts";

/** Where these lived before they synced (read once into an empty namespace, then left alone). */
const LEGACY = { searchRecents: "sw-search:recent", tradeQty: "sw-chart:tradeQty" } as const;

const readJson = (key: string): unknown => {
  try {
    const raw = localStorage.getItem(key);
    return raw === null ? undefined : JSON.parse(raw);
  } catch {
    return undefined;
  }
};

let store: AppStore | null = null;

/**
 * The page's store, created on first use. No device slice: on the web, CandleChart keeps the
 * screen-layout values (toolbox place, pane heights, tools shown) itself under its persistKey.
 */
export function getAppStore(): AppStore {
  store ??= createAppStore({
    sync: getSettingsSync(),
    legacy: { searchRecents: readJson(LEGACY.searchRecents), tradeQty: readJson(LEGACY.tradeQty) },
  });
  return store;
}

/**
 * One slice of the app store as React state. Select the smallest thing a component needs: a
 * selector returning a new object or array every time re-renders on every store change (wrap it
 * in zustand's useShallow if one is really needed).
 */
export function useApp<T>(selector: (s: AppState) => T): T {
  return useStore(getAppStore(), selector);
}
