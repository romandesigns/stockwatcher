import { describe, expect, test } from "bun:test";

import { noticeRowView } from "./model";

const AT = Date.UTC(2026, 9, 26, 12, 23, 47); // 08:23:47 ET

describe("Insights row on the phone", () => {
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

  test("a sent notice shows its text; the latency reaches the read-out", () => {
    const r = noticeRowView({ event_id: "b", at: AT, symbol: "PLAG", notice: "runners.burst", title: "PLAG · Buying burst", body: "Alert $2.41", phone: { decision: "sent", held: null, devices: 1 }, latency: { trade_to_expo_ms: 1040 } });
    expect(r.status).toBe("Sent");
    expect(r.detail).toBe("PLAG · Buying burst · Alert $2.41");
    expect(r.icon).toBe("pathBurst");
    expect(r.desk).toBeNull();
    expect(r.a11y).toContain("trade → push 1.04 s");
  });
});
