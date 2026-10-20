import { describe, expect, test } from "bun:test";
import { DEFAULT_CHART_SETTINGS, daySlotCount, easeOut, openView, parseChartSettings, presetBarSpacing, presetSlots, presetViewport, viewKey } from "./chartSettings.ts";
import { GestureController, applyGesture } from "./gestures.ts";
import { autoFitRange, computeChartModel, type ChartState } from "./layout.ts";
import { buildSlots, defaultViewport, isAutoPrice, MIN_BAR_SPACING, MAX_BAR_SPACING, panSlots, RIGHT_MARGIN_PX, slotX } from "./scale.ts";
import { sessionSlots } from "./chartTypes.ts";
import { etToUtc } from "./time.ts";
import { fixtureCandles } from "./testFixture.ts";
import type { Timeframe } from "./types.ts";

const OPEN = etToUtc(2026, 10, 15, 9, 30);
const PRE = etToUtc(2026, 10, 15, 4, 0);
const PREV_OPEN = etToUtc(2026, 10, 14, 9, 30);

const day = (n: number, tf: Timeframe = "1m", start = OPEN) => {
  const { candles, gaps } = fixtureCandles(n, tf, start);
  return { candles, gaps, slots: buildSlots(candles, gaps) };
};
const stateOf = (d: ReturnType<typeof day>, timeframe: Timeframe = "1m"): ChartState => ({ timeframe, candles: d.candles, gaps: d.gaps, slots: d.slots, profile: null, inspectT: null, pointer: null });
const size = { width: 800, height: 400 };

describe("settings: defaults, validation, migration", () => {
  test("defaults keep today's look and turn the new zoom on", () => {
    expect(DEFAULT_CHART_SETTINGS).toMatchObject({ chartType: "candles", priceScale: "linear", extendedHours: true, volume: true, grid: true, sessionShading: true, sessionBreaks: false, highLow: false, autoStretch: true, zoom: 100 });
    expect(DEFAULT_CHART_SETTINGS.visibility).toEqual({ indicators: true, drawings: true, trading: true, linkStudies: true });
  });

  test("junk and nothing fall back to the defaults, and never share the default object", () => {
    for (const raw of [null, undefined, 3, "x", [], { chartType: "pie", priceScale: 7, zoom: 33, volume: "yes", visibility: 1 }]) {
      const s = parseChartSettings(raw);
      expect(s).toEqual(DEFAULT_CHART_SETTINGS);
      expect(s).not.toBe(DEFAULT_CHART_SETTINGS);
      expect(s.visibility).not.toBe(DEFAULT_CHART_SETTINGS.visibility);
    }
  });

  test("each valid field is kept on its own; a save missing new fields gets their defaults", () => {
    const s = parseChartSettings({ chartType: "heikinAshi", volume: false, visibility: { drawings: false } });
    expect(s.chartType).toBe("heikinAshi");
    expect(s.volume).toBe(false);
    expect(s.visibility).toEqual({ indicators: true, drawings: false, trading: true, linkStudies: true });
    // A save from before the link switch existed keeps today's behaviour: linked.
    expect(parseChartSettings({ visibility: { linkStudies: false } }).visibility.linkStudies).toBe(false);
    expect(s.grid).toBe(true);
    expect(s.version).toBe(1);
  });

  test("older shapes migrate: fractional or string zoom, Robinhood's own type and scale words", () => {
    expect(parseChartSettings({ zoom: 0.25 }).zoom).toBe(25);
    expect(parseChartSettings({ zoom: 1 }).zoom).toBe(100);
    expect(parseChartSettings({ zoom: "50%" }).zoom).toBe(50);
    expect(parseChartSettings({ chartType: "candlestick" }).chartType).toBe("candles");
    expect(parseChartSettings({ chartType: "ha" }).chartType).toBe("heikinAshi");
    expect(parseChartSettings({ priceScale: "logarithmic" }).priceScale).toBe("log");
    expect(parseChartSettings({ priceScale: "percentage" }).priceScale).toBe("percent");
  });

  test("round-trips through JSON (what localStorage and the phone's preferences store)", () => {
    const s = { ...DEFAULT_CHART_SETTINGS, chartType: "area" as const, zoom: 50 as const, extendedHours: false };
    expect(parseChartSettings(JSON.parse(JSON.stringify(s)))).toEqual(s);
  });
});

