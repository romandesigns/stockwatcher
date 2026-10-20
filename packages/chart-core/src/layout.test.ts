import { describe, expect, test } from "bun:test";
import { type ChartTheme, type DisplayList, type LinePrim, LAYER_ORDER, sortByLayer } from "./displayList.ts";
import { describeCounted, describeSlot } from "./inspect.ts";
import { candleCountdown, computeChartModel, formatCountdown, hitTest, layout, layoutProfile, stepInspection, type ChartState } from "./layout.ts";
import { buildSlots, defaultViewport, slotX } from "./scale.ts";
import { computeIndicators } from "./indicators.ts";
import { DEFAULT_CHART_THEME } from "./theme.ts";
import { etToUtc } from "./time.ts";
import { fixtureSnapshot, T0 } from "./testFixture.ts";
import { sessionBands } from "./time.ts";
import { layoutTimeline, timelineTimeAt, volumeBuckets } from "./timeline.ts";

// Token NAMES as colors, so snapshots show which token each primitive uses.
const theme = new Proxy({} as ChartTheme, { get: (_, k) => (k === "fontSans" ? "sans" : k === "fontMono" ? "mono" : String(k)) });

const snap = fixtureSnapshot("1m", 20, [5]);
const state: ChartState = { timeframe: "1m", candles: snap.candles, gaps: snap.gaps, profile: snap.profile, inspectT: null, pointer: null };
const size = { width: 400, height: 300 };
const vp = defaultViewport(16);

const round = (list: DisplayList) => JSON.parse(JSON.stringify(list, (_, v) => (typeof v === "number" ? Math.round(v * 100) / 100 : v)));

function finite(list: DisplayList) {
  for (const p of list) for (const v of Object.values(p)) if (typeof v === "number") expect(Number.isFinite(v)).toBe(true);
}

describe("layout", () => {
  test("display list snapshot for the fixture", () => {
    const list = layout({ ...state, options: { showProfile: true } }, vp, theme, size);
    finite(list);
    expect(round(list)).toMatchSnapshot();
  });

  test("primitives come out in paint order", () => {
    const list = layout({ ...state, pointer: { x: 100, y: 100 }, options: { showProfile: true } }, vp, theme, size);
    expect(sortByLayer(list)).toEqual(list);
    expect(new Set(list.map((p) => p.layer)).size).toBeGreaterThan(5);
    expect(LAYER_ORDER[0]).toBe("background");
  });

  test("the forming candle is a dashed hollow rect; final candles are filled", () => {
    const list = layout(state, vp, theme, size);
    const bodies = list.filter((p) => p.kind === "rect" && p.layer === "candles");
    expect(bodies.filter((b) => b.kind === "rect" && b.dash)).toHaveLength(1);
    const dashed = bodies.find((b) => b.kind === "rect" && b.dash)!;
    expect(dashed.kind === "rect" && dashed.fill).toBeFalsy();
    expect(bodies.filter((b) => b.kind === "rect" && b.fill)).toHaveLength(snap.candles.length - 1);
  });

  test("gaps are hatched inside their slot and never drawn as candles", () => {
    const model = computeChartModel(state, vp, size);
    const gapIndex = model.slots.findIndex((s) => s.kind === "gap");
    const cx = slotX(model.index, gapIndex);
    const hatch = layout(state, vp, theme, size).filter((p): p is LinePrim => p.kind === "line" && p.layer === "gaps");
    expect(hatch.length).toBeGreaterThan(3);
    for (const l of hatch) {
      for (const x of [l.x1, l.x2]) {
        expect(x).toBeGreaterThanOrEqual(cx - 8 - 1e-9);
        expect(x).toBeLessThanOrEqual(cx + 8 + 1e-9);
      }
    }
  });

  test("last-price tag and pointer crosshair label", () => {
    const list = layout({ ...state, pointer: { x: 50, y: 60 } }, vp, theme, size);
    const texts = list.filter((p) => p.kind === "text").map((p) => (p.kind === "text" ? p.text : ""));
    expect(texts).toContain(snap.candles.at(-1)!.c.toFixed(2));
    expect(list.some((p) => p.kind === "path" && p.fill === "up")).toBe(true);
    expect(list.some((p) => p.kind === "rect" && p.fill === "crosshairLabelBg")).toBe(true);
  });
});

