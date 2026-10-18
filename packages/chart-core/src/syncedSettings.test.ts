import { describe, expect, test } from "bun:test";
import type { Drawing } from "./drawings.ts";
import { INDICATOR_PRESETS } from "./indicators.ts";
import { diffDrawings, drawingsFromDoc, drawingsNs, drawingsNsSymbol, drawingsToDoc, parseDrawing, parseIndicators } from "./syncedSettings.ts";

const hline = (id: string, p = 2.5): Drawing => ({ id, kind: "hline", a: { t: 1, p }, b: { t: 1, p } });

describe("parseIndicators", () => {
  test("keeps the presets as they are", () => {
    expect(parseIndicators([...INDICATOR_PRESETS])).toEqual([...INDICATOR_PRESETS]);
  });
  test("drops junk, unknown kinds, repeats and bad parameters (never trusts another device)", () => {
    const raw = [
      { id: "ema-9", kind: "ema", period: 9, extra: "dropped" },
      { id: "ema-9", kind: "sma", period: 50 },
      { id: "x", kind: "ichimoku" },
      "junk",
      null,
      { id: "bb", kind: "bb", period: -3, mult: 2 },
      { id: "m", kind: "macd", fast: 12.5, slow: 26, signal: 9 },
    ];
    expect(parseIndicators(raw)).toEqual([
      { id: "ema-9", kind: "ema", period: 9 },
      { id: "bb", kind: "bb", mult: 2 },
      { id: "m", kind: "macd", slow: 26, signal: 9 },
    ]);
    expect(parseIndicators({ not: "a list" })).toEqual([]);
    expect(parseIndicators(undefined)).toEqual([]);
    expect(parseIndicators(Array.from({ length: 40 }, (_, i) => ({ id: `v${i}`, kind: "vwap" })))).toHaveLength(16);
  });
});

describe("parseDrawing", () => {
  test("keeps known fields of a valid drawing", () => {
    const d: Drawing = { id: "p1", kind: "long", a: { t: 1, p: 2 }, b: { t: 3, p: 4 }, c: { t: 3, p: 1 }, risk: 100, qty: null, angle: 0.5 };
    expect(parseDrawing({ ...d, junk: 1 })).toEqual(d);
    expect(parseDrawing({ ...hline("h"), color: "#3987e5" })).toEqual({ ...hline("h"), color: "#3987e5" });
  });
  test("refuses what cannot be drawn", () => {
    for (const bad of [null, {}, { ...hline("h"), kind: "measure" }, { ...hline("h"), a: { t: "1", p: 2 } }, { ...hline("h"), id: "" }, { ...hline("h"), b: null }]) {
      expect(parseDrawing(bad)).toBeNull();
    }
    expect(parseDrawing({ ...hline("h"), color: "red; background: url(x)" })).toEqual(hline("h"));
  });
});

describe("drawings as a per-symbol map", () => {
  test("namespaces are per upper-case symbol", () => {
    expect(drawingsNs(" smpl ")).toBe("drawings:SMPL");
    expect(drawingsNsSymbol("drawings:SMPL")).toBe("SMPL");
    expect(drawingsNsSymbol("layers")).toBeNull();
  });

  test("a list round-trips through the map in order, whatever order the server keeps keys in", () => {
    const list = [hline("zeta"), hline("alpha"), hline("mid")];
    const doc = drawingsToDoc(list);
    // The server sorts object keys: rebuild in that order.
    const sorted = Object.fromEntries(Object.entries(doc).sort(([a], [b]) => a.localeCompare(b)));
    expect(drawingsFromDoc(sorted)).toEqual(list);
  });

  test("bad entries are dropped, including one whose drawing claims another id", () => {
    const doc = { a: { z: 0, d: hline("a") }, b: { z: 1, d: hline("not-b") }, c: "junk", d: { z: 2, d: { id: "d" } } };
    expect(drawingsFromDoc(doc)).toEqual([hline("a")]);
    expect(drawingsFromDoc(null)).toEqual([]);
  });

  test("the diff sets only added or changed drawings and removes only missing ones", () => {
    const doc = drawingsToDoc([hline("a"), hline("b")]);
    expect(diffDrawings(doc, [hline("a"), hline("b")])).toBeNull();
    // Moving b, adding c (on top), keeping a untouched.
    expect(diffDrawings(doc, [hline("a"), hline("b", 3), hline("c")])).toEqual({ set: { b: { z: 1, d: hline("b", 3) }, c: { z: 2, d: hline("c") } }, remove: [] });
    expect(diffDrawings(doc, [hline("b")])).toEqual({ set: {}, remove: ["a"] });
    expect(diffDrawings(undefined, [hline("a")])).toEqual({ set: { a: { z: 0, d: hline("a") } }, remove: [] });
  });

  test("key order and unknown fields from the server are not a change", () => {
    const stored = { a: { z: 0, d: { b: { p: 2.5, t: 1 }, a: { p: 2.5, t: 1 }, kind: "hline", id: "a", fromNewerBuild: true } } };
    expect(diffDrawings(stored, [hline("a")])).toBeNull();
  });
});
