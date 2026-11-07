import { describe, expect, test } from "bun:test";
import type { DisplayList } from "./displayList.ts";
import { computeIndicators } from "./indicatorMath.ts";
import {
  assembleIndicators,
  classicMacd,
  indicatorColor,
  indicatorColorRoles,
  indicatorLabel,
  indicatorMenuColor,
  indicatorSwatches,
  macdMarkersOn,
  replaceIndicator,
  resetIndicatorColors,
  serverIndicatorResults,
  specKey,
  specLineNames,
  withIndicatorColor,
  type IndicatorSpec,
} from "./indicators.ts";
import { computeChartModel, layoutChart, type ChartState } from "./layout.ts";
import { buildSlots, defaultViewport } from "./scale.ts";
import { parseIndicators } from "./syncedSettings.ts";
import { fixtureSnapshot } from "./testFixture.ts";
import { DEFAULT_CHART_THEME as T, withAlpha } from "./theme.ts";

// The MACD styles and indicator line colours (owner-approved 2026-11-07). The values are the
// server's (crates/chart-engine/src/indicators.rs, unit-tested there); here: how specs key, parse
// and degrade, and how chart-core draws each style from given values.

const macd = (extra: Partial<IndicatorSpec> = {}): IndicatorSpec => ({ id: "macd", kind: "macd", fast: 12, slow: 26, signal: 9, ...extra });

describe("MACD style specs", () => {
  test("a computed style is its own server study; classic and 4-color share classic's key", () => {
    expect(specKey(macd())).toBe("macd:12:26:9");
    expect(specKey(macd({ style: "classic" }))).toBe("macd:12:26:9");
    expect(specKey(macd({ style: "four-color" }))).toBe("macd:12:26:9");
    expect(specKey(macd({ style: "ppo" }))).toBe("macd:12:26:9:ppo");
    expect(specKey(macd({ style: "signal-area", markers: true, colors: { macd: "#00ff00" } }))).toBe("macd:12:26:9:signal-area");
  });

  test("labels name the style, keeping the periods", () => {
    expect(indicatorLabel(macd())).toBe("MACD 12 26 9");
    expect(indicatorLabel(macd({ style: "ppo" }))).toBe("PPO 12 26 9");
    expect(indicatorLabel(macd({ style: "macd-v" }))).toBe("MACD-V 12 26 9");
    expect(indicatorLabel(macd({ style: "signal-area" }))).toBe("MACD 12 26 · signal SMA 9");
  });

  test("markers: off by default for classic (today's charts unchanged), on for the rest, and as set", () => {
    expect(macdMarkersOn(macd())).toBe(false);
    expect(macdMarkersOn(macd({ style: "ppo" }))).toBe(true);
    expect(macdMarkersOn(macd({ markers: true }))).toBe(true);
    expect(macdMarkersOn(macd({ style: "zero-lag", markers: false }))).toBe(false);
  });

  test("stored settings keep style, markers and valid colours; old ones and junk read as classic", () => {
    const [a, b, c] = parseIndicators([
      { id: "a", kind: "macd", style: "macd-v", markers: false, colors: { macd: "#AABBCC", signal: "red", nope: "#000000" } },
      { id: "b", kind: "macd", fast: 12, slow: 26, signal: 9 },
      { id: "c", kind: "macd", style: "from-the-future", markers: "yes" },
    ]);
    expect(a).toEqual({ id: "a", kind: "macd", style: "macd-v", markers: false, colors: { macd: "#aabbcc" } });
    expect(b).toEqual({ id: "b", kind: "macd", fast: 12, slow: 26, signal: 9 });
    expect(c).toEqual({ id: "c", kind: "macd" });
    expect(parseIndicators([{ id: "e", kind: "ema", period: 9, style: "ppo", colors: { line: "#112233" } }])).toEqual([{ id: "e", kind: "ema", period: 9, colors: { line: "#112233" } }]);
  });

  test("against a server without the styles, computed ones draw as classic and keep their markers", () => {
    const specs = [macd({ style: "ppo" }), { id: "vwap", kind: "vwap" } as IndicatorSpec];
    const out = classicMacd(specs);
    expect(out[0]).toEqual(macd({ markers: true }));
    expect(out[1]).toBe(specs[1]!);
    const kept = [macd({ style: "four-color" })];
    expect(classicMacd(kept)).toBe(kept);
  });

  test("signal area has no histogram line", () => {
    expect(specLineNames(macd({ style: "signal-area" })).map((l) => l.name)).toEqual(["MACD", "Signal"]);
    expect(specLineNames(macd({ style: "ppo" })).map((l) => l.name)).toEqual(["Hist", "MACD", "Signal"]);
  });

  test("the server's values for a styled study are read under its key", () => {
    const snap = fixtureSnapshot("1m", 5, []);
    const slots = buildSlots(snap.candles, snap.gaps);
    const t = snap.candles.map((c) => c.t);
    const studies = new Map([["macd:12:26:9:signal-area", { lines: [{ name: "MACD", values: t.map(() => 1) }, { name: "Signal", values: t.map(() => 2) }] }]]);
    const [r] = serverIndicatorResults(slots, [macd({ style: "signal-area" })], { t, studies });
    expect(r!.lines.map((l) => [l.name, l.values[0]])).toEqual([
      ["MACD", 1],
      ["Signal", 2],
    ]);
  });
});

