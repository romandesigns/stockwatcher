import { EMPTY_DETECTION, type DetectionChannel } from "@stockwatcher/shared";
import { useCallback, useSyncExternalStore } from "react";
const NOOP = () => {};
export function useDetection(channel: DetectionChannel | undefined) {
  const subscribe = useCallback((cb: () => void) => channel ? channel.subscribe(cb) : NOOP, [channel]);
  const get = useCallback(() => channel ? channel.get() : EMPTY_DETECTION, [channel]);
  return useSyncExternalStore(subscribe, get);
}
