import { afterEach, expect, test } from "bun:test";
import type { ChartMessage } from "@stockwatcher/chart-core";
import { createServerFeed } from "./serverFeed.ts";

// The empty chart (owner, 2026-11-15: "sometimes the chart opens empty; closing and reopening it
// fixes it", web and phone, no pattern). The connection had died without the socket noticing
// (laptop asleep, wifi or cellular handover, a phone back from the background): readyState stays
// OPEN, `send` succeeds, nothing comes back. The chart's `chart.subscribe` goes into that socket
// and is lost; the only thing that ever noticed was the liveness timer, 2.5 heartbeats = 37.5 s
// later (and on the phone that JS timer is paused while the app is in the background, so it
// counts from the resume). Reopening the chart "fixed" it only because by then the timer had
// reconnected. These tests replay that: a socket that went silent, then a chart opened on it.

class Socket {
  static instances: Socket[] = [];
  readyState = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  sent: { type: string; [k: string]: unknown }[] = [];
  listeners: ((ev: { data: unknown }) => void)[] = [];
  /** A half-open TCP connection: sends "succeed", nothing arrives, and close never completes. */
  dead = false;
  constructor() { Socket.instances.push(this); }
  addEventListener(_type: string, fn: (ev: { data: unknown }) => void) { this.listeners.push(fn); }
  send(data: string) { this.sent.push(JSON.parse(data)); }
  close() {
    // A browser socket to a dead peer sits in CLOSING until TCP gives up: no close event soon.
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

function connect(opts: { probeTimeoutMs?: number; snapshotTimeoutMs?: number } = {}) {
  globalThis.WebSocket = Socket as unknown as typeof WebSocket;
  const feed = createServerFeed({ url: "ws://local-fixture.invalid/ws", source: "live", ...opts });
  cleanup = () => feed.close();
  const first = Socket.instances[0]!;
  first.open();
  first.receive(welcome);
  return { feed, first };
}

test("a chart opened on a silently dead connection gets its candles without being reopened", async () => {
  const { feed, first } = connect({ probeTimeoutMs: 60, snapshotTimeoutMs: 60 });
  first.dead = true; // the connection died; the socket still says OPEN
  const got: ChartMessage[] = [];
  const off = feed.subscribe({ symbol: "ABCD", timeframe: "1m", profileBin: 0.01 }, (m) => got.push(m));
  expect(first.sent.some((m) => m.type === "chart.subscribe")).toBe(true); // lost in the dead socket

  // No snapshot came: the feed checks the connection, finds it dead, and reconnects at once
  // (not after the 37.5 s liveness timeout, and without waiting for a close that never comes).
  await sleep(250);
  const second = Socket.instances[1];
  expect(second).toBeDefined();
  second!.open();
  second!.receive(welcome);
  // The reconnect resubscribes the open chart, and its snapshot arrives.
  expect(second!.sent).toContainEqual(expect.objectContaining({ type: "chart.subscribe", symbol: "ABCD" }));
  second!.receive(snapshot("ABCD"));
  expect(got.some((m) => m.type === "chart.snapshot")).toBe(true);
  off();
});

test("coming back to the page checks the connection and recovers a dead one", async () => {
  const { feed, first } = connect({ probeTimeoutMs: 60, snapshotTimeoutMs: 60_000 });
  first.dead = true;
  feed.checkConnection!();
  expect(first.sent).toContainEqual(expect.objectContaining({ type: "ping" }));
  await sleep(150);
  expect(Socket.instances[1]).toBeDefined();
});

test("a live connection answers the check and is kept", async () => {
  const { feed, first } = connect({ probeTimeoutMs: 60, snapshotTimeoutMs: 60_000 });
  feed.checkConnection!();
  const ping = first.sent.find((m) => m.type === "ping")!;
  first.receive({ type: "pong", ts: ping.ts });
  await sleep(150);
  expect(Socket.instances.length).toBe(1);
  expect(first.readyState).toBe(1);
});

test("a chart whose snapshot arrives in time causes no check", async () => {
  const { feed, first } = connect({ probeTimeoutMs: 60, snapshotTimeoutMs: 60 });
  const off = feed.subscribe({ symbol: "ABCD", timeframe: "1m", profileBin: 0.01 }, () => {});
  first.receive(snapshot("ABCD"));
  await sleep(200);
  expect(first.sent.some((m) => m.type === "ping")).toBe(false);
  expect(Socket.instances.length).toBe(1);
  off();
});
