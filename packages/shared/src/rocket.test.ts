import { describe, expect, test } from "bun:test";
import { rocketSectionCounts } from "./panels.ts";
import { parseServerMessage } from "./protocol.ts";
import { createRocketChannel, mergeRocketDayLines, newRocketFlags, rocketAge, rocketAttention, rocketFreshCount, rocketHeaderView, rocketNow, rocketRowsView, rocketRowView, rocketSectionsView, type RocketPanelWire, type RocketRowWire, type RocketSnapshotMessage } from "./rocket.ts";

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

describe("buying bursts (2026-10-22)", () => {
  type Burst = NonNullable<RocketRowWire["burst"]>;
  const outcome = (over: Partial<Burst["outcome"]> = {}): Burst["outcome"] => ({ state: "pending", c10: false, c20: false, c30: false, stopped: false, mfe: null, mae: null, settled: false, ...over });
  const burst = (over: Partial<RocketRowWire> = {}, b: Partial<Burst> = {}): RocketRowWire =>
    row({
      kind: "burst",
      id: "burst-1",
      s: "BRST",
      at: T - 2 * 60_000,
      reasons: ["$ rate 6.4× base", "12 new highs in 15 s", "3 sweeps", "88% up-tick volume"],
      burst: { start: T - 6 * 60_000, start_price: 0.9, gain_pct: 18.04, move_pct: 4.21, rate_x: 6.4, new_highs: 12, sweeps: 3, buy_pct: 88, outcome: outcome(), ...b },
      ...over,
    });
  const detector: NonNullable<RocketPanelWire["burst"]> = { enabled: true, threshold: 0.42, per_day: 20, today: 3, typical: { mfe30_med_pct: 13.4, mae_med_pct: -2.8, cont10_pct: 37, cont20_pct: 23, fires_after_min: [5, 9] } };

  test("a burst row: headline, gain since the burst start with its age, the last 30 s, live while fresh and pending", () => {
    const v = rocketRowView(burst(), T);
    expect(v.burst).toEqual({ headline: "Buying burst", since: "+18.0% since burst start · 6m ago", move: "+4.2% last 30 s", live: true });
    expect(v.reasons).toEqual(["$ rate 6.4× base", "12 new highs in 15 s"]);
    expect(v.outcome).toBeNull();
    expect(v.aria).toContain("Buying burst alerted 2m ago");
    // Never "buy" advice: the only buy-word is the headline's "Buying".
    expect(v.aria.toLowerCase()).not.toMatch(/\bbuy\b/);
    // Older than BURST_LIVE_MS, or settled: no longer the live accent.
    expect(rocketRowView(burst({ at: T - 6 * 60_000 }), T).burst?.live).toBe(false);
    expect(rocketRowView(burst({}, { outcome: outcome({ state: "cont10", c10: true, settled: true }) }), T).burst?.live).toBe(false);
    // A Runners flag has no burst view; `kind: "burst"` without its block reads as a flag rather than breaking.
    expect(rocketRowView(row(), T).burst).toBeNull();
    expect(rocketRowView(row({ kind: "burst" }), T).burst).toBeNull();
  });

  test("strict outcome tags: continuation success, stopped loss, faded and pending neutral", () => {
    const tag = (o: Partial<Burst["outcome"]>) => rocketRowView(burst({ outcome: { state: "rocketed", gain: 40, drawdown: -1, partial: false } }, { outcome: outcome(o) }), T).outcome;
    expect(tag({ state: "cont30" })).toEqual({ label: "+30% before −5% ✓", tone: "good" });
    expect(tag({ state: "cont20" })).toEqual({ label: "+20% before −5% ✓", tone: "good" });
    expect(tag({ state: "cont10" })).toEqual({ label: "+10% before −5% ✓", tone: "good" });
    expect(tag({ state: "stopped", stopped: true })).toEqual({ label: "−5% first", tone: "loss" });
    expect(tag({ state: "faded", mfe: 7.6 })).toEqual({ label: "faded · max +8%", tone: "dim" });
    expect(tag({ state: "faded" })).toEqual({ label: "faded", tone: "dim" });
    // Pending: no tag, though the row's Runners outcome says rocketed (the burst's own strict outcome rules).
    expect(tag({ state: "pending", mfe: 9 })).toBeNull();
  });

  test("sections: bursts on top newest first, runners below in momentum order, header counts runners only", () => {
    const p = panel({
      burst: detector,
      rows: [row({ id: "r1", s: "RUN", state: "watching" }), burst({ id: "b-old", s: "OLD", at: T - 20 * 60_000 }), row({ id: "r2", s: "BRK", state: "breaking_out" }), burst({ id: "b-new", s: "NEW", at: T - 60_000 })],
    });
    const v = rocketSectionsView(p, T);
    expect(v.bursts?.rows.map((r) => r.symbol)).toEqual(["NEW", "OLD"]);
    expect(v.runners.map((r) => r.symbol)).toEqual(["BRK", "RUN"]);
    expect(v.bursts?.head).toEqual({
      count: "3 today",
      what: "Heavy buying confirmed, typically 5–9 min into a move (about halfway)",
      typical: "Typical from here: +13% max in 30 min (median), 1 in 4 reach +20% before −5%",
    });
    expect(rocketHeaderView(p).record).toBe("4 of 9 rocketed today");
  });

  test("sections: no detector and no burst rows = no section; detector on with none yet = an empty section", () => {
    expect(rocketSectionsView(panel(), T).bursts).toBeNull();
    const empty = rocketSectionsView(panel({ burst: { ...detector, today: 0 } }), T);
    expect(empty.bursts?.rows).toEqual([]);
    expect(empty.bursts?.head.count).toBe("0 today");
    // An older server's burst rows without the detector block: counted from the rows, no typical line.
    expect(rocketSectionsView(panel({ rows: [burst()] }), T).bursts?.head).toMatchObject({ count: "1 today", typical: "" });
  });

  test("the frames parse with the burst fields", () => {
    const m = parseServerMessage(JSON.stringify({ type: "rocket.snapshot", ...panel({ burst: detector, rows: [burst()] }) }));
    expect(m?.type).toBe("rocket.snapshot");
  });
});

