import { describe, expect, test } from "bun:test";
import { DEFAULT_TOOLBOX_PHONE, drawingsNs, drawingsToDoc, SYNC_NS, type Drawing } from "@stockwatcher/chart-core";
import { createSettingsSync, type SettingsClientMessage } from "@stockwatcher/shared";
import { APP_NS, DEFAULT_VIEW, parseSearchRecents, parseSymbol, parseTradeQty, parseView } from "./namespaces.ts";
import { createAppStore, type DevicePrefs } from "./store.ts";

/** A store on a real (offline) sync engine, plus a way to play another device's write. */
function rig(opts: { legacy?: { searchRecents?: unknown; tradeQty?: unknown }; device?: boolean } = {}) {
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
  test("symbols, trade sizes, recents", () => {
    expect(parseSymbol("brk.b")).toBe("BRK.B");
    expect(parseSymbol(42)).toBeNull();
    expect(parseTradeQty(500)).toBe(500);
    expect(parseTradeQty(123)).toBe(100);
    expect(parseSearchRecents(["a", "A", 3, "bb", "cc", "dd", "ee", "ff"])).toEqual(["A", "BB", "CC", "DD", "EE"]);
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
      { symbol: "AAL", timeframe: "1m" },
      { symbol: null, timeframe: "1m" },
      { symbol: "TSLA", timeframe: "5m" },
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
      { symbol: null, timeframe: "1m" },
      { symbol: null, timeframe: "1m" },
      { symbol: "AAL", timeframe: "30s" },
    ]);
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
