import { describe, expect, test } from "bun:test";
import type { RossRowWire } from "./protocol.ts";
import { createRossChannel, rossAttention, rossFreshness, rossPrice, rossReadinessView, rossRowView } from "./ross.ts";

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
    expect(v.setup).toMatchObject({ label: "Crossing", note: "6.05 · close pending · 3.6% stop", tone: "accent", pulse: true, attention: true, meter: 1 });
    expect(v.tagTone).toBe("critical");
    expect(v.badge).toBeNull();
  });
  test("near, NEW, unknown pillar, float when nothing is missing, unranked", () => {
    const near = rossRowView(row({ setup: { state: "near", trigger: { price: 2_490_000, version: "t" }, distance_to_trigger_bp: 80, meter_milli: 820 }, membership: { status: "qualified", episodes: [{ id: "e", admitted_at: cutoff - MIN, retained_since: null, exited_at: null }], nonpassing: [] }, float_shares: 3_100_000, price: 2_470_000 }), cutoff, hhmm);
    expect(near.setup.note).toBe("2.49 · 0.8% away");
    expect(near.badge?.label).toBe("NEW");
    expect(near.tag).toBe("Float 3.1M");
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
  test("attention counts near, crossing and triggered when the server sends no count", () => {
    const panel = { revision: 1, bar_end: 0, cutoff, rows: [row(), row({ setup: { state: "forming" } }), row({ setup: { state: "triggered" } })] };
    expect(rossAttention(panel)).toBe(2);
    expect(rossAttention({ ...panel, attention: 5 })).toBe(5);
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
