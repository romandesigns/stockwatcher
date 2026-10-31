import { describe, expect, test } from "bun:test";
import { applyNoticesMessage, createNoticesChannel, NOTICE_LOG_MAX_ITEMS, NOTICES_FEATURE } from "./noticeFeed.ts";
import { createWsClient, type WebSocketLike } from "./ws.ts";

const item = (id: string, at: number, extra: Record<string, unknown> = {}) => ({ event_id: id, at, symbol: "AAA", notice: "runners.burst", ...extra });
const snapshot = (items: unknown[], day = "2026-10-30") => ({ type: "notices.snapshot" as const, day, days: [day], items, truncated: false });
const append = (items: unknown[], day = "2026-10-30") => ({ type: "notices.append" as const, day, items });

describe("applyNoticesMessage", () => {
  test("a snapshot replaces; an append upserts by event_id, new items newest first by `at`", () => {
    let log = applyNoticesMessage(null, snapshot([item("b", 200), item("a", 100)]));
    expect(log?.items.map((i) => i.event_id)).toEqual(["b", "a"]);
    // The phone decision for "a" arrives: replaced in place, not duplicated.
    log = applyNoticesMessage(log, append([item("a", 100, { phone: { decision: "held", held: "quiet_hours", devices: 1 } })]));
    expect(log?.items.map((i) => i.event_id)).toEqual(["b", "a"]);
    expect(log?.items[1]?.phone?.held).toBe("quiet_hours");
    log = applyNoticesMessage(log, append([item("c", 300)]));
    log = applyNoticesMessage(log, append([item("m", 150)]));
    expect(log?.items.map((i) => i.event_id)).toEqual(["c", "b", "m", "a"]);
  });

  test("an append for another day, or before any snapshot, changes nothing", () => {
    const log = applyNoticesMessage(null, snapshot([item("a", 1)]));
    expect(applyNoticesMessage(log, append([item("z", 9)], "2026-10-31"))).toBe(log);
    expect(applyNoticesMessage(null, append([item("z", 9)]))).toBeNull();
  });

  test("the day stays capped like the server's", () => {
    const many = Array.from({ length: NOTICE_LOG_MAX_ITEMS }, (_, i) => item(`e${i}`, NOTICE_LOG_MAX_ITEMS - i));
    const log = applyNoticesMessage(applyNoticesMessage(null, snapshot(many)), append([item("new", 1e9)]));
    expect(log?.items.length).toBe(NOTICE_LOG_MAX_ITEMS);
    expect(log?.items[0]?.event_id).toBe("new");
    expect(log?.truncated).toBe(true);
  });
});

describe("createNoticesChannel", () => {
  test("opts in and out with its listeners and drops the log on disconnect", () => {
    const calls: string[] = [];
    const ch = createNoticesChannel({ subscribe: () => calls.push("sub"), unsubscribe: () => calls.push("unsub") });
    ch.setAvailable(true);
    const off = ch.subscribe(() => {});
    ch.apply(snapshot([item("a", 1)]));
    expect(ch.get().log?.items.length).toBe(1);
    ch.setConnected(false);
    expect(ch.get().log).toBeNull();
    off();
    expect(calls).toEqual(["sub", "unsub"]);
    expect(ch.get().available).toBe(true);
  });
});

class FakeSocket implements WebSocketLike {
  readyState = 0;
  sent: string[] = [];
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.readyState = 3;
    this.onclose?.({});
  }
  open() {
    this.readyState = 1;
    this.onopen?.({});
  }
  receive(msg: object) {
    this.onmessage?.({ data: JSON.stringify(msg) });
  }
}

test("the ws client asks for the log only from a server with notices.v1", () => {
  const welcome = { type: "welcome", version: "0.1.0", commit: "abc", heartbeatSecs: 15 };
  for (const [features, expected] of [[[], false], [[NOTICES_FEATURE], true]] as const) {
    const sockets: FakeSocket[] = [];
    const got: string[] = [];
    const client = createWsClient({ url: "ws://x/ws", createSocket: () => (sockets.push(new FakeSocket()), sockets.at(-1)!), onNotices: (m) => got.push(m.type) });
    client.subscribeNotices();
    const s = sockets[0]!;
    s.open();
    s.receive({ ...welcome, features });
    expect(s.sent.some((x) => JSON.parse(x).type === "notices.subscribe")).toBe(expected);
    s.receive(snapshot([]));
    expect(got).toEqual(["notices.snapshot"]);
    client.close();
  }
});
