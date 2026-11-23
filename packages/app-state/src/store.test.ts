import { describe, expect, test } from "bun:test";
import { DEFAULT_TOOLBOX_PHONE, drawingsNs, drawingsToDoc, INDICATOR_PRESETS, replaceIndicator, resetIndicator, SYNC_NS, toggleIndicator, type Drawing, type IndicatorSpec } from "@stockwatcher/chart-core";
import { applyPatch, createSettingsSync, type SettingsClientMessage } from "@stockwatcher/shared";
import { APP_NS, DEFAULT_VIEW, parseDashboard, parseFavorites, parseSearchRecents, parseSymbol, parseTradeQty, parseView } from "./namespaces.ts";
import { createAppStore, shownTicker, type DevicePrefs } from "./store.ts";

/** A store on a real (offline) sync engine, plus a way to play another device's write. */
function rig(opts: { legacy?: { searchRecents?: unknown; tradeQty?: unknown; tickerIndicators?: unknown }; device?: boolean } = {}) {
  let n = 0;
  const sync = createSettingsSync({ initial: null, newOpId: () => `op-${++n}`, setTimer: () => 0, clearTimer: () => {} });
  const saved: [keyof DevicePrefs, unknown][] = [];
  const store = createAppStore({
    sync,
    legacy: opts.legacy,
    device: opts.device ? { initial: { toolbox: DEFAULT_TOOLBOX_PHONE, paneHeights: {}, toolsVisible: true }, save: (k, v) => saved.push([k, v]) } : undefined,
  });
  let rev = 100;
  const remote = (ns: string, value: unknown) => sync.handle({ type: "settings.changed", ns, rev: ++rev, value, opId: "other-device", updatedAt: 1 });
  const connect = (docs: { ns: string; value: unknown }[] = []) => {
    const sent: SettingsClientMessage[] = [];
    sync.attach((m) => (sent.push(m), true));
    sync.handle({ type: "settings.snapshot", docs: docs.map((d) => ({ ...d, rev: ++rev, updatedAt: 1 })) });
    return sent;
  };
  return { sync, store, remote, connect, saved, s: () => store.getState() };
}

const trend = (id: string, p = 10): Drawing => ({ id, kind: "trend", a: { t: 1, p }, b: { t: 2, p: p + 1 } });

describe("namespaces", () => {
  test("view: junk falls back field by field", () => {
    expect(parseView(undefined)).toEqual(DEFAULT_VIEW);
    expect(parseView({ symbol: " aapl ", timeframe: "5m", bin: 0.05 })).toEqual({ symbol: "AAPL", timeframe: "5m", bin: 0.05 });
    expect(parseView({ symbol: "not a ticker!", timeframe: "2h", bin: 7 })).toEqual(DEFAULT_VIEW);
  });
  test("view and dashboard slots keep 10s and 1h; anything else still falls back", () => {
    for (const tf of ["10s", "1h"] as const) {
      expect(parseView({ symbol: "AAPL", timeframe: tf, bin: 0.01 }).timeframe).toBe(tf);
      expect(parseDashboard({ count: 1, slots: [{ symbol: "AAPL", timeframe: tf }] }).slots[0]!.timeframe).toBe(tf);
    }
    for (const tf of ["4h", "1H", "10S", 10, null]) {
      expect(parseView({ symbol: "AAPL", timeframe: tf, bin: 0.01 }).timeframe).toBe(DEFAULT_VIEW.timeframe);
      expect(parseDashboard({ count: 1, slots: [{ symbol: "AAPL", timeframe: tf }] }).slots[0]!.timeframe).toBe("1m");
    }
  });
  test("symbols, trade sizes, recents", () => {
    expect(parseSymbol("brk.b")).toBe("BRK.B");
    expect(parseSymbol(42)).toBeNull();
    expect(parseTradeQty(500)).toBe(500);
    expect(parseTradeQty(123)).toBe(100);
    expect(parseSearchRecents(["a", "A", 3, "bb", "cc", "dd", "ee", "ff", "gg", "hh", "ii", "jj", "kk", "ll"])).toEqual(["A", "BB", "CC", "DD", "EE", "FF", "GG", "HH", "II", "JJ"]);
  });
});

