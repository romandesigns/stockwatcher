# Simulated chart trading (paper broker)

Status: contract, 2026-10-17. Owner decision: chart trading in this phase is **simulator only**. Orders
fill inside our own server against the replay (or live) tape. No broker, no keys, no money, and nothing
leaves the process. Every message carries `"simulated": true` so no client can mistake it for a real
account. Real routing can later sit behind the same messages.

## Where things live

| Piece | Location | Notes |
|---|---|---|
| Matching, orders, positions | `crates/paper-broker` (pure, deterministic) | No I/O, no clock of its own: it is fed trades and the virtual time |
| Wiring | `crates/server` (`market.rs` feeds it every tape trade; `ws.rs` parses commands and broadcasts state) | One paper account per server process (single-user app behind the API token) |
| TS wire types, order-line layout, hit testing | `packages/chart-core/src/trading.ts` | Chart-only logic, no transport |
| Order lines, drag-to-modify, cancel, order ticket UI | `packages/chart-web`, `packages/chart-native` | The chart emits intents; the host app sends them |

## Client → server

```jsonc
{ "type": "order.place", "clientOrderId": "c-1697", "symbol": "SMPL", "side": "buy",
  "orderType": "limit", "qty": 100, "limitPrice": 3.38 }
// orderType: "market" | "limit" | "stop" | "stop_limit"
// limitPrice: required for limit and stop_limit; stopPrice: required for stop and stop_limit
{ "type": "order.modify", "id": "o-7", "limitPrice": 3.36 }   // any of qty, limitPrice, stopPrice
{ "type": "order.cancel", "id": "o-7" }
{ "type": "paper.reset" }                                      // flat account, all orders gone
```

Prices are dollars as JSON numbers. The server converts them to micro-dollars and **rejects** any price
that is not on the tick grid: $0.01 at or above $1.00, $0.0001 below (the Reg NMS sub-penny rule). The
chart snaps dragged prices to that grid, so a rejection means a client bug. `qty` is a positive whole
number of shares, at most 1,000,000.

## Server → client

`paper.state` is broadcast to every connection after any change, and sent once right after `welcome`.
It is the whole truth, so clients replace their copy and never merge.

```jsonc
{ "type": "paper.state", "simulated": true, "seq": 12,
  "account": { "startingCash": 100000, "cash": 99662, "realizedPnl": 0 },
  "orders": [ {
      "id": "o-7", "clientOrderId": "c-1697", "symbol": "SMPL", "side": "buy", "orderType": "limit",
      "qty": 100, "filledQty": 0, "avgFillPrice": null, "limitPrice": 3.38, "stopPrice": null,
      "status": "open",            // "open" | "triggered" | "filled" | "cancelled" | "rejected"
      "rejectReason": null,        // e.g. "sub_penny", "bad_qty", "unknown_symbol", "missing_price", "not_open"
      "createdAt": 1760540400000,  // virtual clock, epoch ms
      "updatedAt": 1760540400000 } ],
  "positions": [ { "symbol": "SMPL", "qty": 100, "avgPrice": 3.38, "realizedPnl": 0 } ] }
```

- `orders` holds every open or triggered order, plus finished ones from the last 50 status changes, so a
  client can show a fill or a rejection that just happened.
- A position `qty` is signed: shorting is allowed in the simulator. A flat position is dropped from the
  list, but its realized P&L stays in `account.realizedPnl`.
- Unrealized P&L is not sent. Clients compute it from the last trade they already have.

`paper.fill` is sent alongside `paper.state` for each fill, so clients can play a haptic cue or show a
toast without diffing:

```jsonc
{ "type": "paper.fill", "simulated": true, "orderId": "o-7", "symbol": "SMPL", "side": "buy",
  "qty": 100, "price": 3.38, "t": 1760540412345 }
```

A malformed or unknown command gets the existing `error` message (`code: "bad_order"`). A well-formed order
that fails validation is created with `status: "rejected"` and a `rejectReason`, so the rejection shows up
in state.

## Matching rules (deliberately conservative)

The broker only sees **price-qualifying trades**, the same filter that updates OHLC
(`trade_effect(...).updates_ohlc`). Volume-only prints such as odd lots and average-price trades never
fill or trigger anything.

| Order | Fills when a qualifying trade prints at P | Fill price |
|---|---|---|
| market | the first trade after it was placed | P |
| buy limit L | P **< L** (traded through; a touch at L does not fill) | L |
| sell limit L | P **> L** | L |
| buy stop S | P ≥ S triggers it, and it fills on the same trade as a market order | P |
| sell stop S | P ≤ S | P |
| stop-limit S/L | P crosses S: becomes a limit L (status `triggered`), checked from the **next** trade | L |

