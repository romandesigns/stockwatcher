import { ChartStore, type ChartMessage, type Clock, type PaperState, type Series, type TapeTrade, type Timeframe } from "@stockwatcher/chart-core";
import type { ChartTrading } from "@stockwatcher/chart-native";
import { authenticatedFetch } from "@stockwatcher/shared";
import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";

import { SERVER_URL } from "./server";
import type { MobileFeed } from "./serverFeed";

const TAPE_MAX = 50;

/** Feed subscription → ChartStore + a short tape (the web's useChartFeed, trimmed for the phone). */
export function useChartFeed(feed: MobileFeed, symbol: string, timeframe: Timeframe, profileBin = 0.01) {
  const store = useMemo(() => new ChartStore({ maxCandles: 3000 }), []);
  const [tape, setTape] = useState<TapeTrade[]>([]);
  useEffect(() => {
    let pending: TapeTrade[] = [];
    let frame = 0;
    // Prints arrive in bursts at high replay speed: batch per frame.
    const flush = () => {
      frame = 0;
      const batch = pending;
      pending = [];
      setTape((prev) => [...batch.reverse(), ...prev].slice(0, TAPE_MAX));
    };
    const onMessage = (m: ChartMessage) => {
      if (m.type === "chart.snapshot") {
        pending = [];
        setTape([]);
        store.applySnapshot(m);
      } else if (m.type === "chart.update" || m.type === "chart.remove" || m.type === "chart.gap") store.applyIncrement(m);
      else if (m.type === "replay.clock") store.applyClock(m.clock);
      else if (m.type === "chart.halt" || m.type === "chart.luld") store.applySymbolEvent(m);
      else if (m.symbol === symbol) {
        pending.push(m);
        if (!frame) frame = requestAnimationFrame(flush);
      }
    };
    const off = feed.subscribe({ symbol, timeframe, profileBin }, onMessage);
    return () => {
      off();
      cancelAnimationFrame(frame);
    };
  }, [feed, store, symbol, timeframe, profileBin]);
  const subscribe = useCallback((cb: () => void) => store.subscribe(cb), [store]);
  const series = useSyncExternalStore(subscribe, useCallback((): Series | undefined => store.getSeries(symbol, timeframe), [store, symbol, timeframe]));
  const clock = useSyncExternalStore(subscribe, useCallback((): Clock | null => store.getClock(), [store]));
  return { series, tape, clock };
}

/** The feed's paper broker as the chart's `trading` prop. */
export function usePaperTrading(feed: MobileFeed): ChartTrading {
  const subscribe = useCallback((cb: () => void) => feed.onPaper(cb), [feed]);
  const state = useSyncExternalStore(subscribe, useCallback((): PaperState | null => feed.paperLatest(), [feed]));
  return useMemo(
    () => ({
      state,
      onPlace: (o) => feed.sendPaper({ type: "order.place", ...o }),
      onModify: (c) => feed.sendPaper({ type: "order.modify", ...c }),
      onCancel: (id) => feed.sendPaper({ type: "order.cancel", id }),
    }),
    [feed, state],
  );
}

/** The server's price-band candidates, refreshed every 10 s (they change as prices move). */
export function useCandidates(token: string | null): string[] {
  const [symbols, setSymbols] = useState<string[]>([]);
  useEffect(() => {
    let alive = true;
    const refresh = async () => {
      try {
        const res = await authenticatedFetch(`${SERVER_URL}/api/symbols`, token);
        if (!res.ok) return;
        const d = (await res.json()) as { symbols?: unknown };
        if (alive && Array.isArray(d.symbols)) setSymbols(d.symbols.filter((x): x is string => typeof x === "string"));
      } catch {
        // Offline or server down: keep the last list; the socket status says so.
      }
    };
    void refresh();
    const id = setInterval(() => void refresh(), 10_000);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [token]);
  return symbols;
}