describe("app store: synced state", () => {
  test("starts from the engine's (cached) values, defaults when nothing is stored", () => {
    const r = rig();
    expect(r.s().symbol).toBeNull();
    expect(r.s().timeframe).toBe("1m");
    expect(r.s().tradeQty).toBe(100);
    expect(r.s().indicators).toEqual([]);
  });

  test("opening a ticker moves the synced view, puts it first in the recents and clears the readout", () => {
    const r = rig();
    r.s().setInspectT(123);
    r.s().openTicker("aal");
    r.s().openTicker("smpl");
    r.s().openTicker("aal");
    expect(r.s().symbol).toBe("AAL");
    expect(r.s().searchRecents).toEqual(["AAL", "SMPL"]);
    expect(r.s().inspectT).toBeNull();
    expect(r.sync.get(APP_NS.view)).toEqual({ symbol: "AAL", timeframe: "1m", bin: 0.01 });
  });

  test("remembering a ticker only touches the recents, never the view", () => {
    const r = rig();
    r.s().rememberTicker("smpl");
    r.s().rememberTicker("aal");
    r.s().rememberTicker("not a ticker!");
    expect(r.s().searchRecents).toEqual(["AAL", "SMPL"]);
    expect(r.sync.get(APP_NS.view)).toBeUndefined();
  });

  test("a ticker, timeframe or bin change keeps the other view fields", () => {
    const r = rig();
    r.s().openTicker("AAL");
    r.s().setTimeframe("5m");
    r.s().setBin(0.02);
    expect(r.sync.get(APP_NS.view)).toEqual({ symbol: "AAL", timeframe: "5m", bin: 0.02 });
  });

  test("another device's ticker change arrives, and the inspected candle goes with the old chart", () => {
    const r = rig();
    r.s().setInspectT(42);
    r.remote(APP_NS.view, { symbol: "TSLA", timeframe: "30s", bin: 0.01 });
    expect(r.s().symbol).toBe("TSLA");
    expect(r.s().timeframe).toBe("30s");
    expect(r.s().inspectT).toBeNull();
  });

  test("a remote bin change alone keeps the readout", () => {
    const r = rig();
    r.s().setInspectT(42);
    r.remote(APP_NS.view, { ...DEFAULT_VIEW, bin: 0.05 });
    expect(r.s().bin).toBe(0.05);
    expect(r.s().inspectT).toBe(42);
  });

  test("chart settings remember whether the last change was remote (no re-zoom on other devices)", () => {
    const r = rig();
    r.remote(SYNC_NS.chartSettings, { ...r.s().chartSettings, grid: false });
    expect(r.s().chartSettings.grid).toBe(false);
    expect(r.s().chartSettingsFromRemote).toBe(true);
    r.s().setChartSettings({ ...r.s().chartSettings, grid: true });
    expect(r.s().chartSettings.grid).toBe(true);
    expect(r.s().chartSettingsFromRemote).toBe(false);
  });

  test("the trade size syncs and refuses sizes the menus do not offer", () => {
    const r = rig();
    r.remote(APP_NS.tradeQty, 500);
    expect(r.s().tradeQty).toBe(500);
    r.s().setTradeQty(1000);
    expect(r.s().tradeQty).toBe(1000);
    r.s().setTradeQty(7);
    expect(r.s().tradeQty).toBe(100);
  });

  test("this device's unconfirmed edit stays on top of a remote change (the engine's rule, seen through the store)", () => {
    const r = rig();
    r.s().setTradeQty(1000);
    r.remote(APP_NS.tradeQty, 500);
    expect(r.s().tradeQty).toBe(1000);
  });

  test("the store follows only the changed namespace: an unrelated write keeps every other slice's identity", () => {
    const r = rig();
    const before = r.s();
    r.remote(drawingsNs("OTHER"), drawingsToDoc([trend("x")]));
    expect(r.s()).toBe(before);
    r.remote(SYNC_NS.layers, { halts: false });
    expect(r.s().indicators).toBe(before.indicators);
    expect(r.s().chartSettings).toBe(before.chartSettings);
  });

  test("the Volume shelves level: on by default, switched off here it syncs, and another device's switch arrives", () => {
    const r = rig();
    expect(r.s().layers.shelves).toBe(true);
    // A set stored before the switch existed keeps the bands on.
    r.remote(SYNC_NS.layers, { halts: false });
    expect(r.s().layers).toMatchObject({ halts: false, shelves: true });
    r.s().setLayers({ ...r.s().layers, shelves: false });
    expect(r.sync.get(SYNC_NS.layers)).toMatchObject({ shelves: false });
    const sent = r.connect();
    expect(sent.some((m) => m.type === "settings.set" && m.ns === SYNC_NS.layers && (m.value as { shelves?: boolean }).shelves === false)).toBe(true);
    // Another device (no unconfirmed edit of its own here) turns them off: this one follows.
    const other = rig();
    other.remote(SYNC_NS.layers, { ...other.s().layers, shelves: false });
    expect(other.s().layers.shelves).toBe(false);
  });
});

describe("app store: runtime detector switches", () => {
  test("on until a device switches one off; the flip is an entry patch and another device's arrives", () => {
    const r = rig();
    const sent = r.connect([]);
    expect(r.s().detectors).toMatchObject({ "x-float-1": true, run: false });
    r.s().setDetectorOn("x-float-1", false);
    const patch = sent.find((m) => m.type === "settings.patch");
    expect(patch).toMatchObject({ ns: "detectors", set: { "x-float-1": { on: false } } });
    expect(r.s().detectors).toMatchObject({ "x-float-1": false, run: false });
    // Another device (this one has nothing pending): its flips show here.
    const o = rig();
    o.connect([]);
    o.remote("detectors", { "x-float-1": { on: false } });
    expect(o.s().detectors).toMatchObject({ "x-float-1": false, run: false });
    o.remote("detectors", { "x-float-1": { on: true } });
    expect(o.s().detectors).toMatchObject({ "x-float-1": true, run: false });
  });
});

