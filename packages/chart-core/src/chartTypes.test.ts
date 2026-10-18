import { describe, expect, test } from "bun:test";
import { CHART_TYPE_ICONS, CHART_TYPES, heikinAshi, hollowStyle, prevCloseAt, startsTradingDay } from "./chartTypes.ts";
import { type ChartTheme, type DisplayList, type LinePrim, type PathPrim, type RectPrim, type TextPrim } from "./displayList.ts";
import { hitDrawing, layoutDrawings, type Drawing } from "./drawings.ts";
import { ICONS } from "./icons.ts";
import { computeChartModel, layout, layoutChart, type ChartState } from "./layout.ts";
import { keyLevels, layoutKeyLevels, layoutLuld } from "./levels.ts";
import { buildSlots, defaultViewport, manualPrice, priceY, yPrice, type PriceScaleType } from "./scale.ts";
import { etToUtc } from "./time.ts";
import { fixtureCandles, fixtureSnapshot } from "./testFixture.ts";
import type { Candle } from "./types.ts";

// Token NAMES as colours, so snapshots and assertions show which token each primitive uses.
const theme = new Proxy({} as ChartTheme, { get: (_, k) => (k === "fontSans" ? "sans" : k === "fontMono" ? "mono" : String(k)) });
const round = (list: DisplayList) => JSON.parse(JSON.stringify(list, (_, v) => (typeof v === "number" ? Math.round(v * 100) / 100 : v)));
const c = (t: number, o: number, h: number, l: number, cl: number, final = true): Candle => ({ t, o, h, l, c: cl, v: 100, n: 1, final });

const snap = fixtureSnapshot("1m", 20, [5]);
const base: ChartState = { timeframe: "1m", candles: snap.candles, gaps: snap.gaps, profile: null, inspectT: null, pointer: null };
const size = { width: 400, height: 300 };
const vp = defaultViewport(16);

describe("Heikin-Ashi", () => {
  test("close = OHLC/4, open = mid of the previous HA body, high/low include the HA open and close", () => {
    const slots = buildSlots([c(0, 10, 12, 9, 11), c(60_000, 11, 15, 10.5, 14), c(120_000, 14, 14.5, 12, 12.5)], []);
    const ha = heikinAshi(slots);
    expect(ha[0]).toEqual({ o: 10.5, c: 10.5, h: 12, l: 9 });
    expect(ha[1]!.c).toBeCloseTo((11 + 15 + 10.5 + 14) / 4);
    expect(ha[1]!.o).toBeCloseTo((10.5 + 10.5) / 2);
    expect(ha[1]!.h).toBe(15);
    expect(ha[1]!.l).toBe(10.5);
    const o2 = (ha[1]!.o + ha[1]!.c) / 2;
    expect(ha[2]!.o).toBeCloseTo(o2);
    // High and low take the HA open and close into account, not only the real extremes.
    expect(ha[2]!.h).toBeCloseTo(Math.max(14.5, o2, (14 + 14.5 + 12 + 12.5) / 4));
    expect(ha[2]!.l).toBeCloseTo(Math.min(12, o2, (14 + 14.5 + 12 + 12.5) / 4));
  });

  test("a gap slot is null and the recursion carries over it; results are cached per slots array", () => {
    const { candles, gaps } = fixtureCandles(8, "1m", etToUtc(2026, 10, 15, 10, 0), [3]);
    const slots = buildSlots(candles, gaps);
    const ha = heikinAshi(slots);
    expect(ha[3]).toBeNull();
    expect(ha[4]!.o).toBeCloseTo((ha[2]!.o + ha[2]!.c) / 2);
    expect(heikinAshi(slots)).toBe(ha);
  });

  test("Heikin-Ashi is synthetic: the last-price tag and auto-fit stay honest", () => {
    const list = layout({ ...base, chartType: "heikinAshi" }, vp, theme, size);
    const tag = list.find((p): p is TextPrim => p.kind === "text" && p.fontWeight === 600 && p.layer === "axes")!;
    expect(tag.text).toBe(snap.candles.at(-1)!.c.toFixed(2));
  });
});

