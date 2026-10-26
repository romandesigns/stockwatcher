// The Favorites panel's rows while it is mounted: subscribing opts the connection in to
// favorites.* frames, the last unsubscribe opts it out (useRoss's pattern).
import { EMPTY_FAVORITES, type FavoritesChannel, type FavoritesState } from "@stockwatcher/shared";
import { useCallback, useSyncExternalStore } from "react";

const NOOP = () => {};

export function useFavorites(channel: FavoritesChannel | undefined): FavoritesState {
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP), [channel]);
  const get = useCallback(() => (channel ? channel.get() : EMPTY_FAVORITES), [channel]);
  return useSyncExternalStore(subscribe, get, get);
}