describe("extended hours", () => {
  test("off: only regular-session slots remain, pre-market candles and gaps included, so nothing leaves a hole", () => {
    const { candles, gaps } = fixtureCandles(400, "1m", PRE, [10, 200, 380]);
    const all = buildSlots(candles, gaps);
    const regular = sessionSlots(all, false);
    expect(all.length).toBe(400);
    // 04:00 + 330 min = 09:30: the first 330 slots are pre-market.
    expect(regular.length).toBe(70);
    expect(regular[0]!.t).toBe(OPEN);
    expect(regular.some((s) => s.kind === "gap")).toBe(true); // the gap at 380 is regular
    expect(regular.every((s) => s.t >= OPEN)).toBe(true);
    // On: the same array (no copy), and the filter is cached per slots array.
    expect(sessionSlots(all, true)).toBe(all);
    expect(sessionSlots(all, false)).toBe(regular);
  });

  test("off across two days: the next open sits in the slot right after the previous 15:59", () => {
    const a = fixtureCandles(390 + 240, "1m", PREV_OPEN).candles; // regular + after hours of the day before
    const b = fixtureCandles(400, "1m", PRE).candles;
    const slots = sessionSlots(buildSlots([...a, ...b], []), false);
    const i = slots.findIndex((s) => s.t === OPEN);
    expect(slots[i - 1]!.t).toBe(etToUtc(2026, 10, 14, 15, 59));
  });

  test("computeChartModel filters only the slots it builds itself", () => {
    const { candles, gaps } = fixtureCandles(400, "1m", PRE);
    const m = computeChartModel({ timeframe: "1m", candles, gaps, profile: null, inspectT: null, pointer: null, extendedHours: false }, defaultViewport(), size);
    expect(m.slots.length).toBe(70);
  });
});

describe("auto stretch: the fitted range", () => {
  const { candles, gaps } = fixtureCandles(30, "1m", OPEN, [4]);
  const slots = buildSlots(candles, gaps);

  test("fits the visible candles only, with the padding on both sides", () => {
    const window = slots.slice(10, 21).flatMap((s) => (s.kind === "candle" ? [s.candle] : []));
    const lo = Math.min(...window.map((c) => c.l));
    const hi = Math.max(...window.map((c) => c.h));
    const r = autoFitRange(slots, 10, 20, {}, 0.1)!;
    expect(r.lo).toBeCloseTo(lo - (hi - lo) * 0.1, 9);
    expect(r.hi).toBeCloseTo(hi + (hi - lo) * 0.1, 9);
    // Candles outside the window (the whole series spans more) do not count.
    const all = autoFitRange(slots, 0, slots.length - 1, {}, 0.1)!;
    expect(all.hi - all.lo).toBeGreaterThan(r.hi - r.lo);
  });

  test("a flat window still gets a height; a window of gaps only gets no range", () => {
    const flat = buildSlots([{ t: OPEN, o: 2, h: 2, l: 2, c: 2, v: 1, n: 1, final: true }], []);
    const r = autoFitRange(flat, 0, 0)!;
    expect(r.hi).toBeGreaterThan(2);
    expect(r.lo).toBeLessThan(2);
    expect(autoFitRange(slots, 4, 4)).toBeNull();
  });

  test("line types fit closes, not wicks; overlays on the price pane are included", () => {
    const wide = buildSlots([{ t: OPEN, o: 5, h: 9, l: 1, c: 5, v: 1, n: 1, final: true }, { t: OPEN + 60_000, o: 5, h: 9, l: 1, c: 6, v: 1, n: 1, final: true }], []);
    const line = autoFitRange(wide, 0, 1, { chartType: "line" }, 0)!;
    expect([line.lo, line.hi]).toEqual([5, 6]);
    const candle = autoFitRange(wide, 0, 1, { chartType: "candles" }, 0)!;
    expect([candle.lo, candle.hi]).toEqual([1, 9]);
    const withBand = autoFitRange(wide, 0, 1, { chartType: "line", overlays: [Float64Array.from([4, Number.NaN])] }, 0)!;
    expect([withBand.lo, withBand.hi]).toEqual([4, 6]);
  });

  test("re-fits as the view pans: the range follows what is on screen", () => {
    const d = day(200);
    const st = stateOf(d);
    const vp = defaultViewport(10);
    const a = computeChartModel(st, vp, size).price!;
    const b = computeChartModel(st, panSlots(vp, d.slots, 120), size).price!;
    expect([a.lo, a.hi]).not.toEqual([b.lo, b.hi]);
  });
});

