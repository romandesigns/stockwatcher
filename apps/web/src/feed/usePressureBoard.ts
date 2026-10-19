// The server's Halt watcher board while the panel is mounted: subscribing opts the connection in
// to pressure.* frames, the last unsubscribe opts it out (useQuoteBoard's pattern).
import { EMPTY_PRESSURE_BOARD, type PressureBoard, type PressureChannel } from "@stockwatcher/shared";
import { useCallback, useSyncExternalStore } from "react";

const NOOP = () => {};

export function usePressureBoard(channel: PressureChannel | undefined): PressureBoard {
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP), [channel]);
  const get = useCallback(() => (channel ? channel.get() : EMPTY_PRESSURE_BOARD), [channel]);
  return useSyncExternalStore(subscribe, get);
}
