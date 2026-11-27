import { expect, test } from "bun:test";
import { createWsClient, type WebSocketLike } from "./ws.ts";

// `checkConnection` (the empty chart, 2026-11-15): see ws.ts.

class Socket implements WebSocketLike {
  static instances: Socket[] = [];
  readyState = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  sent: { type: string; [k: string]: unknown }[] = [];
  dead = false;
  closedWith: number | null = null;
  constructor() { Socket.instances.push(this); }
  send(data: string) { this.sent.push(JSON.parse(data)); }
  close(code?: number) {
    this.closedWith = code ?? null;
    this.readyState = this.dead ? 2 : 3;
    if (!this.dead) this.onclose?.({});
  }
  open() { this.readyState = 1; this.onopen?.({}); }
  receive(v: object) { if (!this.dead) this.onmessage?.({ data: JSON.stringify(v) }); }
}

const welcome = { type: "welcome", version: "t", commit: "t", heartbeatSecs: 15 };
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function client(opts: Partial<Parameters<typeof createWsClient>[0]> = {}) {
  Socket.instances = [];
  const statuses: string[] = [];
  const c = createWsClient({ url: "ws://x.invalid/ws", createSocket: () => new Socket(), onStatus: (s) => statuses.push(s), probeTimeoutMs: 40, ...opts });
  return { c, statuses };
}

test("a dead connection is dropped and replaced without waiting for its close event", async () => {
  const { c, statuses } = client();
  const a = Socket.instances[0]!;
  a.open();
  a.receive(welcome);
  a.dead = true;
  c.checkConnection();
  expect(a.sent.at(-1)?.type).toBe("ping");
  await sleep(80);
  expect(a.closedWith).toBe(4001);
  const b = Socket.instances[1]!;
  expect(b).toBeDefined();
  b.open();
  b.receive(welcome);
  expect(statuses.at(-1)).toBe("open");
  // The abandoned socket's late close (TCP finally giving up) changes nothing.
  a.dead = false;
  a.onclose?.({});
  expect(Socket.instances.length).toBe(2);
  expect(statuses.at(-1)).toBe("open");
  c.close();
});

test("any frame answers the check", async () => {
  const { c } = client();
  const a = Socket.instances[0]!;
  a.open();
  a.receive(welcome);
  c.checkConnection();
  a.receive({ type: "ping", ts: 1 }); // a heartbeat counts as much as the pong
  await sleep(80);
  expect(Socket.instances.length).toBe(1);
  expect(a.closedWith).toBeNull();
  c.close();
});

test("between retries the check reconnects now instead of waiting out the backoff", async () => {
  const { c } = client({ baseDelayMs: 60_000, maxDelayMs: 60_000, random: () => 1 });
  const a = Socket.instances[0]!;
  a.close(); // the first attempt failed: the next one is a minute away
  expect(Socket.instances.length).toBe(1);
  c.checkConnection();
  expect(Socket.instances.length).toBe(2);
  c.close();
});

test("repeated checks while one waits send a single ping", () => {
  const { c } = client();
  const a = Socket.instances[0]!;
  a.open();
  a.receive(welcome);
  c.checkConnection();
  c.checkConnection();
  expect(a.sent.filter((m) => m.type === "ping").length).toBe(1);
  c.close();
});

test("a check whose timer fires late (a busy page) waits for the frames queued behind it (the Practice-page freeze)", async () => {
  const { c } = client();
  const a = Socket.instances[0]!;
  a.open();
  a.receive(welcome);
  c.checkConnection();
  // The answer is already there, queued behind a long task that outlasts the check's 40 ms.
  setTimeout(() => a.receive({ type: "pong", ts: 1 }), 60);
  const until = Date.now() + 1_200;
  while (Date.now() < until) {
    // a long task
  }
  await sleep(150);
  expect(a.closedWith).toBeNull();
  expect(Socket.instances.length).toBe(1);
  c.close();
});

test("a late check still gives up a connection that really is silent", async () => {
  const { c } = client();
  const a = Socket.instances[0]!;
  a.open();
  a.receive(welcome);
  a.dead = true;
  c.checkConnection();
  const until = Date.now() + 1_200;
  while (Date.now() < until) {
    // a long task
  }
  await sleep(150);
  expect(a.closedWith).toBe(4001);
  expect(Socket.instances.length).toBe(2);
  c.close();
});
