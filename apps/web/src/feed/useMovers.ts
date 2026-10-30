// Top Gainers and Hot stocks Bull / Bear while the dashboard shows them: subscribing opts the
// connection in to movers.* frames, the last unsubscribe opts it out (useFavorites' pattern).
import { EMPTY_MOVERS, type MoversChannel, type MoversState } from "@stockwatcher/shared";
import { useCallback, useSyncExternalStore } from "react";

const NOOP = () => {};

export function useMovers(channel: MoversChannel | undefined): MoversState {
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP), [channel]);
  const get = useCallback(() => (channel ? channel.get() : EMPTY_MOVERS), [channel]);
  // The same snapshot server-side, so the dashboard layout also renders in tests (renderToStaticMarkup).
  return useSyncExternalStore(subscribe, get, get);
}
