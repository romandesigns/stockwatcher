import { describe, expect, test } from "bun:test";
import { createPressureChannel, gaugeCaption, gaugeGeometry, gaugeTone, haltLine, haltSentence, LULD_PAUSE_MS, pressureBoardFrom, pressurePhrase } from "./pressure.ts";
import { parseServerMessage, type PressureMessage } from "./protocol.ts";
import { createWsClient, type WebSocketLike } from "./ws.ts";

const update: PressureMessage = {
  type: "pressure.update",
  t: 1_792_075_200_000,
  board: [
    { s: "SAFX", p: 0.2881, pc: 0.1403, o: 0.15, pressure: 91, side: "bull", buyShare: 0.84, pace: 9.2 },
    { s: "ZTG", p: 0.7639, o: 0.623, pressure: 88, side: "bull", buyShare: 0.81, pace: 7.7, halt: { start: 1_792_075_200_000, reason: "LUDP" } },
    { s: "OPENW", p: 0.1196, pc: 0.1364, o: 0.13, pressure: 74, side: "bear", buyShare: 0.21, pace: 4.9 },
  ],
};

describe("pressure board", () => {
  test("cards keep the server's order and take the quote board's change rule", () => {
    const b = pressureBoardFrom(update);
    expect(b.map((c) => c.symbol)).toEqual(["SAFX", "ZTG", "OPENW"]);
    expect(b[0]!.changePct).toBeCloseTo(105.35, 2); // against the previous close
    expect(b[1]!.changePct).toBeCloseTo(22.62, 2); // no previous close: the open
    expect(b[1]!.halt?.reason).toBe("LUDP");
    expect(b[2]!.side).toBe("bear");
    expect("halt" in b[0]!).toBe(false);
  });

  test("parseServerMessage knows the pressure frames", () => {
    expect(parseServerMessage(JSON.stringify(update))?.type).toBe("pressure.update");
    expect(parseServerMessage(JSON.stringify({ ...update, type: "pressure.snapshot" }))?.type).toBe("pressure.snapshot");
  });

  test("the channel opts in with the first listener and out (emptied) with the last", () => {
    const calls: string[] = [];
    const ch = createPressureChannel({ subscribe: () => calls.push("sub"), unsubscribe: () => calls.push("unsub") });
    let seen = 0;
    const a = ch.subscribe(() => seen++);
    const b = ch.subscribe(() => seen++);
    ch.apply(update);
    expect(seen).toBe(2);
    expect(ch.get().length).toBe(3);
    a();
    expect(calls).toEqual(["sub"]);
    b();
    expect(calls).toEqual(["sub", "unsub"]);
    expect(ch.get().length).toBe(0);
  });
});

describe("halt lines", () => {
  const hhmm = (ms: number) => new Date(ms).toISOString().slice(11, 16);
  const start = Date.UTC(2026, 9, 16, 14, 42);
  test("a LULD pause shows its expected reopening, approximate", () => {
    expect(haltLine({ start, reason: "LUDP" }, hhmm)).toBe("Halted ~14:47");
    expect(start + LULD_PAUSE_MS).toBe(Date.UTC(2026, 9, 16, 14, 47));
    expect(haltSentence({ start, reason: "LUDP" }, hhmm)).toBe("halted at 14:42 by a LULD pause, reopening about 14:47");
  });
  test("any other halt shows when it started (it has no expected end)", () => {
    expect(haltLine({ start, reason: "T1" }, hhmm)).toBe("Halted 14:42");
    expect(haltSentence({ start, reason: "T1" }, hhmm)).toBe("halted at 14:42 (T1)");
  });
});

describe("gauge", () => {
  test("zero draws no fill; the track is the top half ring", () => {
    const g = gaugeGeometry(0);
    expect(g.value).toBeNull();
    expect(g.label).toBe(0);
    expect(g.track).toBe("M 26.00 100.00 A 74 74 0 0 1 174.00 100.00");
  });
  test("half ends at the top, full is the whole track, out of range clamps", () => {
    expect(gaugeGeometry(50).value).toMatch(/ 100\.00 26\.00$/);
    const full = gaugeGeometry(140);
    expect(full.label).toBe(100);
    expect(full.value).toBe(full.track);
    expect(gaugeGeometry(Number.NaN).label).toBe(0);
  });
  test("the leading side takes the colour; halted is neutral", () => {
    expect(gaugeTone("bull", false)).toBe("good");
    expect(gaugeTone("bear", false)).toBe("critical");
    expect(gaugeTone("bear", true)).toBe("halted");
    expect(gaugeCaption("bear", false)).toBe("sell pressure");
    expect(gaugeCaption("bull", false)).toBe("buy pressure");
    expect(pressurePhrase({ side: "bear", pressure: 74.4 })).toBe("selling pressure 74 of 100");
  });
});

class FakeSocket implements WebSocketLike {
  static last: FakeSocket | null = null;
  readyState = 0;
  sent: string[] = [];
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  constructor() {
    FakeSocket.last = this;
  }
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.readyState = 3;
    this.onclose?.({});
  }
}

test("the ws client forwards pressure frames only while subscribed, and resubscribes after a reconnect", async () => {
  const got: string[] = [];
  const client = createWsClient({ url: "ws://x/ws", createSocket: () => new FakeSocket(), onPressure: (m) => got.push(m.type), baseDelayMs: 1, maxDelayMs: 1, random: () => 0 });
  const open = (s: FakeSocket) => {
    s.readyState = 1;
    s.onopen?.({});
    s.onmessage?.({ data: JSON.stringify({ type: "welcome", version: "0", commit: "x", heartbeatSecs: 15 }) });
  };
  const first = FakeSocket.last!;
  open(first);
  first.onmessage?.({ data: JSON.stringify(update) });
  expect(got).toEqual([]);
  client.subscribePressure();
  expect(first.sent.at(-1)).toBe(JSON.stringify({ type: "pressure.subscribe" }));
  first.onmessage?.({ data: JSON.stringify(update) });
  expect(got).toEqual(["pressure.update"]);
  first.close();
  await new Promise((r) => setTimeout(r, 10));
  const second = FakeSocket.last!;
  expect(second).not.toBe(first);
  open(second);
  expect(second.sent).toContain(JSON.stringify({ type: "pressure.subscribe" }));
  client.unsubscribePressure();
  second.onmessage?.({ data: JSON.stringify(update) });
  expect(got).toEqual(["pressure.update"]);
  client.close();
});
