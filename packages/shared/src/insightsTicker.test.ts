import { describe, expect, test } from "bun:test";
import { alertKind, createTickerChannel, etDayShort, NO_TICKER, INSIGHTS_TICKER_FEATURE, money, parseTickerPackage, signedPct, tickerPageHash, tickerPageOf, tickerPageView, type TickerSnapshotMessage } from "./insightsTicker.ts";
import { applyNoticesMessage } from "./noticeFeed.ts";
import { logSummary, rowHeldText, rowStatus } from "./noticeLog.ts";
import { createWsClient, type WebSocketLike } from "./ws.ts";

// A synthetic package in the server's shape (crates/server/src/insights/package.rs), for a
// fictional symbol: no real market data in the repo.
const T0 = Date.UTC(2026, 9, 30, 20, 32, 30); // 4:32:30 PM ET
const outcome = (o: Record<string, unknown> = {}) => ({ ref_price: 0.7264, ref_at: T0, best_price: 2.71, best_at: T0 + 1_500_000, best_pct: 273.1, plus5_price: 0.7627, plus5_at: T0 + 20_000, stop: null, stop_hit_at: null, crossing: null, crossed_after_stop_at: null, through: T0 + 9_000_000, ...o });
const PKG = {
  v: 1,
  symbol: "ZZTK",
  day: "2026-10-30",
  today: false,
  name: "Fictional Corp",
  prev_day: { day: "2026-10-29", archived: false },
  next_day: null,
  available: true,
  stats: {
    prev_close: { day: "2026-10-29", price: 0.77 },
    close: 0.6038,
    change: { abs: -0.17, pct: -21.6 },
    last: 2.06,
    last_at: T0 + 12_000_000,
    last_session: "after",
    regular_close_at: T0 - 1_950_000,
    sessions: { pre: null, regular: { open: 0.86, high: 0.86, high_at: T0 - 9e6, low: 0.59, low_at: T0 - 8e6, close: 0.6038, volume: 1000, dollar_volume: 800 }, after: { open: 0.6, high: 2.71, high_at: T0 + 1_500_000, low: 0.59, low_at: T0 + 200_000, close: 2.06, volume: 63_541_395, dollar_volume: 131_539_613 } },
    volume: 80_501_707,
    dollar_volume: 143_837_011,
    high: { price: 2.71, at: T0 + 1_500_000, session: "after" },
    float: { available: false, reason: "no_float_record_on_or_before_day" },
    short_interest: { available: false, reason: "not_collected" },
  },
  bars: [{ t: T0 - 30_000, o: 0.7, h: 0.73, l: 0.69, c: 0.72, v: 100, n: 3, final: true }, { t: "bad" }],
  alerts: [
    { event_id: "runners-fast-x", n: 1, marker: 1, at: T0, price: 0.7264, notice: "runners.fast", label: "Fast move", title: "ZZTK · Fast move · $0.7264", body: "+17.7% in 15 s · 108 trades, $41k\nAlert $0.7264", status: "sent", reason: null, reason_text: null, outcome: outcome() },
    { event_id: "runners-pullback-x", n: 2, at: T0 + 700_000, price: 1.855, price_text: "$1.86", notice: "runners.pullback", label: "Pullback", title: "ZZTK · Pullback setting up · $1.86", body: "Trigger $2.00", status: "held", reason: "quiet_hours", reason_text: "quiet hours", outcome: outcome({ ref_price: 1.855, best_pct: 46.1, stop: 1.84, stop_hit_at: T0 + 900_000 }) },
  ],
  markers: [{ kind: "alert", n: 1, at: T0, price: 0.7264 }, { kind: "missed", at: T0 + 300_000, price: 1.3 }, { kind: "bogus", at: 1, price: 1 }],
  scorecard: { alerts: 2, sent: 1, held: 1, reached_5pct: 2, best_entry: { notice: "runners.fast", marker: 1, best_pct: 273.1 }, setups_armed: 3, missed: 5, by_kind: [{ notice: "runners.fast", label: "Fast move", alerts: 1, sent: 1, reached_5pct: 1, best_pct: 273.1 }] },
  halts: { halts: [], move_session: "after", outside_regular: true, luld_applies: false, closest: { up: { bp: 1746, at: T0 - 2e7, price: 0.8377, band: 0.984 }, down: null, regular_prints: 10 } },
  missed: Array.from({ length: 5 }, (_, i) => ({ episode_id: `ep:${i + 1}`, armed_at: T0 + i * 60_000, trigger: 1.36, stop: 1.22, reason: i < 2 ? "warmup" : "one_per_day", reason_text: i < 2 ? "muted: warm-up after a server restart" : "held: one setup notice per stock per day, already sent", restart_at: i < 2 ? T0 - 60_000 : null, flagged_at: null, outcome: outcome({ ref_price: 1.3, best_pct: i === 3 ? 0.4 : 108.5, plus5_at: i === 3 ? null : T0, stop_hit_at: i >= 2 ? T0 + 9e5 : null }) })),
};
const fmt = (ms: number, s?: boolean) => `t${(ms - T0) / 1000}${s ? "s" : ""}`;

