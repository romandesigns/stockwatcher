import { expect, test } from "bun:test";
import { applyQuotesMessage, createQuoteChannel, EMPTY_QUOTE_BOARD, quoteChangePct } from "./quotes.ts";

const snapshot = applyQuotesMessage(EMPTY_QUOTE_BOARD, {
  type: "quotes.snapshot",
  t: 1,
  quotes: [
    { s: "AAA", p: 5.5, o: 5, v: 100, n: 2, name: "Triple A Corp", x: "NYSE" },
    { s: "BBB", p: 0.25, o: 0.2, v: 50, n: 1, pre: true },
  ],
});

test("a snapshot builds the board, with change % against the reference", () => {
  expect(snapshot.size).toBe(2);
  const a = snapshot.get("AAA")!;
  expect(a).toMatchObject({ price: 5.5, open: 5, volume: 100, trades: 2, preOpen: false, name: "Triple A Corp", exchange: "NYSE" });
  expect(a.changePct).toBeCloseTo(10, 9);
  expect(snapshot.get("BBB")!.preOpen).toBe(true);
  expect(snapshot.get("BBB")!.name).toBeUndefined();
});

test("an update upserts its rows, keeps names it leaves out and drops gone symbols", () => {
  const next = applyQuotesMessage(snapshot, {
    type: "quotes.update",
    t: 2,
    quotes: [
      { s: "AAA", p: 4.5, o: 5, v: 300, n: 5 },
      { s: "CCC", p: 1, o: 1, v: 10, n: 1, name: "Entering Inc.", x: "NASDAQ" },
    ],
    gone: ["BBB"],
  });
  expect([...next.keys()].sort()).toEqual(["AAA", "CCC"]);
  expect(next.get("AAA")).toMatchObject({ price: 4.5, volume: 300, name: "Triple A Corp", exchange: "NYSE" });
  expect(next.get("AAA")!.changePct).toBeCloseTo(-10, 9);
  expect(next.get("CCC")!.name).toBe("Entering Inc.");
  expect(snapshot.get("AAA")!.price).toBe(5.5); // the old board is not mutated
});

test("a snapshot replaces the board (after a seek, stale rows must not linger)", () => {
  const next = applyQuotesMessage(snapshot, { type: "quotes.snapshot", t: 3, quotes: [{ s: "AAA", p: 5, o: 5, v: 1, n: 1 }] });
  expect([...next.keys()]).toEqual(["AAA"]);
  expect(next.get("AAA")!.changePct).toBe(0);
});

test("a zero reference gives 0 %, not Infinity", () => {
  const b = applyQuotesMessage(EMPTY_QUOTE_BOARD, { type: "quotes.snapshot", t: 0, quotes: [{ s: "Z", p: 1, o: 0, v: 1, n: 1 }] });
  expect(b.get("Z")!.changePct).toBe(0);
});

test("the channel opts in with the first listener and out with the last", () => {
  const calls: string[] = [];
  const ch = createQuoteChannel({ subscribe: () => calls.push("sub"), unsubscribe: () => calls.push("unsub") });
  let seen = 0;
  const off1 = ch.subscribe(() => seen++);
  const off2 = ch.subscribe(() => seen++);
  expect(calls).toEqual(["sub"]);
  ch.apply({ type: "quotes.snapshot", t: 0, quotes: [{ s: "A", p: 1, o: 1, v: 1, n: 1 }] });
  expect(seen).toBe(2);
  expect(ch.get().size).toBe(1);
  off1();
  off1(); // twice is harmless
  expect(calls).toEqual(["sub"]);
  off2();
  expect(calls).toEqual(["sub", "unsub"]);
  expect(ch.get().size).toBe(0); // stale values never outlive the subscription
});

test("change % is against the previous close when the server knows it, else the open", () => {
  const b = applyQuotesMessage(EMPTY_QUOTE_BOARD, {
    type: "quotes.snapshot",
    t: 0,
    quotes: [
      // Up 10% on the open but down 8.3% on yesterday's close: a broker shows the latter.
      { s: "AAA", p: 5.5, o: 5, pc: 6, v: 1, n: 1 },
      { s: "BBB", p: 5.5, o: 5, v: 1, n: 1, pre: true },
    ],
  });
  expect(b.get("AAA")!.prevClose).toBe(6);
  expect(b.get("AAA")!.changePct).toBeCloseTo(-8.3333, 3);
  expect(b.get("BBB")!.prevClose).toBeUndefined();
  expect(b.get("BBB")!.changePct).toBeCloseTo(10, 9);
  expect(quoteChangePct(1, 0, 2)).toBe(-50); // a 0 close is no reference
  expect(quoteChangePct(1, undefined, 0)).toBe(0);
});

test("updates leave pc out unless it changed; a later snapshot is the whole truth", () => {
  const b = applyQuotesMessage(EMPTY_QUOTE_BOARD, { type: "quotes.snapshot", t: 0, quotes: [{ s: "AAA", p: 5.5, o: 5, pc: 6, v: 1, n: 1 }] });
  const u = applyQuotesMessage(b, { type: "quotes.update", t: 1, quotes: [{ s: "AAA", p: 6.6, o: 5, v: 2, n: 2 }] });
  expect(u.get("AAA")!.prevClose).toBe(6);
  expect(u.get("AAA")!.changePct).toBeCloseTo(10, 9);
  // The row's trading date moved on: the update carries the new reference.
  const next = applyQuotesMessage(u, { type: "quotes.update", t: 2, quotes: [{ s: "AAA", p: 6.6, o: 6.5, pc: 6.6, v: 3, n: 3 }] });
  expect(next.get("AAA")!.changePct).toBe(0);
  // A snapshot without pc (e.g. a server restarted without the closes file) does not keep the old one.
  const fresh = applyQuotesMessage(next, { type: "quotes.snapshot", t: 3, quotes: [{ s: "AAA", p: 6.6, o: 6, v: 3, n: 3 }] });
  expect(fresh.get("AAA")!.prevClose).toBeUndefined();
  expect(fresh.get("AAA")!.changePct).toBeCloseTo(10, 9);
});
