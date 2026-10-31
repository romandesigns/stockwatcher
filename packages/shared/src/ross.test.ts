import { describe, expect, test } from "bun:test";
import type { RossRowWire, RossSetupState, RossSetupWire } from "./protocol.ts";
import { createRossChannel, mergeRossDayLines, rossAttention, rossDemoPanel, rossFreshness, rossLinePaths, rossPrice, rossReadinessView, rossRowView } from "./ross.ts";

const MIN = 60 * 1_000_000_000;
const cutoff = 1_000 * MIN;
const hhmm = () => "10:41";

function row(over: Partial<RossRowWire> = {}): RossRowWire {
  return {
    symbol: "BBBB",
    membership: { status: "retained", episodes: [{ id: "e1", admitted_at: cutoff - 20 * MIN, retained_since: null, exited_at: null }], nonpassing: [["float", "FAIL", "at_or_above_max"]] },
    counts: { pass: 4, fail: 1, unknown: 0 },
    price: 6_080_000,
    gain_bp: 2150,
    momentum: { status: "ready", reason: "ready", r_short_bp: 110, r_primary_bp: 300, r_context_bp: 510, accel_bp: 20, below_high_bp: 40 },
    rank: 1,
    prev_rank: 3,
    setup: { state: "crossing", trigger: { price: 6_050_000, version: "t1" }, invalidation: { price: 5_860_000, version: "i1" }, meter_milli: 1000, risk_to_stop_bp: 362 },
    rvol_milli: 7400,
    float_shares: 30_100_000,
    spark: [5_700_000, 6_080_000],
    ...over,
  };
}

describe("ross row view", () => {
  test("a crossing 4/5 member reads like the design", () => {
    const v = rossRowView(row(), cutoff, hhmm);
    expect([v.symbol, v.price, v.gain, v.m1, v.fromHigh, v.rvol, v.tag]).toEqual(["BBBB", "6.08", "+21.5%", "+3.0%", "−0.4%", "7.4×", "Float 30M ✗"]);
    expect(v.move).toBe("▲");
    expect(v.setup).toMatchObject({ label: "Crossing", note: "6.05 · close pending · 3.6% over low", tone: "accent", pulse: true, attention: true, meter: 1 });
    expect(v.tagTone).toBe("critical");
    expect(v.badge).toBeNull();
  });
  test("near, NEW, unknown pillar, float when nothing is missing, unranked", () => {
    const near = rossRowView(row({ setup: { state: "near", trigger: { price: 2_490_000, version: "t" }, distance_to_trigger_bp: 80, meter_milli: 820 }, membership: { status: "qualified", episodes: [{ id: "e", admitted_at: cutoff - MIN, retained_since: null, exited_at: null }], nonpassing: [] }, float_shares: 3_100_000, price: 2_470_000 }), cutoff, hhmm);
    expect(near.setup.note).toBe("2.49 · 0.8% away");
    expect(near.badge?.label).toBe("NEW");
    expect(near.tag).toBe("Float 3.1M");
    // pillars-v4 keeps a float for 4 days: a figure a day old or more says how old.
    const DAY = 86_400 * 1_000_000_000;
    const aged = (age: number) => rossRowView(row({ membership: { status: "qualified", episodes: [], nonpassing: [] }, float_shares: 3_100_000, float_as_of: cutoff - age }), cutoff, hhmm).tag;
    expect([aged(2 * DAY + MIN), aged(DAY - MIN)]).toEqual(["Float 3.1M · 2d old", "Float 3.1M"]);
    const unknown = rossRowView(row({ membership: { status: "retained", episodes: [], nonpassing: [["news", "UNKNOWN", "news_coverage_unknown"]] } }), cutoff, hhmm);
    expect([unknown.tag, unknown.tagTone]).toEqual(["News ?", "warning"]);
    const warm = rossRowView(row({ momentum: { status: "unavailable", reason: "no_one_minute_endpoint", r_short_bp: null, r_primary_bp: null, r_context_bp: null, accel_bp: null, below_high_bp: null }, rank: null }), cutoff, hhmm);
    expect([warm.m1, warm.m1Tone, warm.move]).toEqual(["—", "dim", ""]);
  });
  test("invalidated is dimmed with no meter; halted shows the badge; sub-$1 prints four decimals", () => {
    const inv = rossRowView(row({ setup: { state: "invalidated", invalidation: { price: 3_440_000, version: "i" }, meter_milli: 100 } }), cutoff, hhmm);
    expect([inv.dimmed, inv.setup.meter, inv.setup.note]).toEqual([true, null, "below 3.44"]);
    expect(rossRowView(row({ setup: { state: "halted" } }), cutoff, hhmm).badge?.label).toBe("HALTED");
    expect(rossPrice(421_000)).toBe("0.4210");
    expect(rossRowView(row({ setup: null }), cutoff, hhmm).setup.label).toBe("Watching");
  });
  test("attention counts at-trigger and crossing (never a broke-trigger) when the server sends no count", () => {
    const panel = { revision: 1, bar_end: 0, cutoff, rows: [row(), row({ setup: { state: "forming" } }), row({ setup: { state: "triggered" } })] };
    expect(rossAttention(panel)).toBe(1);
    expect(rossAttention({ ...panel, attention: 5 })).toBe(5);
  });
});

