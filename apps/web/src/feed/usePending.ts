// The News pending card while its panel is mounted: subscribing opts the connection in to
// pending.* frames, the last unsubscribe opts it out (useHod's pattern).
import { EMPTY_PENDING, type PendingChannel, type PendingState } from "@stockwatcher/shared";
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";

const NOOP = () => {};

export function usePending(channel: PendingChannel | undefined): PendingState {
  const subscribe = useCallback((cb: () => void) => (channel ? channel.subscribe(cb) : NOOP), [channel]);
  const get = useCallback(() => (channel ? channel.get() : EMPTY_PENDING), [channel]);
  return useSyncExternalStore(subscribe, get, get);
}

/**
 * The wall clock, re-read once a second while `on` (a card is on screen) and never otherwise: the
 * card's timers are computed from its timestamps, so the server sends nothing per tick.
 */
export function useSecondClock(on: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    // Aligned to the second boundary so every card's timer turns over together.
    let id: ReturnType<typeof setTimeout>;
    const step = () => {
      setNow(Date.now());
      id = setTimeout(step, 1000 - (Date.now() % 1000));
    };
    id = setTimeout(step, 1000 - (Date.now() % 1000));
    return () => clearTimeout(id);
  }, [on]);
  return now;
}
