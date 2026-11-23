import { describe, expect, test } from "bun:test";
import { attachPracticeToasts, createToastOnce, isPracticeNoticeFrame, parsePracticeNotice, practiceToast } from "./practice.ts";
import { DEFAULT_RUNNERS_NOTIFY, parseRunnersNotice } from "./runnersNotify.ts";

const wire = { type: "practice.notice", practice: true, id: "practice-runners-new-2026-11-17-CJMB-1", alertId: "2026-11-17-CJMB-1", symbol: "CJMB", kind: "runners.new", title: "CJMB $2.41 · Runners", body: "$120k in 5 min\n$2.41", data: { symbol: "CJMB" }, at: 1763395200000, day: "2026-11-17" };

describe("practice notices", () => {
  test("parsed from their own frame type only", () => {
    const raw = JSON.stringify(wire);
    expect(isPracticeNoticeFrame(raw)).toBe(true);
    expect(parsePracticeNotice(JSON.parse(raw))).toMatchObject({ type: "practice.notice", practice: true, symbol: "CJMB", kind: "runners.new", day: "2026-11-17", at: 1763395200000 });
    // A live notifier's parser never takes one for a real alert.
    expect(parseRunnersNotice(wire)).toBeNull();
    expect(isPracticeNoticeFrame(JSON.stringify({ ...wire, type: "runners.notice" }))).toBe(false);
    // Neither may a live notice pass for a practice one.
    expect(parsePracticeNotice({ ...wire, type: "runners.notice" })).toBeNull();
    expect(parsePracticeNotice({ ...wire, practice: false })).toBeNull();
    expect(parsePracticeNotice({ ...wire, id: "runners-new-x" })).toBeNull();
  });

  test("practice and live toasts carry different labels and follow the device's switches", () => {
    const practice = parsePracticeNotice(wire)!;
    const live = parseRunnersNotice({ ...wire, type: "runners.notice", id: "runners-new-2026-11-17-CJMB-1" })!;
    expect(practiceToast(practice, DEFAULT_RUNNERS_NOTIFY)).toEqual({ label: "practice", symbol: "CJMB", text: "CJMB $2.41 · Runners · $120k in 5 min" });
    expect(practiceToast(live, DEFAULT_RUNNERS_NOTIFY)?.label).toBe("live");
    expect(practiceToast(practice, { ...DEFAULT_RUNNERS_NOTIFY, new: false })).toBeNull();
  });

  test("each id toasts once", () => {
    const once = createToastOnce(2);
    expect(once("a")).toBe(true);
    expect(once("a")).toBe(false);
    once("b");
    once("c");
    expect(once("a")).toBe(true);
  });
});

describe("attachPracticeToasts", () => {
  test("both streams toast once per id with their own label; unsubscribing stops them", () => {
    const listeners = { practice: [] as ((m: never) => void)[], live: [] as ((m: never) => void)[] };
    const stream = (k: "practice" | "live") => ({ subscribe: (fn: (m: never) => void) => (listeners[k].push(fn), () => void (listeners[k] = listeners[k].filter((f) => f !== fn))) });
    const shown: string[] = [];
    const off = attachPracticeToasts({ practiceNotices: stream("practice"), liveNotices: stream("live") }, { kinds: () => DEFAULT_RUNNERS_NOTIFY, show: (t) => shown.push(`${t.label}:${t.symbol}`) });
    const practice = parsePracticeNotice(wire)!;
    const live = parseRunnersNotice({ ...wire, type: "runners.notice", id: "runners-new-2026-11-17-CJMB-1", symbol: "LIVE" })!;
    for (const f of listeners.practice) f(practice as never);
    for (const f of listeners.practice) f(practice as never);
    for (const f of listeners.live) f(live as never);
    expect(shown).toEqual(["practice:CJMB", "live:LIVE"]);
    off();
    expect(listeners.practice).toHaveLength(0);
    expect(listeners.live).toHaveLength(0);
  });
});