describe("app store: drawings follow the shown symbol", () => {
  test("showing a symbol loads its drawings; edits patch only what changed", () => {
    const r = rig();
    const sent = r.connect([{ ns: drawingsNs("AAL"), value: drawingsToDoc([trend("a"), trend("b")]) }]);
    expect(r.s().drawings).toEqual([]);
    r.s().showSymbol("AAL");
    expect(r.s().drawings.map((d) => d.id)).toEqual(["a", "b"]);
    r.s().setDrawings([trend("a", 20), trend("b")]);
    const patch = sent.find((m) => m.type === "settings.patch");
    expect(patch).toMatchObject({ ns: "drawings:AAL" });
    expect((patch as { remove?: string[] }).remove ?? []).toEqual([]);
    expect(Object.keys((patch as { set: object }).set)).toEqual(["a"]);
    expect(r.s().drawings[0]!.a.p).toBe(20);
  });

  test("another device's drawing on the shown symbol appears; on another symbol it does not", () => {
    const r = rig();
    r.s().showSymbol("AAL");
    r.remote(drawingsNs("AAL"), drawingsToDoc([trend("z")]));
    expect(r.s().drawings.map((d) => d.id)).toEqual(["z"]);
    r.remote(drawingsNs("TSLA"), drawingsToDoc([trend("t")]));
    expect(r.s().drawings.map((d) => d.id)).toEqual(["z"]);
  });

  test("no shown symbol: drawing edits go nowhere", () => {
    const r = rig();
    r.s().setDrawings([trend("a")]);
    expect(r.sync.namespaces()).toEqual([]);
  });
});

describe("app store: explicit picks (owner, 2026-10-23)", () => {
  test("every way of opening a ticker records it as this device's pick, newest first, once", () => {
    const r = rig();
    r.s().loadIntoSelected("cjmb");
    r.s().openTicker("aal");
    r.s().setDashboardSymbol(0, "CJMB");
    r.s().setDashboardSymbol(0, "junk!");
    expect(r.s().picked).toEqual(["CJMB", "AAL"]);
  });

  test("another device's slot write is not a pick here: the band fallback still applies to it", () => {
    const r = rig();
    r.remote(APP_NS.dashboard, { count: 1, slots: [{ symbol: "FXHI", timeframe: "1m" }] });
    expect(r.s().picked).toEqual([]);
    expect(shownTicker(r.s().dashboard.slots[0]!.symbol!, ["FXLO"], r.s().picked)).toBe("FXLO");
    r.s().loadIntoSelected("FXHI");
    expect(shownTicker(r.s().dashboard.slots[0]!.symbol!, ["FXLO"], r.s().picked)).toBe("FXHI");
  });
});

describe("app store: the dashboard panel", () => {
  test("starts with one empty chart; the count clamps to 1–4", () => {
    const r = rig();
    expect(r.s().dashboard.count).toBe(1);
    expect(r.s().dashboard.slots).toHaveLength(4);
    r.s().setDashboardCount(9);
    expect(r.s().dashboard.count).toBe(4);
    r.s().setDashboardCount(0);
    expect(r.s().dashboard.count).toBe(1);
  });

  test("a ticker loads into the selected chart and joins the recents; others keep theirs", () => {
    const r = rig();
    r.s().setDashboardCount(3);
    r.s().setDashboardSymbol(0, "aal");
    r.s().selectDashboardChart(2);
    r.s().loadIntoSelected("tsla");
    r.s().setDashboardTimeframe(2, "5m");
    expect(r.s().dashboard.slots.slice(0, 3)).toEqual([
      { symbol: "AAL", timeframe: "1m", indicators: null, layers: null },
      { symbol: null, timeframe: "1m", indicators: null, layers: null },
      { symbol: "TSLA", timeframe: "5m", indicators: null, layers: null },
    ]);
    expect(r.s().searchRecents).toEqual(["TSLA", "AAL"]);
    expect(r.sync.get(APP_NS.dashboard)).toMatchObject({ count: 3 });
  });

  test("selection is per device, never synced, and stays inside the count when another device shrinks it", () => {
    const r = rig();
    r.s().setDashboardCount(4);
    r.s().selectDashboardChart(3);
    expect(r.sync.get(APP_NS.dashboard)).not.toHaveProperty("selected");
    r.s().selectDashboardChart(7);
    expect(r.s().dashboardSelected).toBe(3);
    const other = rig();
    other.remote(APP_NS.dashboard, { count: 4, slots: [] });
    other.s().selectDashboardChart(3);
    other.remote(APP_NS.dashboard, { count: 2, slots: [] });
    expect(other.s().dashboardSelected).toBe(1);
  });

  test("the dashboard reads live by default; the Data setting switches it to replay and syncs", () => {
    const r = rig();
    expect(r.s().dashboard.source).toBe("live");
    r.s().setDashboardSource("replay");
    expect(r.s().dashboard.source).toBe("replay");
    expect(r.sync.get(APP_NS.dashboard)).toMatchObject({ source: "replay" });
    // Other dashboard writes keep it.
    r.s().setDashboardCount(2);
    r.s().setDashboardSymbol(0, "aal");
    expect(r.s().dashboard.source).toBe("replay");
    // Anything unknown from another device reads as live.
    const other = rig();
    other.remote(APP_NS.dashboard, { count: 1, slots: [], source: "paper" });
    expect(other.s().dashboard.source).toBe("live");
  });

  test("a hidden slot keeps its ticker for when the panel grows again", () => {
    const r = rig();
    r.s().setDashboardCount(2);
    r.s().setDashboardSymbol(1, "TSLA");
    r.s().setDashboardCount(1);
    r.s().setDashboardCount(2);
    expect(r.s().dashboard.slots[1]!.symbol).toBe("TSLA");
  });

  test("any symbol's drawings through a handle: read, patched back, remote edits seen", () => {
    const r = rig();
    const h = r.store.drawingsFor("aal");
    expect(r.store.drawingsFor("AAL")).toBe(h);
    let changes = 0;
    h.subscribe(() => changes++);
    h.set([trend("a")]);
    expect(h.get().map((d) => d.id)).toEqual(["a"]);
    r.remote(drawingsNs("AAL"), drawingsToDoc([trend("a"), trend("b")]));
    expect(h.get().map((d) => d.id)).toEqual(["a", "b"]);
    expect(changes).toBeGreaterThanOrEqual(2);
  });

  test("junk in the dashboard namespace falls back field by field", () => {
    const r = rig();
    r.remote(APP_NS.dashboard, { count: "x", slots: [{ symbol: "!!", timeframe: "2h" }, null, { symbol: "aal", timeframe: "30s" }] });
    expect(r.s().dashboard.count).toBe(1);
    expect(r.s().dashboard.slots.slice(0, 3)).toEqual([
      { symbol: null, timeframe: "1m", indicators: null, layers: null },
      { symbol: null, timeframe: "1m", indicators: null, layers: null },
      { symbol: "AAL", timeframe: "30s", indicators: null, layers: null },
    ]);
  });

  test("a chart's own indicators and levels stay on its slot, survive other writes, and parse junk away", () => {
    const r = rig();
    r.s().setDashboardIndicators(1, r.s().indicators.slice(0, 1));
    r.s().setDashboardLayers(1, { ...r.s().layers, fills: false });
    r.s().setDashboardSymbol(1, "aal");
    r.s().setDashboardCount(2);
    const slot = r.s().dashboard.slots[1]!;
    expect(slot.indicators).toEqual(r.s().indicators.slice(0, 1));
    expect(slot.layers).toMatchObject({ fills: false });
    // Other charts follow the shared set (null), and the shared set is untouched.
    expect(r.s().dashboard.slots[0]!.indicators).toBeNull();
    const synced = r.sync.get(APP_NS.dashboard) as { slots: { symbol: string | null; indicators: unknown }[] };
    expect(synced.slots[0]).toMatchObject({ indicators: null });
    expect(synced.slots[1]).toMatchObject({ symbol: "AAL", indicators: [] });
    // Junk from another build: a non-array set reads as "follows the shared one".
    const other = rig();
    other.remote(APP_NS.dashboard, { count: 1, slots: [{ symbol: "aal", timeframe: "1m", indicators: "x", layers: 3 }] });
    expect(other.s().dashboard.slots[0]).toMatchObject({ indicators: null, layers: null });
  });
});

