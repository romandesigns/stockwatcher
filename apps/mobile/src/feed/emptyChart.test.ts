import { afterEach, expect, mock, test } from "bun:test";
import type { ChartMessage } from "@stockwatcher/chart-core";

// The empty chart on the phone (owner, 2026-11-15; the web twin and the reasoning are in
// apps/web/src/feed/emptyChart.test.ts): back from the background, the socket says OPEN but the
// connection is gone; a chart opened then is subscribed into nothing and stays empty until the
// liveness timeout (37.5 s, counted from the resume because JS timers stop in the background).
mock.module("../notifications/push", () => ({ pushToken: () => null, subscribePushToken: () => () => {}, deviceInfo: () => ({ platform: "test", name: "test" }) }));
mock.module("../notifications/runnersPush", () => ({ runnersRegisterFields: () => ({}), subscribeRunnersPush: () => () => {} }));
mock.module("./server", () => ({ wsUrl: () => "ws://local-fixture.invalid/ws" }));

const { createMobileFeed } = await import("./serverFeed");

class Socket {
  static instances: Socket[] = [];
  readyState = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  sent: { type: string; [k: string]: unknown }[] = [];
  listeners: ((ev: { data: unknown }) => void)[] = [];
  dead = false;
  constructor() { Socket.instances.push(this); }
  addEventListener(_type: string, fn: (ev: { data: unknown }) => void) { this.listeners.push(fn); }
  send(data: string) { this.sent.push(JSON.parse(data)); }
  close() {
    this.readyState = this.dead ? 2 : 3;
    if (!this.dead) this.onclose?.({});
  }
  open() { this.readyState = 1; this.onopen?.({}); }
  receive(value: object) { if (!this.dead) for (const fn of this.listeners) fn({ data: JSON.stringify(value) }); }
}

const welcome = { type: "welcome", version: "test", commit: "test", heartbeatSecs: 15 };
const snapshot = (symbol: string): object => ({ type: "chart.snapshot", symbol, timeframe: "1m", session: null, candles: [{ t: 1, o: 1, h: 1, l: 1, c: 1, v: 100 }], gaps: [], profile: null, quality: { complete: true }, clock: null, halts: [], luld: [] });
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const realSocket = globalThis.WebSocket;
let cleanup: (() => void) | undefined;
afterEach(() => {
  cleanup?.();
  cleanup = undefined;
  globalThis.WebSocket = realSocket;
  Socket.instances = [];
});

function connect(timeouts: { probeTimeoutMs?: number; snapshotTimeoutMs?: number }) {
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  const feed = createMobileFeed(null, undefined, undefined, "mobile", "live", undefined, undefined, timeouts);
  cleanup = () => feed.close();
  const first = Socket.instances[0]!;
  first.open();
  first.receive(welcome);
  return { feed, first };
}

test("a chart opened on a connection that died in the background gets its candles", async () => {
  const { feed, first } = connect({ probeTimeoutMs: 60, snapshotTimeoutMs: 60 });
  first.dead = true;
  const got: ChartMessage[] = [];
  const off = feed.subscribe({ symbol: "ABCD", timeframe: "1m", profileBin: 0.01 }, (m) => got.push(m));
  await sleep(250);
  const second = Socket.instances[1];
  expect(second).toBeDefined();
  second!.open();
  second!.receive(welcome);
  expect(second!.sent).toContainEqual(expect.objectContaining({ type: "chart.subscribe", symbol: "ABCD" }));
  second!.receive(snapshot("ABCD"));
  expect(got.some((m) => m.type === "chart.snapshot")).toBe(true);
  off();
});

test("back in the foreground, a dead connection is replaced at once", async () => {
  const { feed, first } = connect({ probeTimeoutMs: 60, snapshotTimeoutMs: 60_000 });
  first.dead = true;
  feed.checkConnection(); // what useConnection does on AppState "active"
  await sleep(150);
  expect(Socket.instances[1]).toBeDefined();
});

test("a healthy connection survives the foreground check", async () => {
  const { feed, first } = connect({ probeTimeoutMs: 60, snapshotTimeoutMs: 60_000 });
  feed.checkConnection();
  const ping = first.sent.find((m) => m.type === "ping")!;
  first.receive({ type: "pong", ts: ping.ts });
  await sleep(150);
  expect(Socket.instances.length).toBe(1);
});