describe("parseTickerPackage", () => {
  test("keeps the shape the screens index into and drops malformed rows", () => {
    const p = parseTickerPackage(PKG)!;
    expect(p.symbol).toBe("ZZTK");
    expect(p.bars).toHaveLength(1);
    expect(p.markers.map((m) => m.kind)).toEqual(["alert", "missed"]);
    expect(p.next_day).toBeNull();
    expect(p.focus).toBeNull();
    // The chart's opening window passes through as the server sent it; a malformed one is dropped.
    expect(parseTickerPackage({ ...PKG, focus: { from: T0 - 1_800_000, to: T0 + 2_400_000, reason: "alerts" } })!.focus).toEqual({ from: T0 - 1_800_000, to: T0 + 2_400_000 });
    expect(parseTickerPackage({ ...PKG, focus: { from: T0, to: T0 - 1 } })!.focus).toBeNull();
    expect(parseTickerPackage({ ...PKG, focus: { from: "x", to: T0 } })!.focus).toBeNull();
    expect(parseTickerPackage({ day: "2026-10-30" })).toBeNull();
    const off = parseTickerPackage({ symbol: "ZZTK", day: "2026-10-29", available: false, unavailable_reason: "not_archived" })!;
    expect(off.available).toBe(false);
    expect(off.stats).toBeNull();
  });
});

describe("point-in-time wording", () => {
  test("a float retrieved on an ET evening is dated that ET day, not the next UTC one", () => {
    const eve = Date.UTC(2026, 9, 31, 1, 30); // Fri Oct 30, 9:30 PM ET = Oct 31 UTC
    expect(etDayShort(eve)).toBe("Oct 30");
    const st = { ...PKG.stats, float: { available: true, shares: 12_000_000, as_of: eve, provider: "fmp" } };
    const v = tickerPageView(parseTickerPackage({ ...PKG, stats: st })!, fmt);
    expect(v.stats.find((x) => x.id === "float")!.note).toBe("as of Oct 30");
  });
  test("a feed without a ticker channel never builds a page (not 'no welcome yet')", () => {
    expect(NO_TICKER.available).toBe(false);
  });
});

