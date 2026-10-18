import { describe, expect, test } from "bun:test";
import { GestureController, type GestureAction, type GestureInput } from "./gestures.ts";
import { computeIndicators } from "./indicators.ts";
import { applyPaneGesture, chartGeometry, formatSignificant, computeChartModel, DEFAULT_CHART_OPTIONS, expandedPaneHeight, hitTest, layoutChart, MAX_PANES_SHARE, MIN_PANE_HEIGHT, paneIdsOf, type ChartState } from "./layout.ts";
import { buildSlots } from "./scale.ts";
import { defaultViewport } from "./scale.ts";
import { fixtureSnapshot } from "./testFixture.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";

// RSI and MACD panes on a 400×600 fixture chart: oscillator panes, their borders, and how they are drawn.
const snap = fixtureSnapshot("1m", 60, []);
const slots = buildSlots(snap.candles, snap.gaps);
const indicators = computeIndicators(slots, [
  { id: "rsi-14", kind: "rsi", period: 14 },
  { id: "macd", kind: "macd", fast: 12, slow: 26, signal: 9 },
]);
// Tall enough that two default panes stay under the share cap (a 300 px chart would scale them down).
const size = { width: 400, height: 600 };
const base: ChartState = { timeframe: "1m", candles: snap.candles, gaps: snap.gaps, profile: null, slots, inspectT: null, pointer: null, indicators };
const vp = defaultViewport(6);
const modelOf = (s: ChartState) => computeChartModel(s, vp, size);

describe("pane geometry", () => {
  test("stored heights apply per indicator id; others keep the default", () => {
    const m = modelOf({ ...base, paneHeights: { macd: 70 } });
    expect(m.geometry.panes.map((r) => r.h)).toEqual([DEFAULT_CHART_OPTIONS.indicatorPaneHeight, 70]);
    expect(paneIdsOf(base)).toEqual(["rsi-14", "macd"]);
  });

  test("panes never take more than their share of the room, nor go below the minimum", () => {
    const o = DEFAULT_CHART_OPTIONS;
    const g = chartGeometry(size, o, [500, 500]);
    const room = size.height - o.timeAxisHeight - o.volumeHeight - o.paneGap;
    const total = g.panes.reduce((a, r) => a + r.h + o.paneGap, 0);
    expect(total).toBeLessThanOrEqual(Math.ceil(room * MAX_PANES_SHARE));
    expect(g.plot.h).toBeGreaterThan(0);
    expect(chartGeometry(size, o, [5]).panes[0]!.h).toBe(MIN_PANE_HEIGHT);
  });
});

describe("pane border", () => {
  const m = modelOf(base);
  const pane = m.geometry.panes[1]!;

  test("a press on a pane's top edge hits its border, inside it hits the pane", () => {
    expect(hitTest(m, 100, pane.y + 2)).toMatchObject({ region: "paneBorder", pane: 1 });
    expect(hitTest(m, 100, pane.y - 2)).toMatchObject({ region: "paneBorder", pane: 1 });
    expect(hitTest(m, 100, pane.y + pane.h / 2 + 5)).toMatchObject({ region: "pane", pane: 1 });
    expect(hitTest(m, 100, 50).pane).toBeNull();
  });

  const ev = (type: GestureInput["type"], y: number, t: number): GestureInput => ({ type, pointerId: 1, pointerType: "touch", x: 100, y, t });

  test("dragging the border resizes the pane, then finishes", () => {
    const g = new GestureController({ haptics: true });
    const ctx = { model: m, inspectT: null };
    const log: GestureAction[] = [];
    log.push(...g.handle(ev("down", pane.y + 2, 0), ctx));
    log.push(...g.handle(ev("move", pane.y - 20, 30), ctx));
    log.push(...g.handle(ev("up", pane.y - 20, 60), ctx));
    const resizes = log.filter((a) => a.kind === "resizePane");
    expect(resizes.at(-1)).toEqual({ kind: "resizePane", pane: 1, dy: 0, done: true });
    const dy = resizes.filter((a) => a.kind === "resizePane" && !a.done).reduce((s, a) => s + (a.kind === "resizePane" ? a.dy : 0), 0);
    expect(dy).toBe(-22);
    expect(log.some((a) => a.kind === "haptic")).toBe(true);
    expect(log.some((a) => a.kind === "pan")).toBe(false);
  });

  test("a double tap on a pane toggles it; on the price plot it still auto-scales", () => {
    const g = new GestureController();
    const ctx = { model: m, inspectT: null };
    const y = pane.y + pane.h / 2 + 5;
    g.handle(ev("down", y, 0), ctx);
    g.handle(ev("up", y, 40), ctx);
    g.handle(ev("down", y, 120), ctx);
    expect(g.handle(ev("up", y, 160), ctx)).toEqual([{ kind: "togglePane", pane: 1 }]);
    g.handle(ev("down", 50, 1000), ctx);
    g.handle(ev("up", 50, 1040), ctx);
    g.handle(ev("down", 50, 1120), ctx);
    expect(g.handle(ev("up", 50, 1160), ctx).map((a) => a.kind)).toContain("autoScalePrice");
  });
});

