import { afterEach, expect, mock, test } from "bun:test";
import type { DetectionMessage, PressureMessage } from "@stockwatcher/shared";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

// The routing test exercises the real shared WS client/channels without importing native
// notification modules or opening an external socket.
let token: string | null = null;
let runners = { runnersNew: true, runnersBreakout: false };
const runnersListeners = new Set<() => void>();
mock.module("../notifications/push", () => ({ pushToken: () => token, subscribePushToken: () => () => {}, deviceInfo: () => ({ platform: "test", name: "test" }) }));
mock.module("../notifications/runnersPush", () => ({
  runnersRegisterFields: () => runners,
  subscribeRunnersPush: (fn: () => void) => {
    runnersListeners.add(fn);
    return () => runnersListeners.delete(fn);
  },
}));
mock.module("./server", () => ({ wsUrl: () => "ws://local-fixture.invalid/ws" }));

const { createMobileFeed } = await import("./serverFeed");

class Socket {
  static instances: Socket[] = [];
  readyState = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  sent: object[] = [];
  listeners: ((ev: { data: unknown }) => void)[] = [];
  constructor() { Socket.instances.push(this); }
  addEventListener(_type: string, fn: (ev: { data: unknown }) => void) { this.listeners.push(fn); }
  send(data: string) { this.sent.push(JSON.parse(data)); }
  close() { this.readyState = 3; this.onclose?.({}); }
  open() { this.readyState = 1; this.onopen?.({}); }
  receive(value: object) { for (const fn of this.listeners) fn({ data: JSON.stringify(value) }); }
}

const realSocket = globalThis.WebSocket;
const welcome = { type: "welcome", version: "test", commit: "test", heartbeatSecs: 15 };
const disabled = (source: DetectionMessage["source"] = "replay"): DetectionMessage => ({
  type: "detection.snapshot", enabled: false, source, runEpoch: "test-replay-epoch", cursor: 0,
  snapshot: null, transitions: [], history: { capacity: 2048, dropped: 0, first_sequence: null, last_sequence: null },
  telemetry: {}, profile: { price_min_micros: 100_000, price_max_micros: 25_000_000, research_min_micros: 150_000, research_max_micros: 25_000_000 }, experimental: true, alertsEnabled: false,
});
let cleanup: (() => void) | undefined;
afterEach(() => {
  cleanup?.();
  cleanup = undefined;
  globalThis.WebSocket = realSocket;
  Socket.instances = [];
});

test("push.register carries this phone's Runners opt-ins, and goes again when a switch changes", () => {
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  token = "ExponentPushToken[fold]";
  const feed = createMobileFeed(null, undefined, undefined, "mobile-dashboard", "live");
  cleanup = () => {
    feed.close();
    token = null;
    runners = { runnersNew: true, runnersBreakout: false };
  };
  const socket = Socket.instances[0]!;
  socket.open();
  socket.receive(welcome);
  const registers = () => socket.sent.filter((m) => (m as { type: string }).type === "push.register");
  expect(registers()).toEqual([{ type: "push.register", token: "ExponentPushToken[fold]", platform: "test", name: "test", runnersNew: true, runnersBreakout: false }]);
  runners = { runnersNew: true, runnersBreakout: true };
  for (const l of runnersListeners) l();
  expect(registers().at(-1)).toMatchObject({ runnersNew: true, runnersBreakout: true });
  // Closed: a change no longer reaches the (gone) socket, and the listener is released.
  feed.close();
  expect(runnersListeners.size).toBe(0);
});

test("the modal's mobile feed handshakes with its explicit market source", () => {
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  const feed = createMobileFeed(null, undefined, undefined, "mobile-dashboard", "replay");
  cleanup = () => feed.close();
  const socket = Socket.instances[0]!;
  socket.open();
  expect(socket.sent).toContainEqual({ type: "hello", client: "mobile-dashboard", source: "replay" });
});

test("Detection frames reach the shared channel and multiple mounted views share one subscription", () => {
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  let alerts = 0;
  const feed = createMobileFeed(null, undefined, undefined, "mobile-dashboard", "replay", () => { alerts++; });
  cleanup = () => feed.close();
  const socket = Socket.instances[0]!;
  socket.open();
  socket.receive(welcome);
  const offPanel = feed.detection.subscribe(() => {});
  const offModal = feed.detection.subscribe(() => {});
  expect(socket.sent.filter((m) => (m as { type: string }).type === "detection.subscribe")).toHaveLength(1);
  // Type-first is the normal server fast path. A peer with reordered JSON properties must
  // still reach the shared parser through the chart/paper tee's fallback.
  const { type, ...payload } = disabled();
  socket.receive({ ...payload, type });
  expect(feed.detection.get().enabled).toBe(false);
  expect(feed.detection.get().connected).toBe(true);
  expect(feed.detection.get().frame?.source).toBe("replay");
  expect(alerts).toBe(0);
  offPanel();
  expect(socket.sent).not.toContainEqual({ type: "detection.unsubscribe" });
  offModal();
  expect(socket.sent).toContainEqual({ type: "detection.unsubscribe" });
});

