# Halts, LULD bands and fill history on the chart

Status: contract, 2026-10-17. Owner asked for these after the chart review: halts and LULD bands first
(the most important things in small caps), then key levels and your fills. Key levels are computed on
the client from candles; this contract covers what the server adds.

## 1. Halts (chart-engine → wire)

The engine stops ignoring `MarketEvent::Status` for a symbol: `Halt` opens a halt and `Resume` closes
it. Market-wide statuses (`symbol: None`), and `MarketOpen`, `MarketClose` and `Other`, change
nothing here.

```jsonc
// HaltWire
{ "start": 1760540700000, "end": 1760540790000, "reason": "LUDP", "code": "H" }
// end: null while the halt is still on
```

- `chart.snapshot` gains `halts: HaltWire[]`: every retained halt for the symbol, ascending by
  `start`. Halts do not depend on the timeframe, so every timeframe sends the same list. Retention
  follows the candles: a halt that ended before the oldest retained candle may be dropped.
- New message, sent to every subscriber of the symbol whatever their timeframe:
  `{ "type": "chart.halt", "symbol": "SMPL", "halt": HaltWire }` when a halt starts, and again
  (same `start`, `end` now set) when it resumes. Clients upsert by `start`.
- Times are `event_ts` in epoch ms. A second `Halt` while already halted is ignored. A `Resume`
  with no open halt is ignored. Both count in `quality` (new counters: `haltsUnmatched`).
- Replay rebuilds (`seek`, a `day` switch) rebuild halts like candles.

## 2. LULD bands (chart-engine → wire)

Limit Up–Limit Down price bands, computed by the engine from the same price-qualifying trades as OHLC.
We **model** them as follows. This is our reading of the LULD Plan (consolidated text through
Amendment 20, `cdn.luldplan.com/plans/LULD-Plan-Amendment-20.pdf`; the proposed 23rd Amendment
leaves Section V and the percentages unchanged) for Tier 2 NMS stocks, which is where nearly every
$0.10–$25 small cap sits; Tier 1 is not modelled. Treat it as a guide on the chart, not as the
exchange's numbers. Verify against the official bands before any real use. Section numbers are the
Plan's. Revised 2026-10-31 to follow the Plan (the first version re-tested on every print, opened on
the first trade, bracketed by the moving reference and doubled at the open); see "Why" below.

- **Active window:** only 09:30–16:00 ET (§V(A)(1)). Outside it there are no bands.
- **Percentage bracket**, fixed for the day by the **previous close** on the primary listing
  exchange (Appendix A §II(2)–(4), (6)), applied to the current reference P:
  - previous close more than $3.00: 10%;
  - $0.75 up to and including $3.00: 20%;
  - less than $0.75: the lesser of $0.15 or 75%.
  The previous close comes from `daily-closes.json` (split-adjusted, at most 10 days old). Without
  one the session's first reference stands in.
- **Doubling** (§V(A)(1)): only 15:35–16:00 (the last 25 minutes before an early close), when the
  previous close is at or below $3.00. **No doubling at the open.**
- **Reference price** (§V(A)(2)): the arithmetic mean of price-qualifying trade prices in the preceding
  5 minutes. It replaces the reference in effect only when it moved at least 1% **and** that
  reference has been in effect at least **30 seconds**. At the end of a hold the mean as of that
  instant is tested (by the clock if no print comes), then every print after it. With no trade in
  5 minutes the reference in effect stays.
- **First reference** (§V(B)): the primary listing exchange's opening print (a print from the
  listing venue in `assets.json` with condition `O`, `Q` or `5`; with the venue unknown, any `Q`
  print) if it comes before 09:35. The mean restarts from it. Otherwise the 5-minute mean at 09:35,
  or the first trade after 09:35 if nothing traded. No bands before that.
- **Bands:** `up = P + P·pct` and `down = P − P·pct` (`P ± $0.15` for that bracket). Both are rounded to the
  tick grid ($0.01 at or above $1, $0.0001 below), the upper band down and the lower band up, so a
  band is never wider than the rule (the Plan text does not say how to round). `down` never goes
  below one tick.