describe("applyPaneGesture", () => {
  const m = modelOf(base);
  const ids = paneIdsOf(base);

  test("dragging the border up grows the pane, and it stops at the minimum", () => {
    expect(applyPaneGesture({}, { kind: "resizePane", pane: 0, dy: -30 }, m, ids, size)).toEqual({ "rsi-14": DEFAULT_CHART_OPTIONS.indicatorPaneHeight + 30 });
    expect(applyPaneGesture({}, { kind: "resizePane", pane: 0, dy: 500 }, m, ids, size)).toEqual({ "rsi-14": MIN_PANE_HEIGHT });
  });

  test("toggle expands a pane, and a second toggle returns it to the default", () => {
    const expanded = expandedPaneHeight(size, m.options);
    const once = applyPaneGesture({}, { kind: "togglePane", pane: 1 }, m, ids, size);
    expect(once).toEqual({ macd: expanded });
    const m2 = modelOf({ ...base, paneHeights: once });
    expect(applyPaneGesture(once, { kind: "togglePane", pane: 1 }, m2, ids, size)).toEqual({});
  });
});

describe("oscillator look", () => {
  const list = layoutChart(modelOf(base), base, DEFAULT_CHART_THEME);

  test("RSI gets its shaded 30–70 zone and a 50 midline", () => {
    expect(list.some((p) => p.kind === "rect" && p.fill === DEFAULT_CHART_THEME.oscZone)).toBe(true);
    expect(list.some((p) => p.kind === "line" && p.stroke === DEFAULT_CHART_THEME.oscMid)).toBe(true);
  });

  test("MACD's histogram uses growing and shrinking shades", () => {
    const fills = new Set(list.filter((p) => p.kind === "rect" && p.layer === "volume").map((p) => (p.kind === "rect" ? p.fill : "")));
    const shades = [DEFAULT_CHART_THEME.histUp, DEFAULT_CHART_THEME.histUpFade, DEFAULT_CHART_THEME.histDown, DEFAULT_CHART_THEME.histDownFade].filter((c) => fills.has(c));
    expect(shades.length).toBeGreaterThanOrEqual(3);
  });

  test("each pane legend sits on a backing, and MACD's lists the histogram value", () => {
    expect(list.filter((p) => p.kind === "rect" && p.fill === DEFAULT_CHART_THEME.paneLegendBg).length).toBe(2);
    const macdPane = modelOf(base).geometry.panes[1]!;
    const values = list.filter((p) => p.kind === "text" && p.fontFamily === DEFAULT_CHART_THEME.fontMono && Math.abs(p.y - (macdPane.y + 8)) < 1);
    expect(values.length).toBe(3);
  });
});

describe("formatSignificant", () => {
  test("a sub-$1 stock's MACD keeps three significant digits instead of reading -0.0000", () => {
    expect(formatSignificant(-0.0000123, 3)).toBe("-0.000012"); // capped at 6 decimals
    expect(formatSignificant(0.0004567, 3)).toBe("0.000457");
    expect(formatSignificant(1.23456, 3)).toBe("1.23");
    expect(formatSignificant(123.4, 3)).toBe("123");
    expect(formatSignificant(0, 3)).toBe("0");
  });
});

describe("overlay legend", () => {
  // EMA 9, EMA 20 and VWAP: the three overlays the owner runs together.
  const overlays = computeIndicators(slots, [
    { id: "ema-9", kind: "ema", period: 9 },
    { id: "ema-20", kind: "ema", period: 20 },
    { id: "vwap", kind: "vwap" },
  ]);
  const names = ["EMA 9", "EMA 20", "VWAP"];
  const legendOf = (width: number) => {
    const s: ChartState = { ...base, indicators: overlays };
    const m = computeChartModel(s, vp, { width, height: 400 });
    const list = layoutChart(m, s, DEFAULT_CHART_THEME);
    const backings = list.filter((p) => p.kind === "rect" && p.fill === DEFAULT_CHART_THEME.paneLegendBg) as { x: number; w: number }[];
    return { m, backings, texts: list.filter((p) => p.kind === "text" && names.includes(p.text)) as { x: number; y: number; text: string }[] };
  };

  test("all overlays share one line, left to right, when the plot is wide enough", () => {
    const { texts, backings } = legendOf(800);
    expect(texts.map((t) => t.text)).toEqual(names);
    expect(backings.length).toBe(1); // one line, one backing
    expect(new Set(texts.map((t) => t.y)).size).toBe(1);
    expect(texts[0]!.x).toBeLessThan(texts[1]!.x);
    expect(texts[1]!.x).toBeLessThan(texts[2]!.x);
  });

  test("a narrow plot wraps whole entries onto a new line and never past the right edge", () => {
    const { m, texts, backings } = legendOf(200);
    const ys = [...new Set(texts.map((t) => t.y))];
    expect(ys.length).toBeGreaterThan(1);
    expect(backings.length).toBe(ys.length); // a backing per line
    for (const b of backings) expect(b.x + b.w).toBeLessThanOrEqual(m.geometry.plot.x + m.geometry.plot.w);
    // A wrapped entry starts back at the left margin.
    const firstX = texts[0]!.x;
    for (const y of ys.slice(1)) expect(texts.find((t) => t.y === y)!.x).toBe(firstX);
    for (const t of texts) expect(t.x).toBeLessThan(m.geometry.plot.x + m.geometry.plot.w);
  });
});
