import { describe, expect, test } from "bun:test";

import { parseServerMessage } from "./protocol.ts";
import { applyPrewatchMessage, createPrewatchChannel, EMPTY_PREWATCH, prewatchItemCount, watchCardView, type PrewatchSnapshotMessage } from "./prewatch.ts";

// AKAN 11-24 as the server's card carries it (crates/server/src/prewatch tests).
const AKAN = { symbol: "AKAN", since: Date.parse("2026-11-24T13:00:00Z"), price: 3.27, c1: 2.58, gap: 0.2674, pmVol: 3_560_000, float: 2_390_000, pmVolPct: 1.49, bigDay: { date: "2026-11-18", run: 1.28 }, fails: [], partial: false };
const snap = (cards: unknown[], extra: Partial<PrewatchSnapshotMessage> = {}) => ({ type: "prewatch.snapshot", enabled: true, date: "2026-11-24", floatSource: "fmp", cards, ...extra }) as PrewatchSnapshotMessage;

describe("pre-open Watch card", () => {
  test("a snapshot parses, replaces the cards, puts still-matching ones first and drops bad ones", () => {
    const m = parseServerMessage(JSON.stringify(snap([{ ...AKAN, symbol: "FADE", since: AKAN.since + 1, fails: ["gap", "bogus"] }, AKAN, { symbol: "BAD" }])));
    expect(m?.type).toBe("prewatch.snapshot");
    const s = applyPrewatchMessage(EMPTY_PREWATCH, m as PrewatchSnapshotMessage);
    expect(s.cards.map((c) => c.symbol)).toEqual(["AKAN", "FADE"]);
    expect(s.cards[1]!.fails).toEqual(["gap"]);
    expect(prewatchItemCount(s)).toBe(2);
    expect(prewatchItemCount(applyPrewatchMessage(s, snap([AKAN], { enabled: false })))).toBe(0);
  });

  test("the card says why it matched, and what stopped holding", () => {
    const v = watchCardView(AKAN);
    expect(v.price).toBe("$3.27");
    expect(v.gap).toBe("+27% gap");
    expect(v.reasons).toEqual([
      { label: "Gap", value: "+27% over 2.58" },
      { label: "Pre-market vol", value: "3.56M · 149% of float" },
      { label: "Float", value: "2.39M" },
      { label: "Big day", value: "+128% high on 11/18" },
    ]);
    expect(v.since).toBe("Since 08:00");
    expect(v.faded).toBeNull();
    expect(watchCardView({ ...AKAN, fails: ["gap"], partial: true })).toMatchObject({ faded: "No longer: gap under +15%", since: "Since 08:00 · volume counted from the server's start" });
  });

  test("the channel opts in on the first listener and out on the last", () => {
    const io = { n: 0, subscribe() { this.n++; }, unsubscribe() { this.n--; } };
    const ch = createPrewatchChannel(io);
    const off = ch.subscribe(() => {});
    expect(io.n).toBe(1);
    ch.apply(snap([AKAN]));
    expect(ch.get().cards.length).toBe(1);
    off();
    expect(io.n).toBe(0);
    expect(ch.get().cards.length).toBe(0);
  });
});
