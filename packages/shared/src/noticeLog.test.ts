import { describe, expect, test } from "bun:test";
import { filterNotices, heldText, noticeEvidence, noticeIcon, noticeMergedInto, latencyText, noticeKind, noticeStatus, noticeSummary, parseNoticeLog, pathText } from "./noticeLog.ts";
import { breakoutTestFixture } from "./breakoutTestFixture.ts";
import { ALERT_SWITCH_SECTIONS, RUNNERS_NOTICE_KINDS, runnersNotifyKey } from "./runnersNotify.ts";

const body = {
  day: "2026-10-26",
  days: ["2026-10-26"],
  truncated: false,
  items: [
    { event_id: "a", at: 3, symbol: "SRTM", notice: "runners.pullback", title: "SRTM · Pullback setting up", phone: { decision: "held", held: "symbol_limit", devices: 1 }, desk: { decision: "held", held: "symbol_limit", devices: 1 } },
    { event_id: "b", at: 2, symbol: "SRTM", notice: "runners.burst", phone: { decision: "sent", held: null, devices: 1 }, latency: { trade_to_expo_ms: 1044.6, trade_to_board_ms: 100.9 } },
    { event_id: "c", at: 1, symbol: "AAPL", notice: "price.alert", phone: { decision: "off", held: null, devices: 0 } },
    { nope: true },
  ],
};

