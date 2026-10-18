// The web app's (and the desktop shell's) synced settings: one engine for the page
// (@stockwatcher/shared settingsSync.ts), cached in localStorage so the chart starts with the
// last known settings before the socket connects, and works with no server at all (the
// in-browser fixture never attaches it). docs/architecture/settings-sync.md.

import { migrateLegacySettings } from "@stockwatcher/chart-core";
import { createSettingsSync, type SettingsSync } from "@stockwatcher/shared";

/** The engine's cache: confirmed values with their revs, and unconfirmed edits. */
export const SYNC_CACHE_KEY = "sw-sync:v1";

/**
 * Where these settings lived before sync (CandleChart's `persistKey` "sw-chart" and the app's
 * Settings dialog). Read once, on the first run with sync, then left alone: the engine's
 * cache replaces them. The device-local ones (`:toolbox`, `:panes`, `:toolsVisible`,
 * `:tradeQty`) stay where they are, still owned by CandleChart.
 */
export const LEGACY_KEYS = {
  indicators: "sw-chart:indicators",
  layers: "sw-chart:layers",
  chartSettings: "sw-chart:settings",
  drawingSettings: "sw-chart:drawing",
  drawings: "sw-chart:drawings",
} as const;

const readJson = (read: (key: string) => string | null, key: string): unknown => {
  try {
    const raw = read(key);
    return raw === null ? undefined : JSON.parse(raw);
  } catch {
    return undefined;
  }
};

/**
 * Queues this browser's pre-sync settings as `ifAbsent` seeds (chart-core
 * migrateLegacySettings, the phone's rule too): the first client to connect makes its setup the
 * shared one; on a later client the server's value wins and replaces the local one.
 */
export function migrateLegacy(sync: SettingsSync, read: (key: string) => string | null): void {
  migrateLegacySettings(
    {
      indicators: readJson(read, LEGACY_KEYS.indicators),
      layers: readJson(read, LEGACY_KEYS.layers),
      chartSettings: readJson(read, LEGACY_KEYS.chartSettings),
      drawingSettings: readJson(read, LEGACY_KEYS.drawingSettings),
      drawings: readJson(read, LEGACY_KEYS.drawings),
    },
    (ns, value) => sync.seed(ns, value),
  );
}

let instance: SettingsSync | null = null;

/** The page's one engine, created on first use. */
export function getSettingsSync(): SettingsSync {
  if (instance) return instance;
  const storage = (() => {
    try {
      return window.localStorage;
    } catch {
      return null; // blocked storage: settings still sync, nothing is cached
    }
  })();
  const read = (key: string) => {
    try {
      return storage?.getItem(key) ?? null;
    } catch {
      return null;
    }
  };
  const initial = read(SYNC_CACHE_KEY);
  const sync = createSettingsSync({
    initial,
    save: (raw) => storage?.setItem(SYNC_CACHE_KEY, raw),
    onReject: (ns, code, message) => console.warn(`settings sync: ${ns} refused (${code}): ${message}`),
  });
  if (initial === null) migrateLegacy(sync, read);
  // A change made in the last throttle window still goes out when the tab is hidden or closed.
  window.addEventListener("pagehide", () => sync.flush());
  instance = sync;
  return sync;
}