- **Halts** (§V(C)): while a symbol is halted its bands are not published. After a resume the
  reopening print (the first eligible print, or the listing venue's `5`/`O`/`Q` print) is the new
  reference, with a fresh 30-second hold.

Not modelled, because trades alone cannot show them: an open or reopening on a quote (the Plan then
uses the previous close or the quote midpoint), the Limit State freeze of reference updates
(§VI(B)(2)), and Tier 1 membership.

**Why (2026-10-31).** Five years of tape against the official halt list
(`J:/research/stockwatcher/luld-halts`) showed the first version's bands chasing fast spikes so the
stock never reached them (NNDM 2022-11-03 paused at 2.97 with our band at 3.27; the Plan's 2.96).
Rerun on every 4th trading day of that history (104,581 symbol-days, 5,372 official LULD pauses in
session): a tape-inferred pause (5 minutes of silence after a print in the outer 10% of the band)
matched **23.3%** of official pauses with the first version and **58.0%** with these rules, at
similar precision (74.4% vs 72.4% of inferred pauses confirmed by an official halt). In the first
15 minutes it went from 0.3% to 50.8%: the first version doubled bands the Plan does not double.

```jsonc
// LuldWire: the bands in effect from t until the next point; up/down null = no bands from t
{ "t": 1760535000000, "ref": 3.38, "up": 3.71, "down": 3.05 }
```

- `chart.snapshot` gains `luld: LuldWire[]`: the band changes, ascending. The list is
  timeframe-independent, sparse (one point per published change), and starts with the first one of the
  current session.
- New message `{ "type": "chart.luld", "symbol": "SMPL", "point": LuldWire }` is sent to every
  subscriber of the symbol on each change. It includes a `{up: null, down: null}` point at a halt and
  at 16:00.

## 3. Fill history (paper broker)

- `paper.state` gains `fills`: the last 200 fills, oldest first. Each one is
  `{ "orderId", "symbol", "side", "qty", "price", "t", "role" }`, where `role` is the order's bracket
  role or null.
- It is cleared with the account (`paper.reset`, a seek, a replay day switch).
- The existing `paper.fill` events are unchanged. This list lets a client that connects later, or
  reloads, still draw fill markers and the trade list.

## Implemented (2026-10-17, server side)

Code: `crates/chart-engine` (`src/luld.rs` holds the model and the pure, unit-tested
`luld_bands`; halts and wiring in `src/engine.rs`; wire types in `src/wire.rs`),
`crates/paper-broker` (`fills`), `crates/server` (routing). Wire as written above, with these exact
shapes:

- `chart.snapshot` gains `halts: HaltWire[]` and `luld: LuldWire[]`; `quality` gains
  `haltsUnmatched`.
- `{"type":"chart.halt","symbol","halt":{"start","end","reason","code"}}` and
  `{"type":"chart.luld","symbol","point":{"t","ref","up","down"}}`. Neither has a `timeframe`, so the
  server forwards them to every subscriber of the symbol, like `tape.trade`.
- `paper.state` gains `fills: [{"orderId","symbol","side","qty","price","t","role"}]`. `paper.fill`
  is unchanged.

Decisions where the contract was silent:

- **The halt record** keeps the `Halt` event's `code` and `reason`. The `Resume`'s own code (for
  example `T`) is not shown. `end` is never before `start`.
- **Unmatched statuses** count in the quality bucket of the status's own session (`classify(event_ts)`).
  A status for a symbol that has never traded still creates the symbol, so its halts can be
  snapshotted.
- **Halt retention:** a halt is dropped once it ended before the oldest interval still retained by
  every timeframe. An open halt is never dropped.
- **Reference price:** the 5-minute window is `(t − 5 min, t]`. Only price-qualifying (OHLC) prints of
  the regular session count, and only while not halted (plus the listing venue's opening print,
  which counts once even when it is a non-qualifying `Q`). The mean is tested on each such trade
  once the 30-second hold has passed, and once by the clock at the hold's end, using exact `i128`
  sums. When it moves at least 1%, the published reference is the mean rounded half up to the
  micro-dollar (not to the tick), so `ref` can carry up to six decimals.
- **After a resume** the reference and the window restart: the reopening print is the new reference
  (§V(C)). During the halt nothing is averaged.
- **Doubled percentages:** `P < $0.75` doubles to the lesser of $0.30 or 150%, and the lower band then
  floors at $0.0001. On an early-close day the active window is 09:30–13:00, the closing doubling is
  the last 25 minutes (12:35–13:00), and the null point is at 13:00.
- **Tick for rounding** is chosen by the unrounded band's own price ($0.01 at or above $1.00, else
  $0.0001). For example, `$1.01 · 0.8 = $0.808` stays `0.808`.
- **When a point is published:** on a trade that moves the reference, at a hold's end when the mean
  moved, at 09:35 when no opening print came, at the 15:35 regime change (a point stamped at exactly that time, published by the clock or by the first later
  print, whichever comes first), at a halt (a null point at the halt time) and at the close (a null
  point at 16:00). A point is published only when `ref`, `up` or `down` changes, and a null point
  only if bands were in effect. A null point also nulls `ref`.
- **Ordering:** points have strictly increasing `t`. A late print inside the settle window is stamped
  at the latest evaluation time, never in the past. If two changes fall on the same `t` (for
  example, the clock crosses 15:35 and then a print at exactly 15:35:00.000 moves the reference),
  the second replaces the first in the list, and **both are sent**. Clients upsert LULD points by
  `t`, as they upsert halts by `start`.
- **Arrival order:** candles stay independent of arrival order inside the settle window, but the
  published bands don't, because they are published as trades arrive. Replaying the same stream is
  still deterministic, so a seek rebuild equals linear play. That case is tested.
- **Corrections and cancels** do not revise the averaging window. Published bands are history.
- **The snapshot's `luld`** is the list of the latest regular session whose trading date equals the
  symbol's current session date. So after-hours trading still shows the day's bands, ending with
  the 16:00 null point, and from the next 20:00 overnight session the list is empty until the
  next open.
- **Fills** are kept independently of the 50 retained finished orders. The history can name an
  order that has aged out of `orders`.

Tests:
- `luld.rs` unit tests cover each bracket boundary (including $3.00 in the 20% bracket), the bracket
  following the previous close, the $0.15 rule, the doubling window (15:35 only at or below $3,
  none at the open, early close), the 30-second hold (by the clock and by prints), the opening
  print (another venue's, the 09:35 mean, nothing by 09:35, unknown venue), the 1% threshold,
  rounding direction, halts and the reopening print, and the 16:00 null.
- `tests/halts_luld.rs` covers start and resume, unmatched statuses, a double halt, retention, the
  current-date filter, and the rebuild under two clock patterns.
- `tests/wire.rs` holds the golden JSON for both messages and the snapshot fields.
- `paper-broker` tests cover the 200-fill cap, roles and reset.
- The server test `fixture_halt_and_luld_bands_reach_subscribers_and_snapshots` replays the
  fixture day from 10:04 and sees `chart.halt` start and end, the 10:05 null point and the first
  bands after the 10:06:30 resume, then checks a 5m snapshot carrying both.
