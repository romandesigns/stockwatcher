import { describe, expect, test } from "bun:test";
import { applyIndicatorColors, colorIdentity, indicatorColorPatch, migrateSpecColors, parseIndicatorColors, resetIndicatorColorsPatch, stripIndicatorColors } from "./indicatorColors.ts";
import { INDICATOR_PRESETS, type IndicatorSpec } from "./indicators.ts";

const preset = (id: string) => INDICATOR_PRESETS.find((p) => p.id === id)!;

describe("global indicator colours", () => {
  test("identity: type plus period for moving averages, type alone for the single studies", () => {
    expect(colorIdentity(preset("ema-9"))).toBe("ema:9");
    expect(colorIdentity(preset("ema-20"))).toBe("ema:20");
    expect(colorIdentity(preset("sma-50"))).toBe("sma:50");
    expect(colorIdentity({ kind: "ema" })).toBe("ema:9");
    expect(colorIdentity(preset("vwap"))).toBe("vwap");
    expect(colorIdentity({ ...preset("bb-20"), period: 30 })).toBe("bb");
    const tuned: IndicatorSpec = { ...preset("macd"), fast: 8, style: "ppo" };
    expect(colorIdentity(tuned)).toBe("macd");
  });

  test("applied to two charts with different lists, each gets the colour of the lines it shows", () => {
    const colors = { "ema:9/line": "#ff0000", "macd/signal": "#00ff00", "bb/fill": "#0000ff" };
    const a = applyIndicatorColors([preset("vwap"), preset("ema-9")], colors);
    const b = applyIndicatorColors([preset("ema-9"), preset("ema-20"), { ...preset("macd"), style: "four-color" }], colors);
    expect(a[1]!.colors).toEqual({ line: "#ff0000" });
    expect(a[0]!.colors).toBeUndefined();
    expect(b[0]!.colors).toEqual({ line: "#ff0000" });
    expect(b[1]!.colors).toBeUndefined();
    expect(b[2]!.colors).toEqual({ signal: "#00ff00" });
  });

  test("per-spec colours are replaced once a map exists, kept while none does; unchanged input is the same array", () => {
    const own: IndicatorSpec[] = [{ ...preset("ema-9"), colors: { line: "#123456" } }];
    expect(applyIndicatorColors(own, null)).toBe(own);
    expect(applyIndicatorColors(own, {})[0]!.colors).toBeUndefined();
    const plain = [preset("vwap")];
    expect(applyIndicatorColors(plain, {})).toBe(plain);
    expect(stripIndicatorColors(own)).toEqual([preset("ema-9")]);
    expect(stripIndicatorColors(plain)).toBe(plain);
  });

  test("patches: one role per entry; reset removes every role of the identity, none of another", () => {
    expect(indicatorColorPatch(preset("ema-9"), "line", "#ABCDEF")).toEqual({ set: { "ema:9/line": "#abcdef" }, remove: [] });
    expect(indicatorColorPatch(preset("ema-9"), "line", null)).toEqual({ set: {}, remove: ["ema:9/line"] });
    const stored = { "macd/upSoft": "#111111", "macd/macd": "#222222", "ema:9/line": "#333333" };
    const removed = resetIndicatorColorsPatch(preset("macd"), stored);
    expect(removed).toContain("macd/upSoft");
    expect(removed).toContain("macd/macd");
    expect(removed).toContain("macd/histDown");
    expect(removed).not.toContain("ema:9/line");
  });

  test("parse: absent is null (not migrated yet), junk entries dropped, colours lower-cased", () => {
    expect(parseIndicatorColors(undefined)).toBeNull();
    expect(parseIndicatorColors([])).toEqual({});
    expect(parseIndicatorColors({ "ema:9/line": "#ABCDEF", "ema:0/line": "#000000", "ema:9/bogus": "#000000", "foo/line": "#000000", "vwap/line": 3, "macd/histUp": "#00ff00" })).toEqual({ "ema:9/line": "#abcdef", "macd/histUp": "#00ff00" });
  });

  test("migration: later sources win, roles kept apart", () => {
    const memory: IndicatorSpec[] = [{ ...preset("macd"), colors: { macd: "#000001", signal: "#000002" } }];
    const main: IndicatorSpec[] = [{ ...preset("macd"), colors: { macd: "#00000a" } }, { ...preset("ema-20"), colors: { line: "#00000b" } }];
    expect(migrateSpecColors([memory, main])).toEqual({ "macd/macd": "#00000a", "macd/signal": "#000002", "ema:20/line": "#00000b" });
    expect(migrateSpecColors([[preset("vwap")]])).toEqual({});
  });
});
