import { describe, expect, test } from "bun:test";
import { parseServerMessage } from "./protocol.ts";
import { createRocketChannel, mergeRocketDayLines, newRocketFlags, rocketAge, rocketAttention, rocketFreshCount, rocketHeaderView, rocketNow, rocketRowsView, rocketRowView, type RocketPanelWire, type RocketRowWire, type RocketSnapshotMessage } from "./rocket.ts";

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

describe("runners momentum and later moves (2026-10-22)", () => {
  test("ran later: neutral text once the strict outcome is known and the 2-hour gain reached +15 %", () => {
    const o = (state: RocketRowWire["outcome"]["state"], gain_2h: number | null) => rocketRowView(row({ outcome: { state, gain: 3, drawdown: -2, partial: false, gain_2h, drawdown_2h: -4, gain_session: 50, rocket_later: true } }), T).outcome;
    expect(o("faded", 43.4)).toEqual({ label: "ran later +43%", tone: "text" });
    expect(o("faded", 14.9)).toEqual({ label: "faded", tone: "dim" });
    expect(o("pending", 43)).toBeNull();
    // The strict hit keeps its green tag.
    expect(rocketRowView(row({ outcome: { state: "rocketed", gain: 34.2, drawdown: -1, partial: false, gain_2h: 60 } }), T).outcome).toEqual({ label: "+34% ✓", tone: "good" });
  });
  test("states map to the Ross row language; pullback shows depth, trigger and the meter", () => {
    const v = (over: Partial<RocketRowWire>) => rocketRowView(row(over), T);
    expect(v({}).momentum).toMatchObject({ state: "watching", label: "Watching", tone: "muted", edge: null, pulse: false });
    expect(v({ state: "building" }).momentum).toMatchObject({ label: "Building", tone: "warning", edge: "warning", attention: true });
    expect(v({ state: "breaking_out", provisional: true }).momentum).toMatchObject({ label: "Breaking out", tone: "accent", pulse: true, edge: "accent", provisional: true });
    expect(v({ state: "running", from_flag_pct: 23.4 }).momentum).toMatchObject({ label: "Running", tone: "accent", pulse: false, note: "+23.4% from flag" });
    expect(v({ state: "fading" }).momentum).toMatchObject({ tone: "dim", dimmed: true });
    const h = v({ state: "halted" });
    expect(h.halted).toBe(true);
    expect(h.momentum.tone).toBe("warning");
    const p = v({ state: "pullback", state_since: T - 2 * 60_000, from_high_pct: -4.2, pullback_depth_pct: 4.2, levels: { state: "forming", trigger: 1_150_000, invalidation: 1_050_000, meter_milli: 620, distance_to_trigger_bp: 270 } });
    expect(p.momentum.note).toBe("−4.2% off high · trigger 1.15 · 2.7% away");
    expect(p.momentum.since).toBe("2m");
    expect(p.meter).toBeCloseTo(0.62);
    expect(p.fromHigh).toBe("−4.2%");
    // No meter outside a pullback; an unknown state token reads as watching.
    expect(v({ state: "running", levels: { state: "forming", trigger: 1, invalidation: 0, meter_milli: 500 } }).meter).toBeNull();
    expect(v({ state: "zooming" as never }).momentum.state).toBe("watching");
  });
  test("row metrics are never green; the day line ends at the live price", () => {
    const r = rocketRowView(row({ from_flag_pct: 9.8, move_1m_pct: -0.4, from_high_pct: -0.5, day_line: [1_000_000, 1_050_000], last: 1.12 }), T);
    expect([r.fromFlag, r.fromFlagTone, r.move1m, r.move1mTone, r.fromHigh, r.fromHighTone]).toEqual(["+9.8%", "text", "−0.4%", "muted", "−0.5%", "text"]);
    expect(r.line?.values).toEqual([1_000_000, 1_050_000, 1_120_000]);
    expect(rocketRowView(row(), T).line).toBeNull();
  });
  test("sort: breaking out, running, halted, building, pullback, watching, fading; header attention and ran later", () => {
    const states = ["fading", "watching", "pullback", "building", "running", "breaking_out", "halted"] as const;
    const p = panel({ rows: states.map((s, i) => row({ id: String(i), s: s.toUpperCase(), state: s })), today: { date: "2026-10-15", alerts: 7, hits: 1, hits30: 1, known: 7, ran_later: 2 } });
    expect(rocketRowsView(p, T).map((v) => v.momentum.state)).toEqual(["breaking_out", "running", "halted", "building", "pullback", "watching", "fading"]);
    expect(rocketAttention(p)).toBe(2);
    const h = rocketHeaderView(p);
    expect(h.ranLater).toBe("2 ran ≥ +15% later");
    expect(h.attention).toBe(2);
  });
  test("day-line slices merge per symbol; one that cannot be placed asks for a snapshot", () => {
    const calls: string[] = [];
    const ch = createRocketChannel({ subscribe: () => calls.push("sub"), unsubscribe() {} }, () => 0);
    ch.subscribe(() => {});
    ch.apply({ type: "rocket.snapshot", ...panel({ rows: [row({ day_line_start: 5, day_line_from: 0, day_line: [1, 2, 3] })] }) });
    ch.apply({ type: "rocket.update", ...panel({ rows: [row({ day_line_start: 5, day_line_from: 3, day_line: [4] })] }) });
    expect(ch.get().panel?.rows[0]?.day_line).toEqual([1, 2, 3, 4]);
    expect(ch.get().panel?.rows[0]?.day_line_from).toBe(0);
    ch.apply({ type: "rocket.update", ...panel({ rows: [row({ day_line_start: 5, day_line_from: 9, day_line: [5] })] }) });
    expect(ch.get().panel?.rows[0]?.day_line).toBeUndefined();
    expect(calls).toEqual(["sub", "sub"]);
    expect(mergeRocketDayLines(null, panel()).resync).toBe(false);
  });
});
