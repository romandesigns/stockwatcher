import { createNavigationContainerRef } from "@react-navigation/native";

import type { RootStackParamList } from "./RootStack";

/**
 * The app's navigation, reachable outside a screen: a fired alert's banner ("View") and a tapped
 * phone notification open the Alerts tab from wherever the person is.
 */
export const navigationRef = createNavigationContainerRef<RootStackParamList>();

/** Opens a ticker's chart modal (it covers the dashboard, so nothing can open a second one over it). */
export function openTickerChart(symbol: string, detectionEpisodeId?: string, detectionRunEpoch?: string): void {
  if (navigationRef.isReady()) navigationRef.navigate("TickerChart", { symbol, ...(detectionEpisodeId ? { detectionEpisodeId } : {}), ...(detectionRunEpoch ? { detectionRunEpoch } : {}) });
}

export function navigateToAlerts(): void {
  if (navigationRef.isReady()) navigationRef.navigate("Home", { screen: "Alerts" });
}

// A notification tapped while the app was closed is read at launch, before the navigation exists:
// its navigation waits here until the container's onReady (App.tsx) runs it.
let waiting: (() => void) | null = null;

/** Runs `go` now if the navigation is ready, else once it is (the latest call wins). */
export function whenNavigationReady(go: () => void): void {
  if (navigationRef.isReady()) go();
  else waiting = go;
}

/** The NavigationContainer's onReady. */
export function flushNavigation(): void {
  const go = waiting;
  waiting = null;
  go?.();
}