- Each order fills in full on one trade. There are no partial fills and no liquidity model. This is
  optimistic for size and is stated as such in the UI ("Simulated fills").
- Fills are applied in order of arrival. Within one trade, orders fill in creation order.
- Modifying an open order keeps its id. Modifying a filled, cancelled or rejected order rejects the
  modification (`error` with `code: "not_open"`) and leaves the order alone.
- **Time travel resets the broker.** A replay `seek`, or a `day` switch (which rebuilds exactly like a
  seek), rebuilds the market from the session start, and the account goes back to flat with no orders.
  Orders cannot survive the tape being rewritten under them. `step`, `play`, `pause` and `speed` do not
  reset it.
- State lives only in memory. A server restart is a reset.
- The account starts with $100,000. Buying power is not enforced. Cash is debited on buys and credited on
  sells, so it can go negative.

## Bracket orders (position tool, 2026-10-17)

The chart's position tool (Long/Short drawing: entry, target and stop zones; design canvas
https://claude.ai/artifact/UkBuQdEMyZjAYmdMsTzatW) places one **bracket**: an entry plus two linked
exits that cancel each other (OCO).

```jsonc
{ "type": "order.place", "clientOrderId": "c-1", "symbol": "SMPL", "side": "buy",
  "orderType": "limit", "qty": 1428, "limitPrice": 3.38,
  "bracket": { "takeProfit": 3.50, "stopLoss": 3.31 } }
```

- `bracket` is optional on any entry (`market`, `limit`, `stop`, `stop_limit`). Both legs are required
  when it is present. The broker creates **three** orders in one step: the entry, then
  `takeProfit` as a limit on the opposite side, then `stopLoss` as a stop on the opposite side, each
  for the entry's `qty`. For a buy entry these are a sell limit and a sell stop; for a sell (short)
  entry, a buy limit and a buy stop.
- New order fields, present on every order and `null` outside a bracket:
  - `parentId`: on the two exits, the entry's id.
  - `role`: `"entry" | "take_profit" | "stop_loss"`.
  - `ocoGroup`: the same string on the entry and both exits, e.g. `"b-3"`.
- New status `"pending"`: an exit waiting for its entry to fill. It neither fills nor triggers. It
  counts as working for display (`isWorking` on the client includes it), so its line shows, faintly.
