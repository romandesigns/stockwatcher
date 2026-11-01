import { describe, expect, test } from "bun:test";

import { ALERT_SWITCH_SECTIONS, RUNNERS_NOTICE_KINDS, runnersNotifyKey } from "@stockwatcher/shared";

import { noticeRowView } from "./model";
import { breakoutTestFixture } from "../../../../packages/shared/src/breakoutTestFixture.ts";

const AT = Date.UTC(2026, 9, 26, 12, 23, 47); // 08:23:47 ET

describe("Insights row on the phone", () => {
  test("a row the server assembled (notices.rows.v1) is shown as the server says", () => {
    const r = noticeRowView({ event_id: "e", at: AT, symbol: "ZZTK", notice: "runners.fast", title: "t", phone: { decision: "sent", held: null, devices: 1 }, status: "held", reason: "stale", reason_text: "too old by the time it was decided" });
    expect(r.sent).toBe(false);
    expect(r.detail).toBe("Held: too old by the time it was decided");
  });

  test("measured cause and event context remain separate from history clock and old entry reasons", () => {
    const item = { event_id: "runners-breakout-fixture", at: AT, symbol: "CJMB", notice: "runners.breakout", title: "immutable title", body: "immutable body", breakout: breakoutTestFixture, entry_context: ["original +20% reason"], phone: { decision: "held", held: "quiet_hours", devices: 1 } };
    const r = noticeRowView(item);
    expect(r.time).toBe("8:23:47 AM");
    expect(r.kind).toBe("15s new high since alert");
    expect(r.detail).toBe("Held: quiet hours");
    // One plain line on the row; the measured record only in the Details sheet.
    expect(r.line).toBe("New high $2.04 above alert $2.03 · volume 2.7× normal");
    expect(r.details.join(" ")).toContain("15s bar high $2.040001");
    expect(r.details.join(" ")).toContain("+250 ms");
    expect(r.details).toContain("Entry context (original alert): original +20% reason");
    expect(r.a11y).not.toContain("+250 ms");
    expect(r.a11y).toContain("immutable title · immutable body");
    expect(r.noticeContext?.event_id).toBe(item.event_id);
    const legacy = noticeRowView({ ...item, breakout: undefined });
    expect(legacy.kind).toBe("Breakout");
    expect(legacy.line).toBeNull();
    expect(legacy.details).toContain("Event time and comparison unavailable");
    expect(legacy.noticeContext).toBeUndefined();
  });
  test("a held notice leads with the plain reason and marks the phone path", () => {
    const r = noticeRowView({ event_id: "a", at: AT, symbol: "SRTM", notice: "runners.pullback", title: "SRTM · Pullback setting up", desk: { decision: "sent", held: null, devices: 1 }, phone: { decision: "held", held: "symbol_limit", devices: 1 } });
    expect(r.time).toBe("8:23:47 AM");
    expect(r.status).toBe("Held");
    expect(r.detail).toBe("Held: same stock notified moments earlier");
    expect(r.phone).toEqual({ label: "Phones: held: same stock notified moments earlier", sent: false });
    expect(r.desk?.sent).toBe(true);
    // Settings → Alerts gives "Pullback setting up" the —•— line (owner, 2026-10-30).
    expect(r.icon).toBe("hline");
    expect(r.details).toEqual([]);
    expect(r.a11y).toContain("SRTM, Pullback");
  });

  test("a merged notice names the alert it was folded into", () => {
    const into = { event_id: "runners-running-WFF-1", notice: "runners.running", secs: 1 };
    const r = noticeRowView({ event_id: "f", at: AT, symbol: "WFF", notice: "runners.fast", title: "WFF · Fast move", phone: { decision: "held", held: "merged", devices: 1, merged_into: into, merged_devices: 1 } });
    expect(r.detail).toBe("Held: merged into the Running alert 1 s earlier");
    expect(r.a11y).toContain("held, merged into the Running alert 1 s earlier");
  });

  test("a sent notice shows its text; the latency reaches the read-out", () => {
    const r = noticeRowView({ event_id: "b", at: AT, symbol: "PLAG", notice: "runners.burst", title: "PLAG · Buying burst", body: "Alert $2.41", phone: { decision: "sent", held: null, devices: 1 }, latency: { trade_to_expo_ms: 1040 } });
    expect(r.status).toBe("Sent");
    expect(r.detail).toBe("PLAG · Buying burst · Alert $2.41");
    expect(r.icon).toBe("pathBurst");
    expect(r.desk).toBeNull();
    expect(r.a11y).toContain("trade → push 1.04 s");
  });

  test("every notice kind's row icon is its Settings → Alerts switch's icon", () => {
    const settings = new Map(ALERT_SWITCH_SECTIONS.flatMap((sec) => sec.items.map((i) => [i.key, i.icon] as const)));
    for (const notice of RUNNERS_NOTICE_KINDS) {
      const r = noticeRowView({ event_id: notice, at: AT, symbol: "TNMG", notice, title: "t" });
      expect([notice, r.icon]).toEqual([notice, settings.get(runnersNotifyKey(notice))!]);
    }
    expect(noticeRowView({ event_id: "f", notice: "runners.followup" }).icon).toBe("plus");
  });
});
