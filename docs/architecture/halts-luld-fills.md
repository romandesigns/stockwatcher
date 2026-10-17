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
- Replay rebuilds (`seek`) rebuild halts like candles.

## 2. LULD bands (chart-engine → wire)

Limit Up–Limit Down price bands, computed by the engine from the same price-qualifying trades as OHLC.
We **model** them as follows. This is our reading of the LULD Plan for Tier 2 NMS stocks, which is
where every $0.10–$25 small cap sits. Treat it as a guide on the chart, not as the exchange's numbers.
Verify against the official bands before any real use.

- **Active window:** only 09:30–16:00 ET. Outside it there are no bands.
- **Reference price:** the arithmetic mean of price-qualifying trade prices in the preceding
  5 minutes. The first one is the first regular-session trade. It is published only when it moves
  by at least 1% from the current reference. Otherwise the current reference stays.
- **Percentage by reference price P:**
  - P ≥ $3.00: 10%;
  - $0.75 ≤ P < $3.00: 20%;
  - P < $0.75: the lesser of $0.15 or 75%.
- **Doubling:** the percentage is doubled 09:30–09:45 for all prices, and 15:35–16:00 for P < $3.00.
- **Bands:** `up = P + P·pct` and `down = P − P·pct` (`P ± $0.15` for that tier). Both are rounded to the
  tick grid ($0.01 at or above $1, $0.0001 below), the upper band down and the lower band up, so a
  band is never wider than the rule. `down` never goes below one tick.
- **Halts:** while a symbol is halted its bands are not published. After a resume they continue from
  the next trade.

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
- It is cleared with the account (`paper.reset`, a seek).
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
  the regular session count, and only while not halted. The mean is evaluated on each such trade
  (not on clock ticks), using exact `i128` sums. When it moves at least 1%, the published
  reference is the mean rounded half up to the micro-dollar (not to the tick), so `ref` can carry up
  to six decimals.
- **After a resume** the reference and the window restart: the first trade after the resume is the
  new reference, as the reopening print is under the Plan. During the halt nothing is averaged.
- **Doubled percentages:** `P < $0.75` doubles to the lesser of $0.30 or 150%, and the lower band then
  floors at $0.0001. On an early-close day the active window is 09:30–13:00, the closing doubling is
  the last 25 minutes (12:35–13:00), and the null point is at 13:00.
- **Tick for rounding** is chosen by the unrounded band's own price ($0.01 at or above $1.00, else
  $0.0001). For example, `$1.01 · 0.8 = $0.808` stays `0.808`.
- **When a point is published:** on a trade that moves the reference, at the 09:45 and 15:35 regime
  changes (a point stamped at exactly that time, published by the clock or by the first later
  print, whichever comes first), at a halt (a null point at the halt time) and at the close (a null
  point at 16:00). A point is published only when `ref`, `up` or `down` changes, and a null point
  only if bands were in effect. A null point also nulls `ref`.
- **Ordering:** points have strictly increasing `t`. A late print inside the settle window is stamped
  at the latest evaluation time, never in the past. If two changes fall on the same `t` (for
  example, the clock crosses 09:45 and then a print at exactly 09:45:00.000 moves the reference),
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
- `luld.rs` unit tests cover each tier boundary, the $0.15 rule, the doubling windows (including
  15:35 only below $3), the 1% threshold, rounding direction, halt suppression and the 16:00 null.
- `tests/halts_luld.rs` covers start and resume, unmatched statuses, a double halt, retention, the
  current-date filter, and the rebuild under two clock patterns.
- `tests/wire.rs` holds the golden JSON for both messages and the snapshot fields.
- `paper-broker` tests cover the 200-fill cap, roles and reset.
- The server test `fixture_halt_and_luld_bands_reach_subscribers_and_snapshots` replays the
  fixture day from 10:04 and sees `chart.halt` start and end, the 10:05 null point and the first
  bands after the 10:06:30 resume, then checks a 5m snapshot carrying both.