describe("hit testing and inspection", () => {
  const model = computeChartModel(state, vp, size);

  test("x,y → slot, region and price", () => {
    const i = model.slots.length - 3;
    const hit = hitTest(model, slotX(model.index, i), 40);
    expect(hit.region).toBe("plot");
    expect(hit.index).toBe(i);
    expect(hit.slot?.t).toBe(model.slots[i]!.t);
    expect(hit.price).toBeGreaterThan(model.price!.lo);
    expect(hitTest(model, size.width - 10, 40)).toMatchObject({ region: "priceAxis", index: null });
    const vol = model.geometry.volume!;
    expect(hitTest(model, slotX(model.index, i), vol.y + 5)).toMatchObject({ region: "volume", index: i, price: null });
  });

  test("arrow-key stepping clamps at both ends and starts from the latest", () => {
    const slots = buildSlots(snap.candles, snap.gaps);
    expect(stepInspection(slots, null, -1)).toBe(slots.at(-2)!.t);
    expect(stepInspection(slots, slots[0]!.t, -5)).toBe(slots[0]!.t);
    expect(stepInspection(slots, null, 10)).toBe(slots.at(-1)!.t);
  });

  test("inspector wording for a candle and for a gap", () => {
    const slots = buildSlots(snap.candles, snap.gaps);
    const last = describeSlot(slots.at(-1)!, "1m");
    expect(last.state).toBe("Forming");
    expect(last.timeLabel).toBe("10:19:00 – 10:20:00");
    expect(last.rows.map((r) => r.k)).toEqual(["O", "H", "L", "C", "V", "Δ"]);
    const gap = describeSlot(slots.find((s) => s.kind === "gap")!, "1m");
    expect(gap).toMatchObject({ state: "No trades", timeLabel: "10:05:00 – 10:06:00", note: "1 interval, no trades" });
    const vo = describeSlot({ kind: "gap", t: T0, gap: { t: T0, end: T0 + 120_000, reason: "volume_only_trades", v: 37, intervals: 2 } }, "1m");
    expect(vo).toMatchObject({ state: "Volume only", note: "2 intervals, volume-only prints (37), no qualifying price" });
  });

  test("counted-as wording", () => {
    expect(describeCounted({ counted: "ohlc_and_volume", reason: null }).label).toBe("Regular sale");
    expect(describeCounted({ counted: "volume_only", reason: "odd_lot" })).toMatchObject({ label: "Odd lot · volume only", short: "odd lot · vol only", tone: "partial" });
    expect(describeCounted({ counted: "volume_only", reason: "condition_W" }).label).toBe("Average price · volume only");
    expect(describeCounted({ counted: "excluded:late", reason: null })).toMatchObject({ label: "Late (after settle window) · excluded", tone: "excluded" });
    expect(describeCounted({ counted: "excluded:duplicate", reason: null }).tone).toBe("ignored");
    expect(describeCounted({ counted: "excluded:outside_session", reason: null }).short).toBe("no session");
    expect(describeCounted({ counted: "excluded:condition_M", reason: "condition_M" }).label).toBe("Official close · excluded");
    expect(describeCounted({ counted: "excluded:condition_ZZ", reason: "condition_ZZ" }).label).toBe("Condition ZZ · excluded");
  });
});

describe("profile and timeline layouts", () => {
  test("standalone profile marks POC and value area", () => {
    const list = layoutProfile({ profile: snap.profile! }, theme, { width: 200, height: 120 });
    expect(list.filter((p) => p.kind === "rect" && p.fill === "profilePoc")).toHaveLength(1);
    expect(list.some((p) => p.kind === "rect" && p.fill === "profileValueArea")).toBe(true);
    expect(round(list)).toMatchSnapshot();
  });

  test("timeline: four bands, minimap past/future split at the head", () => {
    const bands = sessionBands({ year: 2026, month: 10, day: 15 });
    const buckets = volumeBuckets(snap.candles, bands[0]!.start, bands[3]!.end, 15 * 60_000);
    expect(buckets).toHaveLength(96);
    expect(buckets.reduce((a, b) => a + b.v, 0)).toBe(snap.candles.reduce((a, c) => a + c.v, 0));
    const list = layoutTimeline({ bands, buckets, bucketMs: 15 * 60_000, now: T0 + 10 * 60_000, variant: "full" }, theme, { width: 960, height: 104 });
    expect(list.filter((p) => p.layer === "background")).toHaveLength(4);
    const texts = list.flatMap((p) => (p.kind === "text" ? [p.text] : []));
    expect(texts).toContain("Regular 09:30–16:00");
    expect(texts).toContain("10:10");
    expect(list.some((p) => p.kind === "rect" && p.fill === "minimapPastRegular")).toBe(true);
    expect(list.some((p) => p.kind === "rect" && p.fill === "minimapFuture")).toBe(true);
    expect(round(layoutTimeline({ bands, buckets, bucketMs: 15 * 60_000, now: T0, variant: "compact" }, theme, { width: 358, height: 64 }))).toMatchSnapshot();
  });
});

