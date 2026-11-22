import { describe, expect, test } from "bun:test";
import { confidenceView, parseRunnersConfidence } from "./confidence.ts";
import { heldText, noticeKind } from "./noticeLog.ts";
import { rocketRowView, type RocketRowWire } from "./rocket.ts";

describe("b-good-1 confidence tag", () => {
  test("only a shown high tier draws a chip", () => {
    const v = confidenceView("high");
    expect(v?.label).toBe("High confidence");
    expect(v?.short).toBe("High");
    // Plain words, never advice.
    expect(v?.aria.toLowerCase()).not.toContain("buy");
    expect(confidenceView("high", null)).toEqual(v);
    expect(confidenceView("none")).toBeNull();
    expect(confidenceView(undefined)).toBeNull();
    expect(confidenceView(null)).toBeNull();
    // Held back by its runtime switch: decided, never shown.
    expect(confidenceView("high", "switched_off")).toBeNull();
  });

  test("the frame is read defensively", () => {
    const f = { type: "runners.confidence", eventId: "runners-fast-X-1", alertId: "X-1", symbol: "XLAB", notice: "runners.fast", tier: "high", score: 0.27, at: 31000 };
    expect(parseRunnersConfidence(f)).toEqual(f as never);
    expect(parseRunnersConfidence({ ...f, tier: "none" })).toBeNull();
    expect(parseRunnersConfidence({ ...f, eventId: 3 })).toBeNull();
    expect(parseRunnersConfidence({ ...f, type: "runners.notice" })).toBeNull();
    expect(parseRunnersConfidence(null)).toBeNull();
    expect(parseRunnersConfidence({ ...f, score: "x" })?.score).toBe(0);
  });

  test("the log names the new kind and the switch hold", () => {
    expect(noticeKind("model.pick")).toEqual({ group: "shadow", label: "Model pick" });
    expect(heldText("switched_off")).toBe("detector switched off (recorded only)");
  });
});

describe("the tag on a Runners row", () => {
  const T = 1_792_075_200_000;
  const row = (over: Partial<RocketRowWire> = {}): RocketRowWire => ({ id: "2026-10-15-ROCK-1", s: "ROCK", at: T - 60_000, p: 1.02, last: 1.12, pc: 0.8, score: 2.3, reasons: [], outcome: { state: "pending", gain: null, drawdown: null, partial: false }, ...over });

  test("a tagged row gets the chip and says it; an untagged or older row neither", () => {
    const v = rocketRowView(row({ confidence: "high" }), T);
    expect(v.confidence?.label).toBe("High confidence");
    expect(v.aria).toContain("High confidence");
    for (const r of [row(), row({ confidence: null })]) {
      const n = rocketRowView(r, T);
      expect(n.confidence).toBeNull();
      expect(n.aria).not.toContain("confidence");
    }
  });
});
