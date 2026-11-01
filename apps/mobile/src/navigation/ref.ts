import { createNavigationContainerRef } from "@react-navigation/native";
import type { NoticeChartContext } from "@stockwatcher/shared";

import { appActions } from "../state/store";
import { alertsAction, chartParams, createReadyQueue, openChartAction } from "./openTicker";
import type { RootStackParamList } from "./RootStack";

/**
 * The app's navigation, reachable outside a screen: a tapped phone notification, a toast's action
 * and every ticker row open the chart modal from wherever the person is.
 */
export const navigationRef = createNavigationContainerRef<RootStackParamList>();

const openListeners = new Set<(symbol: string) => void>();

/**
 * The phone's one "open this ticker" action (owner, 2026-10-23: the chart must always show the
 * ticker just tapped, wherever it was presented). Every entry point calls it: a notification tap
 * (cold start, background, foreground, the watch's "Open on phone"), the dashboard's Halt watcher,
 * Ross, Runners and Signals rows, the ticker search, the alerts list, a fired alert's toast.
 *
 * It puts the ticker first in the search recents, closes the alert sheet (it is drawn over every
 * screen, so it would hide the chart) and opens the chart modal on THIS ticker: an open modal is
 * reused with the new ticker (openTicker.ts), never left on the old one. Before the navigation
 * exists (a cold start from a notification) it waits for the container's onReady.
 */
export function openTickerChart(symbol: string, detectionEpisodeId?: string, detectionRunEpoch?: string, noticeContext?: NoticeChartContext): void {
  const params = chartParams(symbol, detectionEpisodeId, detectionRunEpoch, noticeContext);
  if (!params) return;
  try {
    appActions().rememberTicker(params.symbol);
  } catch {
    // The store has not loaded yet (a cold start): the chart still opens.
  }
  for (const l of openListeners) l(params.symbol);
  whenNavigationReady(() => navigationRef.dispatch(openChartAction(params)));
}

/** Called on every openTickerChart, before it navigates (the alert sheet closes itself: alerts/actions.ts). */
export function onOpenTicker(listener: (symbol: string) => void): () => void {
  openListeners.add(listener);
  return () => {
    openListeners.delete(listener);
  };
}

/** The Insights ticker page for one symbol-day, pushed over the tabs (`screenPush`). */
export function openInsightsTicker(symbol: string, day: string): void {
  whenNavigationReady(() => navigationRef.navigate("InsightsTicker", { symbol, day }));
}

export function navigateToAlerts(): void {
  whenNavigationReady(() => navigationRef.dispatch(alertsAction()));
}

// A notification tapped while the app was closed is read at launch, before the navigation exists:
// its navigation waits here until the container's onReady (App.tsx) runs it.
const ready = createReadyQueue(() => navigationRef.isReady());

/** Runs `go` now if the navigation is ready, else once it is (the latest call wins). */
export function whenNavigationReady(go: () => void): void {
  ready.run(go);
}

/** The NavigationContainer's onReady. */
export function flushNavigation(): void {
  ready.flush();
}