describe("hollow candles", () => {
  test("colour by close vs previous close, fill by close vs open", () => {
    expect(hollowStyle({ o: 10, c: 11 }, 12)).toEqual({ up: false, hollow: true }); // rallied, still under yesterday's close
    expect(hollowStyle({ o: 11, c: 10.5 }, 10)).toEqual({ up: true, hollow: false }); // faded, still above the prior close
    expect(hollowStyle({ o: 10, c: 11 }, 10)).toEqual({ up: true, hollow: true });
    expect(hollowStyle({ o: 11, c: 10 }, 12)).toEqual({ up: false, hollow: false });
    expect(hollowStyle({ o: 10, c: 10 }, null)).toEqual({ up: true, hollow: true }); // first candle: its own open
  });

  test("the previous close skips gap slots", () => {
    const { candles, gaps } = fixtureCandles(6, "1m", etToUtc(2026, 10, 15, 10, 0), [2]);
    const slots = buildSlots(candles, gaps);
    expect(prevCloseAt(slots, 3)).toBe(candles[1]!.c);
    expect(prevCloseAt(slots, 0)).toBeNull();
  });

  test("a hollow body is an outline, a filled one a fill", () => {
    const slots = buildSlots([c(0, 10, 12, 9, 11), c(60_000, 11, 11.5, 9.5, 10)], []);
    const list = layout({ ...base, candles: slots.map((s) => (s.kind === "candle" ? s.candle : c(0, 0, 0, 0, 0))), gaps: [], chartType: "hollow" }, defaultViewport(30), theme, size);
    const bodies = list.filter((p): p is RectPrim => p.kind === "rect" && p.layer === "candles");
    expect(bodies[0]!.stroke).toBe("up");
    expect(bodies[0]!.fill).toBeUndefined();
    expect(bodies[1]!.fill).toBe("down");
  });
});

describe("every chart type draws through the display list", () => {
  for (const type of CHART_TYPES) {
    test(`${type}: snapshot, finite numbers, a mark per candle type`, () => {
      const list = layout({ ...base, chartType: type, baselinePrice: 3.25 }, vp, theme, size);
      for (const p of list) for (const v of Object.values(p)) if (typeof v === "number") expect(Number.isFinite(v)).toBe(true);
      const series = list.filter((p) => p.layer === "candles");
      expect(series.length).toBeGreaterThan(0);
      expect(round(series)).toMatchSnapshot();
      // Volume is the real candles' whatever the type.
      expect(list.filter((p) => p.layer === "volume").length).toBe(snap.candles.length);
    });
  }

  test("line: one colour by last vs first visible close; area adds a gradient fill and no stroke around it", () => {
    const line = layout({ ...base, chartType: "line" }, vp, theme, size).filter((p): p is PathPrim => p.kind === "path" && p.layer === "candles");
    expect(line).toHaveLength(1);
    expect(line[0]!.stroke).toBe("up"); // the fixture drifts up
    const area = layout({ ...base, chartType: "area" }, vp, theme, size).filter((p): p is PathPrim => p.kind === "path" && p.layer === "candles");
    expect(area).toHaveLength(2);
    expect(area[0]!.gradient).toBeDefined();
    expect(area[0]!.stroke).toBeUndefined();
  });

  test("baseline: split at the reference into an up side and a down side, with a dashed guide", () => {
    const list = layout({ ...base, chartType: "baseline", baselinePrice: 3.3 }, vp, theme, size);
    const strokes = list.filter((p): p is PathPrim => p.kind === "path" && !!p.stroke).map((p) => p.stroke);
    expect(strokes).toEqual(["up", "down"]);
    const m = computeChartModel({ ...base, chartType: "baseline" }, vp, size);
    const guide = list.find((p): p is LinePrim => p.kind === "line" && p.layer === "candles" && !!p.dash)!;
    expect(guide.y1).toBeCloseTo(Math.round(priceY(m.price!, 3.3)) + 0.5, 6);
  });

  test("bars: a high-low line plus an open tick left and a close tick right", () => {
    const list = layout({ ...base, chartType: "bars" }, vp, theme, size).filter((p): p is LinePrim => p.kind === "line" && p.layer === "candles");
    expect(list.length).toBe(snap.candles.length * 3);
    const [, open, close] = list;
    expect(open!.x2).toBeLessThanOrEqual(close!.x1);
  });

  test("display switches: grid off, shading off, session breaks and high/low labels on", () => {
    const st = { ...base, candles: [...fixtureCandles(30, "1m", etToUtc(2026, 10, 14, 15, 45)).candles, ...fixtureCandles(30, "1m", etToUtc(2026, 10, 15, 9, 30)).candles], gaps: [] };
    const wide = defaultViewport(5);
    const on = layout(st, wide, theme, size);
    expect(on.some((p) => p.layer === "grid" && p.kind === "line" && p.stroke === "grid")).toBe(true);
    expect(on.some((p) => p.layer === "background")).toBe(true);
    const off = layout({ ...st, chartStyle: { grid: false, sessionShading: false, sessionBreaks: true, highLow: true } }, wide, theme, size);
    expect(off.some((p) => p.layer === "grid" && p.kind === "line" && p.stroke === "grid")).toBe(false);
    expect(off.some((p) => p.layer === "background")).toBe(false);
    expect(off.filter((p) => p.kind === "line" && p.stroke === "guide" && p.layer === "grid")).toHaveLength(1);
    expect(off.filter((p) => p.kind === "text" && p.layer === "overlay")).toHaveLength(2);
  });

  test("a new trading day starts at its first slot", () => {
    const slots = buildSlots([c(etToUtc(2026, 10, 14, 15, 59), 1, 1, 1, 1), c(etToUtc(2026, 10, 15, 4, 0), 1, 1, 1, 1), c(etToUtc(2026, 10, 15, 4, 1), 1, 1, 1, 1)], []);
    expect([1, 2].map((i) => startsTradingDay(slots, i))).toEqual([true, false]);
    // An evening candle (20:00 ET on) already belongs to the next trading day.
    expect(startsTradingDay(buildSlots([c(etToUtc(2026, 10, 14, 19, 59), 1, 1, 1, 1), c(etToUtc(2026, 10, 14, 20, 0), 1, 1, 1, 1)], []), 1)).toBe(true);
  });

  test("every type has a duotone picker icon", () => {
    for (const t of CHART_TYPES) expect(ICONS[CHART_TYPE_ICONS[t]]).toBeDefined();
  });
});