describe("tickerPageView: words only, every number the server's", () => {
  const v = tickerPageView(parseTickerPackage(PKG)!, fmt);
  test("stats", () => {
    expect(v.stats.map((s) => s.label)).toEqual(["Last · t12000", "Change", "Premarket", "Regular session", "After hours", "Volume", "Float (est.)", "Short interest"]);
    // The last print leads, timed; the regular close is the note, quoted at the session's end.
    expect(v.stats[0]).toMatchObject({ id: "last", value: "$2.06", note: "close $0.6038 at t-1950" });
    expect(v.stats[1]).toMatchObject({ value: "-21.6%", tone: "down", note: "vs prev close $0.7700" });
    expect(v.stats[2]).toMatchObject({ value: "—", note: "no trades" });
    expect(v.stats[5]).toMatchObject({ value: "80.5M", note: "$143.8M" });
    expect(v.stats[7]).toMatchObject({ value: "—", note: "not collected" });
    expect(v.headline).toMatchObject({ price: "$2.06", change: "-21.6%", note: "t12000 · close $0.6038 at t-1950" });
  });
  test("Change leads with the last price's move when the day moved after the close (schema 3)", () => {
    const pkg = { ...PKG, stats: { ...PKG.stats, last_change: { abs: 1.29, pct: 167.5 } } };
    const w = tickerPageView(parseTickerPackage(pkg)!, fmt);
    expect(w.stats[1]).toMatchObject({ value: "+167.5%", tone: "up", note: "4 PM close -21.6% · prev close $0.7700" });
    expect(w.headline).toMatchObject({ change: "+167.5%", tone: "up" });
  });
  test("history rows: the notice's own name, outcomes, and the server's hold reason", () => {
    expect(v.alerts[0]).toMatchObject({ marker: 1, kind: "Fast move", detail: "+17.7% in 15 s · 108 trades, $41k", price: "$0.7264", best: "$2.71", bestPct: "+273.1%", plus5: "t20s", stop: "—", held: null });
    // The Price column is the notice's own text: $1.86, not a re-rounded 1.855 ("$1.85").
    expect(v.alerts[1]!.price).toBe("$1.86");
    expect(v.alerts[1]).toMatchObject({ marker: null, sent: false, kind: "Pullback setting up", stop: "hit t900s", stopTone: "down", held: "Quiet hours" });
    expect(v.alertSummary).toBe("1 sent · 1 held");
  });
  test("scorecard, halts and missed setups with the overflow", () => {
    expect(v.score.map((k) => k.value)).toEqual(["1 sent", "2 of 2", "+273.1%", "5"]);
    expect(v.score[2]!.note).toBe("Fast move · #1");
    expect(v.halts.lines).toEqual(["No halts recorded that day."]);
    expect(v.halts.luld).toContain("after hours");
    expect(v.halts.closest).toEqual(["17.5% below the upper band at t-20000 ($0.8377 vs $0.9840)"]);
    expect(v.missed).toHaveLength(4);
    expect(v.missedLater).toHaveLength(1);
    expect(v.missed[0]).toMatchObject({ outcome: "best +108.5%", outcomeTone: "up", why: "Muted: warm-up after a server restart (restart at t-60s)" });
    expect(v.missed[2]!.outcome).toBe("+5%, then stopped t900");
    expect(v.missed[3]).toMatchObject({ outcome: "stopped t900", outcomeTone: "down" });
  });
  test("an unavailable day says why", () => {
    const off = tickerPageView(parseTickerPackage({ symbol: "ZZTK", day: "2026-10-31", available: false, unavailable_reason: "not_a_trading_day" })!, fmt);
    expect(off.unavailable).toBe("The market was closed that day.");
  });
});

test("money, percent and the notice name", () => {
  expect(money(0.7264)).toBe("$0.7264");
  expect(money(2.01)).toBe("$2.01");
  expect(money(null)).toBe("—");
  expect(signedPct(46.1)).toBe("+46.1%");
  expect(alertKind({ title: "ZZTK · Run +20% (after Fast move 20 s ago) · $0.7800", label: "Follow-up" }, "ZZTK")).toBe("Run +20% (after Fast move 20 s ago)");
  expect(alertKind({ title: "something else", label: "Follow-up" }, "ZZTK")).toBe("Follow-up");
});

test("the ticker page's hash round-trips and rejects junk", () => {
  expect(tickerPageHash("zztk", "2026-10-30")).toBe("#/insights/ticker/ZZTK/2026-10-30");
  expect(tickerPageOf("#/insights/ticker/ZZTK/2026-10-30")).toEqual({ symbol: "ZZTK", day: "2026-10-30" });
  expect(tickerPageOf("#/insights")).toBeNull();
  expect(tickerPageOf("#/insights/ticker/ZZTK/yesterday")).toBeNull();
  expect(tickerPageOf("#/insights/ticker/%3Cb%3E/2026-10-30")).toBeNull();
});