describe("line colours", () => {
  test("roles per indicator, as each one's settings list them", () => {
    const roles = (s: IndicatorSpec) => indicatorColorRoles(s).map((r) => r.role);
    expect(roles({ id: "e", kind: "ema" })).toEqual(["line"]);
    expect(roles({ id: "b", kind: "bb" })).toEqual(["upper", "basis", "lower", "fill"]);
    expect(roles(macd())).toEqual(["macd", "signal", "histUp", "histDown"]);
    expect(roles(macd({ style: "four-color" }))).toEqual(["macd", "signal", "upStrong", "upSoft", "downStrong", "downSoft"]);
    expect(roles(macd({ style: "signal-area" }))).toEqual(["macd", "area"]);
  });

  test("set, reset and replace", () => {
    const e: IndicatorSpec = { id: "ema-9", kind: "ema", period: 9 };
    const red = withIndicatorColor(e, "line", "#FF0000");
    expect(red.colors).toEqual({ line: "#ff0000" });
    expect(withIndicatorColor(red, "line", null)).toEqual(e);
    expect(withIndicatorColor(e, "line", "not a colour")).toEqual(e);
    expect(resetIndicatorColors(red)).toEqual(e);
    expect(replaceIndicator([e, macd()], red)).toEqual([red, macd()]);
    expect(indicatorMenuColor(red, "#123456")).toBe("#ff0000");
    expect(indicatorMenuColor(e, "#123456")).toBe("#123456");
  });

  test("defaults are today's colours; a picked one replaces only its role", () => {
    const [ema, bb] = assembleIndicators(
      [
        { id: "ema-9", kind: "ema" },
        { id: "bb", kind: "bb", colors: { upper: "#ff0000" } },
      ],
      () => [new Float64Array(1), new Float64Array(1), new Float64Array(1)],
    );
    expect(indicatorColor(T, ema!, 0)).toBe(T.series1);
    expect(indicatorColor(T, bb!, 0)).toBe(T.series2);
    expect(indicatorColor(T, bb!, 1)).toBe("#ff0000");
    expect(indicatorSwatches(T)).toContain(T.series1);
    expect(indicatorSwatches(T).every((c) => /^#[0-9a-f]{6}$/.test(c))).toBe(true);
  });
});

// --- drawing, from hand-made values on a fixture chart --------------------------------------------

const snap = fixtureSnapshot("1m", 60, []);
const slots = buildSlots(snap.candles, snap.gaps);
const n = slots.length;
const size = { width: 600, height: 600 };
const vp = defaultViewport(6);

/** Layout of a chart whose one pane is `spec` with these line values (by line name). */
function draw(spec: IndicatorSpec, values: Record<string, (i: number) => number>, extra: IndicatorSpec[] = []): { list: DisplayList; pane: { y: number; h: number } } {
  const lines = specLineNames(spec).map((l) => Float64Array.from({ length: n }, (_, i) => values[l.name]?.(i) ?? Number.NaN));
  const indicators = [...computeIndicators(slots, extra), ...assembleIndicators([spec], () => lines)];
  const state: ChartState = { timeframe: "1m", candles: snap.candles, gaps: snap.gaps, profile: null, slots, inspectT: null, pointer: null, indicators };
  const model = computeChartModel(state, vp, size);
  return { list: layoutChart(model, state, T), pane: model.geometry.panes[0]! };
}

const inPane = (list: DisplayList, pane: { y: number; h: number }) =>
  list.filter((p) => {
    const y = p.kind === "rect" ? p.y : p.kind === "line" ? p.y1 : p.kind === "text" ? p.y : (p.points[1] ?? 0);
    return y >= pane.y - 2 && y <= pane.y + pane.h + 2;
  });

// A wave crossing zero and its signal twice in the visible range.
const wave = (i: number) => Math.sin(i / 6);
const lagged = (i: number) => Math.sin((i - 3) / 6);

describe("drawing each style", () => {
  test("classic draws as before and shows no markers", () => {
    const { list, pane } = draw(macd(), { Hist: (i) => wave(i) - lagged(i), MACD: wave, Signal: lagged });
    const p = inPane(list, pane);
    expect(p.some((x) => x.kind === "rect" && x.fill === T.histUp)).toBe(true);
    expect(p.some((x) => x.kind === "path" && x.closed && x.fill === T.up)).toBe(false);
  });

  test("4-color shades by direction: rising above zero strong, falling soft", () => {
    // Above zero throughout: 1, 2, 3, 2, 1, ... so both rising and falling bars show.
    const tri = (i: number) => 1 + (i % 6 < 3 ? i % 6 : 6 - (i % 6));
    const { list, pane } = draw(macd({ style: "four-color", markers: false }), { Hist: tri, MACD: tri, Signal: () => 0 });
    const fills = new Set(inPane(list, pane).flatMap((x) => (x.kind === "rect" && x.layer === "volume" ? [x.fill] : [])));
    expect(fills.has(T.up)).toBe(true);
    expect(fills.has(T.histUpSoft)).toBe(true);
    expect(fills.has(T.histDown)).toBe(false);
  });

  test("signal area: down-hue bars from zero, an up-hue hatch while MACD is above, no histogram", () => {
    const { list, pane } = draw(macd({ style: "signal-area", markers: false }), { MACD: (i) => wave(i) + 0.5, Signal: (i) => lagged(i) + 0.5 });
    const p = inPane(list, pane);
    const bars = p.filter((x) => x.kind === "rect" && x.layer === "volume");
    expect(bars.length).toBeGreaterThan(10);
    expect(bars.every((x) => x.kind === "rect" && x.fill === T.signalArea)).toBe(true);
    const hatch = p.filter((x) => x.kind === "line" && x.stroke === withAlpha(T.up, 0.8));
    expect(hatch.length).toBeGreaterThan(0);
    expect(hatch.every((x) => x.kind === "line" && x.lineWidth === 1 && x.x1 === x.x2)).toBe(true);
    expect(p.some((x) => x.kind === "path" && !x.closed && x.stroke === T.up)).toBe(true);
  });

  test("markers: triangles at signal crosses, rings on zero crosses, in the hue of the direction", () => {
    const { list, pane } = draw(macd({ markers: true }), { Hist: (i) => wave(i) - lagged(i), MACD: wave, Signal: lagged });
    const p = inPane(list, pane);
    const up = p.filter((x) => x.kind === "path" && x.closed && x.fill === T.up);
    const down = p.filter((x) => x.kind === "path" && x.closed && x.fill === T.down);
    const rings = p.filter((x) => x.kind === "path" && x.closed && !x.fill && (x.stroke === T.up || x.stroke === T.down));
    expect(up.length).toBeGreaterThan(0);
    expect(down.length).toBeGreaterThan(0);
    expect(rings.length).toBeGreaterThan(0);
    // An up triangle sits under the line: its tip is the top point.
    const tri = up[0]!;
    if (tri.kind === "path") expect(tri.points[1]!).toBeLessThan(tri.points[3]!);
  });

  test("MACD-V: dotted ±150 levels in the warning hue once the values come near them", () => {
    const big = (i: number) => 180 * Math.sin(i / 6);
    const { list } = draw(macd({ style: "macd-v" }), { Hist: () => 0, MACD: big, Signal: (i) => big(i - 2) });
    const levels = list.filter((x) => x.kind === "line" && x.stroke === T.oscLevel);
    expect(levels.length).toBe(2);
    expect(list.filter((x) => x.kind === "text" && x.fill === T.oscLevelText).map((x) => (x.kind === "text" ? x.text : ""))).toEqual(["+150", "-150"]);
    const quiet = draw(macd({ style: "macd-v" }), { Hist: () => 0, MACD: wave, Signal: lagged });
    expect(quiet.list.some((x) => x.kind === "line" && x.stroke === T.oscLevel)).toBe(false);
  });

  test("a picked colour draws: an EMA's line and a classic histogram's up bars", () => {
    const { list } = draw(macd({ colors: { histUp: "#112233" } }), { Hist: (i) => 1 + (i % 3), MACD: wave, Signal: lagged }, [{ id: "ema-9", kind: "ema", period: 9, colors: { line: "#abcdef" } }]);
    expect(list.some((x) => x.kind === "path" && x.stroke === "#abcdef")).toBe(true);
    expect(list.some((x) => x.kind === "rect" && x.fill === withAlpha("#112233", 0.75))).toBe(true);
  });
});
