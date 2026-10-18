import { describe, expect, test } from "bun:test";
import type { DisplayList } from "@stockwatcher/chart-core";
import { drawDisplayList, type Ctx2D } from "./draw.ts";
import { CSS_VARS, DEFAULT_CHART_THEME, readChartTheme } from "./theme.ts";

/** Records every call and property write, in order. */
function recorder() {
  const log: string[] = [];
  const target: Record<string, unknown> = {};
  const ctx = new Proxy(target, {
    get: (_, k: string) => (...args: unknown[]) => log.push(`${k}(${args.map((a) => JSON.stringify(a)).join(",")})`),
    set: (_, k: string, v) => {
      log.push(`${k}=${JSON.stringify(v)}`);
      return true;
    },
  }) as unknown as Ctx2D;
  return { ctx, log };
}

describe("drawDisplayList", () => {
  test("executes each primitive kind", () => {
    const list: DisplayList = [
      { kind: "rect", x: 1, y: 2, w: 3, h: 4, fill: "red", layer: "candles" },
      { kind: "rect", x: 1, y: 2, w: 3, h: 4, stroke: "red", dash: [3, 2], lineWidth: 1, layer: "candles" },
      { kind: "rect", x: 0, y: 0, w: 9, h: 9, fill: "b", radius: 2, layer: "profile" },
      { kind: "line", x1: 0, y1: 0, x2: 5, y2: 5, stroke: "g", lineWidth: 1, layer: "grid" },
      { kind: "text", x: 1, y: 1, text: "3.20", fill: "w", fontSize: 10, fontFamily: "mono", fontWeight: 400, align: "left", baseline: "middle", layer: "axes" },
      { kind: "path", points: [0, 0, 5, 0, 5, 5], closed: true, fill: "t", layer: "axes" },
    ];
    const { ctx, log } = recorder();
    drawDisplayList(ctx, list);
    expect(log).toMatchSnapshot();
    expect(log).toContain('setLineDash([3,2])');
    expect(log).toContain('roundRect(0,0,9,9,2)');
    expect(log).toContain('font="400 10px mono"');
    expect(log).toContain('closePath()');
    // The dash is reset once a dashed primitive is done.
    expect(log.filter((l) => l === "setLineDash([])").length).toBeGreaterThan(0);
  });

  test("font is only reassigned when it changes", () => {
    const t = { kind: "text", x: 0, y: 0, text: "a", fill: "w", fontSize: 10, fontFamily: "mono", fontWeight: 400, align: "left", baseline: "top", layer: "axes" } as const;
    const { ctx, log } = recorder();
    drawDisplayList(ctx, [t, t, { ...t, fontSize: 11 }]);
    expect(log.filter((l) => l.startsWith("font=")).length).toBe(2);
  });
});

describe("theme", () => {
  test("every token has a --chart-* variable", () => {
    expect(CSS_VARS.upVolume).toBe("--chart-up-volume");
    expect(Object.keys(CSS_VARS).sort()).toEqual(Object.keys(DEFAULT_CHART_THEME).sort());
  });

  test("CSS overrides win; missing variables fall back", () => {
    const globalAny = globalThis as { getComputedStyle?: unknown };
    const saved = globalAny.getComputedStyle;
    globalAny.getComputedStyle = () => ({ getPropertyValue: (n: string) => (n === "--chart-up" ? " #00ff00 " : "") });
    try {
      const t = readChartTheme({} as Element);
      expect(t.up).toBe("#00ff00");
      expect(t.down).toBe(DEFAULT_CHART_THEME.down);
    } finally {
      globalAny.getComputedStyle = saved;
    }
  });
});

test("a gradient path fills with a vertical linear gradient between its two y's (the area chart)", () => {
  const stops: [number, string][] = [];
  let made: number[] = [];
  let filledWith: unknown = null;
  let fillStyle: unknown = null;
  const gradient = { addColorStop: (o: number, c: string) => stops.push([o, c]) };
  const ctx = new Proxy({} as Record<string, unknown>, {
    get: (_, k: string) => (k === "createLinearGradient" ? (...a: number[]) => ((made = a), gradient) : k === "fill" ? () => (filledWith = fillStyle) : () => {}),
    set: (_, k: string, v) => {
      if (k === "fillStyle") fillStyle = v;
      return true;
    },
  }) as unknown as Ctx2D;
  drawDisplayList(ctx, [{ kind: "path", points: [0, 10, 50, 20, 50, 100, 0, 100], closed: true, gradient: { y0: 10, y1: 100, from: "rgba(1, 2, 3, 0.28)", to: "rgba(1, 2, 3, 0.02)" }, layer: "candles" }]);
  expect(made).toEqual([0, 10, 0, 100]);
  expect(stops).toEqual([
    [0, "rgba(1, 2, 3, 0.28)"],
    [1, "rgba(1, 2, 3, 0.02)"],
  ]);
  expect(filledWith).toBe(gradient);
});