describe("createTickerChannel", () => {
  const frame = (o: Record<string, unknown>) => ({ type: "insights.ticker.snapshot", ...o }) as TickerSnapshotMessage;
  test("keeps only the open symbol-day's frames; a failed build lands on the open request", () => {
    const calls: string[] = [];
    const ch = createTickerChannel({ subscribe: (w) => calls.push(`sub ${w.symbol} ${w.day}`), unsubscribe: () => calls.push("unsub") });
    const close = ch.open({ symbol: "zztk", day: "2026-10-30" });
    ch.apply(frame({ ...PKG, day: "2026-10-29" }));
    expect(ch.get().pkg).toBeNull();
    ch.apply(frame(PKG));
    expect(ch.get().pkg?.alerts).toHaveLength(2);
    ch.open({ symbol: "ZZTK", day: "2026-10-29" });
    expect(ch.get().pkg).toBeNull();
    ch.apply(frame({ available: false, unavailable_reason: "build_failed" }));
    expect(ch.get().pkg).toMatchObject({ symbol: "ZZTK", day: "2026-10-29", available: false });
    // The first open's close no longer owns the channel.
    close();
    expect(calls).toEqual(["sub ZZTK 2026-10-30", "sub ZZTK 2026-10-29"]);
  });
});

class FakeSocket implements WebSocketLike {
  readyState = 0;
  sent: string[] = [];
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.readyState = 3;
    this.onclose?.({});
  }
}

test("the ws client asks for a ticker page only from a server with insights.ticker.v1, and again after a reconnect", () => {
  const welcome = { type: "welcome", version: "0.1.0", commit: "abc", heartbeatSecs: 15 };
  for (const [features, expected] of [[[], 0], [[INSIGHTS_TICKER_FEATURE], 1]] as const) {
    const sockets: FakeSocket[] = [];
    const got: string[] = [];
    const client = createWsClient({ url: "ws://x/ws", createSocket: () => (sockets.push(new FakeSocket()), sockets.at(-1)!), onTicker: (m) => got.push(m.symbol), baseDelayMs: 0, maxDelayMs: 0 });
    client.subscribeTicker({ symbol: "ZZTK", day: "2026-10-30" });
    const s = sockets[0]!;
    s.readyState = 1;
    s.onopen?.({});
    s.onmessage?.({ data: JSON.stringify({ ...welcome, features }) });
    const subs = (x: FakeSocket) => x.sent.map((m) => JSON.parse(m)).filter((m) => m.type === "insights.ticker.subscribe");
    expect(subs(s)).toHaveLength(expected);
    if (expected) expect(subs(s)[0]).toEqual({ type: "insights.ticker.subscribe", symbol: "ZZTK", day: "2026-10-30" });
    s.onmessage?.({ data: JSON.stringify({ type: "insights.ticker.snapshot", symbol: "ZZTK", day: "2026-10-30" }) });
    expect(got).toEqual(["ZZTK"]);
    client.unsubscribeTicker();
    expect(s.sent.some((m) => JSON.parse(m).type === "insights.ticker.unsubscribe")).toBe(expected === 1);
    client.close();
  }
});

describe("rows the server assembled (notices.rows.v1)", () => {
  test("its status, words and day summary win; an older server's rows are still derived", () => {
    const server = { event_id: "a", phone: { decision: "sent", held: null, devices: 1 }, status: "held" as const, reason: "stale", reason_text: "too old by the time it was decided" };
    expect(rowStatus(server)).toBe("held");
    expect(rowHeldText(server)).toBe("too old by the time it was decided");
    const old = { event_id: "b", phone: { decision: "held", held: "quiet_hours", devices: 1 } };
    expect(rowStatus(old)).toBe("held");
    expect(rowHeldText(old)).toBe("quiet hours");
    const summary = { sent: 222, held: 84, reasons: [{ reason: "no_device", text: "no device wanted it", n: 84 }] };
    expect(logSummary({ items: [old], summary })).toEqual(summary);
    expect(logSummary({ items: [old] })).toMatchObject({ sent: 0, held: 1 });
  });
  test("an append carries the day's new summary, which replaces the old one", () => {
    const day = "2026-10-30";
    let log = applyNoticesMessage(null, { type: "notices.snapshot", day, days: [day], items: [], truncated: false, summary: { sent: 1, held: 0, reasons: [] } });
    expect(log?.summary?.sent).toBe(1);
    log = applyNoticesMessage(log, { type: "notices.append", day, items: [{ event_id: "x", at: 5, status: "sent" }], summary: { sent: 2, held: 0, reasons: [] } });
    expect(log?.summary?.sent).toBe(2);
    expect(log?.items[0]?.status).toBe("sent");
  });
});
