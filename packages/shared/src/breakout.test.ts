import { describe, expect, test } from "bun:test";
import { BREAKOUT_LABEL, bindNoticeContext, breakoutClockText, breakoutOverlays, breakoutSummary, noticeChartContext, parseBreakoutNotice, parseNoticeChartContext, runnersNoticeChartContext } from "./breakout.ts";
import { breakoutTestFixture as b } from "./breakoutTestFixture.ts";
import { parseNoticeLog } from "./noticeLog.ts";
import { createNoticeDedupe, DEFAULT_RUNNERS_NOTIFY, parseRunnersNotice, shouldShowNotice } from "./runnersNotify.ts";

describe("frozen Breakout evidence", () => {
  test("explicit ms, exact micros, ns strings and source facts round-trip independently", () => {
    const parsed = parseBreakoutNotice({ ...b, invented: "ignored" })!;
    expect(parsed).toEqual(b);
    expect(parsed.high_exec_ns).toBe("1793017420000000001");
    expect(parsed.high_exec_ms).toBe(1793017420000);
    expect(parsed.high_price_micros).toBe(2040001);
    expect(parsed.close_price_micros).toBeLessThan(parsed.reference_price_micros);
    expect(breakoutSummary(parsed)).toContain("bar high $2.040001 > alert price $2.03");
    expect(breakoutSummary(parsed)).toContain("whole 15s bar");
    expect(breakoutClockText(parsed, String)).toContain("evaluated 1793017425250 +250 ms");
  });

  test("unsafe or unsupported optional evidence never destroys legacy text, prices, statuses or history clock", () => {
    for (const patch of [{ v: 2 }, { exact_state: { toString: null, valueOf: null } }, { high_price_micros: Number.MAX_SAFE_INTEGER + 1 }, { reference_price_micros: -1 }, { high_exec_ns: Number("1793017420000000001") }, { high_exec_ns: "bad" }, { high_exec_ms: NaN }, { evaluated_at_ms: Number.MAX_SAFE_INTEGER }, { bar_end_ms: b.bar_end_ms + 1 }, { reference_known_at_ms: b.evaluated_at_ms + 1 }, { volume_ratio: Infinity }, { high_price_micros: b.reference_price_micros }]) {
      const input = { event_id: "old", notice: "runners.breakout", at: 123, price: 2.02, title: "OLD title", body: "OLD body", phone: { decision: "held", held: "quiet_hours", devices: 1 }, breakout: { ...b, ...patch } };
      const row = parseNoticeLog({ day: "2026-10-26", items: [input] })!.items[0]!;
      expect(row.breakout).toBeUndefined();
      expect([row.at, row.price, row.title, row.body, row.phone?.held]).toEqual([123, 2.02, "OLD title", "OLD body", "quiet_hours"]);
      expect(noticeChartContext(row)).toBeUndefined();
    }
  });

  test("zero prior pace has a named fallback and null ratio, never fabricated growth", () => {
    const parsed = parseBreakoutNotice({ ...b, pace_slot: 0, volume_ratio: null, zero_base_fallback: true })!;
    expect(parsed).toBeDefined();
    expect(breakoutSummary(parsed)).toContain("positive-volume fallback (zero prior pace)");
    expect(breakoutSummary(parsed)).not.toContain("2.00×");
    expect(parseBreakoutNotice({ ...b, pace_slot: 0, volume_ratio: 2, zero_base_fallback: true })).toBeUndefined();
  });

  test("contradictory pace ratios discard only optional evidence while leaving the history row readable", () => {
    for (const volume_ratio of [0, 1_000_000]) {
      const input = { event_id: "runners-breakout-fixture", symbol: "CJMB", notice: "runners.breakout", at: b.evaluated_at_ms, title: "original title", body: "original body", price: 2.02, breakout: { ...b, volume_ratio } };
      const row = parseNoticeLog({ day: "2026-10-26", items: [input] })!.items[0]!;
      expect(row.breakout).toBeUndefined();
      expect(noticeChartContext(row)).toBeUndefined();
      expect([row.at, row.title, row.body, row.price]).toEqual([b.evaluated_at_ms, "original title", "original body", 2.02]);
    }
    expect(parseBreakoutNotice({ ...b, volume_ratio: b.volume_ratio + 1e-10 })).toBeDefined();
  });

  test("earlier eligible-high reference carries its own interval and actual known time", () => {
    const parsed = parseBreakoutNotice({ ...b, reference_basis: "eligible_bar_high", reference_bar_start_ms: b.bar_start_ms - 15000, reference_bar_end_ms: b.bar_start_ms, reference_known_at_ms: b.bar_start_ms + 250, reference_known_ns: `${b.bar_start_ms + 250}000000` })!;
    expect(parsed).toBeDefined();
    expect(breakoutSummary(parsed)).toContain("earlier eligible high");
    expect(parseBreakoutNotice({ ...parsed, reference_bar_end_ms: b.bar_end_ms })).toBeUndefined();
  });

  test("live desk at remains the source clock; valid evidence does not change default switch or dedupe identity", () => {
    const raw = { type: "runners.notice", id: "runners-breakout-fixture", alertId: "fixture", symbol: "CJMB", kind: "runners.breakout", title: BREAKOUT_LABEL, body: "original body", at: b.event_at_ms, data: { breakout: b, event_id: "runners-breakout-fixture" } };
    const frame = parseRunnersNotice(raw)!;
    expect(frame.at).toBe(b.event_at_ms);
    const context = runnersNoticeChartContext(frame)!;
    expect(context).toEqual({ event_id: raw.id, symbol: raw.symbol, breakout: b });
    expect(shouldShowNotice(frame, DEFAULT_RUNNERS_NOTIFY, createNoticeDedupe())).toBe(false);
    const dedupe = createNoticeDedupe();
    expect(shouldShowNotice(frame, { ...DEFAULT_RUNNERS_NOTIFY, breakout: true }, dedupe)).toBe(true);
    expect(shouldShowNotice(parseRunnersNotice({ ...raw, data: { breakout: { ...b, v: 99 } } })!, { ...DEFAULT_RUNNERS_NOTIFY, breakout: true }, dedupe)).toBe(false);
    expect(parseRunnersNotice({ ...raw, data: { breakout: { ...b, v: 99 } } })!.data?.breakout).toBeUndefined();
  });

  test("context requires exact event identity and matching symbol; legacy remains ticker-only", () => {
    const context = { event_id: "runners-breakout-fixture", symbol: "CJMB", breakout: b };
    expect(parseNoticeChartContext(context, "CJMB")).toEqual(context);
    expect(parseNoticeChartContext(context, "AAPL")).toBeUndefined();
    expect(parseNoticeChartContext({ ...context, event_id: "" })).toBeUndefined();
    expect(noticeChartContext({ ...context, notice: "runners.new" })).toBeUndefined();
    expect(breakoutOverlays(context, "AAPL")).toEqual([]);
    expect(breakoutOverlays(undefined, "CJMB")).toEqual([]);
  });

  test("source/connection changes clear context before painting and switching back cannot resurrect it", () => {
    const context = { event_id: "runners-breakout-fixture", symbol: "CJMB", breakout: b };
    const feed1 = {}, feed2 = {};
    let binding = bindNoticeContext(null, feed1, "live", context);
    expect(binding.invalidated).toBe(false);
    expect(bindNoticeContext(binding, feed1, "live", context)).toBe(binding);
    binding = bindNoticeContext(binding, feed1, "replay", context);
    expect(binding.invalidated).toBe(true);
    binding = bindNoticeContext(binding, feed1, "live", context);
    expect(binding.invalidated).toBe(true);
    binding = bindNoticeContext(binding, feed1, "live", { ...context }); // another explicit tap
    expect(binding.invalidated).toBe(false);
    expect(bindNoticeContext(binding, feed2, "live", binding.context).invalidated).toBe(true);
  });

  test("strict execution and admission boundaries are compared as integers, including same-ms nanos", () => {
    const sameMs = { ...b, flag_known_at_ms: b.high_exec_ms, reference_known_at_ms: b.high_exec_ms, flag_known_ns: "1793017420000000000", reference_known_ns: "1793017420000000000" };
    expect(parseBreakoutNotice(sameMs)).toBeDefined(); // high is one nanosecond later
    expect(parseBreakoutNotice({ ...sameMs, high_exec_ns: sameMs.flag_known_ns })).toBeUndefined();
    expect(parseBreakoutNotice({ ...b, high_admission: b.flag_creation_admission })).toBeUndefined();
    expect(parseBreakoutNotice({ ...b, high_admission: "31" })).toBeUndefined();
  });

  test("old sparse, pushed-only and failed-only projections retain recorded history facts without geometry", () => {
    const input = [
      { event_id: "sparse" },
      { event_id: "pushed-only", at: 1793017428000, notice: "runners.breakout", title: "original pushed title" },
      { event_id: "failed-only", at: 1793017429000, failed: "push refused" },
    ];
    const rows = parseNoticeLog({ day: "2026-10-26", items: input })!.items;
    expect(rows.map((r) => [r.event_id, r.at, r.title, r.failed])).toEqual([["sparse", undefined, undefined, null], ["pushed-only", 1793017428000, "original pushed title", null], ["failed-only", 1793017429000, undefined, "push refused"]]);
    expect(rows.every((r) => r.breakout === undefined && noticeChartContext(r) === undefined)).toBe(true);
  });
});
