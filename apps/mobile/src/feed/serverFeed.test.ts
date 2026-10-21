import { afterEach, expect, mock, test } from "bun:test";
import type { DetectionMessage } from "@stockwatcher/shared";

// The routing test exercises the real shared WS client/channels without importing native
// notification modules or opening an external socket.
mock.module("../notifications/push", () => ({ pushToken: () => null, deviceInfo: () => ({ platform: "test", name: "test" }) }));
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