describe("candle countdown", () => {
  test("counts down the forming candle only, rounded up to whole seconds", () => {
    const c = { t: 60_000, o: 1, h: 1, l: 1, c: 1, v: 1, n: 1, final: false };
    expect(candleCountdown(c, 60_000, 78_000)).toBe(42_000);
    expect(formatCountdown(42_000)).toBe("0:42");
    expect(formatCountdown(271_500)).toBe("4:32");
    expect(candleCountdown({ ...c, final: true }, 60_000, 78_000)).toBeNull();
    expect(candleCountdown(c, 60_000, 130_000)).toBeNull();
    expect(candleCountdown(c, 60_000, null)).toBeNull();
    // A clock trailing the candle's opening print by a moment still shows the full interval.
    expect(candleCountdown(c, 60_000, 59_900)).toBe(60_000);
  });
});

test("session shading: candles outside regular hours sit on a faint band; regular hours stay plain", () => {
  const mk = (h: number, m: number) => ({ t: etToUtc(2026, 10, 15, h, m), o: 3, h: 3.1, l: 2.9, c: 3, v: 100, n: 1, final: true });
  const candles = [mk(9, 27), mk(9, 28), mk(9, 29), mk(9, 30), mk(9, 31), mk(9, 32)];
  const theme = DEFAULT_CHART_THEME;
  const list = layout({ timeframe: "1m", candles, gaps: [], profile: null, inspectT: null, pointer: null }, defaultViewport(20), theme, { width: 400, height: 300 });
  const bands = list.filter((p) => p.layer === "background" && p.kind === "rect");
  expect(bands.length).toBe(1);
  expect((bands[0] as { fill?: string }).fill).toBe(theme.shadePre);
});

test("timelineTimeAt maps x back to the day, clamped", () => {
  const bands = sessionBands({ year: 2026, month: 10, day: 15 });
  const start = bands[0]!.start;
  const end = bands[3]!.end;
  expect(timelineTimeAt(bands, 1000, 0)).toBe(start);
  expect(timelineTimeAt(bands, 1000, 500)).toBe(Math.round(start + (end - start) / 2));
  expect(timelineTimeAt(bands, 1000, 5000)).toBe(end);
  expect(timelineTimeAt([], 1000, 10)).toBeNull();
});

describe("auto scale leaves the overlay legend room", () => {
  const size = { width: 360, height: 300 };
  const vp = defaultViewport();
  const slots = buildSlots(snap.candles, snap.gaps);
  const highY = (s: ChartState) => {
    const m = computeChartModel(s, vp, size);
    let hi = -Infinity;
    for (let i = m.index.from; i <= m.index.to; i++) {
      const x = m.slots[i];
      if (x?.kind === "candle" && x.candle.h > hi) hi = x.candle.h;
    }
    const p = m.price!;
    return p.top + ((p.hi - hi) / (p.hi - p.lo)) * (p.bottom - p.top);
  };
  test("without overlays the high sits at the usual 8 % pad; with them it clears the legend's rows", () => {
    const plain = highY({ ...state, slots });
    const ind = computeIndicators(slots, [{ id: "ema-9", kind: "ema", period: 9 }, { id: "ema-20", kind: "ema", period: 20 }, { id: "vwap", kind: "vwap" }]);
    const withLegend = highY({ ...state, slots, indicators: ind });
    expect(plain).toBeLessThan(25);
    // One legend row at fontSize 10: 8 + 5 + 3 + 4 = 20 px above the old fit.
    expect(withLegend).toBeGreaterThan(plain + 15);
    // A manual range is left alone.
    const m = computeChartModel({ ...state, slots, indicators: ind }, { ...vp, price: { mode: "manual", lo: 1, hi: 2 } }, size);
    expect(m.price!.hi).toBe(2);
  });
});

test("hiding the indicator legend drops its text and gives the candles its room back", () => {
  const size = { width: 360, height: 300 };
  const slots = buildSlots(snap.candles, snap.gaps);
  const ind = computeIndicators(slots, [{ id: "ema-9", kind: "ema", period: 9 }, { id: "vwap", kind: "vwap" }]);
  const texts = (s: ChartState) => layout(s, defaultViewport(), DEFAULT_CHART_THEME, size).filter((p) => p.kind === "text").map((p) => (p as { text: string }).text);
  const shown = { ...state, slots, indicators: ind };
  const hidden = { ...shown, chartStyle: { indicatorLegend: false } };
  expect(texts(shown).some((t) => t.startsWith("EMA"))).toBe(true);
  expect(texts(hidden).some((t) => t.startsWith("EMA") || t === "VWAP")).toBe(false);
  expect(computeChartModel(hidden, defaultViewport(), size).price!.hi).toBeLessThan(computeChartModel(shown, defaultViewport(), size).price!.hi);
});