describe("horizontal presets", () => {
  test("100 / 50 / 25 % of the day's slots, in every timeframe", () => {
    for (const [tf, n] of [["1m", 390], ["5m", 78], ["30s", 780], ["15s", 1560]] as const) {
      const d = day(n, tf);
      expect(daySlotCount(d.slots, tf, false)).toBe(n);
      expect(presetSlots(d.slots, tf, false, 100)).toBe(n);
      expect(presetSlots(d.slots, tf, false, 50)).toBe(Math.round(n / 2));
      expect(presetSlots(d.slots, tf, false, 25)).toBe(Math.round(n / 4));
    }
  });

  test("only the LATEST trading day counts; the day before fills the left side", () => {
    const prev = fixtureCandles(390, "1m", PREV_OPEN).candles;
    const today = fixtureCandles(200, "1m", OPEN).candles;
    expect(daySlotCount(buildSlots([...prev, ...today], []), "1m", false)).toBe(200);
  });

  test("early in the day the window is floored at a quarter session, so 100 % is never two giant candles", () => {
    expect(daySlotCount(day(5).slots, "1m", false)).toBe(98); // ceil(390 / 4)
    expect(daySlotCount(day(5).slots, "1m", true)).toBe(240); // 960 / 4
    expect(daySlotCount([], "5m", false)).toBe(78);
  });

  test("the overnight session before the pre-market is not part of the day", () => {
    const night = fixtureCandles(480, "1m", etToUtc(2026, 10, 14, 20, 0)).candles; // 20:00 → 04:00, trading day Oct 15
    const today = fixtureCandles(400, "1m", PRE).candles;
    expect(daySlotCount(buildSlots([...night, ...today], []), "1m", true)).toBe(400);
  });

  test("with extended hours shown the day starts at its pre-market", () => {
    const d = day(400, "1m", PRE);
    expect(daySlotCount(d.slots, "1m", true)).toBe(400);
    expect(daySlotCount(sessionSlots(d.slots, false), "1m", false)).toBe(98); // 70 regular slots, floored
  });

  test("bar spacing fits the window plus the right margin, clamped to the zoom limits", () => {
    const d = day(390);
    // The day (390 slots at 100 %, 98 at 25 %) plus RIGHT_MARGIN_PX fills the plot exactly.
    expect(presetBarSpacing(d.slots, "1m", false, 100, 783, 1.5)).toBeCloseTo((783 - RIGHT_MARGIN_PX) / 390, 9);
    expect(presetBarSpacing(d.slots, "1m", false, 25, 783, 1.5)).toBeCloseTo((783 - RIGHT_MARGIN_PX) / 98, 9);
    // Zoomed far in, the 1.5-slot floor is the wider margin and decides instead.
    expect(presetBarSpacing(day(20, "5m").slots, "5m", false, 25, 300, 1.5)).toBeCloseTo(300 / (5 + 1.5), 9); // 5 slots: 46 px each
    expect(presetBarSpacing(day(1560, "15s").slots, "15s", false, 100, 300, 1.5)).toBe(MIN_BAR_SPACING);
    expect(presetBarSpacing(day(10, "5m").slots, "5m", false, 25, 2000, 1.5)).toBe(MAX_BAR_SPACING);
  });

  test("a preset view follows the latest candle and shows exactly the window", () => {
    const d = day(390);
    const vp = presetViewport(panSlots(defaultViewport(), d.slots, 50), d.slots, "1m", false, 50, 800);
    expect(vp.anchorT).toBeNull();
    const m = computeChartModel(stateOf(d), vp, { width: 800 + 56, height: 400 }, true);
    expect(m.index.to).toBe(389);
    expect(m.index.to - m.index.from + 1).toBeGreaterThanOrEqual(195);
    expect(m.index.to - m.index.from + 1).toBeLessThanOrEqual(197);
  });
});

