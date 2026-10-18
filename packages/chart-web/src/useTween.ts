import { easeOut, SETTINGS_MOTION_MS } from "@stockwatcher/chart-core";
import { useEffect, useRef, useState } from "react";

/** Whether the OS asks for reduced motion (read at call time: it can change while the page is open). */
export function prefersReducedMotion(): boolean {
  return typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * A number that slides to `target` with chart-core's ease-out instead of
 * jumping (the volume pane growing in or out when the setting flips: the
 * workspace rule is that every visible layout change animates). With reduced
 * motion it jumps. A new target mid-slide starts from where the value is.
 */
export function useTween(target: number, duration = SETTINGS_MOTION_MS): number {
  const [value, setValue] = useState(target);
  const current = useRef(target);
  useEffect(() => {
    const from = current.current;
    if (from === target) return;
    if (prefersReducedMotion() || duration <= 0) {
      current.current = target;
      setValue(target);
      return;
    }
    const start = performance.now();
    let frame = 0;
    const step = (now: number) => {
      const v = easeOut(from, target, now - start, duration);
      current.current = v;
      setValue(v);
      if (v !== target) frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => cancelAnimationFrame(frame);
  }, [target, duration]);
  return value;
}