describe("running entries (2026-10-22)", () => {
  test("each row says where it came from; a running row measures from its entry", () => {
    const p = panel({
      strategy: { id: "precursors-R2", version: "1", placeholder: false },
      rows: [row({ id: "f", s: "FLAG", state: "watching" }), row({ id: "r", s: "TANH", kind: "running", state: "running", from_flag_pct: 12.5 })],
    });
    const v = rocketSectionsView(p, T).runners;
    // Strongest first across sources: the running row's state outranks the watched flag.
    expect(v.map((x) => [x.symbol, x.source.label])).toEqual([
      ["TANH", "Running"],
      ["FLAG", "R2 flag"],
    ]);
    expect(v[0]?.fromWhat).toBe("entry");
    expect(v[0]?.momentum.note).toBe("+12.5% from entry");
    expect(v[0]?.aria).toContain("running, entered");
    expect(v[1]?.fromWhat).toBe("flag");
    expect(rocketRowView(row(), T).source).toEqual({ kind: "runner", label: "Flag" });
    expect(rocketRowView(row(), T, "precursors-R0").source.label).toBe("R0 flag");
  });
});

describe("Run +20% entries (2026-10-22)", () => {
  test("a run row has its own quiet source, measures from its entry and shares the order", () => {
    const p = panel({
      strategy: { id: "precursors-R2", version: "1", placeholder: false },
      rows: [
        row({ id: "f", s: "FLAG", state: "watching" }),
        row({ id: "g", s: "LEGS", kind: "run", state: "running", from_flag_pct: 6.4, outcome: { state: "pending", gain: null, drawdown: null, partial: false } }),
        row({ id: "r", s: "TANH", kind: "running", state: "fading" }),
      ],
    });
    const v = rocketSectionsView(p, T).runners;
    expect(v.map((x) => [x.symbol, x.source.kind, x.source.label])).toEqual([
      ["LEGS", "run", "Run +20%"],
      ["FLAG", "runner", "R2 flag"],
      ["TANH", "running", "Running"],
    ]);
    expect(v[0]?.fromWhat).toBe("entry");
    expect(v[0]?.momentum.note).toBe("+6.4% from entry");
    expect(v[0]?.aria).toContain("run +20%, entered");
    // No outcome tag: the strict Runners outcome does not apply to an entry on a move under way.
    expect(v[0]?.outcome).toBeNull();
    // Not a burst: it never goes to the Bursts section.
    expect(rocketSectionsView(p, T).bursts).toBeNull();
    // Hide-empty counts it with the flags: a panel holding only a run row is not empty.
    const only = panel({ rows: [row({ id: "g", s: "LEGS", kind: "run" })] });
    expect(rocketSectionCounts({ enabled: true, panel: only, receivedAt: T, connected: true })).toEqual({ bursts: 0, flags: 1 });
  });
});
