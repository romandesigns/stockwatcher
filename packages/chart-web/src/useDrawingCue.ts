import { CUE_FLASH_MS, CUE_FRAME_MS, cueFlash, cuePulse, drawingCueStates, fireKey, type Drawing, type DrawingCueLayer, type WatchedDrawing } from "@stockwatcher/chart-core";
import { useEffect, useMemo, useRef, useState } from "react";
import { prefersReducedMotion } from "./useTween.ts";

const NONE: readonly WatchedDrawing[] = [];

/**
 * The drawing alerts' cue for chart-web (chart-core drawingAlerts.ts draws it): which watched
 * drawings the last price touches, and the clock for their pulse and for a new fire's flash. The
 * clock ticks only while something animates, and never under reduced motion (a steady glow, no
 * flash), so an idle chart costs nothing. The phone's renderer has the same hook
 * (chart-native useDrawingCue.ts).
 */
export function useDrawingCue(watched: readonly WatchedDrawing[] | undefined, drawings: readonly Drawing[], slots: readonly { t: number }[], timeframeMs: number, last: number | null): DrawingCueLayer | null {
  const list = watched ?? NONE;
  const states = useMemo(() => drawingCueStates(drawings, list, slots, timeframeMs, last), [drawings, list, slots, timeframeMs, last]);
  const touching = states.some((s) => s.touching);
  // When each fire was first seen here: its flash runs from then.
  const heard = useRef(new Map<string, number>());
  const [now, setNow] = useState(() => performance.now());
  const keys = list.map(fireKey).filter((k): k is string => k !== null);
  const keySig = keys.join("|");
  useEffect(() => {
    const t = performance.now();
    for (const k of keys) if (!heard.current.has(k)) heard.current.set(k, t);
    for (const k of heard.current.keys()) if (!keys.includes(k)) heard.current.delete(k);
    setNow(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `keySig` is `keys` as a value
  }, [keySig]);
  const newest = Math.max(-Infinity, ...heard.current.values());
  const flashing = now - newest < CUE_FLASH_MS;
  const reduce = prefersReducedMotion();
  useEffect(() => {
    if (reduce || (!touching && !flashing)) return;
    const id = setInterval(() => setNow(performance.now()), CUE_FRAME_MS);
    return () => clearInterval(id);
  }, [reduce, touching, flashing]);
  return useMemo(() => {
    if (!list.length) return null;
    return { watched: list, last, pulse: reduce ? null : cuePulse(now), flash: reduce || !Number.isFinite(newest) ? 0 : cueFlash(newest, now) };
  }, [list, last, reduce, now, newest]);
}