describe("app store: migration and device state", () => {
  test("legacy recents and trade size are seeded once, only into an empty namespace", () => {
    const r = rig({ legacy: { searchRecents: ["aal", "smpl"], tradeQty: 500 } });
    expect(r.s().searchRecents).toEqual(["AAL", "SMPL"]);
    expect(r.s().tradeQty).toBe(500);
    const sent = r.connect();
    expect(sent.filter((m) => m.type === "settings.set" && m.ifAbsent).map((m) => (m as { ns: string }).ns).sort()).toEqual(["searchRecents", "tradeQty"]);
  });

  test("device prefs update the store and are persisted by the host; without a device slice they are ignored", () => {
    const r = rig({ device: true });
    r.s().setToolsVisible(false);
    r.s().setPaneHeights({ macd: 80 });
    expect(r.s().device).toMatchObject({ toolsVisible: false, paneHeights: { macd: 80 } });
    expect(r.saved).toEqual([
      ["toolsVisible", false],
      ["paneHeights", { macd: 80 }],
    ]);
    const web = rig();
    web.s().setToolsVisible(false);
    expect(web.s().device).toBeNull();
  });

  test("session state stays local: nothing written to the engine", () => {
    const r = rig();
    r.s().setSettingsOpen(true);
    r.s().setSearchOpen(true);
    r.s().setInspectT(5);
    expect(r.sync.namespaces()).toEqual([]);
  });

  test("dispose stops following the engine", () => {
    const r = rig();
    r.store.dispose();
    r.remote(APP_NS.tradeQty, 1000);
    expect(r.s().tradeQty).toBe(100);
  });
});

describe("favorites", () => {
  test("parse keeps valid unique tickers in order, capped", () => {
    expect(parseFavorites([" smpl", "AAPL", "SMPL", "1BAD", 4, "brk.b"])).toEqual(["SMPL", "AAPL", "BRK.B"]);
    expect(parseFavorites({})).toEqual([]);
    expect(parseFavorites(Array.from({ length: 60 }, (_, i) => `S${i}`)).length).toBe(40);
  });

  test("toggle adds at the end and removes; move reorders; another device's write lands", () => {
    const { store, s } = rig();
    store.getState().toggleFavorite("smpl");
    store.getState().toggleFavorite("AAPL");
    expect(s().favorites).toEqual(["SMPL", "AAPL"]);
    store.getState().moveFavorite("AAPL", -1);
    expect(s().favorites).toEqual(["AAPL", "SMPL"]);
    store.getState().moveFavorite("AAPL", -5);
    expect(s().favorites).toEqual(["AAPL", "SMPL"]);
    store.getState().toggleFavorite("AAPL");
    expect(s().favorites).toEqual(["SMPL"]);
    // Another device's list (this one has no unconfirmed edit of its own).
    const other = rig();
    other.remote(APP_NS.favorites, ["TSLA", "junk!", "SMPL"]);
    expect(other.s().favorites).toEqual(["TSLA", "SMPL"]);
  });
});

