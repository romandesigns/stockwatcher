import { describe, expect, test } from "bun:test";
import { parseServerMessage } from "./protocol.ts";
import { createNoticeDedupe, DEFAULT_RUNNERS_NOTIFY, parseRunnersNotice, runnersNotifyKinds, shouldShowNotice, type RunnersNoticeMessage } from "./runnersNotify.ts";
import { createWsClient, type WebSocketLike } from "./ws.ts";

// The frame the server sends (crates/server/src/alerts/runners.rs desk_frame test).
const frame = {
  type: "runners.notice",
  id: "runners-fast-2026-10-23-CJMB-7",
  alertId: "2026-10-23-CJMB-7",
  symbol: "CJMB",
  kind: "runners.fast",
  title: "CJMB · Fast move · $2.41",
  body: "+8.4% in 60 s · 52 trades, $71k",
  data: { symbol: "CJMB", kind: "runners.fast", alert_id: "2026-10-23-CJMB-7" },
  at: 1_761_200_000_123,
};

describe("runners notices", () => {
  test("the defaults are the phone's: everything on but breakouts", () => {
    expect(DEFAULT_RUNNERS_NOTIFY).toEqual({ new: true, breakout: false, burst: true, running: true, run: true, fast: true });
    expect(runnersNotifyKinds(undefined)).toEqual({ ...DEFAULT_RUNNERS_NOTIFY });
    expect(runnersNotifyKinds({ fast: false, breakout: true, junk: 1, run: "no" })).toEqual({ ...DEFAULT_RUNNERS_NOTIFY, fast: false, breakout: true });
  });

  test("the server's text is taken as is; malformed frames are dropped", () => {
    const m = parseServerMessage(JSON.stringify(frame));
    expect(m).toEqual(frame as never);
    expect(parseRunnersNotice({ ...frame, symbol: "not a ticker" })).toBeNull();
    expect(parseRunnersNotice({ ...frame, kind: "runners.other" })).toBeNull();
    expect(parseRunnersNotice({ ...frame, title: "" })).toBeNull();
    expect(parseRunnersNotice({ ...frame, id: undefined })).toBeNull();
  });

  test("each id shows once, per the kind's switch, across a reload sharing storage", () => {
    let stored: string[] = [];
    const store = { load: () => [...stored], save: (ids: string[]) => void (stored = ids) };
    const a = createNoticeDedupe(store);
    const n = frame as RunnersNoticeMessage;
    expect(shouldShowNotice(n, DEFAULT_RUNNERS_NOTIFY, a)).toBe(true);
    // A reconnect replaying it, then a reload (a new dedupe on the same storage).
    expect(shouldShowNotice(n, DEFAULT_RUNNERS_NOTIFY, a)).toBe(false);
    expect(shouldShowNotice(n, DEFAULT_RUNNERS_NOTIFY, createNoticeDedupe(store))).toBe(false);
    // Switched off: not shown (and not claimed, so turning it back on could still show it).
    const other = { ...n, id: "runners-fast-x" };
    expect(shouldShowNotice(other, { ...DEFAULT_RUNNERS_NOTIFY, fast: false }, a)).toBe(false);
    expect(shouldShowNotice({ ...n, id: "runners-breakout-y", kind: "runners.breakout" }, DEFAULT_RUNNERS_NOTIFY, a)).toBe(false);
    // Storage gone (private mode): the session still remembers.
    const broken = createNoticeDedupe({
      load: () => {
        throw new Error("denied");
      },
    });
    expect(broken.claim("z")).toBe(true);
    expect(broken.claim("z")).toBe(false);
    // Bounded.
    const small = createNoticeDedupe({ max: 2, ...store });
    stored = [];
    small.claim("1");
    small.claim("2");
    small.claim("3");
    expect(stored).toEqual(["2", "3"]);
  });

  test("the client sends its opt-in on every welcome and routes notices", () => {
    const sockets: Fake[] = [];
    let wanted: { type: "runners.notify"; new?: boolean } | null = { type: "runners.notify", new: false };
    const got: string[] = [];
    const client = createWsClient({ url: "ws://x/ws", baseDelayMs: 1, random: () => 0, createSocket: () => sockets[sockets.push(new Fake()) - 1]!, runnersNotify: () => wanted, onRunnersNotice: (m) => got.push(m.id) });
    const s = sockets[0]!;
    s.open();
    s.receive({ type: "welcome", version: "0", commit: "x", heartbeatSecs: 15 });
    expect(s.sent.map((x) => JSON.parse(x))).toContainEqual({ type: "runners.notify", new: false });
    s.receive(frame);
    expect(got).toEqual([frame.id]);
    wanted = null;
    client.refreshRunnersNotify();
    expect(JSON.parse(s.sent.at(-1)!)).toEqual({ type: "runners.notify", enabled: false });
    client.close();
  });
});

class Fake implements WebSocketLike {
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