describe("opening a view, and the manual override lifecycle", () => {
  const d = day(390);
  const st = stateOf(d);

  test("auto stretch on: the new view is auto and shows the whole day at 100 %", () => {
    const vp = openView(st, { autoStretch: true, zoom: 100, extendedHours: true }, size, defaultViewport(9));
    expect(isAutoPrice(vp)).toBe(true);
    const m = computeChartModel(st, vp, size);
    expect(m.index.from).toBe(0);
    expect(m.index.to).toBe(389);
  });

  test("auto stretch off: fitted once at the new zoom, then held as a manual range", () => {
    const vp = openView(st, { autoStretch: false, zoom: 25, extendedHours: true }, size, defaultViewport(9));
    expect(vp.price?.mode).toBe("manual");
    const auto = computeChartModel(st, { ...vp, price: { mode: "auto" } }, size).price!;
    expect(vp.price).toEqual({ mode: "manual", lo: auto.lo, hi: auto.hi });
    // Panning keeps that range: it is the owner's scale now.
    const panned = computeChartModel(st, panSlots(vp, d.slots, 100), size).price!;
    expect([panned.lo, panned.hi]).toEqual([auto.lo, auto.hi]);
  });

  test("drag → manual; double-click on the price axis → auto; a new ticker → auto again", () => {
    let vp = openView(st, { autoStretch: true, zoom: 100, extendedHours: true }, size, defaultViewport(9));
    let m = computeChartModel(st, vp, size);
    // A price-axis drag scales price: manual from here on, through pans and new data.
    vp = applyGesture(vp, { kind: "scalePrice", factor: 1.5, anchorY: 100 }, m);
    expect(isAutoPrice(vp)).toBe(false);
    vp = applyGesture(vp, { kind: "pan", dxSlots: 20, dy: 0 }, computeChartModel(st, vp, size));
    expect(isAutoPrice(vp)).toBe(false);
    // Double-click on the price axis: back to auto stretch.
    m = computeChartModel(st, vp, size);
    const g = new GestureController();
    const ax = m.geometry.priceAxis.x + 10;
    const ay = m.geometry.plot.y + 50;
    g.handle({ type: "down", pointerId: 1, pointerType: "mouse", x: ax, y: ay, t: 0, button: 0 }, { model: m, inspectT: null });
    g.handle({ type: "up", pointerId: 1, pointerType: "mouse", x: ax, y: ay, t: 50 }, { model: m, inspectT: null });
    const acts = g.handle({ type: "dblclick", pointerId: 1, pointerType: "mouse", x: ax, y: ay, t: 120 }, { model: m, inspectT: null });
    expect(acts).toEqual([{ kind: "autoScalePrice" }]);
    vp = applyGesture(vp, acts[0]!, m);
    expect(isAutoPrice(vp)).toBe(true);
    // Manual again, then another ticker: a different view key opens a fresh, auto view.
    vp = applyGesture(vp, { kind: "scalePrice", factor: 2, anchorY: 100 }, m);
    expect(isAutoPrice(vp)).toBe(false);
    expect(viewKey("ABCD", "1m")).not.toBe(viewKey("WXYZ", "1m"));
    expect(viewKey("ABCD", "1m")).not.toBe(viewKey("ABCD", "5m"));
    const next = openView(stateOf(day(120)), { autoStretch: true, zoom: 100, extendedHours: true }, size, defaultViewport(9));
    expect(isAutoPrice(next)).toBe(true);
  });
});

