import { afterEach, expect, mock, test } from "bun:test";

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
