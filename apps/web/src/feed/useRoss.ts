// The server's Ross Panel while it is mounted: subscribing opts the connection in to ross.*
// frames, the last unsubscribe opts it out (usePressureBoard's pattern).
import { EMPTY_ROSS, type RossChannel, type RossState } from "@stockwatcher/shared";
import { useCallback, useSyncExternalStore } from "react";

const NOOP = () => {};

export function useRoss(channel: RossChannel | undefined): RossState {
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP), [channel]);
  const get = useCallback(() => (channel ? channel.get() : EMPTY_ROSS), [channel]);
  return useSyncExternalStore(subscribe, get);
}
