// The phone's one "open this ticker" navigation, as plain actions (no React Native import, so bun
// tests run them through React Navigation's real StackRouter). ref.ts dispatches them; every
// entry point goes through ref.ts `openTickerChart` (owner, 2026-10-23: the chart must always be
// replaced by the ticker just tapped, wherever it was presented).
//
// `pop: true` is what makes "replace the open chart" hold in every stack shape:
// - the chart modal on top: its route is reused and its params replaced (not merged, so an
//   earlier pick's detection episode never sticks to the new ticker); TickerChartScreen keys the
//   chart by ticker, so the subscription, drawings and readout start over for it;
// - the modal somewhere below the top (another screen pushed over it): everything above it is
//   popped and the modal shows the new ticker, instead of a second modal stacking up with the old
//   one still under it;
// - no modal: one is pushed.

import { parseSymbol } from "@stockwatcher/app-state";

export type ChartParams = { symbol: string; detectionEpisodeId?: string; detectionRunEpoch?: string };

/** The chart modal's params for `raw`, or null when it is not a ticker. */
export function chartParams(raw: string, detectionEpisodeId?: string, detectionRunEpoch?: string): ChartParams | null {
  const symbol = parseSymbol(raw);
  if (!symbol) return null;
  return { symbol, ...(detectionEpisodeId ? { detectionEpisodeId } : {}), ...(detectionRunEpoch ? { detectionRunEpoch } : {}) };
}

export function openChartAction(params: ChartParams) {
  return { type: "NAVIGATE", payload: { name: "TickerChart", params, pop: true } } as const;
}

/**
 * Runs navigation once the container is ready: a notification tapped while the app was closed is
 * read at launch, before the navigation exists, and waits for the container's onReady. The latest
 * call wins: two taps queued before then open the second one's ticker, never the first.
 */
export function createReadyQueue(isReady: () => boolean) {
  let waiting: (() => void) | null = null;
  return {
    run(go: () => void): void {
      if (isReady()) go();
      else waiting = go;
    },
    flush(): void {
      const go = waiting;
      waiting = null;
      go?.();
    },
  };
}

/** The Alerts tab: pops back to Home (closing a chart modal over it) instead of stacking a second Home. */
export function alertsAction() {
  return { type: "NAVIGATE", payload: { name: "Home", params: { screen: "Alerts" }, pop: true } } as const;
}
