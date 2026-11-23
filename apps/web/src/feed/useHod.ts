// The HOD Momentum scanner while its panel is mounted: subscribing opts the connection in to
// hod.* frames, the last unsubscribe opts it out (useMovers' pattern).
import { EMPTY_HOD, type HodChannel, type HodState } from "@stockwatcher/shared";
import { useCallback, useSyncExternalStore } from "react";

const NOOP = () => {};

export function useHod(channel: HodChannel | undefined): HodState {
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP), [channel]);
  const get = useCallback(() => (channel ? channel.get() : EMPTY_HOD), [channel]);
  return useSyncExternalStore(subscribe, get, get);
}
