import { NO_TICKER, type TickerChannel, type TickerRequest, type TickerState } from "@stockwatcher/shared";
import { useCallback, useEffect, useSyncExternalStore } from "react";

const NOOP = () => () => {};

/** The open symbol-day's package from the feed's ticker channel (`insights.ticker.v1`). */
export function useTickerPackage(channel: TickerChannel | undefined, want: TickerRequest): TickerState {
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP()), [channel]);
  const get = useCallback(() => (channel ? channel.get() : NO_TICKER), [channel]);
  const state = useSyncExternalStore(subscribe, get, get);
  const { symbol, day } = want;
  useEffect(() => (channel ? channel.open({ symbol, day }) : undefined), [channel, symbol, day]);
  return state;
}

/** Whether the connected server builds ticker pages: null until its welcome (then the log's card shows or not). */
export function useTickerAvailable(channel: TickerChannel | undefined): boolean | null {
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP()), [channel]);
  const get = useCallback(() => (channel ? channel.get().available : false), [channel]);
  return useSyncExternalStore(subscribe, get, get);
}
