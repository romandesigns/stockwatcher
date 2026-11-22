import { describe, expect, test } from "bun:test";
import { parseServerMessage } from "./protocol.ts";
import { ALERT_SWITCH_SECTIONS, createNoticeDedupe, isHaltNotice, DEFAULT_RUNNERS_NOTIFY, parseRunnersNotice, RUNNERS_NOTICE_KINDS, runnersNotifyKey, runnersNotifyKinds, shouldShowNotice, wantsNotice, type RunnersNoticeMessage } from "./runnersNotify.ts";
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
  test("every notice is its own: no merge fields survive parsing (every distinct event is delivered)", () => {
    const m = parseRunnersNotice({ type: "runners.notice", id: "runners-fast-WFF-1", alertId: "WFF-1", symbol: "WFF", kind: "runners.fast", title: "WFF · Fast move · $0.1057", body: "b", at: 1, mergeInto: "runners-running-WFF-1", mergeLine: "+ Fast move 1 s later · $0.1057", mergeOnly: true })!;
    expect(m.id).toBe("runners-fast-WFF-1");
    expect("mergeInto" in m || "mergeOnly" in m || "mergeLine" in m).toBe(false);
  });

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
    expect(DEFAULT_RUNNERS_NOTIFY).toEqual({ new: true, breakout: false, burst: true, running: true, run: true, fast: true, shelf: true, followUp: true, rossCrossing: true, signalsCrossing: true, rossTrigger: false, signalsTrigger: false, halt: false, pullback: true, halted: true, resumed: true, n1: true, f1u: true, haltBull: true, resumedUp: true, xFloat: true, xGap: true, xCtx: true, modelPick: true, vwapReclaim: true, levelBreak: true });
    expect(runnersNotifyKinds(undefined)).toEqual({ ...DEFAULT_RUNNERS_NOTIFY });
    // A value stored before the new kinds existed: the new switches take their defaults.
    expect(runnersNotifyKinds({ new: false, breakout: true, burst: true, running: true, run: true, fast: true })).toEqual({ ...DEFAULT_RUNNERS_NOTIFY, new: false, breakout: true });
    expect(runnersNotifyKinds({ fast: false, breakout: true, junk: 1, run: "no" })).toEqual({ ...DEFAULT_RUNNERS_NOTIFY, fast: false, breakout: true });
  });

  test("exchange halts: their own kinds and switches, on by default, parsed and deduped like the rest", () => {
    expect(isHaltNotice("halt.halted") && isHaltNotice("halt.resumed")).toBe(true);
    expect(isHaltNotice("halt.watch")).toBe(false);
    const raw = { type: "runners.notice", id: "runners-halted-XLAB-halt-1763300000", alertId: "XLAB-halt-1763300000", symbol: "XLAB", kind: "halt.halted", title: "XLAB · Halted · $2.76", body: "LULD pause · +42.3% on the day", data: { symbol: "XLAB", kind: "halt.halted", sound: "siren" }, at: 1 };
    const n = parseRunnersNotice(raw)!;
    expect(n.kind).toBe("halt.halted");
    expect(n.data?.sound).toBe("siren");
    const d = createNoticeDedupe();
    expect(shouldShowNotice(n, DEFAULT_RUNNERS_NOTIFY, d)).toBe(true);
    expect(shouldShowNotice(n, DEFAULT_RUNNERS_NOTIFY, d)).toBe(false);
    expect(shouldShowNotice({ ...n, id: "other" }, { ...DEFAULT_RUNNERS_NOTIFY, halted: false }, d)).toBe(false);
    expect(ALERT_SWITCH_SECTIONS.find((s) => s.id === "halts")!.items.map((i) => i.key)).toEqual(["halted", "resumed"]);
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
      ["halt.halted", "halted"],
      ["halt.resumed", "resumed"],
      ["shadow.n1", "n1"],
      ["shadow.f1u", "f1u"],
      ["halt.bull", "haltBull"],
      ["halt.resumed_up", "resumedUp"],
      ["v2.xfloat", "xFloat"],
      ["v2.xgap", "xGap"],
      ["v2.xctx", "xCtx"],
      ["v2.vwapr", "vwapReclaim"],
      ["v2.level", "levelBreak"],
      ["model.pick", "modelPick"],
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

describe("N1 shadow", () => {
  test("its own switch, on by default (T2-c6 confirmed), in the Shadow section; a stored value from before it takes the default", () => {
    const n = parseRunnersNotice({ type: "runners.notice", id: "runners-n1-2026-11-18-ABCD-n1-1795012262", alertId: "2026-11-18-ABCD-n1-1795012262", symbol: "ABCD", kind: "shadow.n1", title: "ABCD · N1 · shadow · $2.12", body: "+6.4% off the 3-min low $2.00 · $112K in 3 min · research shadow", data: { symbol: "ABCD", kind: "shadow.n1", shadow: true }, at: 1 })!;
    expect(n.kind).toBe("shadow.n1");
    expect(runnersNotifyKey(n.kind)).toBe("n1");
    expect(wantsNotice(n, DEFAULT_RUNNERS_NOTIFY)).toBe(true);
    expect(wantsNotice(n, { ...DEFAULT_RUNNERS_NOTIFY, n1: false })).toBe(false);
    expect(runnersNotifyKinds({ halted: false }).n1).toBe(true);
    const shadow = ALERT_SWITCH_SECTIONS.find((s) => s.id === "shadow")!;
    expect(shadow.items).toEqual([expect.objectContaining({ key: "n1", icon: "insights", name: "N1 · shadow" }), expect.objectContaining({ key: "f1u", icon: "insights", name: "F1 · first pullback" }), expect.objectContaining({ key: "xGap", icon: "insights", name: "Pre-market high" }), expect.objectContaining({ key: "xCtx", icon: "insights", name: "Day 2 · above yesterday's high" }), expect.objectContaining({ key: "modelPick", icon: "insights", name: "Model picks" }), expect.objectContaining({ key: "vwapReclaim", icon: "insights", name: "VWAP reclaim" }), expect.objectContaining({ key: "levelBreak", icon: "insights", name: "20-day high break" })]);
    // Its description says what it is now: X 8 %, on by default (it was "+6%" and "Off unless you turn it on").
    const n1 = shadow.items[0]!.description;
    expect(n1).toContain("+8%");
    expect(n1).not.toContain("Off unless");
  });

  test("a kind an older client does not know is dropped, never shown as another kind", () => {
    expect(parseRunnersNotice({ ...frame, kind: "shadow.n2" })).toBeNull();
  });
});

describe("research v2 x-halt-1 and the run cut", () => {
  const mk = (kind: string) => parseRunnersNotice({ type: "runners.notice", id: `runners-${kind}-x`, alertId: "x", symbol: "ABCD", kind, title: "ABCD · Resumed up · $2.25", body: "b", at: 1 })!;

  test("Resumed up has its own switch, on by default; a stored value from before takes it", () => {
    expect(mk("halt.resumed_up").kind).toBe("halt.resumed_up");
    expect(runnersNotifyKey(mk("halt.resumed_up").kind)).toBe("resumedUp");
    expect(wantsNotice(mk("halt.resumed_up"), DEFAULT_RUNNERS_NOTIFY)).toBe(true);
    expect(wantsNotice(mk("halt.resumed_up"), { ...DEFAULT_RUNNERS_NOTIFY, resumedUp: false })).toBe(false);
    expect(runnersNotifyKinds({ haltBull: false })).toMatchObject({ haltBull: false, resumedUp: true });
  });

  test("the Run +20% switch says it is cut", () => {
    const run = ALERT_SWITCH_SECTIONS.find((s) => s.id === "runners")!.items.find((i) => i.key === "run")!;
    expect(run.description).toContain("Cut since Nov 22");
  });
});

describe("research v2 x-float-1", () => {
  const mk = (kind: string) => parseRunnersNotice({ type: "runners.notice", id: `runners-${kind}-x`, alertId: "x", symbol: "ABCD", kind, title: "ABCD · Float rotation · $2.71", body: "b", at: 1 })!;

  test("its own switch, on by default, listed with the Runners paths; a stored value from before takes the default", () => {
    expect(mk("v2.xfloat").kind).toBe("v2.xfloat");
    expect(runnersNotifyKey(mk("v2.xfloat").kind)).toBe("xFloat");
    expect(wantsNotice(mk("v2.xfloat"), DEFAULT_RUNNERS_NOTIFY)).toBe(true);
    expect(wantsNotice(mk("v2.xfloat"), { ...DEFAULT_RUNNERS_NOTIFY, xFloat: false })).toBe(false);
    expect(runnersNotifyKinds({ n1: false })).toMatchObject({ n1: false, xFloat: true });
    const runners = ALERT_SWITCH_SECTIONS.find((s) => s.id === "runners")!;
    const x = runners.items.find((i) => i.key === "xFloat")!;
    expect(x).toMatchObject({ name: "Float rotation", icon: "pathRunning" });
    expect(x.description).toContain("On by default");
  });
});

describe("Engine A T2-c4", () => {
  const mk = (kind: string) => parseRunnersNotice({ type: "runners.notice", id: `runners-${kind}-x`, alertId: "x", symbol: "ABCD", kind, title: "ABCD · x · $2.12", body: "b", at: 1 })!;

  test("F1u and the buying-side gauge each have their own switch, on by default; a stored value from before takes it", () => {
    expect(runnersNotifyKey(mk("shadow.f1u").kind)).toBe("f1u");
    expect(runnersNotifyKey(mk("halt.bull").kind)).toBe("haltBull");
    expect(wantsNotice(mk("shadow.f1u"), DEFAULT_RUNNERS_NOTIFY)).toBe(true);
    expect(wantsNotice(mk("shadow.f1u"), { ...DEFAULT_RUNNERS_NOTIFY, f1u: false })).toBe(false);
    expect(runnersNotifyKinds({ n1: false })).toMatchObject({ n1: false, f1u: true, haltBull: true });
    const halt = ALERT_SWITCH_SECTIONS.find((s) => s.id === "halt")!;
    expect(halt.items.map((i) => i.key)).toEqual(["halt", "haltBull", "resumedUp"]);
  });

  test("a buying-side gauge is shown once: not again when the gauge's own switch is on", () => {
    expect(wantsNotice(mk("halt.bull"), DEFAULT_RUNNERS_NOTIFY)).toBe(true);
    expect(wantsNotice(mk("halt.bull"), { ...DEFAULT_RUNNERS_NOTIFY, halt: true })).toBe(false);
    expect(wantsNotice(mk("halt.watch"), { ...DEFAULT_RUNNERS_NOTIFY, halt: true })).toBe(true);
    expect(wantsNotice(mk("halt.bull"), { ...DEFAULT_RUNNERS_NOTIFY, haltBull: false })).toBe(false);
  });
});

describe("v2 x-gap-1 / x-ctx-1", () => {
  const mk = (kind: string) => parseRunnersNotice({ type: "runners.notice", id: `runners-${kind}-x`, alertId: "x", symbol: "ABCD", kind, title: "ABCD · x · $2.12", body: "b", at: 1 })!;

  test("each has its own switch, on by default; a stored value from before them takes it", () => {
    expect(runnersNotifyKey(mk("v2.xgap").kind)).toBe("xGap");
    expect(runnersNotifyKey(mk("v2.xctx").kind)).toBe("xCtx");
    expect(wantsNotice(mk("v2.xgap"), DEFAULT_RUNNERS_NOTIFY)).toBe(true);
    expect(wantsNotice(mk("v2.xgap"), { ...DEFAULT_RUNNERS_NOTIFY, xGap: false })).toBe(false);
    expect(wantsNotice(mk("v2.xctx"), { ...DEFAULT_RUNNERS_NOTIFY, xGap: false })).toBe(true);
    expect(runnersNotifyKinds({ f1u: false })).toMatchObject({ f1u: false, xGap: true, xCtx: true });
  });
});

describe("research v2 b-good", () => {
  test("model picks: their own kind and switch, on by default; a stored value from before takes it", () => {
    const n = parseRunnersNotice({ type: "runners.notice", id: "runners-bgood-2026-11-30-ABCD-bg-1796034000", alertId: "2026-11-30-ABCD-bg-1796034000", symbol: "ABCD", kind: "model.pick", title: "ABCD · Model pick · $2.12", body: "Model score 0.43 · pick 3 today · research v2 model", data: { symbol: "ABCD", kind: "model.pick", detector: "b-good-2" }, at: 1 })!;
    expect(n.kind).toBe("model.pick");
    expect(runnersNotifyKey(n.kind)).toBe("modelPick");
    expect(wantsNotice(n, DEFAULT_RUNNERS_NOTIFY)).toBe(true);
    expect(wantsNotice(n, { ...DEFAULT_RUNNERS_NOTIFY, modelPick: false })).toBe(false);
    expect(runnersNotifyKinds({ n1: false }).modelPick).toBe(true);
  });
});

describe("v2 rework-2 rw2-vwapr-1 / rw2-level-1", () => {
  test("their own kinds and switches, on by default, each independent", () => {
    const mk = (kind: string) => parseRunnersNotice({ type: "runners.notice", id: `runners-x-${kind}`, alertId: "2026-11-30-ABCD-level", symbol: "ABCD", kind, title: "ABCD · 20-day high break · $1.31", body: "Held 60 s over 20-day high $1.20", data: { symbol: "ABCD", kind }, at: 1 })!;
    expect(mk("v2.vwapr").kind).toBe("v2.vwapr");
    expect(runnersNotifyKey(mk("v2.vwapr").kind)).toBe("vwapReclaim");
    expect(runnersNotifyKey(mk("v2.level").kind)).toBe("levelBreak");
    expect(wantsNotice(mk("v2.level"), DEFAULT_RUNNERS_NOTIFY)).toBe(true);
    expect(wantsNotice(mk("v2.level"), { ...DEFAULT_RUNNERS_NOTIFY, levelBreak: false })).toBe(false);
    expect(wantsNotice(mk("v2.vwapr"), { ...DEFAULT_RUNNERS_NOTIFY, levelBreak: false })).toBe(true);
    expect(runnersNotifyKinds({ xGap: false })).toMatchObject({ xGap: false, vwapReclaim: true, levelBreak: true });
  });
});
