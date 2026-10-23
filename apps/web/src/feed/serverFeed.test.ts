import { afterEach, expect, test } from "bun:test";
import type { PressureMessage } from "@stockwatcher/shared";
import { createServerFeed } from "./serverFeed.ts";

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

test("web pressure stays last-known through reconnect until the replacement frame arrives", async () => {
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  const feed = createServerFeed({ url: "ws://local-fixture.invalid/ws", source: "live" });
  cleanup = () => feed.close();
  const pressure = feed.pressure!;
  const off = pressure.subscribe(() => {});
  const socket = Socket.instances[0]!;
  const welcome = { type: "welcome", version: "test", commit: "test", heartbeatSecs: 15 };
  socket.open(); socket.receive(welcome);
  expect(pressure.status()).toEqual({ connected: false, asOf: null });
  const frame: PressureMessage = { type: "pressure.snapshot", t: 1234, board: [{ s: "TEST", p: 2, o: 2, risk: 75, side: "bull", rise: 40, riseSecs: 30, band: 2.1, toBand: 5, pressure: 80, buyShare: 0.8, pace: 4 }] };
  socket.receive(frame);
  const board = pressure.get();
  expect(pressure.status()).toEqual({ connected: true, asOf: 1234 });
  socket.close();
  expect(pressure.get()).toBe(board);
  expect(pressure.status()).toEqual({ connected: false, asOf: 1234 });
  await new Promise((resolve) => setTimeout(resolve, 550));
  const reopened = Socket.instances[1]!;
  expect(reopened).toBeDefined();
  reopened.open(); reopened.receive(welcome);
  expect(reopened.sent).toContainEqual({ type: "pressure.subscribe" });
  expect(pressure.status().connected).toBe(false);
  socket.receive({ ...frame, t: 9999 });
  expect(pressure.status()).toEqual({ connected: false, asOf: 1234 });
  reopened.receive({ ...frame, t: 2234, board: [] });
  expect(pressure.status()).toEqual({ connected: true, asOf: 2234 });
  expect(pressure.get()).toEqual([]);
  off();
  expect(pressure.status()).toEqual({ connected: false, asOf: null });
});
