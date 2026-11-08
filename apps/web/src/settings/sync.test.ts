import { describe, expect, test } from "bun:test";
import { createAppStore } from "@stockwatcher/app-state";
import { DEFAULT_CHART_SETTINGS, drawingsFromDoc, parseIndicators } from "@stockwatcher/chart-core";
import { createSettingsSync, type SettingsClientMessage } from "@stockwatcher/shared";
import { LEGACY_KEYS, migrateLegacy } from "./sync.ts";

const store = (entries: Record<string, unknown>) => (key: string) => (key in entries ? (typeof entries[key] === "string" ? (entries[key] as string) : JSON.stringify(entries[key])) : null);

describe("migrating this browser's pre-sync settings", () => {
  test("what was saved becomes validated seeds; a fresh browser seeds nothing", () => {
    const sync = createSettingsSync({ initial: null, newOpId: (() => { let n = 0; return () => `m${++n}`; })() });
    migrateLegacy(
      sync,
      store({
        [LEGACY_KEYS.indicators]: [{ id: "ema-9", kind: "ema", period: 9 }, { id: "x", kind: "nope" }],
        [LEGACY_KEYS.layers]: { halts: false },
        [LEGACY_KEYS.chartSettings]: { chartType: "line" },
        [LEGACY_KEYS.drawingSettings]: "not json {",
        [LEGACY_KEYS.drawings]: {
          SMPL: [{ id: "d1", kind: "hline", a: { t: 1, p: 2 }, b: { t: 1, p: 2 } }, { id: "bad" }],
          FXLO: [],
        },
      }),
    );
    expect(sync.get("indicators")).toEqual([{ id: "ema-9", kind: "ema", period: 9 }]);
    expect(sync.get("layers")).toMatchObject({ halts: false, luld: true });
    expect(sync.get("chartSettings")).toEqual({ ...DEFAULT_CHART_SETTINGS, chartType: "line" });
    expect(sync.get("drawingSettings")).toBeUndefined();
    expect(drawingsFromDoc(sync.get("drawings:SMPL"))).toEqual([{ id: "d1", kind: "hline", a: { t: 1, p: 2 }, b: { t: 1, p: 2 } }]);
    expect(sync.get("drawings:FXLO")).toBeUndefined();

    // Connected to a server that already has indicators: those win, the rest are seeded.
    const sent: SettingsClientMessage[] = [];
    sync.attach((m) => (sent.push(m), true));
    sync.handle({ type: "settings.snapshot", docs: [{ ns: "indicators", rev: 2, updatedAt: 1, value: [{ id: "vwap", kind: "vwap" }] }] });
    expect(sync.get("indicators")).toEqual([{ id: "vwap", kind: "vwap" }]);
    expect(sent.map((m) => [m.ns, m.type === "settings.set" && m.ifAbsent]).sort()).toEqual([
      ["chartSettings", true],
      ["drawings:SMPL", true],
      ["layers", true],
    ]);

    const fresh = createSettingsSync({ initial: null });
    migrateLegacy(fresh, store({}));
    expect(fresh.namespaces()).toEqual([]);
  });
});

describe("a cache written by a build from before the indicator memory", () => {
  test("loads, toggles and starts remembering without touching the indicators shape", () => {
    // What an older build left in localStorage: the engine's cache with an `indicators` doc only.
    const older = createSettingsSync({ initial: null, newOpId: () => "old-1" });
    older.set("indicators", [{ id: "macd", kind: "macd", fast: 12, slow: 26, signal: 9, style: "ppo" }]);
    let raw = "";
    const sync = createSettingsSync({ initial: null, save: (r) => (raw = r) });
    sync.set("indicators", older.get("indicators"));
    const reloaded = createSettingsSync({ initial: raw });
    const app = createAppStore({ sync: reloaded });
    expect(app.getState().indicators).toEqual([{ id: "macd", kind: "macd", fast: 12, slow: 26, signal: 9, style: "ppo" }]);
    expect(app.getState().indicatorMemory).toEqual({});
    app.getState().setIndicators([]);
    app.getState().setIndicators([{ id: "macd", kind: "macd", fast: 12, slow: 26, signal: 9 }]);
    expect(app.getState().indicators).toEqual([{ id: "macd", kind: "macd", fast: 12, slow: 26, signal: 9, style: "ppo" }]);
    // The list is still the plain IndicatorSpec[] an older build reads.
    expect(parseIndicators(reloaded.get("indicators"))).toEqual(app.getState().indicators);
  });
});
