import { CUE_FLASH_MS, CUE_FRAME_MS, cueFlash, cuePulse, drawingCueStates, fireKey, type Drawing, type DrawingCueLayer, type WatchedDrawing } from "@stockwatcher/chart-core";
import { useEffect, useMemo, useRef, useState } from "react";

const NONE: readonly WatchedDrawing[] = [];

/**
 * The drawing alerts' cue for the native chart, the twin of chart-web's useDrawingCue: which
 * watched drawings the last price touches, and the clock for the pulse and a new fire's flash
 * (chart-core drawingAlerts.ts draws them, so web and phone look the same). The clock ticks only
 * while something animates and never under reduced motion (the OS setting, read by the chart).
 */
export function useDrawingCue(watched: readonly WatchedDrawing[] | undefined, drawings: readonly Drawing[], slots: readonly { t: number }[], timeframeMs: number, last: number | null, reduce: boolean): DrawingCueLayer | null {
  const list = watched ?? NONE;
  const states = useMemo(() => drawingCueStates(drawings, list, slots, timeframeMs, last), [drawings, list, slots, timeframeMs, last]);
  const touching = states.some((s) => s.touching);
  const heard = useRef(new Map<string, number>());
  const [now, setNow] = useState(() => Date.now());
  const keys = list.map(fireKey).filter((k): k is string => k !== null);
  const keySig = keys.join("|");
  useEffect(() => {
    const t = Date.now();
    for (const k of keys) if (!heard.current.has(k)) heard.current.set(k, t);
    for (const k of heard.current.keys()) if (!keys.includes(k)) heard.current.delete(k);
    setNow(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `keySig` is `keys` as a value
  }, [keySig]);
  const newest = Math.max(-Infinity, ...heard.current.values());
  const flashing = now - newest < CUE_FLASH_MS;
  useEffect(() => {
    if (reduce || (!touching && !flashing)) return;
    const id = setInterval(() => setNow(Date.now()), CUE_FRAME_MS);
    return () => clearInterval(id);
  }, [reduce, touching, flashing]);
  return useMemo(() => {
    if (!list.length) return null;
    return { watched: list, last, pulse: reduce ? null : cuePulse(now), flash: reduce || !Number.isFinite(newest) ? 0 : cueFlash(newest, now) };
  }, [list, last, reduce, now, newest]);
}
