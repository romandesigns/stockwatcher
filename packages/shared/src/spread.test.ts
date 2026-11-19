import { describe, expect, test } from "bun:test";
import { parseRunnersNotice } from "./runnersNotify.ts";
import { rocketRowView, type RocketRowWire } from "./rocket.ts";
import { SPREAD_NORMAL_MAX_BP, SPREAD_WARNING_MAX_BP, spreadBand, spreadPct, spreadView } from "./spread.ts";

describe("spread cost bands (owner, 2026-11-19)", () => {
  test("≤ 100 bp normal, 100–300 warning, > 300 danger, edges included in the cheaper band", () => {
    expect([SPREAD_NORMAL_MAX_BP, SPREAD_WARNING_MAX_BP]).toEqual([100, 300]);
    expect(spreadBand(0)).toBe("normal");
    expect(spreadBand(42)).toBe("normal");
    expect(spreadBand(100)).toBe("normal");
    expect(spreadBand(101)).toBe("warning");
    expect(spreadBand(157)).toBe("warning");
    expect(spreadBand(300)).toBe("warning");
    expect(spreadBand(301)).toBe("danger");
    expect(spreadBand(5000)).toBe("danger");
  });

  test("the label matches the server's push text rule (two decimals under 1 %, one from 1 %)", () => {
    // crates/server/src/alerts/runners.rs `the_spread_ends_the_reference_line_when_known`.
    expect(spreadPct(42)).toBe("0.42%");
    expect(spreadPct(99)).toBe("0.99%");
    expect(spreadPct(100)).toBe("1.0%");
    expect(spreadPct(157)).toBe("1.6%");
    expect(spreadPct(2100)).toBe("21.0%");
    expect(spreadPct(0)).toBe("0.00%");
  });

  test("status tones only, never the accent; a normal spread stays quiet", () => {
    expect(spreadView(42)).toEqual({ bp: 42, band: "normal", tone: "muted", label: "Spread 0.42%", value: "0.42%", aria: "Spread 0.42%, normal cost to trade" });
    expect(spreadView(210)?.tone).toBe("warning");
    expect(spreadView(310)?.tone).toBe("critical");
    expect(spreadView(310)?.aria).toBe("Spread 3.1%, very costly to trade");
  });

  test("no spread shows nothing: absent, null or nonsense is never drawn as a cost", () => {
    for (const v of [undefined, null, Number.NaN, Number.POSITIVE_INFINITY, -5]) expect(spreadView(v)).toBeNull();
  });
});

describe("the spread on the wire", () => {
  const T = 1_792_075_200_000;
  const row = (over: Partial<RocketRowWire> = {}): RocketRowWire => ({ id: "2026-10-15-ROCK-1", s: "ROCK", at: T - 60_000, p: 1.02, last: 1.12, pc: 0.8, score: 2.3, reasons: [], outcome: { state: "pending", gain: null, drawdown: null, partial: false }, ...over });

  test("a Runners row's fire-time spread becomes its chip and joins its spoken description", () => {
    const v = rocketRowView(row({ spread_bp: 157 }), T);
    expect(v.spread?.label).toBe("Spread 1.6%");
    expect(v.spread?.band).toBe("warning");
    expect(v.aria).toContain("Spread 1.6%, costly to trade");
    // An older server, a replay, a restored row: no chip, nothing said.
    const none = rocketRowView(row(), T);
    expect(none.spread).toBeNull();
    expect(none.aria).not.toContain("Spread");
  });

  test("runners.notice keeps spreadBp (a number or an explicit null) and drops anything else", () => {
    const frame = { type: "runners.notice", id: "runners-run-X-1", alertId: "X-1", symbol: "X", kind: "runners.run", title: "X · Run +20% · $1.22", body: "b", at: 1 };
    expect(parseRunnersNotice({ ...frame, spreadBp: 157 })?.spreadBp).toBe(157);
    expect(parseRunnersNotice({ ...frame, spreadBp: null })).toHaveProperty("spreadBp", null);
    expect(parseRunnersNotice(frame)).not.toHaveProperty("spreadBp");
    expect(parseRunnersNotice({ ...frame, spreadBp: "157" })).not.toHaveProperty("spreadBp");
  });
});
