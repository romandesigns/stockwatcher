import type { PaperState } from "@stockwatcher/chart-core";
import type { ChartTrading } from "@stockwatcher/chart-web";
import { useCallback, useMemo, useSyncExternalStore } from "react";
import type { ChartFeed } from "./types.ts";

/**
 * Binds the feed's paper broker to the chart's `trading` prop: the latest
 * broadcast state in, order intents out. Null when the feed has no broker
 * (the in-browser fixture), which turns chart trading off.
 */
export function usePaperTrading(feed: ChartFeed): ChartTrading | null {
  const paper = feed.paper;
  const subscribe = useCallback((cb: () => void) => (paper ? paper.subscribe(cb) : () => {}), [paper]);
  const state = useSyncExternalStore(subscribe, useCallback((): PaperState | null => paper?.latest() ?? null, [paper]));
  return useMemo(
    () =>
      paper
        ? {
            state,
            onPlace: (o) => paper.send({ type: "order.place", ...o }),
            onModify: (c) => paper.send({ type: "order.modify", ...c }),
            onCancel: (id) => paper.send({ type: "order.cancel", id }),
          }
        : null,
    [paper, state],
  );
}