describe("notice log", () => {
  test("a merged notice says what it was folded into", () => {
    const into = { event_id: "runners-running-WFF-1", notice: "runners.running", secs: 1.2 };
    const item = { event_id: "runners-fast-WFF-1", notice: "runners.fast", phone: { decision: "held", held: "merged", devices: 2, merged_into: into, merged_devices: 2 } };
    expect(noticeMergedInto(item)).toEqual(into);
    expect(heldText("merged", noticeMergedInto(item))).toBe("merged into the Running alert 1 s earlier");
    expect(heldText("merged", { ...into, secs: 0.4 })).toBe("merged into the Running alert moments earlier");
    expect(heldText("merged")).toBe("merged into an earlier alert");
    expect(pathText(item.phone)).toBe("held: merged into the Running alert 1 s earlier");
    // Sent only to the device the first push missed.
    expect(pathText({ decision: "sent", held: null, devices: 1, merged_into: into, merged_devices: 1 })).toBe("sent to 1 device · 1 already had the Running alert");
  });

  test("new Breakout evidence survives history parsing without replacing decision chronology or stored text", () => {
    const input = { event_id: "runners-breakout-fixture", at: 1793017427000, notice: "runners.breakout", title: "stored title", body: "stored body", breakout: breakoutTestFixture };
    const item = parseNoticeLog({ day: "2026-10-26", items: [input] })!.items[0]!;
    expect((item as unknown as Record<string, unknown>).breakout).toEqual(breakoutTestFixture);
    expect(item.at).toBe(input.at);
    expect(item.title).toBe("stored title");
    expect(item.body).toBe("stored body");
  });
  test("parses, drops malformed items, groups kinds", () => {
    const log = parseNoticeLog(body)!;
    expect(log.items.map((i) => i.event_id)).toEqual(["a", "b", "c"]);
    expect(noticeKind("runners.burst")).toEqual({ group: "runners", label: "Buying burst" });
    expect(noticeKind("ross.crossing").group).toBe("ross");
    expect(noticeKind("something.new")).toEqual({ group: "runners", label: "something.new" });
    expect(parseNoticeLog({ items: [] })).toBeNull();
  });

  test("status, hold reason in plain words, latency text", () => {
    const [a, b, c] = parseNoticeLog(body)!.items;
    expect(noticeStatus(a!)).toBe("held");
    expect(noticeStatus(b!)).toBe("sent");
    expect(pathText(a!.phone)).toBe("held: same stock notified moments earlier");
    expect(pathText(b!.phone)).toBe("sent to 1 device");
    expect(pathText(c!.phone)).toBe("not sent: phone alerts off for it");
    expect(pathText(undefined)).toBeNull();
    expect(heldText("quiet_hours")).toBe("quiet hours");
    expect(latencyText(b!)).toBe("trade → push 1.04 s · detect 0.10 s");
  });

  test("filters by group, status and ticker; summarises", () => {
    const items = parseNoticeLog(body)!.items;
    expect(filterNotices(items, { groups: new Set(["pullback"]), status: null, ticker: "" }).map((i) => i.event_id)).toEqual(["a"]);
    expect(filterNotices(items, { groups: new Set(), status: "sent", ticker: "" }).map((i) => i.event_id)).toEqual(["b"]);
    expect(filterNotices(items, { groups: new Set(), status: null, ticker: "aa" }).map((i) => i.event_id)).toEqual(["c"]);
    const s = noticeSummary(items);
    expect(s.sent).toBe(1);
    expect(s.held).toBe(2);
    expect(s.reasons.map((r) => [r.reason, r.n])).toEqual([["symbol_limit", 1], ["off", 1]]);
  });

  // Owner, 2026-10-30: the log showed Follow-ups with the shelf's icon and Pullback with the
  // crossings', unlike Settings → Alerts. One table now; this pins it for every kind.
  test("every notice kind has the same icon in the log as its Settings → Alerts switch", () => {
    const settings = new Map(ALERT_SWITCH_SECTIONS.flatMap((s) => s.items.map((i) => [i.key, i.icon] as const)));
    for (const kind of RUNNERS_NOTICE_KINDS) {
      const key = runnersNotifyKey(kind);
      expect(settings.has(key)).toBe(true);
      expect([kind, noticeIcon(kind)]).toEqual([kind, settings.get(key)!]);
    }
    expect(noticeIcon("runners.followup")).toBe("plus");
    expect(noticeIcon("runners.pullback")).toBe("hline");
    expect(noticeIcon("runners.shelf")).toBe("levels");
    expect(noticeIcon("price.alert")).toBe("bell");
    expect(noticeIcon("something.new")).toBe("pathRunner");
  });

  test("a breakout row is one plain line; the technical record is only in details", () => {
    const item = parseNoticeLog({ day: "d", items: [{ event_id: "runners-breakout-x", at: 1793017427000, symbol: "CJMB", notice: "runners.breakout", title: "t", body: "b", breakout: breakoutTestFixture, entry_context: ["Run +20%"] }] })!.items[0]!;
    const e = noticeEvidence(item, String);
    // $2.040001 vs $2.03: different at the cent, so cents; 800/300 = 2.67× → 2.7×.
    expect(e.line).toBe("New high $2.04 above alert $2.03 · volume 2.7× normal");
    expect(e.line).not.toMatch(/rule|evaluated|ms|ohlc|pace/);
    expect(e.details.join(" ")).toContain("15s bar high $2.040001");
    expect(e.details.join(" ")).toContain("evaluated");
    expect(e.details.join(" ")).toContain("post_flag_exact_high_v1");
    expect(e.details.at(-1)).toBe("Entry context (original alert): Run +20%");
    // The TNMG case: 80.44× prior pace reads as a whole number.
    const big = noticeEvidence({ ...item, breakout: { ...item.breakout!, high_price_micros: 8_810_000, reference_price_micros: 8_680_000, volume: 80.44 * 300, volume_ratio: (80.44 * 300) / 300 } }, String);
    expect(big.line).toBe("New high $8.81 above alert $8.68 · volume 80× normal");
    // Same cent: sub-cent prices so the line never says "$2.03 above $2.03".
    const close = noticeEvidence({ ...item, breakout: { ...item.breakout!, high_price_micros: 2_031_000 } }, String);
    expect(close.line).toBe("New high $2.0310 above alert $2.0300 · volume 2.7× normal");
    // Legacy rows: no plain line, the gap is a detail; other kinds have neither.
    expect(noticeEvidence({ ...item, breakout: undefined, entry_context: undefined }, String)).toEqual({ line: null, details: ["Event time and comparison unavailable"] });
    expect(noticeEvidence({ event_id: "p", notice: "runners.pullback" }, String)).toEqual({ line: null, details: [] });
  });
});