/**
 * Two devices on one in-memory server that stores and pushes like crates/server/src/settings.rs:
 * every set or patch is applied, its rev bumped, and the result broadcast to every connection as
 * `settings.changed` (the writer recognises its own opId as the echo).
 */
function twoDevices() {
  const docs = new Map<string, { rev: number; value: unknown }>();
  const engines: ReturnType<typeof createSettingsSync>[] = [];
  const make = (name: string) => {
    let n = 0;
    const sync = createSettingsSync({ initial: null, newOpId: () => `${name}-${++n}`, setTimer: () => 0, clearTimer: () => {} });
    engines.push(sync);
    sync.attach((m) => {
      if (m.type !== "settings.set" && m.type !== "settings.patch") return true;
      const cur = docs.get(m.ns);
      const value = m.type === "settings.set" ? m.value : applyPatch(cur?.value, m.set ?? {}, m.remove ?? []);
      const doc = { rev: (cur?.rev ?? 0) + 1, value };
      docs.set(m.ns, doc);
      // Pushed after the send returns, as a socket frame would be.
      queueMicrotask(() => {
        for (const e of engines) e.handle({ type: "settings.changed", ns: m.ns, rev: doc.rev, value: doc.value, opId: m.opId, updatedAt: 1 });
      });
      return true;
    });
    sync.handle({ type: "settings.snapshot", docs: [...docs].map(([ns, d]) => ({ ns, ...d, updatedAt: 1 })) });
    return { sync, store: createAppStore({ sync }) };
  };
  const settle = async () => {
    for (let i = 0; i < 4; i++) {
      for (const e of engines) e.flush();
      await Promise.resolve();
    }
  };
  return { phone: make("phone"), web: make("web"), docs, settle };
}

describe("app store: indicator settings persist and sync live", () => {
  const preset = (id: string) => INDICATOR_PRESETS.find((p) => p.id === id)!;
  // Per-chart settings only: colours are global since 2026-11-07 (the describe below).
  const styled: IndicatorSpec = { ...preset("macd"), style: "ppo", markers: false, fast: 8 };

  test("off then on restores style, markers and periods; the memory is synced as its own namespace", () => {
    const r = rig();
    r.s().setIndicators([styled]);
    r.s().setIndicators(toggleIndicator(r.s().indicators, preset("macd")));
    expect(r.s().indicators).toEqual([]);
    expect(r.sync.get(SYNC_NS.indicatorMemory)).toEqual({ macd: styled });
    expect(r.s().indicatorMemory).toEqual({ macd: styled });
    r.s().setIndicators(toggleIndicator(r.s().indicators, preset("macd")));
    expect(r.s().indicators).toEqual([styled]);
  });

  test("Reset to default clears the remembered settings", () => {
    const r = rig();
    r.s().setIndicators([styled]);
    r.s().setIndicators([]);
    r.s().setIndicators([preset("macd")]);
    expect(r.s().indicators).toEqual([styled]);
    r.s().setIndicators(replaceIndicator(r.s().indicators, resetIndicator(styled)));
    expect(r.s().indicatorMemory).toEqual({});
    r.s().setIndicators([]);
    r.s().setIndicators([preset("macd")]);
    expect(r.s().indicators).toEqual([preset("macd")]);
  });

  test("an older build's stored settings (no memory namespace) load and toggle as before", () => {
    const r = rig();
    r.connect([{ ns: SYNC_NS.indicators, value: [{ id: "macd", kind: "macd", fast: 12, slow: 26, signal: 9 }] }]);
    expect(r.s().indicators).toEqual([preset("macd")]);
    expect(r.s().indicatorMemory).toEqual({});
    r.s().setIndicators([]);
    r.s().setIndicators([preset("macd")]);
    expect(r.s().indicators).toEqual([preset("macd")]);
    // An older device rewriting the list whole never touches the memory.
    r.s().setIndicators([styled]);
    r.s().setIndicators([]);
    r.remote(SYNC_NS.indicators, [{ id: "vwap", kind: "vwap", junk: 1 }]);
    expect(r.s().indicatorMemory).toEqual({ macd: styled });
  });

  test("a dashboard chart's own set and the ticker page's set remember too (one memory)", () => {
    const r = rig();
    r.s().setDashboardIndicators(1, [styled]);
    r.s().setDashboardIndicators(1, []);
    r.s().setTickerIndicators([preset("macd")]);
    expect(r.s().tickerIndicators).toEqual([styled]);
    r.s().setDashboardIndicators(1, [preset("macd")]);
    expect(r.s().dashboard.slots[1]?.indicators).toEqual([styled]);
  });

  test("the ticker page's old per-device list is seeded once into the synced namespace", () => {
    const r = rig({ legacy: { tickerIndicators: [{ id: "vwap", kind: "vwap" }, { junk: true }] } });
    expect(r.s().tickerIndicators).toEqual([preset("vwap")]);
  });

  test("a change on one device reaches the other's store live: MACD style, a colour, and the remembered settings", async () => {
    const { phone, web, settle } = twoDevices();
    const seen: IndicatorSpec[][] = [];
    web.store.subscribe((s, p) => s.indicators !== p.indicators && seen.push(s.indicators));
    phone.store.getState().setIndicators([preset("macd")]);
    await settle();
    expect(web.store.getState().indicators).toEqual([preset("macd")]);
    // The phone picks a style and a colour: the open web chart follows without a reload.
    phone.store.getState().setIndicators([{ ...preset("macd"), style: "ppo" }]);
    await settle();
    phone.store.getState().setIndicatorColor(phone.store.getState().indicators[0]!, "macd", "#112233");
    await settle();
    expect(web.store.getState().indicators).toEqual([{ ...preset("macd"), style: "ppo", colors: { macd: "#112233" } }]);
    expect(seen.length).toBeGreaterThanOrEqual(3);
    // Turned off on the phone, on again on the web: the web restores the phone's settings.
    phone.store.getState().setIndicators([]);
    await settle();
    expect(web.store.getState().indicatorMemory.macd).toEqual({ ...preset("macd"), style: "ppo" });
    web.store.getState().setIndicators(toggleIndicator(web.store.getState().indicators, preset("macd")));
    await settle();
    expect(phone.store.getState().indicators).toEqual([{ ...preset("macd"), style: "ppo", colors: { macd: "#112233" } }]);
    // The ticker page's list syncs the same way, web to phone.
    web.store.getState().setTickerIndicators([preset("rsi-14")]);
    await settle();
    expect(phone.store.getState().tickerIndicators).toEqual([preset("rsi-14")]);
  });
});