describe("cues describe stages, not entry signals (owner, 2026-10-21)", () => {
  // The detection-v2 micro-pullback cue as the server sends it: coarse state + detector stage.
  const cue = (state: RossSetupState, extra: Partial<RossSetupWire> = {}) =>
    rossRowView(row({ setup: { family: "micro_pullback", variant: "micro_pullback_mp5", state, trigger: { price: 5_300_000, version: "e:trigger:v1" }, invalidation: { price: 5_200_000, version: "e:invalidation:v1" }, meter_milli: 600, to_invalidation_bp: 115, distance_to_trigger_bp: 76, state_since: cutoff - MIN, ...extra } }), cutoff, hhmm);
  test("every stage has its label and tone", () => {
    const look = (s: Parameters<typeof cue>[0]) => [cue(s).setup.label, cue(s).setup.tone, cue(s).setup.pulse];
    expect(look("forming")).toEqual(["Pullback", "muted", false]);
    expect(look("near")).toEqual(["At trigger", "warning", false]);
    expect(look("crossing")).toEqual(["Crossing", "accent", true]);
    expect(look("triggered")).toEqual(["Broke trigger", "text", false]);
    expect(look("invalidated")).toEqual(["Failed", "dim", false]);
  });
  test("a broke-trigger is never green, never attention, and no wording implies a buy", () => {
    const views = (["watching", "forming", "near", "crossing", "triggered", "invalidated", "halted"] as const).map((s) => cue(s));
    for (const v of views) {
      expect(v.setup.tone).not.toBe("good");
      expect(`${v.setup.label} ${v.setup.note} ${v.aria}`).not.toMatch(/\b(buy|entry|enter|go|stop|signal|triggered)\b/i);
    }
    expect(cue("triggered").setup.attention).toBe(false);
    expect(cue("triggered").setup.note).toBe("5.30 · 10:41");
  });
  test("notes name the levels; the new distance wins over an old server's", () => {
    expect(cue("forming").setup.note).toBe("trigger 5.30");
    expect(cue("near").setup.note).toBe("5.30 · 0.8% away · 1.1% over low");
    expect(cue("near", { to_invalidation_bp: null, risk_to_stop_bp: 200 }).setup.note).toBe("5.30 · 0.8% away · 2.0% over low");
    expect(cue("forming", { trigger: null }).setup.note).toBe("");
    expect(cue("forming").setup.meter).toBe(0.6);
  });
  test("an unknown state token from a newer server reads as watching", () => {
    const v = rossRowView(row({ setup: { state: "something_new" as never } }), cutoff, hhmm);
    expect([v.setup.state, v.setup.label]).toEqual(["watching", "Watching"]);
    expect(rossAttention({ revision: 1, bar_end: 0, cutoff, rows: [row({ setup: { state: "something_new" as never } })] })).toBe(0);
  });
});

test("the channel opts in with the first listener and out (emptied) with the last", () => {
  const calls: string[] = [];
  const ch = createRossChannel({ subscribe: () => calls.push("sub"), unsubscribe: () => calls.push("unsub") });
  const off = ch.subscribe(() => {});
  ch.apply({ type: "ross.snapshot", enabled: false, panel: null });
  expect(ch.get().enabled).toBe(false);
  off();
  expect(calls).toEqual(["sub", "unsub"]);
  expect(ch.get().enabled).toBeNull();
});

test("the engine's own row fields win: move_1m_bp, missing[], meter_milli in thousandths", () => {
  const v = rossRowView(row({ move_1m_bp: -150, missing: [{ pillar: "news", state: "UNKNOWN", reason: "news_coverage_unknown" }], setup: { state: "forming", meter_milli: 375 } }), cutoff, hhmm);
  expect([v.m1, v.tag, v.setup.meter]).toEqual(["−1.5%", "News ?", 0.375]);
});

// --- freshness and readiness (S2a: the cues a client needs to stop guessing) -----------------

