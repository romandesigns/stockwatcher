import { describe, expect, test } from "bun:test";
import {
  applyPendingMessage,
  createPendingChannel,
  EMPTY_PENDING,
  fmtPendingClock,
  fmtPendingPct,
  pendingItemCount,
  pendingReopenPct,
  pendingSentence,
  pendingSourceLabel,
  pendingStatus,
  sortPendingCards,
  validPendingCard,
  visiblePendingCards,
  type PendingCardWire,
  type PendingSnapshotMessage,
  type PendingUpdateMessage,
} from "./pending.ts";

const T0 = 1_792_075_200_000;
const card = (over: Partial<PendingCardWire> = {}): PendingCardWire => ({
  id: "XLAB-" + T0,
  sym: "XLAB",
  halted_at: T0,
  code: "T1",
  reason: "News pending",
  price: 2.76,
  prev_close: 1.9,
  chg_pct: 45.3,
  phase: "halted",
  released_at: null,
  news: [],
  offering: false,
  resume_at: null,
  resume_est: false,
  resumed_at: null,
  reopen_price: null,
  updated: T0,
  ...over,
});
const snap = (cards: PendingCardWire[], gen = 3): PendingSnapshotMessage => ({ type: "pending.snapshot", enabled: true, gen, grace_secs: 120, band: [0.1, 25], cards });
const upd = (prev_gen: number, gen: number, cards: PendingCardWire[], removed: string[] = []): PendingUpdateMessage => ({ type: "pending.update", enabled: true, prev_gen, gen, cards, removed });

describe("pending reducer", () => {
  test("a snapshot replaces everything", () => {
    const { state, resync } = applyPendingMessage(EMPTY_PENDING, snap([card()]));
    expect(resync).toBe(false);
    expect(state.enabled).toBe(true);
    expect(state.snapshot).toBe(true);
    expect(state.band).toEqual([0.1, 25]);
    expect(state.cards.map((c) => c.sym)).toEqual(["XLAB"]);
  });

  test("an update replaces cards by id and drops removed ids", () => {
    const s1 = applyPendingMessage(EMPTY_PENDING, snap([card(), card({ id: "B-1", sym: "BBB", halted_at: T0 + 1000 })])).state;
    const news = card({ phase: "news", news: [{ id: "n1", source: "alpaca/benzinga", label: "Benzinga", headline: "XLAB wins contract", url: "https://example.com/x", at: T0 + 60_000, seen: T0 + 61_000, kind: "catalyst" }] });
    const { state, resync } = applyPendingMessage(s1, upd(3, 5, [news], ["B-1"]));
    expect(resync).toBe(false);
    expect(state.gen).toBe(5);
    expect(state.cards).toHaveLength(1);
    expect(state.cards[0]!.phase).toBe("news");
    expect(state.cards[0]!.news[0]!.headline).toBe("XLAB wins contract");
  });

  test("an update that does not continue this state asks for a snapshot", () => {
    const s1 = applyPendingMessage(EMPTY_PENDING, snap([card()])).state;
    const r = applyPendingMessage(s1, upd(7, 8, [card({ phase: "resuming" })]));
    expect(r.resync).toBe(true);
    expect(r.state.cards[0]!.phase).toBe("halted");
    // Before any snapshot too.
    expect(applyPendingMessage(EMPTY_PENDING, upd(0, 1, [])).resync).toBe(true);
  });

  test("the channel resubscribes on a gap and opts out with the last listener", () => {
    const calls: string[] = [];
    const ch = createPendingChannel({ subscribe: () => calls.push("sub"), unsubscribe: () => calls.push("unsub") });
    const off = ch.subscribe(() => {});
    ch.apply(snap([card()]));
    ch.apply(upd(9, 10, []));
    off();
    expect(calls).toEqual(["sub", "sub", "unsub"]);
    expect(ch.get().cards).toHaveLength(0);
  });
});

describe("pending cards", () => {
  test("invalid cards are dropped and unsafe links never followed", () => {
    expect(validPendingCard({ sym: "X" })).toBeNull();
    const c = validPendingCard({ ...card(), news: [{ id: "n", source: "sec_edgar/424B5", label: "", headline: null, url: "javascript:alert(1)", at: T0, seen: T0, kind: "offering" }], offering: false });
    expect(c?.news[0]!.url).toBeNull();
    expect(c?.news[0]!.label).toBe("SEC 424B5");
    // An offering item flags the card even if the server's flag were missing.
    expect(c?.offering).toBe(true);
  });

  test("live halts first, soonest reopen first, resumed last", () => {
    const order = sortPendingCards([
      card({ id: "r", phase: "resumed", resumed_at: T0 }),
      card({ id: "h", halted_at: T0 + 5000 }),
      card({ id: "s", phase: "resuming", resume_at: T0 + 300_000 }),
    ]).map((c) => c.id);
    expect(order).toEqual(["s", "h", "r"]);
  });

  test("resumed cards leave after the grace period even without the removal", () => {
    const s = applyPendingMessage(EMPTY_PENDING, snap([card({ phase: "resumed", resumed_at: T0 })])).state;
    expect(visiblePendingCards(s, T0 + 119_000)).toHaveLength(1);
    expect(pendingItemCount(s, T0 + 120_000)).toBe(0);
  });

  test("timers, labels and the spoken sentence", () => {
    expect(fmtPendingClock(272_400)).toBe("4:32");
    expect(fmtPendingClock(-5)).toBe("0:00");
    expect(fmtPendingClock(3_725_000)).toBe("1:02:05");
    expect(fmtPendingPct(45.25)).toBe("+45.3%");
    expect(pendingSourceLabel("alpaca/benzinga")).toBe("Benzinga");
    expect(pendingSourceLabel("press_wire/globenewswire")).toBe("GlobeNewswire");
    expect(pendingSourceLabel("sec_edgar/8-K")).toBe("SEC 8-K");
    expect(pendingStatus(card())).toBe("News pending");
    expect(pendingStatus(card({ phase: "news", offering: true }))).toBe("Offering filed");
    expect(pendingStatus(card({ phase: "resuming" }))).toBe("Resumption set");
    expect(pendingReopenPct(card({ reopen_price: 3.036 }))).toBeCloseTo(10, 5);
    const s = pendingSentence(card({ phase: "resuming", resume_at: T0 + 600_000, resume_est: true }), T0 + 400_000);
    expect(s).toContain("about 3:20 to the reopen");
    expect(pendingSentence(card(), T0 + 61_000)).toContain("halted for 1:01");
  });
});
