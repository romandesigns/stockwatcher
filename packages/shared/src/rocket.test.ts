import { describe, expect, test } from "bun:test";
import { parseServerMessage } from "./protocol.ts";
import { createRocketChannel, newRocketFlags, rocketAge, rocketFreshCount, rocketHeaderView, rocketNow, rocketRowView, type RocketPanelWire, type RocketRowWire, type RocketSnapshotMessage } from "./rocket.ts";

const T = 1_792_075_200_000;

function row(over: Partial<RocketRowWire> = {}): RocketRowWire {
  return {
    id: "2026-10-15-ROCK-1",
    s: "ROCK",
    at: T - 3 * 60_000,
    p: 1.02,
    last: 1.12,
    pc: 0.8,
    score: 2.3456,
    reasons: ["volume 8.2× 30-min avg", "grinding: 7 higher closes", "near premarket high"],
    outcome: { state: "pending", gain: null, drawdown: null, partial: false },
    ...over,
  };
}

function panel(over: Partial<RocketPanelWire> = {}): RocketPanelWire {
  return {
    t: T,
    enabled: true,
    strategy: { id: "placeholder-b2", version: "1", placeholder: true },
    mode: "budget",
    budget: 10,
    threshold: 1.5,
    today: { date: "2026-10-15", alerts: 9, hits: 4, hits30: 5, known: 7 },
    days: [
      { date: "2026-10-13", alerts: 10, hits10: 2, hits30: 3 },
      { date: "2026-10-14", alerts: 10, hits10: 1, hits30: 1 },
    ],
    rows: [row()],
    ...over,
  };
}

describe("rocket row view", () => {
  test("price, day gain, age, two reasons, subtle score, no outcome while pending", () => {
    const v = rocketRowView(row(), T);
    expect([v.symbol, v.price, v.gain, v.age, v.fresh, v.score]).toEqual(["ROCK", "1.12", "+40.0%", "3m", false, "2.35"]);
    expect(v.reasons).toEqual(["volume 8.2× 30-min avg", "grinding: 7 higher closes"]);
    expect(v.outcome).toBeNull();
    expect(v.aria).toContain("flagged 3m ago");
  });
  test("a fresh flag is 'now' and the only accent", () => {
    const v = rocketRowView(row({ at: T - 20_000 }), T);
    expect(v.age).toBe("now");
    expect(v.fresh).toBe(true);
  });
  test("outcome tags: rocketed ✓ (the only green), late, moved, faded", () => {
    const o = (state: RocketRowWire["outcome"]["state"], gain: number | null) => rocketRowView(row({ outcome: { state, gain, drawdown: -2, partial: false } }), T).outcome;
    expect(o("rocketed", 34.4)).toEqual({ label: "+34% ✓", tone: "good" });
    expect(o("late", 31)).toEqual({ label: "+31% late", tone: "muted" });
    expect(o("moved", 30.2)).toEqual({ label: "+30%", tone: "muted" });
    expect(o("faded", 3)).toEqual({ label: "faded", tone: "dim" });
  });
  test("no previous close: no day gain; sub-dollar prices keep their digits", () => {
    const v = rocketRowView(row({ pc: null, last: 0.1234 }), T);
    expect(v.gain).toBe("");
    expect(v.price).toBe("0.1234");
  });
  test("ages", () => {
    expect(rocketAge(59_999)).toBe("now");
    expect(rocketAge(60_000)).toBe("1m");
    expect(rocketAge(65 * 60_000)).toBe("1h 05m");
  });
});

describe("rocket header", () => {
  test("today's record and the last days", () => {
    const h = rocketHeaderView(panel());
    expect(h.record).toBe("4 of 9 rocketed today");
    expect(h.history).toBe("last 2 days: 3 of 20 (15%)");
    expect(h.strategy).toBe("placeholder-b2 v1 · budget 10/day");
    expect(h.placeholder).toBe(true);
  });
  test("nothing yet", () => {
    const h = rocketHeaderView(panel({ today: { date: null, alerts: 0, hits: 0, hits30: 0, known: 0 }, days: [], mode: "fixed" }));
    expect(h.record).toBe("Nothing flagged today");
    expect(h.history).toBe("");
    expect(h.strategy).toBe("placeholder-b2 v1 · fixed threshold");
  });
});

describe("rocket channel", () => {
  test("opts in with the first listener, out with the last; a frame replaces the panel", () => {
    const calls: string[] = [];
    let wall = 1000;
    const ch = createRocketChannel({ subscribe: () => calls.push("sub"), unsubscribe: () => calls.push("unsub") }, () => wall);
    const a = ch.subscribe(() => {});
    const b = ch.subscribe(() => {});
    expect(calls).toEqual(["sub"]);
    const m: RocketSnapshotMessage = { type: "rocket.snapshot", ...panel() };
    ch.apply(m);
    expect(ch.get()).toMatchObject({ enabled: true, connected: true, receivedAt: 1000 });
    expect(ch.get().panel?.rows.length).toBe(1);
    expect((ch.get().panel as unknown as { type?: string }).type).toBeUndefined();
    // Ages keep ticking between frames from the market clock.
    wall = 31_000;
    expect(rocketNow(ch.get(), wall)).toBe(T + 30_000);
    ch.setConnected(false);
    expect(ch.get().connected).toBe(false);
    expect(ch.get().panel).not.toBeNull();
    a();
    b();
    expect(calls).toEqual(["sub", "unsub"]);
    expect(ch.get().panel).toBeNull();
  });
  test("off on the server: enabled false, no panel", () => {
    const ch = createRocketChannel({ subscribe() {}, unsubscribe() {} });
    ch.subscribe(() => {});
    ch.apply({ type: "rocket.snapshot", ...panel({ enabled: false, strategy: null, rows: [] }) });
    expect(ch.get().enabled).toBe(false);
    expect(ch.get().panel).toBeNull();
  });
  test("fresh count and new flags (primed first, so opening the app announces nothing)", () => {
    const ch = createRocketChannel({ subscribe() {}, unsubscribe() {} }, () => 0);
    ch.subscribe(() => {});
    ch.apply({ type: "rocket.update", ...panel({ rows: [row({ id: "a", at: T - 10_000 }), row({ id: "b" })] }) });
    expect(rocketFreshCount(ch.get(), 0)).toBe(1);
    const seen = new Set<string>();
    expect(newRocketFlags(ch.get(), seen, false)).toEqual([]);
    ch.apply({ type: "rocket.update", ...panel({ rows: [row({ id: "c" }), row({ id: "a" })] }) });
    expect(newRocketFlags(ch.get(), seen, true).map((r) => r.id)).toEqual(["c"]);
  });
  test("the frames parse", () => {
    const m = parseServerMessage(JSON.stringify({ type: "rocket.update", ...panel() }));
    expect(m?.type).toBe("rocket.update");
  });
});