test("a drop keeps the last panel, and a reopened socket alone does not make it live again", () => {
  const ch = createRossChannel({ subscribe: () => {}, unsubscribe: () => {} });
  ch.subscribe(() => {});
  const panel = { revision: 1, bar_end: cutoff - 2_000_000_000, cutoff, rows: [] };
  ch.apply({ type: "ross.snapshot", enabled: true, panel });
  expect(ch.get().connected).toBe(true);
  expect(rossFreshness(ch.get(), hhmm).stale).toBe(false);

  // The socket drops. The rows stay — the last known state is useful — but they are labelled.
  ch.setConnected(false);
  const dropped = rossFreshness(ch.get(), hhmm);
  expect(dropped.stale).toBe(true);
  expect(dropped.label).toContain("Reconnecting");
  expect(dropped.label).toContain(hhmm());
  expect(ch.get().panel).toBe(panel);

  // The socket comes back. These are still the rows from before the drop: nothing has been
  // delivered on the new connection yet, so they must not read as live.
  ch.setConnected(true);
  expect(ch.get().connected).toBe(false);
  expect(rossFreshness(ch.get(), hhmm).stale).toBe(true);
  expect(ch.get().panel).toBe(panel);

  // The server answers the re-subscribe with a fresh snapshot: live again, with no label.
  const fresh = { revision: 2, bar_end: cutoff, cutoff: cutoff + 2_000_000_000, rows: [] };
  ch.apply({ type: "ross.snapshot", enabled: true, panel: fresh });
  expect(ch.get().connected).toBe(true);
  expect(rossFreshness(ch.get(), hhmm).stale).toBe(false);
  expect(ch.get().panel).toBe(fresh);
});

test("before any frame the panel is connecting, not reconnecting", () => {
  const ch = createRossChannel({ subscribe: () => {}, unsubscribe: () => {} });
  expect(rossFreshness(ch.get(), hhmm)).toEqual({ stale: true, label: "Connecting…", tone: "dim" });
  ch.setConnected(true);
  expect(rossFreshness(ch.get(), hhmm).label).toBe("Connecting…");
});

test("readiness says why the panel is empty instead of 'nothing qualifies'", () => {
  const r = {
    symbols: 1204,
    qualified: 0,
    members: 0,
    pillars: [{ pillar: "rvol", pass: 12, fail: 0, unknown: 1192 }],
    blocking: [
      { pillar: "rvol", reason: "no_daily_baseline", symbols: 812 },
      { pillar: "news", reason: "news_coverage_unknown", symbols: 391 },
      { pillar: "gain", reason: "actions_coverage_expired", symbols: 44 },
    ],
    evidence_records: 9123,
    observed_at: cutoff,
  };
  const v = rossReadinessView(r, hhmm);
  expect(v.headline).toBe("1,204 symbols · none at 5/5");
  expect(v.reasons).toEqual(["812: no volume history", "391: news not searched yet", "44: share-basis check out of date"]);
  expect(v.takenAt).toBe(`taken ${hhmm()}`);

  // Members are reported when there are some, and an unmapped reason keeps its own words.
  const v2 = rossReadinessView({ ...r, members: 3, blocking: [{ pillar: "x", reason: "some_new_reason", symbols: 1 }] }, hhmm);
  expect(v2.headline).toBe("1,204 symbols · 3 on the panel");
  expect(v2.reasons).toEqual(["1: some new reason"]);
});

test("a readiness-free frame leaves readiness null rather than inventing one", () => {
  const ch = createRossChannel({ subscribe: () => {}, unsubscribe: () => {} });
  ch.apply({ type: "ross.snapshot", enabled: true, panel: null });
  expect(ch.get().readiness).toBeNull();
  ch.apply({ type: "ross.update", enabled: true, panel: null, readiness: { symbols: 1, qualified: 0, members: 0, pillars: [], blocking: [], evidence_records: 0, observed_at: 1 } });
  expect(ch.get().readiness?.symbols).toBe(1);
});

// --- the day line (owner, 2026-10-21: the row chart of the day's 30 s closes) ----------------