test("a mobile replay connection rejects a live Detection frame and requests resync", () => {
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  const feed = createMobileFeed(null, undefined, undefined, "mobile-dashboard", "replay");
  cleanup = () => feed.close();
  const socket = Socket.instances[0]!;
  socket.open();
  socket.receive(welcome);
  feed.detection.subscribe(() => {});
  socket.receive(disabled());
  socket.receive({ ...disabled("live"), runEpoch: "live-epoch" });
  expect(feed.detection.get().frame?.source).toBe("replay");
  expect(feed.detection.get().connected).toBe(false);
  expect(feed.detection.get().resyncing).toBe(true);
  expect(socket.sent.filter((m) => (m as { type: string }).type === "detection.subscribe")).toHaveLength(2);
});

test("a reopened mobile socket keeps Detection stale until its replacement snapshot arrives", async () => {
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  const feed = createMobileFeed(null, undefined, undefined, "mobile-dashboard", "replay");
  cleanup = () => feed.close();
  const socket = Socket.instances[0]!;
  socket.open();
  socket.receive(welcome);
  feed.detection.subscribe(() => {});
  socket.receive(disabled());
  socket.close();
  expect(feed.detection.get().connected).toBe(false);
  await new Promise((resolve) => setTimeout(resolve, 550));
  const reopened = Socket.instances[1]!;
  expect(reopened).toBeDefined();
  reopened.open();
  reopened.receive(welcome);
  expect(reopened.sent).toContainEqual({ type: "detection.subscribe" });
  expect(feed.detection.get().connected).toBe(false);
  reopened.receive(disabled());
  expect(feed.detection.get().connected).toBe(true);
});

test("mobile pressure retains last-known rows through reconnect, and refreshes only on the new frame", async () => {
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  const feed = createMobileFeed(null, undefined, undefined, "mobile-dashboard", "live");
  cleanup = () => feed.close();
  const off = feed.pressure.subscribe(() => {});
  const socket = Socket.instances[0]!;
  socket.open(); socket.receive(welcome);
  expect(feed.pressure.status()).toEqual({ connected: false, asOf: null });
  const frame: PressureMessage = { type: "pressure.snapshot", t: 1234, board: [{ s: "TEST", p: 2, o: 2, risk: 75, side: "bull", rise: 40, riseSecs: 30, band: 2.1, toBand: 5, pressure: 80, buyShare: 0.8, pace: 4 }] };
  socket.receive(frame);
  expect(feed.pressure.status()).toEqual({ connected: true, asOf: 1234 });
  const board = feed.pressure.get();
  socket.close();
  expect(feed.pressure.get()).toBe(board);
  expect(feed.pressure.status()).toEqual({ connected: false, asOf: 1234 });
  await new Promise((resolve) => setTimeout(resolve, 550));
  const reopened = Socket.instances[1]!;
  expect(reopened).toBeDefined();
  reopened.open(); reopened.receive(welcome);
  expect(reopened.sent).toContainEqual({ type: "pressure.subscribe" });
  expect(feed.pressure.status().connected).toBe(false);
  socket.receive({ ...frame, t: 9999 }); // queued delivery from the retired socket is ignored
  expect(feed.pressure.status()).toEqual({ connected: false, asOf: 1234 });
  reopened.receive({ ...frame, t: 2234, board: [] });
  expect(feed.pressure.status()).toEqual({ connected: true, asOf: 2234 });
  expect(feed.pressure.get()).toEqual([]);
  off();
  expect(feed.pressure.status()).toEqual({ connected: false, asOf: null });
});

