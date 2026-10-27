import { describe, expect, test } from "bun:test";
import { filterNotices, heldText, latencyText, noticeKind, noticeStatus, noticeSummary, parseNoticeLog, pathText } from "./noticeLog.ts";
import { breakoutTestFixture } from "./breakoutTestFixture.ts";

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
});