describe("day line", () => {
  const panel = (rows: RossRowWire[]) => ({ revision: 1, bar_end: 0, cutoff, rows });
  const START = 7 * MIN;

  test("an update's slice appends to the line held; a snapshot replaces it", () => {
    const calls: string[] = [];
    const ch = createRossChannel({ subscribe: () => calls.push("sub"), unsubscribe: () => calls.push("unsub") });
    ch.subscribe(() => {});
    ch.apply({ type: "ross.snapshot", enabled: true, panel: panel([row({ day_line_start: START, day_line_from: 0, day_line: [1, 2, 3] })]) });
    ch.apply({ type: "ross.update", enabled: true, panel: panel([row({ day_line_start: START, day_line_from: 3, day_line: [4, 5] })]) });
    expect(ch.get().panel?.rows[0]?.day_line).toEqual([1, 2, 3, 4, 5]);
    expect(ch.get().panel?.rows[0]?.day_line_from).toBe(0);
    // A correction revised the last close: the slice starts earlier and replaces the tail.
    ch.apply({ type: "ross.update", enabled: true, panel: panel([row({ day_line_start: START, day_line_from: 4, day_line: [9, 6] })]) });
    expect(ch.get().panel?.rows[0]?.day_line).toEqual([1, 2, 3, 4, 9, 6]);
    ch.apply({ type: "ross.snapshot", enabled: true, panel: panel([row({ day_line_start: START, day_line_from: 0, day_line: [7] })]) });
    expect(ch.get().panel?.rows[0]?.day_line).toEqual([7]);
    expect(calls).toEqual(["sub"]);
  });

  test("a slice that cannot be placed drops the line and asks for a fresh snapshot", () => {
    const calls: string[] = [];
    const ch = createRossChannel({ subscribe: () => calls.push("sub"), unsubscribe: () => calls.push("unsub") });
    ch.subscribe(() => {});
    ch.apply({ type: "ross.snapshot", enabled: true, panel: panel([row({ day_line_start: START, day_line_from: 0, day_line: [1, 2] })]) });
    // A missed frame: the client holds 2 points, the slice starts at 5.
    ch.apply({ type: "ross.update", enabled: true, panel: panel([row({ day_line_start: START, day_line_from: 5, day_line: [6] })]) });
    expect(ch.get().panel?.rows[0]?.day_line).toBeUndefined();
    expect(calls).toEqual(["sub", "sub"]);
    // Another line start (a new day) with a nonzero index cannot be placed either.
    ch.apply({ type: "ross.snapshot", enabled: true, panel: panel([row({ day_line_start: START, day_line_from: 0, day_line: [1, 2] })]) });
    ch.apply({ type: "ross.update", enabled: true, panel: panel([row({ day_line_start: START + MIN, day_line_from: 1, day_line: [3] })]) });
    expect(calls).toEqual(["sub", "sub", "sub"]);
  });

  test("the row chart is the day line plus the live price, toned against the day's first price", () => {
    const up = rossRowView(row({ price: 2_300_000, day_line: [2_000_000, 1_900_000, 2_200_000], prev_close: 1_500_000 }), cutoff, hhmm);
    expect(up.line).toEqual({ kind: "day", values: [2_000_000, 1_900_000, 2_200_000, 2_300_000], prevClose: 1_500_000, tone: "good" });
    const down = rossRowView(row({ price: 1_950_000, day_line: [2_000_000, 2_100_000] }), cutoff, hhmm);
    expect([down.line?.tone, down.line?.prevClose]).toEqual(["critical", null]);
    // An older server: the 15 s spark, as before.
    const old = rossRowView(row({ spark: [5_700_000, 6_080_000] }), cutoff, hhmm);
    expect(old.line).toMatchObject({ kind: "spark", values: [5_700_000, 6_080_000], tone: "good" });
    expect(old.spark).toEqual([5_700_000, 6_080_000]);
    expect(rossRowView(row({ spark: [1] }), cutoff, hhmm).line).toBeNull();
  });

  test("paths: every point drawn when few, thinned per column keeping each spike when many", () => {
    const few = rossLinePaths({ kind: "day", values: [1, 3, 2], prevClose: null, tone: "good" }, 80, 28);
    expect(few.line).toBe("M0 26 L40 2 L80 14");
    expect(few.area.endsWith("L80 28 L0 28 Z")).toBe(true);
    expect(few.baseY).toBeNull();
    // A full day (1,920 points) in 80 units: one spike in the middle must survive the thinning.
    const values = Array.from({ length: 1920 }, (_, i) => (i === 960 ? 500 : 100));
    const many = rossLinePaths({ kind: "day", values, prevClose: null, tone: "good" }, 80, 28);
    const ys = many.line.slice(1).split(" L").map((p) => Number(p.split(" ")[1]));
    expect(ys.length).toBeLessThanOrEqual(2 * 80 + 2);
    expect(Math.min(...ys)).toBe(2);
    expect(many.line.startsWith("M0 26")).toBe(true);
    expect(many.line.endsWith("L80 26")).toBe(true);
  });

  test("the previous close is a baseline only when it is near the line", () => {
    const near = rossLinePaths({ kind: "day", values: [100, 120], prevClose: 98, tone: "good" }, 80, 28);
    expect(near.baseY).toBe(26);
    const far = rossLinePaths({ kind: "day", values: [140, 150], prevClose: 100, tone: "good" }, 80, 28);
    expect(far.baseY).toBeNull();
  });

  test("the demo panel carries day lines", () => {
    const demo = rossDemoPanel(cutoff);
    expect(demo.rows.every((r) => (r.day_line?.length ?? 0) > 100)).toBe(true);
    expect(mergeRossDayLines(null, demo).resync).toBe(false);
  });
});
