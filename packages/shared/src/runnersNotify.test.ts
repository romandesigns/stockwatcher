import { describe, expect, test } from "bun:test";
import { parseServerMessage } from "./protocol.ts";
import { ALERT_SWITCH_SECTIONS, createNoticeDedupe, DEFAULT_RUNNERS_NOTIFY, parseRunnersNotice, RUNNERS_NOTICE_KINDS, runnersNotifyKey, runnersNotifyKinds, shouldShowNotice, wantsNotice, type RunnersNoticeMessage } from "./runnersNotify.ts";
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
  test("frozen candle observations survive parsing and do not create another notice", () => {
    const data = { ...frame.data, kind: "runners.pullback", mp5_wick: { rule: "mp5-wick-v1", status: "flagged", known_at: 32, upper_wick_milli: 600, close_location_milli: 400 } };
    const original = parseRunnersNotice({ ...frame, kind: "runners.pullback", data })!;
    expect(original.data?.mp5_wick).toEqual(data.mp5_wick);
    const seen = createNoticeDedupe();
    expect(shouldShowNotice(original, DEFAULT_RUNNERS_NOTIFY, seen)).toBe(true);
    const repeated = parseRunnersNotice({ ...frame, kind: "runners.pullback", data: { ...data, mp5_wick: { ...data.mp5_wick, status: "clear" } } })!;
    expect(shouldShowNotice(repeated, DEFAULT_RUNNERS_NOTIFY, seen)).toBe(false);
  });

  test("the defaults are the phone's: every evidence kind on; breakouts, at-trigger and the Halt watcher off", () => {
    expect(DEFAULT_RUNNERS_NOTIFY).toEqual({ new: true, breakout: false, burst: true, running: true, run: true, fast: true, shelf: true, followUp: true, rossCrossing: true, signalsCrossing: true, rossTrigger: false, signalsTrigger: false, halt: false, pullback: true });
    expect(runnersNotifyKinds(undefined)).toEqual({ ...DEFAULT_RUNNERS_NOTIFY });
    // A value stored before the new kinds existed: the new switches take their defaults.
    expect(runnersNotifyKinds({ new: false, breakout: true, burst: true, running: true, run: true, fast: true })).toEqual({ ...DEFAULT_RUNNERS_NOTIFY, new: false, breakout: true });
    expect(runnersNotifyKinds({ fast: false, breakout: true, junk: 1, run: "no" })).toEqual({ ...DEFAULT_RUNNERS_NOTIFY, fast: false, breakout: true });
  });

  test("every kind answers to its own switch", () => {
    expect(RUNNERS_NOTICE_KINDS.map((k) => [k, runnersNotifyKey(k)])).toEqual([
      ["runners.new", "new"],
      ["runners.breakout", "breakout"],
      ["runners.burst", "burst"],
      ["runners.running", "running"],
      ["runners.run", "run"],
      ["runners.fast", "fast"],
      ["runners.shelf", "shelf"],
      ["runners.followup", "followUp"],
      ["ross.crossing", "rossCrossing"],
      ["signals.crossing", "signalsCrossing"],
      ["ross.trigger", "rossTrigger"],
      ["signals.trigger", "signalsTrigger"],
      ["halt.watch", "halt"],
      ["runners.pullback", "pullback"],
    ]);
    for (const k of RUNNERS_NOTICE_KINDS) expect(runnersNotifyKey(k) in DEFAULT_RUNNERS_NOTIFY).toBe(true);
  });

  test("pullback setting up: on by default, its own switch, and a crossing that Signals lists too shows with that switch", () => {
    const setup = parseRunnersNotice({ ...frame, id: "runners-pullback-e9", kind: "runners.pullback", data: { symbol: "CJMB", kind: "runners.pullback", alert_id: "e9", stage: "setup" } })!;
    expect(setup.kind).toBe("runners.pullback");
    expect(wantsNotice(setup, DEFAULT_RUNNERS_NOTIFY)).toBe(true);
    const off = { ...DEFAULT_RUNNERS_NOTIFY, pullback: false };
    expect(wantsNotice(setup, off)).toBe(false);
    // A stored value from before the switch existed takes the default (on).
    expect(runnersNotifyKinds({ new: true, halt: true }).pullback).toBe(true);
    const crossing = parseRunnersNotice({ ...frame, id: "runners-pullback_crossing-e9", kind: "runners.pullback", data: { symbol: "CJMB", kind: "runners.pullback", alert_id: "e9", stage: "crossing", also_signals: true } })!;
    expect(wantsNotice(crossing, off)).toBe(true);
    expect(wantsNotice(crossing, { ...off, signalsCrossing: false })).toBe(false);
    // The switch is listed under Runners on every client.
    expect(ALERT_SWITCH_SECTIONS.find((s) => s.id === "runners")!.items.some((i) => i.key === "pullback")).toBe(true);
  });

  test("a Ross notice for a stock in Signals too shows with either switch, once", () => {
    const off = { ...DEFAULT_RUNNERS_NOTIFY, rossCrossing: false, rossTrigger: false };
    const cross = parseRunnersNotice({ ...frame, id: "ross.crossing-e1", kind: "ross.crossing", data: { symbol: "CJMB", kind: "ross.crossing", alert_id: "e1", also_signals: true } })!;
    expect(cross.alsoSignals).toBe(true);
    expect(parseRunnersNotice({ ...frame, kind: "ross.trigger", alsoSignals: true })!.alsoSignals).toBe(true);
    expect(wantsNotice(cross, off)).toBe(true);
    expect(wantsNotice(cross, { ...off, signalsCrossing: false })).toBe(false);
    expect(wantsNotice({ kind: "ross.crossing" }, off)).toBe(false);
    expect(wantsNotice({ kind: "ross.trigger", alsoSignals: true }, { ...off, signalsTrigger: true })).toBe(true);
    expect(wantsNotice({ kind: "ross.trigger", alsoSignals: true }, off)).toBe(false);
    // Opt-in kinds are off by default.
    expect(wantsNotice({ kind: "halt.watch" }, DEFAULT_RUNNERS_NOTIFY)).toBe(false);
    expect(wantsNotice({ kind: "runners.shelf" }, DEFAULT_RUNNERS_NOTIFY)).toBe(true);
    const d = createNoticeDedupe();
    expect(shouldShowNotice(cross, off, d)).toBe(true);
    expect(shouldShowNotice(cross, DEFAULT_RUNNERS_NOTIFY, d)).toBe(false);
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