test("actual Rust synthetic server snapshots reach the mobile channel without losing episodes or enabling alerts", () => {
  const data = readFileSync(new URL("./__fixtures__/detection-server-v1.json", import.meta.url));
  const provenance = JSON.parse(readFileSync(new URL("./__fixtures__/detection-server-v1.provenance.json", import.meta.url), "utf8")) as { synthetic: boolean; frame_count: number; sha256: string };
  const records = JSON.parse(data.toString("utf8")) as { scenario: string; frame: DetectionMessage }[];
  expect(createHash("sha256").update(data).digest("hex")).toBe(provenance.sha256);
  expect(provenance.synthetic).toBe(true);
  expect(records).toHaveLength(provenance.frame_count);
  // The real momentum output includes an unarmed range-low reference. Do not trim that
  // schema case out to make an incomplete client validator pass.
  expect(records.some((r) => [...r.frame.snapshot!.active, ...r.frame.snapshot!.recent].some((e) => e.levels.some((l) => String(l.kind) === "range_low")))).toBe(true);
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  let alerts = 0;
  const feed = createMobileFeed(null, undefined, undefined, "mobile-dashboard", "replay", () => { alerts++; });
  cleanup = () => feed.close();
  const socket = Socket.instances[0]!;
  socket.open(); socket.receive(welcome);
  feed.detection.subscribe(() => {});
  for (const { frame } of records) {
    expect(frame.source).toBe("fixture");
    socket.receive(frame);
    const state = feed.detection.get();
    expect(state.connected).toBe(true);
    expect(state.frame?.runEpoch).toBe(frame.runEpoch);
    expect(state.frame?.snapshot).toEqual(frame.snapshot);
    expect(state.snapshot?.episodes.map((e) => e.episode_id)).toEqual([...frame.snapshot!.active, ...frame.snapshot!.recent].map((e) => e.episode_id));
    socket.receive(frame);
    expect(feed.detection.get().snapshot?.episodes).toHaveLength(state.snapshot!.episodes.length);
  }
  expect(alerts).toBe(0);
});

test("indicators go on chart.subscribe only to a server that offers indicators.v1, and toggles go as chart.indicators.set", async () => {
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  const feed = createMobileFeed(null, undefined, undefined, "mobile", "replay");
  cleanup = () => feed.close();
  const demand = { indicators: [{ id: "ema-9", kind: "ema" as const, period: 9 }], rth: true };
  const chartFrames = (s: Socket) => s.sent.filter((m) => (m as { type: string }).type.startsWith("chart."));
  // An older server (no features): the phone keeps its local math, nothing new on the wire.
  const old = Socket.instances[0]!;
  old.open();
  old.receive(welcome);
  feed.subscribe({ symbol: "SMPL", timeframe: "1m", profileBin: 0.01, id: "m0", indicators: demand }, () => {});
  feed.indicators.set("m0", { ...demand, rth: false });
  expect(feed.indicators.available()).toBe(false);
  expect(chartFrames(old)).toEqual([{ type: "chart.subscribe", symbol: "SMPL", timeframe: "1m", profileBin: 0.01, id: "m0" }]);
  // Reconnected to a server with the feature: the resubscribe carries the LATEST demand.
  old.close();
  await new Promise((resolve) => setTimeout(resolve, 550));
  const next = Socket.instances.at(-1)!;
  expect(next).not.toBe(old);
  next.open();
  next.receive({ ...welcome, features: ["indicators.v1"] });
  expect(feed.indicators.available()).toBe(true);
  expect(chartFrames(next)).toEqual([{ type: "chart.subscribe", symbol: "SMPL", timeframe: "1m", profileBin: 0.01, id: "m0", indicators: demand.indicators, rth: false }]);
  feed.indicators.set("m0", { indicators: [], rth: false });
  expect(chartFrames(next).at(-1)).toEqual({ type: "chart.indicators.set", id: "m0", indicators: [], rth: false });
});

test("a demand's limit goes out only to a server that offers indicators.limit.v1", () => {
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  const feed = createMobileFeed(null, undefined, undefined, "mobile", "replay");
  cleanup = () => feed.close();
  const demand = { indicators: [{ id: "ema-9", kind: "ema" as const, period: 9 }], rth: false, limit: 1500 };
  const chartFrames = (s: Socket) => s.sent.filter((m) => (m as { type: string }).type.startsWith("chart."));
  const socket = Socket.instances[0]!;
  socket.open();
  // indicators.v1 without the limit feature: the list goes, the limit does not (the server would
  // ignore it; leaving it off keeps the frame what that server expects).
  socket.receive({ ...welcome, features: ["indicators.v1"] });
  feed.subscribe({ symbol: "SMPL", timeframe: "15s", profileBin: 0.01, id: "m0", indicators: demand }, () => {});
  expect(chartFrames(socket).at(-1)).toEqual({ type: "chart.subscribe", symbol: "SMPL", timeframe: "15s", profileBin: 0.01, id: "m0", indicators: demand.indicators, rth: false });
  socket.receive({ ...welcome, features: ["indicators.v1", "indicators.limit.v1"] });
  // Zoomed out: the chart asks for every bar, which is a change of demand (`limit` dropped).
  feed.indicators.set("m0", { ...demand, limit: undefined });
  expect(chartFrames(socket).at(-1)).toEqual({ type: "chart.indicators.set", id: "m0", indicators: demand.indicators, rth: false });
  feed.indicators.set("m0", demand);
  expect(chartFrames(socket).at(-1)).toEqual({ type: "chart.indicators.set", id: "m0", indicators: demand.indicators, rth: false, limit: 1500 });
});
