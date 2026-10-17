import type { ChartTheme, Size } from "@stockwatcher/chart-core";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { readChartTheme } from "./theme.ts";

export type Paint = (ctx: CanvasRenderingContext2D, size: Size) => void;

/**
 * A DPR-aware canvas that fills its container. It paints once per animation
 * frame and only when something changed: a new `paint` function (memoize it
 * on your data), a resize, a devicePixelRatio change (zoom, moving between
 * monitors) or web fonts finishing loading. No continuous loop.
 */
export function useCanvas(paint: Paint) {
  const containerRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [size, setSize] = useState<Size>({ width: 0, height: 0 });
  const [dpr, setDpr] = useState(() => (typeof window === "undefined" ? 1 : window.devicePixelRatio || 1));
  const paintRef = useRef(paint);
  const frame = useRef(0);
  const sizeRef = useRef(size);
  const dprRef = useRef(dpr);

  useLayoutEffect(() => {
    paintRef.current = paint;
    sizeRef.current = size;
    dprRef.current = dpr;
  });

  const schedule = useCallback(() => {
    if (frame.current) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = 0;
      const canvas = canvasRef.current;
      const ctx = canvas?.getContext("2d");
      const { width, height } = sizeRef.current;
      if (!canvas || !ctx || width <= 0 || height <= 0) return;
      const ratio = dprRef.current;
      const w = Math.round(width * ratio);
      const h = Math.round(height * ratio);
      if (canvas.width !== w || canvas.height !== h) {
        canvas.width = w;
        canvas.height = h;
      }
      ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
      ctx.clearRect(0, 0, width, height);
      paintRef.current(ctx, { width, height });
    });
  }, []);

  useEffect(() => () => cancelAnimationFrame(frame.current), []);

  useLayoutEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const measure = () => {
      const r = el.getBoundingClientRect();
      setSize((s) => (s.width === r.width && s.height === r.height ? s : { width: r.width, height: r.height }));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    // A resolution query only matches the current DPR, so re-arm on change.
    const mq = matchMedia(`(resolution: ${dpr}dppx)`);
    const onChange = () => setDpr(window.devicePixelRatio || 1);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [dpr]);

  useEffect(() => {
    let alive = true;
    document.fonts?.ready.then(() => alive && schedule());
    return () => {
      alive = false;
    };
  }, [schedule]);

  useEffect(schedule, [paint, size, dpr, schedule]);

  return { containerRef, canvasRef, size };
}

/**
 * The chart theme from `--chart-*` CSS variables on <html> (falling back to
 * DEFAULT_CHART_THEME), re-read when the color scheme changes.
 */
export function useChartTheme(override?: ChartTheme): ChartTheme {
  const [theme, setTheme] = useState<ChartTheme>(() => override ?? readChartTheme(typeof document === "undefined" ? null : document.documentElement));
  useEffect(() => {
    if (override) {
      setTheme(override);
      return;
    }
    const read = () => setTheme(readChartTheme(document.documentElement));
    read();
    const mq = matchMedia("(prefers-color-scheme: dark)");
    mq.addEventListener("change", read);
    return () => mq.removeEventListener("change", read);
  }, [override]);
  return theme;
}
