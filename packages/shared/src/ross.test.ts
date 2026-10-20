import { describe, expect, test } from "bun:test";
import type { RossRowWire } from "./protocol.ts";
import { createRossChannel, rossAttention, rossPrice, rossRowView } from "./ross.ts";

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
    setup: { state: "crossing", trigger: { price: 6_050_000, version: "t1" }, invalidation: { price: 5_860_000, version: "i1" }, meter: 1, risk_to_stop_bp: 362 },
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
    const near = rossRowView(row({ setup: { state: "near", trigger: { price: 2_490_000, version: "t" }, distance_to_trigger_bp: 80, meter: 0.82 }, membership: { status: "qualified", episodes: [{ id: "e", admitted_at: cutoff - MIN, retained_since: null, exited_at: null }], nonpassing: [] }, float_shares: 3_100_000, price: 2_470_000 }), cutoff, hhmm);
    expect(near.setup.note).toBe("2.49 · 0.8% away");
    expect(near.badge?.label).toBe("NEW");
    expect(near.tag).toBe("Float 3.1M");
    const unknown = rossRowView(row({ membership: { status: "retained", episodes: [], nonpassing: [["news", "UNKNOWN", "news_coverage_unknown"]] } }), cutoff, hhmm);
    expect([unknown.tag, unknown.tagTone]).toEqual(["News ?", "warning"]);
    const warm = rossRowView(row({ momentum: { status: "unavailable", reason: "no_one_minute_endpoint", r_short_bp: null, r_primary_bp: null, r_context_bp: null, accel_bp: null, below_high_bp: null }, rank: null }), cutoff, hhmm);
    expect([warm.m1, warm.m1Tone, warm.move]).toEqual(["—", "dim", ""]);
  });
  test("invalidated is dimmed with no meter; halted shows the badge; sub-$1 prints four decimals", () => {
    const inv = rossRowView(row({ setup: { state: "invalidated", invalidation: { price: 3_440_000, version: "i" }, meter: 0.1 } }), cutoff, hhmm);
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
