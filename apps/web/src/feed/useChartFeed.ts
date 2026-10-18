import { ChartStore, type ChartMessage, type Clock, type Series, type TapeTrade, type Timeframe } from "@stockwatcher/chart-core";
import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import type { ChartFeed } from "./types.ts";

const TAPE_MAX = 200;

/**
 * Binds a feed subscription to a chart-core ChartStore and a tape buffer.
 * Changing the timeframe or bin re-subscribes; each timeframe keeps its own
 * series in the store, so switching never mixes candle sets.
 */
/**
 * `slot` names the chart on a connection that carries several (the dashboard's panel: `d0`…`d3`);
 * omitted, it is the single default slot (the chart page).
 */
export function useChartFeed(feed: ChartFeed, symbol: string, timeframe: Timeframe, profileBin: number, slot?: string) {
  const store = useMemo(() => new ChartStore({ maxCandles: 5000 }), []);
  const [tape, setTape] = useState<TapeTrade[]>([]);

  useEffect(() => {
    let pending: TapeTrade[] = [];
    let frame = 0;
    // Tape prints arrive in bursts at high replay speed: batch per frame.
    const flush = () => {
      frame = 0;
      const batch = pending;
      pending = [];
      setTape((prev) => [...batch.reverse(), ...prev].slice(0, TAPE_MAX));
    };
    const onMessage = (m: ChartMessage) => {
      if (m.type === "chart.snapshot") {
        // A snapshot restarts the subscription, tape history included.
        pending = [];
        setTape([]);
        store.applySnapshot(m);
      } else if (m.type === "chart.update" || m.type === "chart.remove" || m.type === "chart.gap") {
        store.applyIncrement(m);
      } else if (m.type === "replay.clock") {
        store.applyClock(m.clock);
      } else if (m.type === "chart.halt" || m.type === "chart.luld") {
        store.applySymbolEvent(m);
      } else if (m.symbol === symbol) {
        pending.push(m);
        if (!frame) frame = requestAnimationFrame(flush);
      }
    };
    const off = feed.subscribe({ symbol, timeframe, profileBin, ...(slot ? { id: slot } : {}) }, onMessage);
    return () => {
      off();
      cancelAnimationFrame(frame);
    };
  }, [feed, store, symbol, timeframe, profileBin, slot]);

  const subscribe = useCallback((cb: () => void) => store.subscribe(cb), [store]);
  const getSeries = useCallback((): Series | undefined => store.getSeries(symbol, timeframe), [store, symbol, timeframe]);
  const series = useSyncExternalStore(subscribe, getSeries);
  const clock = useSyncExternalStore(subscribe, useCallback((): Clock | null => store.getClock(), [store]));
  return { series, tape, clock };
}
