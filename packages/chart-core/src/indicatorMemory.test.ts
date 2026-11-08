import { describe, expect, test } from "bun:test";
import { INDICATOR_PRESETS, isPresetSpec, presetOf, replaceIndicator, resetIndicator, sameIndicatorSettings, toggleIndicator, type IndicatorSpec } from "./indicators.ts";
import { parseIndicatorMemory, parseIndicators, rememberIndicators } from "./syncedSettings.ts";

// Turning an indicator off and on kept nothing (owner, 2026-11-07: "the state should persist"):
// the menus add the bare preset back. rememberIndicators keeps what was turned off and restores it.

const preset = (id: string) => presetOf(id)!;
const macd: IndicatorSpec = { ...preset("macd"), fast: 8, slow: 21, signal: 5, style: "ppo", markers: false, colors: { macd: "#112233", signal: "#445566" } };
const ema: IndicatorSpec = { ...preset("ema-9"), period: 12, colors: { line: "#abcdef" } };

/** One menu toggle as a host applies it: the menu's list, then the store's memory step. */
function toggle(list: IndicatorSpec[], memory: Record<string, IndicatorSpec>, id: string) {
  const r = rememberIndicators(list, toggleIndicator(list, preset(id)), memory);
  const next = { ...memory, ...r.set };
  for (const k of r.remove) delete next[k];
  return { list: r.specs, memory: next, patch: r };
}

describe("indicator settings persist across off and on", () => {
  test("off then on restores the style, colours, periods and markers", () => {
    let s = { list: [ema, macd], memory: {} as Record<string, IndicatorSpec> };
    s = toggle(s.list, s.memory, "macd");
    expect(s.list).toEqual([ema]);
    expect(s.memory.macd).toEqual(macd);
    s = toggle(s.list, s.memory, "ema-9");
    expect(s.list).toEqual([]);
    s = toggle(s.list, s.memory, "macd");
    s = toggle(s.list, s.memory, "ema-9");
    expect(s.list).toEqual([macd, ema]);
  });

  test("an indicator still on its preset needs no memory, and one never changed comes back as its preset", () => {
    const r = rememberIndicators([preset("vwap")], [], {});
    expect(r).toEqual({ specs: [], set: {}, remove: [] });
    expect(rememberIndicators([], [preset("vwap")], {}).specs).toEqual([preset("vwap")]);
  });

  test("an indicator added with settings of its own keeps them (not overwritten by memory)", () => {
    const own: IndicatorSpec = { ...preset("ema-9"), period: 30 };
    expect(rememberIndicators([], [own], { "ema-9": ema }).specs).toEqual([own]);
  });

  test("Reset to default goes back to the preset and clears what was remembered", () => {
    expect(resetIndicator(macd)).toEqual(preset("macd"));
    expect(isPresetSpec(resetIndicator(macd))).toBe(true);
    expect(isPresetSpec(macd)).toBe(false);
    // Remembered from an earlier off; then on (restored), then reset while on.
    const memory = { macd };
    const reset = rememberIndicators([macd], replaceIndicator([macd], resetIndicator(macd)), memory);
    expect(reset.remove).toEqual(["macd"]);
    expect(reset.specs).toEqual([preset("macd")]);
    // Off and on after the reset: the preset, not the old settings.
    let s = { list: reset.specs, memory: {} as Record<string, IndicatorSpec> };
    s = toggle(s.list, s.memory, "macd");
    s = toggle(s.list, s.memory, "macd");
    expect(s.list).toEqual([preset("macd")]);
  });

  test("an id with no preset keeps its parameters on reset, losing only its colours", () => {
    expect(resetIndicator({ id: "ema-33", kind: "ema", period: 33, colors: { line: "#000000" } })).toEqual({ id: "ema-33", kind: "ema", period: 33 });
  });

  test("settings compare by meaning: classic = no style, markers as drawn, colours as a set", () => {
    expect(sameIndicatorSettings({ ...preset("macd"), style: "classic", markers: false }, preset("macd"))).toBe(true);
    expect(sameIndicatorSettings({ ...preset("macd"), markers: true }, preset("macd"))).toBe(false);
    expect(sameIndicatorSettings({ ...ema, colors: { line: "#ABCDEF" } }, ema)).toBe(true);
    expect(sameIndicatorSettings({ ...ema, colors: {} }, { ...ema, colors: undefined })).toBe(true);
  });
});

describe("indicatorMemory: stored shape", () => {
  test("round-trips through JSON and the parser (what another device reads back)", () => {
    const { memory } = toggle([ema, macd], {}, "macd");
    expect(parseIndicatorMemory(JSON.parse(JSON.stringify(memory)))).toEqual({ macd });
  });

  test("absent or junk is an empty memory; bad entries are dropped one by one", () => {
    expect(parseIndicatorMemory(undefined)).toEqual({});
    expect(parseIndicatorMemory([macd])).toEqual({});
    // A style this build does not know (a newer device's) draws as classic, as in parseIndicators.
    const { style: _, ...noStyle } = macd;
    expect(parseIndicatorMemory({ macd: { ...macd, style: "from-the-future" }, x: { kind: "nope" }, "ema-9": { ...ema, id: "other" }, vwap: "junk" })).toEqual({ macd: noStyle });
  });

  test("an older indicators list (no memory anywhere) still loads and toggles as before", () => {
    const old = parseIndicators(JSON.parse(JSON.stringify(INDICATOR_PRESETS.slice(0, 2))));
    expect(old).toEqual(INDICATOR_PRESETS.slice(0, 2) as IndicatorSpec[]);
    const r = rememberIndicators(old, toggleIndicator(old, preset("vwap")), parseIndicatorMemory(undefined));
    expect(r.specs).toEqual([preset("ema-9")]);
    expect(r.set).toEqual({});
  });
});