describe("prices stay attached under every scale", () => {
  const SCALES: PriceScaleType[] = ["linear", "percent", "log"];
  const t = snap.candles[8]!.t;
  const level: Drawing = { id: "lvl", kind: "hline", a: { t, p: 3.27 }, b: { t, p: 3.27 } };
  const trend: Drawing = { id: "tr", kind: "trend", a: { t: snap.candles[2]!.t, p: 3.22 }, b: { t: snap.candles[14]!.t, p: 3.35 } };

  for (const scale of SCALES) {
    test(`${scale}: a level and a trend line sit at their prices, and are hit where they are drawn`, () => {
      const st: ChartState = { ...base, priceScaleType: scale, drawings: [level, trend] };
      const m = computeChartModel(st, vp, size);
      const list = layoutDrawings(m, [level, trend], null, theme);
      const y = priceY(m.price!, 3.27);
      const lines = list.filter((p): p is LinePrim => p.kind === "line");
      expect(lines.some((l) => Math.abs(l.y1 - y) < 1 && Math.abs(l.y2 - y) < 1)).toBe(true);
      expect(yPrice(m.price!, y)).toBeCloseTo(3.27, 9);
      expect(hitDrawing(m, [level], 50, y, 4)?.id).toBe("lvl");
      // The drawing's stored prices never change with the scale: only their y does.
      expect(level.a.p).toBe(3.27);
    });

    test(`${scale}: key levels and LULD bands map through the scale; a manual range keeps them in place`, () => {
      const slots = buildSlots(snap.candles, snap.gaps);
      const levels = keyLevels(slots, 3.21);
      const m = computeChartModel({ ...base, priceScaleType: scale }, vp, size);
      const drawn = layoutKeyLevels(m, levels, theme).filter((p): p is LinePrim => p.kind === "line");
      for (const l of drawn) expect(levels.some((k) => Math.abs(Math.round(priceY(m.price!, k.price)) + 0.5 - l.y1) < 1e-6)).toBe(true);
      const band = { t: snap.candles[0]!.t, ref: 3.25, up: 3.4, down: 3.1 };
      const manual = manualPrice(vp, { lo: 3.0, hi: 3.5 });
      const mm = computeChartModel({ ...base, priceScaleType: scale }, manual, size);
      const luld = layoutLuld(mm, [band], theme).filter((p): p is LinePrim => p.kind === "line");
      expect(luld.some((l) => Math.abs(l.y1 - priceY(mm.price!, 3.4)) < 1e-6)).toBe(true);
      expect(luld.some((l) => Math.abs(l.y1 - priceY(mm.price!, 3.1)) < 1e-6)).toBe(true);
    });
  }

  test("percent: the axis words percents from the first visible open, the tag still a percent of the REAL close", () => {
    const st: ChartState = { ...base, priceScaleType: "percent", chartType: "heikinAshi" };
    const m = computeChartModel(st, vp, size);
    expect(m.price!.base).toBe(snap.candles.find((x) => x.t >= m.slots[m.index.from]!.t)!.o);
    const list = layoutChart(m, st, theme);
    const tag = list.find((p): p is TextPrim => p.kind === "text" && p.fontWeight === 600 && p.layer === "axes")!;
    const last = snap.candles.at(-1)!.c;
    expect(tag.text).toContain(Math.abs((last / m.price!.base! - 1) * 100).toFixed(2));
  });
});