- **Validation** (first failure wins, after the entry's own checks): both legs are on the tick grid
  (`sub_penny`) and positive (`bad_price`). Then `bad_bracket`: for a buy entry,
  `takeProfit > stopLoss` and, when the entry has a price (its limit, or its stop for a stop entry),
  `takeProfit > entry > stopLoss`; for a sell entry, the mirror. A rejected bracket creates the entry
  as `rejected` with that reason, and no exits.
- **Lifecycle:**
  - The entry fills: both exits go `pending → open` on that same trade. They are only checked from the
    **next** trade, the same conservative rule as a triggered stop-limit.
  - One exit fills: the other goes to `cancelled`. `rejectReason` stays `null`; a client tells it
    apart through its sibling's fill.
  - The entry is cancelled (or rejected) while the exits are pending: both exits are cancelled.
  - One exit is cancelled by the user: the other stays open, no longer linked. Its `ocoGroup` stays
    as it was.
  - Modify works on a `pending` exit, as on an open one. Modifying an exit is checked against the
    bracket rule: a long's take-profit must stay above its stop-loss, and above the entry price while
    the entry is unfilled; otherwise the modify is refused with `error bad_bracket`. Modifying the
    entry's price is checked the same way against its pending exits.
- `paper.reset` and time travel clear brackets like everything else.

## Implemented (server, 2026-10-17)

- `crates/paper-broker`: `src/lib.rs` (`Broker`: `place`, `modify`, `cancel`, `reset`, `on_trade`,
  `state`; matching, signed positions, cash and P&L in exact micro-dollars), `src/wire.rs` (the JSON
  edge: command structs in, `paper.state` / `paper.fill` out), `src/tests.rs` (unit tests per
  matching-table row, validation, modify/cancel, P&L, retention, reset).
- `crates/server/src/market.rs`: the broker lives in `Market`; every OHLC-qualifying trade is fed to
  it right after its tape row is broadcast. `FeedItem::Reset` (a seek, and the initial catch-up)
  resets it and broadcasts the flat state; the rebuild's re-read of the day is **not** fed to it.
  Frames go out on the existing broadcast as `OutKind::Paper`.
- `crates/server/src/ws.rs`: the four commands, `paper.state` right after `welcome` (before the
  first `replay.clock`), and a fresh `paper.state` to a client that lagged the broadcast.
- Tests: `crates/server/tests/market.rs` (fill over the WS, seek reset, rejections and errors).

Decisions the contract left open:

- **Frame order:** `paper.state` is sent first, then one `paper.fill` per fill, so the order is
  already in the client's state when the fill cue fires.
- **Ids and `seq` are never reset**, not even by a reset or a seek, so an id is never reused and a
  client can drop a stale state by `seq`.
- **Order of `orders`:** creation order (by id), working and finished mixed.
- **Validation order** (first failure wins): `bad_qty`, `unknown_symbol`, `missing_price`, then each
  price: `bad_price` (zero, negative or too large; a reason added to the list) and `sub_penny`.
- **Float edge:** prices are rounded to the micro-dollar before the tick check, so float noise
  (`0.1 + 0.2`) is accepted and `3.385` is `sub_penny`. A non-whole `qty` (`1.5`) is a `bad_qty`
  rejection; a missing or non-numeric `qty`, an unknown `side`/`orderType`, or an unknown
  `order.*`/`paper.*` type is `error bad_order`. Prices an order type does not use are dropped.
  `clientOrderId` is optional; the symbol is trimmed and upper-cased.
- **Modify** works on `open` and `triggered` orders (a triggered stop-limit is a working limit that
  a chart drag should still move). Errors, all leaving the order alone: `not_open` (also for an
  unknown or aged-out id), `bad_order` (no fields, a price the type does not have, or a
  `stopPrice` on an already triggered order), or the validation reason itself (`bad_qty`,
  `sub_penny`, ...). `order.cancel` uses `not_open` the same way.
- **`createdAt` / `updatedAt` of commands** use the provider's published clock (virtual in replay);
  fills and triggers use the trade's own time. An idle market with no clock stamps 0.
- **Avg price** is the exact average (it can be off the tick grid); per-position `realizedPnl`
  restarts at 0 when a position goes flat and reopens, while flipping through zero in a single fill
  keeps it.

## Implemented (brackets, 2026-10-17)

- `crates/paper-broker`: `NewOrder.bracket` (`Bracket { take_profit, stop_loss }`), `Role`,
  `Status::Pending` (counted by `Status::is_working`), `RejectReason::BadBracket`, and
  `parent_num` / `role` / `oco_group` on `Order`; `wire.rs` adds `PlaceCommand.bracket` and
  `parentId` / `role` / `ocoGroup` on every order. Unit tests in `src/tests.rs` (`// --- brackets`),
  one WS test in `crates/server/tests/market.rs` (market entry, take-profit fills, stop-loss
  cancelled, then a `bad_bracket` modify). The server needed no code change.

Decisions the bracket section left open:

- **A missing leg** (`"bracket": {"takeProfit": 3.5}`) is a `missing_price` rejection, like a limit
  without its limit; a non-numeric leg is `error bad_order`. `"bracket": null` is a plain order.
- **Leg checks** run per leg, take-profit first, each `bad_price` then `sub_penny` (the same order
  as an entry's own prices), then `bad_bracket`. The bracket rule is **strict**: an exit at the
  entry price, or a take-profit equal to the stop-loss, is `bad_bracket`.
- **A stop-limit entry is checked at its limit** (the price it fills at), not its stop.
- **A rejected bracket** still uses one id and one `ocoGroup`: the rejected entry carries
  `role: "entry"` and its `ocoGroup` (so the client can tie the rejection to its position tool),
  with no exits. An entry rejected for its own reasons (`bad_qty`, ...) looks the same.
- **Exits have `clientOrderId: null`**; the ticket's id stays on the entry only, and clients link
  exits through `parentId`.
- **`ocoGroup` numbering** (`b-1`, `b-2`, ...) is its own counter, never reset, like ids.
- **Exit qty does not follow the entry.** Modifying the entry's qty leaves its exits as they are;
  a client resizing a bracket modifies all three.
- **What bounds a modify:** only members still working. Once the entry filled, the exits are
  checked only against each other; once one exit is cancelled, the survivor is checked only
  against the entry while that is unfilled. A qty-only modify is never `bad_bracket`.
- **Cancelling the entry** cancels its pending exits in the same state change, also when the entry
  is a triggered stop-limit. Exits never become `open` without their entry having filled.
- **OCO inside one trade:** an exit filling cancels its sibling on that same trade (`updatedAt` is
  the trade's time) before the sibling could match. A valid bracket cannot have both exits hit by
  one print anyway.
- **Retention:** pending exits are working orders, so `FINISHED_RETAINED` never drops them. An
  exit can outlive its filled entry in `orders`, so a `parentId` may name an order that has aged
  out of the list.
