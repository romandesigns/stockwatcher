// The phone's synced settings: the same engine as the web app (@stockwatcher/shared
// settingsSync.ts; docs/architecture/settings-sync.md), cached in the app's preferences so the
// chart opens with the last known settings before the socket connects and works offline.

import { migrateLegacySettings } from "@stockwatcher/chart-core";
import { createSettingsSync, type SettingsSync } from "@stockwatcher/shared";

import { getPreference, getPreferenceRaw, setPreferenceRaw } from "../storage/preferences";

/** The engine's cache: confirmed values with their revs, and unconfirmed edits. */
const CACHE_KEY = "sync.v1";

/**
 * Where these settings lived before sync. Read once, on the first run with sync, then left alone.
 * The device-local ones (`chart.toolbox`, `chart.paneHeights`, `chart.toolsVisible`,
 * `search.recent`) stay where they are, owned by the chart screen and the search sheet.
 */
const LEGACY = {
  indicators: "chart.indicators",
  layers: "chart.layers",
  chartSettings: "chart.settings",
  drawingSettings: "chart.drawing",
  drawings: "chart.drawings",
} as const;

let loading: Promise<SettingsSync> | null = null;

/** The app's one engine, loaded once (sign-in retries reuse it). Never rejects. */
export function loadSettingsSync(): Promise<SettingsSync> {
  loading ??= (async () => {
    const initial = await getPreferenceRaw(CACHE_KEY).catch(() => null);
    const sync = createSettingsSync({
      initial,
      save: (raw) => void setPreferenceRaw(CACHE_KEY, raw).catch(() => {}),
      onReject: (ns, code, message) => console.warn(`settings sync: ${ns} refused (${code}): ${message}`),
    });
    if (initial === null) {
      const read = (key: string) => getPreference<unknown>(key).catch(() => null);
      const [indicators, layers, chartSettings, drawingSettings, drawings] = await Promise.all([
        read(LEGACY.indicators),
        read(LEGACY.layers),
        read(LEGACY.chartSettings),
        read(LEGACY.drawingSettings),
        read(LEGACY.drawings),
      ]);
      // Same rule as the web: the first client to connect seeds the shared value, later ones adopt it.
      migrateLegacySettings({ indicators, layers, chartSettings, drawingSettings, drawings }, (ns, value) => sync.seed(ns, value));
    }
    return sync;
  })();
  return loading;
}
