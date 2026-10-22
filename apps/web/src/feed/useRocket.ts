// The server's Rocket watch while its tab is mounted: subscribing opts the connection in to
// rocket.* frames, the last unsubscribe opts it out (useRoss's pattern).
import { EMPTY_ROCKET, type RocketChannel, type RocketState } from "@stockwatcher/shared";
import { useCallback, useSyncExternalStore } from "react";

const NOOP = () => {};

export function useRocket(channel: RocketChannel | undefined): RocketState {
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP), [channel]);
  const get = useCallback(() => (channel ? channel.get() : EMPTY_ROCKET), [channel]);
  return useSyncExternalStore(subscribe, get, get);
}