describe("app store: indicator colours are global (owner, 2026-11-07)", () => {
  const preset = (id: string) => INDICATOR_PRESETS.find((p) => p.id === id)!;
  const lineOf = (list: readonly IndicatorSpec[], id: string) => list.find((s) => s.id === id)?.colors;

  test("one colour applies to every chart, whatever else each chart shows", () => {
    const r = rig();
    // Main chart: VWAP + EMA 9. A dashboard chart with its own list: EMA 9 + EMA 20 + MACD. Ticker page: EMA 9 + RSI.
    r.s().setIndicators([preset("vwap"), preset("ema-9")]);
    r.s().setDashboardIndicators(2, [preset("ema-9"), preset("ema-20"), preset("macd")]);
    r.s().setTickerIndicators([preset("rsi-14"), preset("ema-9")]);
    r.s().indicatorColorActions.set(preset("ema-9"), "line", "#AA00FF");
    expect(r.sync.get(SYNC_NS.indicatorColors)).toEqual({ "ema:9/line": "#aa00ff" });
    expect(lineOf(r.s().indicators, "ema-9")).toEqual({ line: "#aa00ff" });
    expect(lineOf(r.s().dashboard.slots[2]!.indicators!, "ema-9")).toEqual({ line: "#aa00ff" });
    expect(lineOf(r.s().tickerIndicators, "ema-9")).toEqual({ line: "#aa00ff" });
    // EMA 20 is another line (its period defines it), and VWAP is untouched.
    expect(lineOf(r.s().dashboard.slots[2]!.indicators!, "ema-20")).toBeUndefined();
    expect(lineOf(r.s().indicators, "vwap")).toBeUndefined();
    // MACD's parts: one role at a time, all charts.
    r.s().setIndicatorColor(preset("macd"), "histUp", "#00ff00");
    r.s().setIndicatorColor(preset("macd"), "signal", "#ff00ff");
    expect(lineOf(r.s().dashboard.slots[2]!.indicators!, "macd")).toEqual({ histUp: "#00ff00", signal: "#ff00ff" });
    // The lists themselves never store a colour.
    expect(JSON.stringify(r.sync.get(SYNC_NS.indicators))).not.toContain("colors");
    expect(JSON.stringify(r.sync.get(APP_NS.dashboard))).not.toContain("colors");
    // Turning EMA 9 off and on keeps its (global) colour; null puts it back to the default everywhere.
    r.s().setIndicators([preset("vwap")]);
    r.s().setIndicators([preset("vwap"), preset("ema-9")]);
    expect(lineOf(r.s().indicators, "ema-9")).toEqual({ line: "#aa00ff" });
    r.s().setIndicatorColor(preset("ema-9"), "line", null);
    expect(lineOf(r.s().tickerIndicators, "ema-9")).toBeUndefined();
  });

  test("Reset to default clears every colour of that indicator only, everywhere", () => {
    const r = rig();
    r.s().setIndicators([preset("macd"), preset("ema-9")]);
    r.s().setIndicatorColor(preset("macd"), "macd", "#111111");
    r.s().setIndicatorColor({ ...preset("macd"), style: "four-color" }, "upSoft", "#222222");
    r.s().setIndicatorColor(preset("ema-9"), "line", "#333333");
    r.s().resetIndicatorColors(preset("macd"));
    expect(r.sync.get(SYNC_NS.indicatorColors)).toEqual({ "ema:9/line": "#333333" });
    expect(lineOf(r.s().indicators, "macd")).toBeUndefined();
  });

  test("migration: the per-spec colours become the global ones once (main chart wins), then per-spec ones are ignored", async () => {
    const r = rig();
    // What an older build stored: colours on the specs of several lists.
    const sent = r.connect([
      { ns: SYNC_NS.indicators, value: [{ id: "ema-9", kind: "ema", period: 9, colors: { line: "#ff0000" } }, { id: "bb-20", kind: "bb", period: 20, mult: 2, colors: { fill: "#00ff00" } }] },
      { ns: SYNC_NS.tickerIndicators, value: [{ id: "ema-9", kind: "ema", period: 9, colors: { line: "#0000ff" } }, { id: "rsi-14", kind: "rsi", period: 14, colors: { line: "#123456" } }] },
      { ns: APP_NS.dashboard, value: { count: 2, slots: [{ symbol: null, timeframe: "1m", indicators: null, layers: null }, { symbol: null, timeframe: "1m", indicators: [{ id: "ema-9", kind: "ema", period: 9, colors: { line: "#00ffff" } }], layers: null }] } },
    ]);
    await Promise.resolve();
    const seed = sent.find((m) => m.type === "settings.set" && m.ns === SYNC_NS.indicatorColors);
    expect(seed).toMatchObject({ ifAbsent: true, value: { "ema:9/line": "#ff0000", "bb/fill": "#00ff00", "rsi/line": "#123456" } });
    // Every chart now shows the one colour, the ticker page and the dashboard included.
    expect(lineOf(r.s().tickerIndicators, "ema-9")).toEqual({ line: "#ff0000" });
    expect(lineOf(r.s().dashboard.slots[1]!.indicators!, "ema-9")).toEqual({ line: "#ff0000" });
    // An older device writes a per-spec colour afterwards: ignored, the global one stands.
    r.remote(SYNC_NS.indicatorColors, { "ema:9/line": "#ff0000", "bb/fill": "#00ff00", "rsi/line": "#123456" });
    r.remote(SYNC_NS.indicators, [{ id: "ema-9", kind: "ema", period: 9, colors: { line: "#999999" } }]);
    expect(lineOf(r.s().indicators, "ema-9")).toEqual({ line: "#ff0000" });
    // The next write of a list drops the per-spec colours (the old namespaces keep their shapes).
    r.s().setIndicators(r.s().indicators);
    expect(r.sync.get(SYNC_NS.indicators)).toEqual([{ id: "ema-9", kind: "ema", period: 9 }]);
  });

  test("migration: a server that already has global colours wins over this device's per-spec ones", async () => {
    const r = rig();
    const sent = r.connect([
      { ns: SYNC_NS.indicators, value: [{ id: "ema-9", kind: "ema", period: 9, colors: { line: "#ff0000" } }] },
      { ns: SYNC_NS.indicatorColors, value: { "ema:9/line": "#abcdef" } },
    ]);
    await Promise.resolve();
    expect(sent.some((m) => m.ns === SYNC_NS.indicatorColors)).toBe(false);
    expect(lineOf(r.s().indicators, "ema-9")).toEqual({ line: "#abcdef" });
  });

  test("old settings with no colour namespace load unchanged, per-spec colours shown until a map exists", () => {
    const r = rig();
    r.connect([{ ns: SYNC_NS.indicators, value: [{ id: "vwap", kind: "vwap" }, { id: "ema-20", kind: "ema", period: 20 }] }]);
    expect(r.s().indicatorColors).toBeNull();
    expect(r.s().indicators).toEqual([preset("vwap"), preset("ema-20")]);
    expect(r.sync.get(SYNC_NS.indicatorColors)).toBeUndefined();
    // Junk in the map is dropped entry by entry.
    r.remote(SYNC_NS.indicatorColors, { "ema:20/line": "#ABCDEF", "ema:20/nope": "#000000", "vwap/line": "red", junk: 1 });
    expect(r.s().indicatorColors).toEqual({ "ema:20/line": "#abcdef" });
  });

  test("live: a colour picked on the phone recolours the web's main chart and its dashboard chart", async () => {
    const { phone, web, settle } = twoDevices();
    web.store.getState().setIndicators([preset("vwap"), preset("ema-9")]);
    web.store.getState().setDashboardIndicators(1, [preset("ema-9"), preset("sma-50")]);
    await settle();
    phone.store.getState().indicatorColorActions.set(preset("ema-9"), "line", "#ff8800");
    // Meanwhile the web turns SMA 50's colour (another entry): both land.
    web.store.getState().indicatorColorActions.set(preset("sma-50"), "line", "#0088ff");
    await settle();
    expect(lineOf(web.store.getState().indicators, "ema-9")).toEqual({ line: "#ff8800" });
    expect(lineOf(web.store.getState().dashboard.slots[1]!.indicators!, "ema-9")).toEqual({ line: "#ff8800" });
    expect(lineOf(web.store.getState().dashboard.slots[1]!.indicators!, "sma-50")).toEqual({ line: "#0088ff" });
    expect(phone.store.getState().indicatorColors).toEqual({ "ema:9/line": "#ff8800", "sma:50/line": "#0088ff" });
    // Reset on the web: gone on the phone too.
    web.store.getState().indicatorColorActions.reset(preset("ema-9"));
    await settle();
    expect(lineOf(phone.store.getState().indicators, "ema-9")).toBeUndefined();
  });
});