test("easeOut: starts at from, ends exactly at to, decelerates", () => {
  expect(easeOut(10, 20, 0, 200)).toBe(10);
  expect(easeOut(10, 20, 200, 200)).toBe(20);
  expect(easeOut(10, 20, 999, 200)).toBe(20);
  expect(easeOut(10, 20, 100, 200)).toBeGreaterThan(15);
  expect(easeOut(10, 20, 5, 0)).toBe(20);
});

describe("right margin in opened views (owner, 2026-10-18)", () => {
  /** px between the latest candle's right side and the plot's right edge, in the model the renderers draw. */
  const gapOf = (st: ChartState, vp: ReturnType<typeof defaultViewport>) => {
    const m = computeChartModel(st, vp, size);
    const last = m.slots.length - 1;
    return { m, gap: m.geometry.plot.x + m.geometry.plot.w - (slotX(m.index, last) + vp.barSpacing / 2) };
  };

  test("every preset on every timeframe opens on the whole window AND the margin", () => {
    const tfs: [Timeframe, number][] = [["15s", 1560], ["30s", 780], ["1m", 390], ["5m", 78]];
    for (const [tf, n] of tfs)
      for (const zoom of [25, 50, 100] as const) {
        const d = day(n, tf);
        const st = stateOf(d, tf);
        const vp = openView(st, { autoStretch: true, zoom, extendedHours: false }, size, defaultViewport(9));
        expect(vp.anchorT).toBeNull();
        const { m, gap } = gapOf(st, vp);
        expect(m.index.to).toBe(n - 1);
        if (zoom === 100 && vp.barSpacing > MIN_BAR_SPACING) expect(m.index.from).toBe(0);
        expect(gap).toBeGreaterThanOrEqual(RIGHT_MARGIN_PX - 1e-6);
      }
  });

  test("a timeframe switch opens a fresh view that follows with the margin", () => {
    const one = stateOf(day(390, "1m"), "1m");
    const five = stateOf(day(78, "5m"), "5m");
    const a = openView(one, { autoStretch: true, zoom: 50, extendedHours: false }, size, defaultViewport(9));
    const b = openView(five, { autoStretch: true, zoom: 50, extendedHours: false }, size, defaultViewport(9));
    expect(b.anchorT).toBeNull();
    expect(b.barSpacing).not.toBe(a.barSpacing);
    expect(gapOf(five, b).gap).toBeGreaterThanOrEqual(RIGHT_MARGIN_PX - 1e-6);
  });

  test("applying a preset to a panned-back view returns to the latest, margin included", () => {
    const d = day(390);
    const st = stateOf(d);
    const plotW = computeChartModel(st, defaultViewport(9), size).geometry.plot.w;
    const vp = presetViewport(panSlots(defaultViewport(), d.slots, 80), d.slots, "1m", false, 100, plotW);
    expect(vp.anchorT).toBeNull();
    expect(gapOf(st, vp).gap).toBeCloseTo(RIGHT_MARGIN_PX, 0);
  });
});

test("the indicator legend is shown by default and its switch survives a save", () => {
  expect(DEFAULT_CHART_SETTINGS.indicatorLegend).toBe(true);
  expect(parseChartSettings({}).indicatorLegend).toBe(true);
  expect(parseChartSettings({ indicatorLegend: false }).indicatorLegend).toBe(false);
  expect(parseChartSettings({ indicatorLegend: "no" }).indicatorLegend).toBe(true);
});

test("the legend's hidden indicators are kept as ids, cleaned of junk", () => {
  expect(DEFAULT_CHART_SETTINGS.legendHidden).toEqual([]);
  expect(parseChartSettings({ legendHidden: ["ema-20", "ema-20", 3, "", "vwap"] }).legendHidden).toEqual(["ema-20", "vwap"]);
  expect(parseChartSettings({ legendHidden: "ema-20" }).legendHidden).toEqual([]);
});
