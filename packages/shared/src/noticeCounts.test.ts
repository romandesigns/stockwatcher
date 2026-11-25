import { describe, expect, test } from "bun:test";

import { createNoticeCards, noticeCardLabel } from "./noticeCounts.ts";
import { parseRunnersNotice, shouldShowNotice, createNoticeDedupe, DEFAULT_RUNNERS_NOTIFY } from "./runnersNotify.ts";
import { parseServerMessage } from "./protocol.ts";

const AT = Date.parse("2026-11-24T14:33:00Z"); // 09:33 ET
const frame = (type: string, id: string, at: number, count?: number) => JSON.stringify({ type, id, alertId: "a1", symbol: "AKAN", kind: "signals.trigger", title: `AKAN · Signals · ${id}`, body: "b", at, count });

describe("group repeat alerts (one card per stock per day)", () => {
  test("a grouped frame parses as a notice that never shows", () => {
    const m = parseServerMessage(frame("runners.grouped", "g1", AT, 5));
    expect(m && m.type === "runners.notice" && m.grouped).toBe(true);
    const n = m as NonNullable<ReturnType<typeof parseRunnersNotice>>;
    expect(n.count).toBe(5);
    expect(shouldShowNotice(n, { ...DEFAULT_RUNNERS_NOTIFY, signalsTrigger: true }, createNoticeDedupe())).toBe(false);
    // An ordinary notice still shows, and carries its count.
    const s = parseRunnersNotice(JSON.parse(frame("runners.notice", "s1", AT, 1)))!;
    expect(s.grouped).toBeUndefined();
    expect(shouldShowNotice(s, { ...DEFAULT_RUNNERS_NOTIFY, signalsTrigger: true }, createNoticeDedupe())).toBe(true);
  });

  test("the card keeps the count and the latest notice, per ET day", () => {
    const cards = createNoticeCards();
    const p = (id: string, at: number, count?: number) => parseRunnersNotice(JSON.parse(frame(id.startsWith("g") ? "runners.grouped" : "runners.notice", id, at, count)))!;
    expect(cards.record(p("s1", AT, 1))).toBe(true);
    expect(noticeCardLabel(cards.get("AKAN", AT))).toBeNull();
    cards.record(p("g2", AT + 60_000, 2));
    cards.record(p("g3", AT + 120_000, 3));
    const c = cards.get("AKAN", AT + 120_000)!;
    expect(c.count).toBe(3);
    expect(c.title).toContain("g3");
    expect(noticeCardLabel(c)).toBe("3 alerts · 09:35");
    // A late, older frame never rolls the card back.
    cards.record(p("g1", AT - 60_000, 1));
    expect(cards.get("AKAN", AT + 120_000)!.count).toBe(3);
    // The next ET day starts a new card.
    expect(cards.get("AKAN", AT + 24 * 3600_000)).toBeNull();
  });
});
