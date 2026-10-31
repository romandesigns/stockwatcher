import { describe, expect, test } from "bun:test";
import { applySymbolsMessage, createSymbolsChannel, EMPTY_SYMBOLS, SYMBOLS_FEATURE, type SymbolsMessage } from "./symbols.ts";
import { createWsClient, type WebSocketLike } from "./ws.ts";

const meta = { mode: "live", status: "running", source: "alpaca", day: "2026-10-30", fixture: false, priceBand: { min: "0.10", max: "25.00" }, screened: { total: 3, candidates: 2 }, session: null };
const snapshot = (rev: number, symbols: string[]): SymbolsMessage => ({ type: "symbols.snapshot", rev, symbols, ...meta });
const update = (rev: number, added: string[], removed: string[], extra = {}): SymbolsMessage => ({ type: "symbols.update", rev, added, removed, ...meta, ...extra });

describe("applySymbolsMessage", () => {
  test("a snapshot replaces; an update on rev - 1 applies, sorted, with the new meta", () => {
    let s = applySymbolsMessage(EMPTY_SYMBOLS, snapshot(4, ["BBB", "AAA"])).state;
    expect(s.latest?.symbols).toEqual(["AAA", "BBB"]);
    expect(s.latest?.mode).toBe("live");
    expect((s.latest as unknown as Record<string, unknown>).type).toBeUndefined();
    const r = applySymbolsMessage(s, update(5, ["CCC", "AAB"], ["BBB"], { screened: { total: 4, candidates: 3 } }));
    expect(r.resync).toBe(false);
    s = r.state;
    expect(s.latest?.symbols).toEqual(["AAA", "AAB", "CCC"]);
    expect(s.latest?.screened).toEqual({ total: 4, candidates: 3 });
    expect(s.rev).toBe(5);
  });

  test("a missed update asks for a fresh snapshot and changes nothing", () => {
    const s = applySymbolsMessage(EMPTY_SYMBOLS, snapshot(4, ["AAA"])).state;
    const r = applySymbolsMessage(s, update(6, ["BBB"], []));
    expect(r.resync).toBe(true);
    expect(r.state).toBe(s);
    expect(applySymbolsMessage(EMPTY_SYMBOLS, update(1, ["AAA"], [])).resync).toBe(true);
  });
});

describe("createSymbolsChannel", () => {
  test("opts in on the first listener, out on the last; resubscribes on a gap; drops the list on disconnect", () => {
    const calls: string[] = [];
    const ch = createSymbolsChannel({ subscribe: () => calls.push("sub"), unsubscribe: () => calls.push("unsub") });
    expect(ch.get().available).toBeNull();
    ch.setAvailable(true);
    const off = ch.subscribe(() => {});
    expect(calls).toEqual(["sub"]);
    ch.apply(snapshot(1, ["AAA"]));
    ch.apply(update(3, ["BBB"], []));
    expect(calls).toEqual(["sub", "sub"]);
    expect(ch.get().latest?.symbols).toEqual(["AAA"]);
    ch.setConnected(false);
    expect(ch.get().latest).toBeNull();
    expect(ch.get().available).toBe(true);
    off();
    expect(calls.at(-1)).toBe("unsub");
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

describe("ws client symbols opt-in", () => {
  const welcome = { type: "welcome", version: "0.1.0", commit: "abc", heartbeatSecs: 15 };
  const types = (s: FakeSocket) => s.sent.map((x) => JSON.parse(x).type as string);

  test("an older server (no symbols.v1) never gets symbols.subscribe; a newer one gets it on welcome", () => {
    const sockets: FakeSocket[] = [];
    const got: string[] = [];
    const client = createWsClient({ url: "ws://x/ws", createSocket: () => (sockets.push(new FakeSocket()), sockets.at(-1)!), onSymbols: (m) => got.push(m.type) });
    client.subscribeSymbols();
    const s = sockets[0]!;
    s.open();
    s.receive(welcome);
    expect(types(s)).not.toContain("symbols.subscribe");
    client.close();

    const newer: FakeSocket[] = [];
    const c2 = createWsClient({ url: "ws://x/ws", createSocket: () => (newer.push(new FakeSocket()), newer.at(-1)!), onSymbols: (m) => got.push(m.type) });
    c2.subscribeSymbols();
    const n = newer[0]!;
    n.open();
    n.receive({ ...welcome, features: [SYMBOLS_FEATURE] });
    expect(types(n).at(-1)).toBe("symbols.subscribe");
    n.receive(snapshot(1, ["AAA"]));
    expect(got).toEqual(["symbols.snapshot"]);
    c2.unsubscribeSymbols();
    expect(types(n).at(-1)).toBe("symbols.unsubscribe");
    c2.close();
  });
});
