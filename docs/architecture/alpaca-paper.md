# Chart trading on the Alpaca paper account

Status: implemented 2026-10-30 (branch `feat/alpaca-paper`). Owner request 2026-10-29: "integrate my
alpaca paper trading account so I can place orders using alpaca paper trading since we already added
the ability to place trades". This extends [chart-trading-sim.md](chart-trading-sim.md): the same
messages, a second account behind them.

## Safety rules (read these first)

1. **Paper host only, by construction.** The trading base is the constant
   `https://paper-api.alpaca.markets` and the stream `wss://paper-api.alpaca.markets/stream`
   (`crates/alpaca-paper/src/host.rs`). No setting, env variable or argument changes them. Every URL
   is checked by `ensure_paper` right before a request leaves (REST in `ReqwestHttp::send`, the
   stream in `stream::session`); `api.alpaca.markets` (live money), look-alike hosts, ports, user
   info and plain http are refused, with tests (`host::tests`,
   `tests::the_real_client_refuses_the_live_host_before_sending`).
2. **Going live is a code change, never a config flip.** It needs a new host constant, a new check,
   new tests and a review that says so. The server's existing `ALPACA_TRADING_BASE` /
   `ALPACA_END_POINT` env names are deliberately NOT read by this crate.
3. **No redirects.** The HTTP client follows none, so the key headers can only reach the paper host.
4. **Keys stay on the server.** From the server environment only: `ALPACA_PAPER_KEY` /
   `ALPACA_PAPER_SECRET` (preferred), else `ALPACA_API_KEY` / `ALPACA_API_SECRET` (or
   `APCA_API_KEY_ID` / `APCA_API_SECRET_KEY`) when they are paper keys. Shared keys starting `AK`
   (Alpaca's live key ids) are not even tried; any key the paper host refuses (401/403) marks the
   route `unauthorized`. Keys are never sent to a client, logged or printed: `Credentials` redacts
   its `Debug`, and Alpaca error bodies are scrubbed of both values.
5. **Replay is never routed.** Only the live market can hold the route
   (`Market::set_paper_route` refuses any other; test `the_replay_market_is_never_routed_to_alpaca`).
6. **No silent fallback.** With "Alpaca paper" chosen but no working keys, every order is rejected
   with "Alpaca paper is not configured on the server: …" rather than quietly going to the simulator.

## Choosing the account

| Namespace | Written by | Value |
|---|---|---|
| `tradingAccount` | clients (Settings → Trading, web and phone) | `{"live": "simulator" \| "alpaca_paper"}`; anything else is `simulator` |
| `broker.status` | the server only (a client write is `bad_request`, like `alerts.state`) | `{"alpacaPaper": {"configured", "state", "message", "keys"}}` |

`state` is `connecting`, `live`, `down`, `unauthorized` or `not_configured`; `message` says why in
words (it names env variables, never values); `keys` names which env pair is in use. Settings shows
"Not configured. <message>" under the Alpaca option when `state` is `unauthorized` or
`not_configured`.

Switching (`crates/server/src/trading.rs`, `follow`) broadcasts the now-current `paper.state` to every
live-market connection. While routed, the live market's simulator keeps matching but sends nothing
(its state would replace the clients' Alpaca state); switching back restores it as it was.

## Wire changes

Commands are unchanged (`order.place`, `order.modify`, `order.cancel`). `paper.reset` while routed
gets `error not_supported` (a paper account is reset from Alpaca's dashboard).

Every `paper.state` / `paper.fill` now carries `broker`: `"simulator"` (with `simulated: true`) or
`"alpaca_paper"` (with `simulated: false`). Clients trust `alpaca_paper` only together with
`simulated: false` (chart-core `brokerOf`). Alpaca paper's `paper.state` adds:

```jsonc
{ "type": "paper.state", "simulated": false, "broker": "alpaca_paper", "seq": 7,
  "account": { "startingCash": 100000, "cash": 99000, "realizedPnl": 0,
               "buyingPower": 396000, "equity": 100012.5 },
  "orders": [ { "id": "61e69015-…", "rejectReason": null, "rejectMessage": null,
                "extendedHours": false, /* …the simulator's order fields… */ } ],
  "positions": [ … ], "fills": [ … ],
  "link": { "state": "live", "message": "" },
  "notice": { "at": 1793196002000, "text": "Alpaca: insufficient buying power", "orderId": "r-3" } }
```

- **Ids are Alpaca's** order UUIDs. **A modify is Alpaca's replace** (`PATCH /v2/orders/{id}`), which
  answers with a NEW id: the old order leaves `orders` and the new one takes its place (also in a
  bracket). Ids that are not UUID-shaped are `not_open` without a request.
- **Status mapping:** `held` → `pending` (a bracket exit waiting for its entry); `filled`;
  `canceled`/`expired`/`replaced`/`done_for_day` → `cancelled`; `rejected`; everything else (`new`,
  `accepted`, `pending_new`, `partially_filled`, …) → `open`. Partial fills show as `filledQty <
  qty`. Alpaca does not report a stop-limit's trigger, so `triggered` never appears.
- **Brackets** map to Alpaca `order_class: "bracket"`: legs become `take_profit` (limit) /
  `stop_loss` (stop) with `parentId` = the entry's id and `ocoGroup` = `b-<entry id>`.
- **Rejections** made before or by Alpaca appear as an order with a local id `r-<n>`, status
  `rejected`, a `rejectReason` code and `rejectMessage` text. Codes: the simulator's validation
  codes (`bad_qty`, `missing_price`, `bad_price`, `sub_penny`, `bad_bracket`), `extended_hours_limit_only`,
  `extended_hours_no_bracket`, `not_configured`, `rate_limited`, `broker_rejected` (Alpaca's own text,
  e.g. shorting not allowed for the symbol or insufficient buying power).
- `notice` is the last refusal (place, modify or cancel); clients toast it once.
- `account.startingCash` is Alpaca's `last_equity` (previous close); `realizedPnl` is 0 (Alpaca does
  not report it). Positions are signed (shorts negative); their `realizedPnl` is 0.
- Bounded like the simulator: working orders, plus the last 50 finished, plus the last 200 fills.
- A refused modify/cancel gets `error` with `code` `not_open`, `bad_order`, a validation code,
  `broker_rejected`, `rate_limited` or `not_configured`, and Alpaca's text as `message`.

## Order rules

- `time_in_force` is always `day`; `client_order_id` is passed through (made by the server when
  missing or not `[A-Za-z0-9._:-]{1,128}`), which makes a resent place idempotent at Alpaca.
- The simulator's validation runs first (`paper_broker::check_new_order` / `check_modify`): same tick
  grid (sub-penny) and bracket rules.
- **Extended hours:** regular hours come from Alpaca's `/v2/clock` (holidays and half days), cached
  until its next open/close, falling back to weekdays 9:30–16:00 ET. Outside them only plain `limit`
  orders go, with `extended_hours: true`; market, stop, stop-limit and brackets are refused before
  sending with a reason (Alpaca would reject them).
- Shorting is whatever Alpaca allows for the account and symbol; its rejection text is surfaced.

## Live state

- **Fills come from the trade updates stream** (`trade_updates`, binary JSON frames on paper), in one
  Rust task with reconnect (equal-jitter backoff 1 s → 60 s, reset after a minute streaming; 5 min
  after an auth refusal), a ping every 30 s and a 75 s silence limit. No polling loop.
- **REST snapshots** (account, positions, open + 50 recent closed orders with legs nested, the last
  100 fills) at start and after every (re)connect. Updates that arrive during a snapshot wait in the
  socket and are applied after it, so the newest state wins.
- After each trade update, the account and positions are re-read once (debounced 300 ms) for buying
  power and average prices.
- Retries: 429 for any method (Alpaca refused before acting), honouring `Retry-After` (capped 5 s);
  network errors and 5xx only for GET/DELETE; max 3 attempts. A POST/PATCH with an unknown outcome is
  never resent: a notice says so and the account resyncs.
- The Alpaca HTTP call runs in its own task, so a slow broker never stalls a socket's market frames.

## Logs

Every order attempt and every broker event is a `tracing` line with target `alpaca_paper` (in the
durable server log, `DATA_DIR/logs/server-<date>.jsonl`): `event` (`order.place`, `order.modify`,
`order.cancel`, `trade_update`, `resync`), `outcome`, symbol, side, type, qty, client and Alpaca order
ids, Alpaca's reason. Never a key, account number or balance.

## Where things live

| Piece | Location |
|---|---|
| Host lock, keys, HTTP + retries, Alpaca shapes, session rule, state, stream, service | `crates/alpaca-paper` |
| Live market route, setting follower, `broker.status` writer | `crates/server/src/trading.rs`, `market.rs` (`paper_route`), `ws.rs` |
| Wire types, `brokerOf`, `buyingPower`, `freshNotice` | `packages/chart-core/src/trading.ts` |
| `tradingAccount` / `broker.status` parsers | `packages/app-state/src/trading.ts` |
| Account badge, buying power, refusal toast | `packages/chart-web/src/AccountBadge.tsx`, `packages/chart-native/src/AccountBadge.tsx` |
| Settings → Trading | `apps/web/src/nav/TradingAccountPanel.tsx`, `apps/mobile/src/settings/TradingAccountPanel.tsx` |
| Manual live smoke test | `cargo run -p alpaca-paper --example paper_smoke -- <.env> [SYMBOL]` |

## Server setup

Add to the server's `.env` (values from Alpaca's dashboard, **Paper** account → API keys):

```
ALPACA_PAPER_KEY=...
ALPACA_PAPER_SECRET=...
```

Without them the server falls back to `ALPACA_API_KEY` / `ALPACA_API_SECRET` only if those are paper
keys (as the local `.env`'s are, smoke-tested 2026-10-30). Then pick Settings → Trading → Alpaca paper.
