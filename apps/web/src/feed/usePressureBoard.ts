// The server's Halt watcher board while the panel is mounted: subscribing opts the connection in
// to pressure.* frames, the last unsubscribe opts it out (the quote board's pattern).
import { EMPTY_PRESSURE_BOARD, EMPTY_PRESSURE_STATUS, type PressureBoard, type PressureChannel, type PressureStatus } from "@stockwatcher/shared";
import { useCallback, useSyncExternalStore } from "react";

const NOOP = () => {};

export function usePressureBoard(channel: PressureChannel | undefined): PressureBoard {
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP), [channel]);
  const get = useCallback(() => (channel ? channel.get() : EMPTY_PRESSURE_BOARD), [channel]);
  // The same snapshot server-side, so the dashboard layout also renders in tests (renderToStaticMarkup).
  return useSyncExternalStore(subscribe, get, get);
}

/** A separate stable snapshot wakes the panel even when only connection status changes. */
export function usePressureStatus(channel: PressureChannel | undefined): PressureStatus {
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP), [channel]);
  const get = useCallback(() => (channel ? channel.status() : EMPTY_PRESSURE_STATUS), [channel]);
  // The same snapshot server-side, so the dashboard layout also renders in tests (renderToStaticMarkup).
  return useSyncExternalStore(subscribe, get, get);
}
