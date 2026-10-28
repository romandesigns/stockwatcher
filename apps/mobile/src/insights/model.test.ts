import { describe, expect, test } from "bun:test";

import { noticeRowView } from "./model";
import { breakoutTestFixture } from "../../../../packages/shared/src/breakoutTestFixture.ts";

const AT = Date.UTC(2026, 9, 26, 12, 23, 47); // 08:23:47 ET

describe("Insights row on the phone", () => {
  test("measured cause and event context remain separate from history clock and old entry reasons", () => {
    const item = { event_id: "runners-breakout-fixture", at: AT, symbol: "CJMB", notice: "runners.breakout", title: "immutable title", body: "immutable body", breakout: breakoutTestFixture, entry_context: ["original +20% reason"], phone: { decision: "held", held: "quiet_hours", devices: 1 } };
    const r = noticeRowView(item);
    expect(r.time).toBe("8:23:47 AM");
    expect(r.kind).toBe("15s new high since alert");
    expect(r.detail).toBe("Held: quiet hours");
    expect(r.evidence).toContain("15s bar high $2.040001");
    expect(r.evidence).toContain("+250 ms");
    expect(r.evidence).toContain("Entry context (original alert): original +20% reason");
    expect(r.a11y).toContain("immutable title · immutable body");
    expect(r.noticeContext?.event_id).toBe(item.event_id);
    const legacy = noticeRowView({ ...item, breakout: undefined });
    expect(legacy.kind).toBe("Breakout");
    expect(legacy.evidence).toContain("Event time and comparison unavailable");
    expect(legacy.noticeContext).toBeUndefined();
  });
  test("a held notice leads with the plain reason and marks the phone path", () => {
    const r = noticeRowView({ event_id: "a", at: AT, symbol: "SRTM", notice: "runners.pullback", title: "SRTM · Pullback setting up", desk: { decision: "sent", held: null, devices: 1 }, phone: { decision: "held", held: "symbol_limit", devices: 1 } });
    expect(r.time).toBe("8:23:47 AM");
    expect(r.status).toBe("Held");
    expect(r.detail).toBe("Held: same stock notified moments earlier");
    expect(r.phone).toEqual({ label: "Phones: held: same stock notified moments earlier", sent: false });
    expect(r.desk?.sent).toBe(true);
    expect(r.icon).toBe("trend");
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
});