describe("app store: linking charts and multi-selection (owner, 2026-11-15)", () => {
  const symbols = (r: ReturnType<typeof rig>) => r.s().dashboard.slots.slice(0, r.s().dashboard.count).map((s) => s.symbol);

  test("off by default: a ticker still loads into the selected chart only", () => {
    const r = rig();
    r.s().setDashboardCount(3);
    expect(r.s().dashboard.linked).toBe(false);
    r.s().selectDashboardChart(1);
    r.s().loadIntoSelected("aal");
    expect(symbols(r)).toEqual([null, "AAL", null]);
  });

  test("the paperclip on: a ticker loads into every chart shown, not the hidden slots", () => {
    const r = rig();
    r.s().setDashboardCount(4);
    r.s().setDashboardSymbol(3, "spy");
    r.s().setDashboardCount(3);
    r.s().setDashboardLinked(true);
    r.s().loadIntoSelected("nvda");
    expect(symbols(r)).toEqual(["NVDA", "NVDA", "NVDA"]);
    expect(r.s().dashboard.slots[3]!.symbol).toBe("SPY");
    // One recents entry and one explicit pick, not one per chart.
    expect(r.s().searchRecents.filter((s) => s === "NVDA")).toHaveLength(1);
    expect(r.s().picked.filter((s) => s === "NVDA")).toHaveLength(1);
  });

  test("only the ticker changes: each chart keeps its timeframe, indicators and levels", () => {
    const r = rig();
    r.s().setDashboardCount(2);
    r.s().setDashboardTimeframe(0, "5m");
    r.s().setDashboardTimeframe(1, "1m");
    r.s().setDashboardIndicators(1, INDICATOR_PRESETS.slice(0, 1));
    const before = r.s().dashboard.slots[1]!;
    r.s().setDashboardLinked(true);
    r.s().loadIntoSelected("amd");
    const [a, b] = r.s().dashboard.slots;
    expect(a).toMatchObject({ symbol: "AMD", timeframe: "5m" });
    expect(b).toEqual({ ...before, symbol: "AMD" });
  });

  test("the linked state syncs with the panel and survives another device's write of other fields", () => {
    const r = rig();
    r.s().setDashboardLinked(true);
    expect(r.sync.get(APP_NS.dashboard)).toMatchObject({ linked: true });
    r.s().setDashboardCount(2);
    expect(r.s().dashboard.linked).toBe(true);
    const other = rig();
    other.remote(APP_NS.dashboard, { count: 2, slots: [], linked: true });
    expect(other.s().dashboard.linked).toBe(true);
    other.remote(APP_NS.dashboard, { count: 2, slots: [], linked: "yes" });
    expect(other.s().dashboard.linked).toBe(false);
    r.s().setDashboardLinked(false);
    expect(r.s().dashboard.linked).toBe(false);
  });

  test("Shift+click adds charts to the selection and a ticker loads into all of them", () => {
    const r = rig();
    r.s().setDashboardCount(4);
    r.s().selectDashboardChart(0);
    r.s().toggleDashboardChart(2);
    r.s().toggleDashboardChart(3);
    expect(r.s().dashboardSelection).toEqual([0, 2, 3]);
    expect(r.s().dashboardSelected).toBe(3);
    r.s().loadIntoSelected("tsla");
    expect(symbols(r)).toEqual(["TSLA", null, "TSLA", "TSLA"]);
  });

  test("Shift+click on a selected chart takes it out, but never empties the selection", () => {
    const r = rig();
    r.s().setDashboardCount(3);
    r.s().selectDashboardChart(0);
    r.s().toggleDashboardChart(1);
    r.s().toggleDashboardChart(1);
    expect(r.s().dashboardSelection).toEqual([0]);
    expect(r.s().dashboardSelected).toBe(0);
    r.s().toggleDashboardChart(0);
    expect(r.s().dashboardSelection).toEqual([0]);
    // Removing the primary hands it to the last one still selected.
    r.s().toggleDashboardChart(2);
    r.s().toggleDashboardChart(2);
    expect(r.s().dashboardSelected).toBe(0);
    r.s().toggleDashboardChart(2);
    r.s().toggleDashboardChart(0);
    expect(r.s().dashboardSelection).toEqual([2]);
    expect(r.s().dashboardSelected).toBe(2);
  });

  test("a plain click returns to a single selection", () => {
    const r = rig();
    r.s().setDashboardCount(4);
    r.s().selectDashboardChart(0);
    r.s().toggleDashboardChart(1);
    r.s().toggleDashboardChart(2);
    r.s().selectDashboardChart(3);
    expect(r.s().dashboardSelection).toEqual([3]);
    r.s().loadIntoSelected("aapl");
    expect(symbols(r)).toEqual([null, null, null, "AAPL"]);
  });

  test("the multi-selection is per device and keeps only charts still shown when the count drops", () => {
    // The count drops on ANOTHER device: this one has no pending edit of the panel.
    const r = rig();
    r.remote(APP_NS.dashboard, { count: 4, slots: [] });
    r.s().selectDashboardChart(0);
    r.s().toggleDashboardChart(3);
    expect(r.sync.get(APP_NS.dashboard)).not.toHaveProperty("selection");
    r.remote(APP_NS.dashboard, { count: 2, slots: [] });
    expect(r.s().dashboardSelected).toBe(1);
    expect(r.s().dashboardSelection).toEqual([0, 1]);
    r.s().toggleDashboardChart(5);
    expect(r.s().dashboardSelection).toEqual([0, 1]);
  });

  test("a notice frozen on one of the charts a linked pick replaces is dropped", () => {
    const r = rig();
    r.s().setDashboardCount(2);
    r.s().selectDashboardChart(1);
    r.s().setDashboardLinked(true);
    r.s().loadIntoSelected("aal");
    expect(r.s().noticeSelection).toBeNull();
  });
});
