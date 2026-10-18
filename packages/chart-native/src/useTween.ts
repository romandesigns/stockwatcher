import { easeOut, SETTINGS_MOTION_MS } from "@stockwatcher/chart-core";
import { useEffect, useRef, useState } from "react";

/**
 * A number that slides to `target` with chart-core's ease-out (the volume
 * pane growing in or out when its setting flips), the twin of chart-web's
 * useTween. `reducedMotion` (the OS setting, which the chart already reads)
 * makes it jump. A new target mid-slide starts from where the value is.
 */
export function useTween(target: number, reducedMotion: boolean, duration = SETTINGS_MOTION_MS): number {
  const [value, setValue] = useState(target);
  const current = useRef(target);
  useEffect(() => {
    const from = current.current;
    if (from === target) return;
    if (reducedMotion || duration <= 0) {
      current.current = target;
      setValue(target);
      return;
    }
    const start = performance.now();
    let frame = 0;
    const step = () => {
      const v = easeOut(from, target, performance.now() - start, duration);
      current.current = v;
      setValue(v);
      if (v !== target) frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => cancelAnimationFrame(frame);
  }, [target, duration, reducedMotion]);
  return value;
}
