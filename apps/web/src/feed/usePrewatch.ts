// The pre-open Watch card while its panel is mounted: subscribing opts the connection in to
// prewatch.* frames, the last unsubscribe opts it out (usePending's pattern).
import { EMPTY_PREWATCH, type PrewatchChannel, type PrewatchState } from "@stockwatcher/shared";
import { useCallback, useSyncExternalStore } from "react";

const NOOP = () => {};

export function usePrewatch(channel: PrewatchChannel | undefined): PrewatchState {
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP), [channel]);
  const get = useCallback(() => (channel ? channel.get() : EMPTY_PREWATCH), [channel]);
  return useSyncExternalStore(subscribe, get, get);
}
