# Architecture: simulator and chart (spec 01)

Status: **built and deployed** (proposed 2026-10-17, merged and live since 2026-10-18). It implements
`docs/specs/01-market-simulator-and-chart.md`. The UI direction is **C · Session timeline** (design canvas
https://claude.ai/artifact/ECxnETgKWmPAEBmPdkvnrY).

The first sections (Principle to Order of work) are the original proposal and stay as written, apart from
corrections marked *(now)*. The appendices are the current reference. For how to embed the chart in a new
screen, read `chart-packages.md`.

## Principle

One pipeline, two sources:

```
          ┌────────────────────┐   normalized    ┌─────────────────┐   increments   ┌──────────────┐
 archive ─► ReplayProvider     ├─── MarketEvent ─► chart-engine    ├── (WS/JSON) ──► @stockwatcher/│
          │ (virtual clock)    │   (one contract)│ per-symbol      │                │ chart        │
 Alpaca  ─► LiveProvider       │                 │ incremental     │                │ (web, RN)    │
   WS     └────────────────────┘                 └─────────────────┘                └──────────────┘
```

- **Aggregation is authoritative in Rust.** Candles and the profile are computed once on the server by the
  same code in replay and live. Clients only render what they receive and never re-aggregate.
- **Provider choice is configuration only:** `MARKET_DATA_MODE=replay|live`. The engine, the wire format and
  the clients are unaware of the mode, except for a status label.
- **No per-symbol threads or polling.** One ingest task normalizes each event once and routes it by symbol to
  incremental state.

## Rust crates (Cargo workspace)

| Crate | Responsibility | Depends on |
|---|---|---|
| `market-events` | The **normalized event contract** (`Trade`, `Quote`, `Correction`, `Cancel`, `Status`), the ET session calendar (chrono-tz America/New_York, DST, early closes, sessions `overnight` / `pre` / `regular` / `after`) and the **trade-condition policy** (which prints update OHLC and/or volume, following Alpaca's documented bar rules, cited in code) | — |
| `chart-engine` | Incremental per-symbol aggregation into 15s/30s/1m/5m candles (forming → final, exact half-open `[start, end)` boundaries); correction and cancel handling; late and out-of-order policy; gap marking (no fake candles); a session volume profile with configurable bin width, anchored per session; snapshot and increment output | `market-events` |
| `market-archive` | On-disk archive: per day/symbol files of normalized events (zstd-compressed, length-prefixed records with a versioned header). Storage root, retention and free-space floor are configurable | `market-events` |
| `market-replay` | A deterministic virtual clock (play, pause, step, seek, speed 1×…max), merging archived streams by `(event_ts, source_seq)` | `market-archive` |
| `market-providers` | A `MarketDataProvider` trait with **`ReplayProvider`** and **`LiveProvider`** (the Alpaca SIP stream client and its normalizer), plus the history importer `swimport` and its rolling-window `maintain` cycle (Appendix F, Appendix B). *(now)* There is no `ArchiveRecorder` and no FMP float lookup in the live path: history comes from the archiver's daily SIP import | above |
| `paper-broker` | *(now)* The simulated matching engine behind paper trading (`chart-trading-sim.md`) | `market-events` |
| `provider-sim` | The local, provider-compatible surface: the Alpaca REST/WS subset and the FMP float route that Stockwatcher uses, served from replay. Dummy auth; no external calls | `market-providers` |
| `server` | Wires provider, engine and client WS plus REST. *(now)* Also the paper broker, the quote board, settings sync and `/updates/*`; routes and messages are listed in Appendix D | all |

## Client: a modular, reusable chart

| Package | Responsibility |
|---|---|
| `packages/chart-core` (TS, framework-free) | Candle and profile store (applies the server's snapshot and increments), time and price scales, viewport (zoom/pan), hit testing (candle inspection), and a **display list**: rects, lines and text with no rendering API calls. Fully unit-tested |
| `packages/chart-web` (React + Canvas 2D) | Draws the display list on `<canvas>` with DPR handling, plus React components. *(now)* The chart is `CandleChart`; the full component list is in `chart-packages.md` |
| `packages/chart-native` | *(now built)* The same `chart-core` with a Skia renderer (`@shopify/react-native-skia`) executing the same display list. One rendering logic for two platforms |

The candle drawing is written once, as a display list, and each platform only executes it. A fixed contract
makes the chart reusable: `ChartData` (from the server) + `ChartView` (timeframe, viewport, bin width) → display list.

## Wire format, server → client (JSON over `/ws`)

```jsonc
// subscribe: {"type":"chart.subscribe","symbol":"SMPL","timeframe":"1m","profileBin":0.01}
{"type":"chart.snapshot","symbol":"SMPL","timeframe":"1m","session":{"id":"2026-10-15/regular","start":"…","end":"…"},
 "candles":[{"t":1792071000000,"o":3.18,"h":3.21,"l":3.17,"c":3.20,"v":41200,"n":87,"final":true}],
 "gaps":[{"t":…,"reason":"no_trades"}],
 "profile":{"bin":0.01,"anchor":"2026-10-15/regular","rows":[{"p":3.18,"v":120400}]},
 "clock":{"mode":"replay","now":…,"speed":10,"playing":true}}
{"type":"chart.update","symbol":"SMPL","timeframe":"1m","candle":{…,"final":false},"profile":[{"p":3.20,"v":500}],"clock":{…}}
{"type":"tape.trade","symbol":"SMPL","t":…,"p":3.47,"s":37,"conditions":["I"],"counted":"volume_only"}
```

Times are epoch ms UTC; sessions and labels are computed in ET. `counted` is `ohlc_and_volume`, `volume_only`
or `excluded:<reason>`. Every client shows the same reason the engine applied, and the tape in design C
displays it.

## Storage and resources

- *(now)* Production archives live on the **VPS**, in the `stockwatcher_data` volume at `/app/data/archive`,
  kept as a rolling 21-trading-day window by the `archiver` compose service (Appendix B). The Pi plan was
  dropped. Locally they go to `ARCHIVE_ROOT` (default `./data/archive`), behind a free-space floor and an
  optional retention limit in days.
- No database, broker or microservices. One server process per mode. Bounded memory per symbol: candle ring
  buffers per timeframe; the profile is per session.

## Credentials

- Simulation needs none (dummy auth).
- Live mode reads `ALPACA_API_KEY` / `ALPACA_API_SECRET` from the environment (a gitignored `.env` on each
  host). *(now)* `FMP_API_KEY` is read only by `swimport`, for its comparison bars. Credentials previously
  shared in chat are **never** used.

## Order of work

1. `market-events` + `chart-engine` with exhaustive deterministic fixtures (no data needed).
2. `chart-core` + `chart-web` with the design C layout against recorded fixtures (in parallel with step 1).
3. **Entitlement verification** (needs fresh credentials from the owner): trade-level SIP history range,
   BOATS, news, FMP float.
4. `market-archive` + `market-replay` + `ReplayProvider` + `provider-sim`.
5. `LiveProvider` + recorder, and a live-vs-replay parity test on a real session.

## Appendix A: chart-engine policies (implemented 2026-10-17)

Code: `crates/market-events` (contract, calendar, condition policy) and `crates/chart-engine`
(aggregation). The crate docs (`cargo doc -p chart-engine`) are the full reference; this is the summary.

**Contract units.** Times are `i64` UTC nanoseconds; prices are `i64` micro-dollars (exact, never `f64`
for keys or sums); sizes are `u64` shares. Persisted events are JSON `{"v":1,"type":"trade",...}`;
readers refuse another `v`. Trade identity is `(source, exchange, source_seq)`.

**Sessions (ET, `chrono-tz` America/New_York).** overnight 20:00 (previous calendar day)–04:00, pre
04:00–09:30, regular 09:30–16:00, after 16:00–20:00. On early-close days: regular to 13:00, after
13:00–17:00, and 17:00–20:00 is no session. The overnight session belongs to the **next trading day**
(`"2026-10-16/overnight"` starts Thursday 20:00) and exists only if that day trades, so Friday evenings and
the evenings before holidays have none. Holidays and early closes come from the NYSE calendar for
2025–2027. Still to verify against live BOATS data: that Blue Ocean follows the NYSE holiday table.

**Trade conditions.** These follow Alpaca's minute-bar table ("How are bars aggregated?"), and the
strictest rule wins:
- `M` `Q` `9` update nothing.
- `B` (on tapes A/B) and `C` `G` `H` `I` `N` `P` `R` `U` `V` `W` `Z` `4` `7` update volume only.
- Everything else in the table updates OHLC and volume. That includes `T`, and `B` on tape C, where it
  means a bunched trade.
- Codes not in the table are treated as regular sales and counted in `unrecognizedConditions`.

**Candles.** Intervals are half-open and epoch-aligned (`10:00:00.000` belongs to the 10:00:00 interval).
O/H/L/C come from price-qualifying trades in `(event_ts, source_seq)` order. V is the sum of
volume-qualifying sizes. All four timeframes come from the same accepted trades. A candle is final once
the symbol's watermark reaches its end; the watermark is its newest trade, or the replay/live clock
through `advance_clock`, whichever is later.

**Gaps.** An interval with no trades gets no candle. A run of empty intervals inside one session is
marked `no_trades`. An interval that has only volume-only prints has volume but no qualifying price, so
it is marked `volume_only_trades` and carries its volume. Alpaca emits no bar for such an interval
either.

**Late and out-of-order trades.** A trade is accepted iff `watermark < end of its 15s interval + 2 s`
(the settle window is configurable). The decision is made once, on the finest timeframe, and applies to
every timeframe, so switching timeframe never changes the source data. An accepted trade is placed in
key order and its interval is recomputed exactly; it never moves or resets the forming candle. Rejected
trades are counted (`lateExcluded`) and shown on the tape as `excluded:late`. Duplicates are ignored and
counted.

**Corrections and cancels.** Trade-level records are kept for the retention window (10 min by default,
pruned on 5-minute boundaries). A cancel or correction recomputes the affected interval of every
timeframe and adjusts the profile's per-price volume exactly. It may revise a final candle, which is
re-emitted. A target beyond retention is counted as unapplied.

**Profile.** There is one profile per symbol per session id. Volume is stored per exact price, so any
bin width is exact; bins are `floor(price/bin)*bin` anchored at zero. The profile includes
price-qualifying trades and odd lots. It excludes other volume-only prints (average price, prior
reference, derivatively priced, …) because their price is not where shares traded at that moment, so a
session's profile total can be below the sum of its candle volumes. POC is the largest bin; the value
area covers 70%.

**Wire additions to the format above.**
- `chart.remove {t}`: a cancel or correction emptied an interval.
- `chart.gap {gap}`: a gap opened.
- Gaps carry `end`, `v` and `intervals`.
- Candle, gap and remove messages carry `profileBin` and `profile`, the touched bins as **absolute**
  totals.
- `tape.trade` also carries `tsNs`, `exchange`, `seq`, `reason` and `session`.
- Snapshots carry `quality`, plus `profile.total`, `profile.poc` and `profile.valueArea`.
- Clients upsert a candle by `t`, and a candle at `t` supersedes any gap that covers `t`. A snapshot is
  authoritative.

## Appendix B: archive, replay and providers (implemented 2026-10-17)

Code: `crates/market-archive`, `crates/market-replay`, `crates/market-providers`. Crate docs are the
full reference.

**Archive layout.** `$ARCHIVE_ROOT/<trading date>/<SYMBOL>.swa` plus `manifest.json` per day. The
day is the trading date of the sessions, so the overnight session that starts at 20:00 ET the
evening before is in the next day's directory.

**File format v1 (`.swa`).** A 16-byte prefix: magic `SWARCHV\0`, format version (u16 LE, 1), event
contract version (u16 LE, 1), and the header length (u32 LE). Then a JSON header
`{symbol, day, created_by}`, then **one zstd frame** of records. Each record is a u32 LE length
(1 B to 1 MiB) and the `VersionedEvent` JSON (`{"v":1,"type":"trade",…}`). Records stay in
**arrival order** and are never re-sorted, so a live recording keeps late and out-of-order prints
where they arrived. JSON inside zstd was chosen over a bespoke binary encoding because:
- the contract's persisted form is already that JSON, so there is no second serialization to keep
  in step;
- zstd (level 6) shrinks a fixture day roughly 10×;
- `swarchive inspect` can show records as they are.
Readers refuse another format or contract version, and any oversized length.

**Writing and integrity.**
- A writer writes `<SYMBOL>.swa.partial`. On finalize it finishes the frame, fsyncs, and renames
  the file atomically. The manifest is then rewritten atomically (temp file and rename).
- SHA-256 is computed while writing and stored in the manifest with the size, the per-type
  counts, first and last `event_ts`, and the sources. `fixture` is true only when every source is
  `fixture`.
- fsync policy: on finalize (the default), every N records, or never.
- Writers refuse to open, and stop writing, below `ARCHIVE_MIN_FREE_MB` (default 1024; checked
  every 8 MiB written).
- `ARCHIVE_RETENTION_DAYS` (default 0, keep forever) prunes whole days older than the ET trading
  date. The server prunes at startup in replay mode.
- The VPS keeps a **rolling window of the last 21 trading days** instead (retention stays 0 there): `swimport
  maintain`, run as the `archiver` compose service, imports each completed SIP day after 21:15 ET
  and deletes days that fall out of the 21 trading days ending at L (the latest completed trading
  day; one in, one out), never the day the
  server is replaying and nothing at all when the server cannot be asked
  (`crates/market-providers/src/import/maintain.rs`). A day it is still importing carries an
  `.importing` marker and is not offered by `ready_days()` (replay's default day, the day menu);
  removal renames a day out of the listing before deleting it. The server re-reads `assets.json`
  and `daily-closes.json` when they change and lists days fresh on every request.

**`swarchive`** (`cargo run -p market-archive --bin swarchive -- …`):
- `days`;
- `fixture <day> [SYMBOL…] [--seed N]` writes a deterministic **fixture** day (default SMPL, FXLO,
  FXMD, FXHI, seed 7);
- `inspect <day> [--symbol S] [--records N]`;
- `verify <day>` recomputes checksums and decodes every record; it exits non-zero on mismatch;
- `prune [--today D]`.

Importing real history is `swimport`'s job (Appendix F); `swarchive` never reads credentials and never
calls the network.

**Fixture day.** A port of the in-browser fixture: mulberry32, the same constants and draw order,
and the first prints identical (tested), so the screens look alike. Real condition codes are used:
- `I` odd lot, `W` average price, `Q`/`M` official open/close, `T` extended hours;
- late prints executed 20 s before their arrival position, carrying `receive_ts`;
- out-of-order prints inside the settle window, and duplicates;
- quotes, and a halt/resume status pair around the 10:05–10:06:30 silence;
- one correction (11:00 print, published 30 s later) and one cancel (12:00, published 20 s later).

Symbol `i` uses seed `7 + 101·i` and a price scale of `[1, 0.27, 6.2, 13][i % 4]`, so FXLO trades
below $1 (with $0.0001 ticks) and FXHI above $25. Every event is `source: fixture`, so none of it
is market evidence.

**Replay ordering.**
- Each event's delivery time is the per-symbol running maximum of `receive_ts`, or of `event_ts`
  when there is no `receive_ts`.
- Symbol streams are merged by
  `(delivery, event_ts, source_seq, exchange, source, symbol, stream)`. A time-sorted file without
  `receive_ts` therefore merges exactly by `(event_ts, source_seq, exchange, source)`.
- The same archive always yields the same event sequence (tested twice with different batch sizes
  and stops, through the sync core and through the async driver at max speed).

**Engine clock.** Replay emits `Advance(t)` marks quantized to 1 s. Before an event delivered at
`D`, the engine has been advanced to exactly `floor_1s(D)`, and at the end of a step to `T`, to
`floor_1s(T)`. Wall pacing only coalesces marks, so the engine state at virtual time `T` is
identical whether `T` is reached by playing at any speed, stepping or seeking. Tested: seek
rebuild equals linear play, comparing candles at all four timeframes, profiles at three bin widths,
and quality counters.

**Controls.**
- `play` / `pause`.
- `step` emits exactly one event.
- `stepInterval <tf>` moves to the next multiple of the interval, inclusive: events delivered
  exactly on the boundary are emitted.
- `seek <t>` emits `Reset`, re-reads the day from its start up to `t` (consumers rebuild silently),
  then a `Clock`.
- `speed` takes 1…3600× or max (wire value 0).
- `day <date>` swaps the source for the same archive's other day (`EventSource::for_day`, which
  reads the manifest first, so a missing or non-trading day is refused with nothing changed), then
  emits `Source` (the new day's label, symbols and fixture flag) and runs the seek rebuild to
  `REPLAY_START` on the new day (`ReplayOptions::start_time`). Playing/paused and speed carry over.
  One provider session serves every day; the old day's source, readers and merger are dropped at
  the swap.

One tokio task drives everything, with no per-symbol tasks. A 50 ms pacing tick while playing,
`virtual = anchor + wall_elapsed × speed`, re-anchored on every control. Output goes through a
bounded channel (8192), so a slow consumer slows replay rather than growing memory; at max speed,
batches of 2048 events keep commands responsive. A seek over a whole fixture day (~48k events by
10:40) takes about 2–3 s in a debug build.

**Providers.** `MarketDataProvider::start() → ProviderSession { info, items, clock, control }`.
- `MARKET_DATA_MODE=replay|live` (default `replay`).
- Replay settings: `REPLAY_DAY` (default: the latest **ready** archived day, so a day still being
  imported is skipped), `REPLAY_START` (ET `HH:MM[:SS]`, default 10:40, matching the browser fixture;
  production sets 09:30), `REPLAY_SPEED` (default 10), `REPLAY_AUTOPLAY` (default 1),
  `REPLAY_FOLLOW_LATEST` (default 0: switch the running replay to a newly imported latest day). The
  full list of settings is in `../reference/configuration.md`.
- `LiveProvider` runs a `LiveFeed`. In production that is `alpaca_stream::AlpacaStream`
  (implemented 2026-10-18): Alpaca's market-data WebSocket, `trades` (corrections and
  cancel/errors come with them) and `statuses` for `LIVE_SYMBOLS` (default `*`, the whole market).
  Without credentials it fails with **not configured**, naming the variables.
  - Settings: `ALPACA_API_KEY` / `ALPACA_API_SECRET` (or `APCA_API_KEY_ID` /
    `APCA_API_SECRET_KEY`, the importer's names), `ALPACA_STREAM_BASE` (default
    `wss://stream.data.alpaca.markets`; provider-sim is `ws://127.0.0.1:8791`),
    `ALPACA_STREAM_FEED` (`sip` default, `iex`, `boats`, `overnight`, or `test` for Alpaca's fake
    `FAKEPACA` stream, labelled fixture), `LIVE_SYMBOLS`.
  - Normalization: a stream trade is the historical trade object plus `T`/`S`, so it goes through
    the importer's `to_trade` and a print is the same contract event live and from history.
    `c` → `Correction`, `x` → `Cancel`, `s` → `Status`: `sc` and `rc` (else `rm`) are kept verbatim;
    `H`/`P` (UTP) and `2` (CTA) are `Halt`, `T` (UTP) and `3` (CTA) are `Resume`, everything else
    (including UTP `Q`, quotation resumption) is `Other`.
  - Reconnect: exponential backoff (1 s doubling to 60 s, equal jitter), reset only after a session
    stayed subscribed 30 s; 402/405/406/409/410 wait the full 60 s. Each reconnect re-authenticates
    and re-subscribes. Events published while disconnected are lost (no backfill yet).
  - Backpressure: the socket reader waits at most 2 s on a full channel, then drops events
    (counted) until the consumer catches up, and reports the gap. It never stops reading long
    enough for Alpaca's 407 slow-client disconnect.
  - Connection state reaches the server as `FeedItem::Link`: status `starting`, `running` or
    `reconnecting`, with an operator-facing detail (never credentials).
  - Live mode **does not archive** what it receives. History comes from the archiver's nightly SIP
    import of each completed day instead (Appendix B), so a replayable day exists the evening after
    the session.
- With a feed, `LiveProvider` stamps `receive_ts` (the Alpaca client stamps it at frame receipt)
  and applies the same mark rule, plus a 1 s idle tick.
- Parity tests:
  - the fixture archive through `ReplayProvider`, and the same events through a `ChannelFeed`
    into `LiveProvider`, give identical tape rows, snapshots (all timeframes), profiles and quality
    counters. The late, cancel and correction paths are exercised.
  - `provider-sim/tests/live_client.rs`: replay drives provider-sim, and the real Alpaca client
    reads its SIP stream. Every event matches the archive's, and engine output is identical. The
    fixture's recorded receive times are stripped there, because Alpaca's wire carries none.

## Appendix C: provider-compatible simulator (`crates/provider-sim`)

It is served by the server process on its own listener: `SIM_HOST` (default 127.0.0.1) and
`SIM_PORT` (default 8791). It runs in replay mode only, while a replay is running, and
`SIM_ENABLED=0` turns it off. Swap the base URLs to `http://127.0.0.1:8791`, or `ws://` for the
streams:
- `data.alpaca.markets`
- `paper-api.alpaca.markets`
- `stream.data.alpaca.markets`
- `financialmodelingprep.com`

Auth uses **dummy** local credentials only:
- Alpaca: `SIM_ALPACA_KEY` / `SIM_ALPACA_SECRET` (default `sim-key` / `sim-secret`), sent as the
  `APCA-API-KEY-ID` / `APCA-API-SECRET-KEY` headers or HTTP Basic;
- FMP: `SIM_FMP_KEY` (default `sim-fmp-key`) as `apikey=`.

Shapes follow docs.alpaca.markets as read on 2026-10-17. Golden tests cover every implemented
route and stream message. Error bodies (`{"code":40010001,"message":…}`,
`{"code":40110000,"message":"request is not authorized"}`) approximate Alpaca's; the docs give no
JSON error bodies.

**No future data.**
- REST reads the archive file up to the replay clock: an event is visible iff its delivery time is
  at or before `now`, and `end` is clamped to `now`.
- Page tokens pin `end`, so pages stay consistent while the clock moves.
- The streams emit exactly what replay emits, in replay order.

| Implemented | Notes |
|---|---|
| `GET /v2/stocks/{symbol}/trades` | `start`/`end` (RFC 3339 or `YYYY-MM-DD` = ET day), `limit` 1–10000 (default 1000), `page_token`, `sort`; `feed=sip` (default; pre/regular/after) or `boats`/`overnight` (overnight session only). Response `{trades:[{t,x,p,s,c,i,z[,u]}],symbol,currency,next_page_token}`. Cancelled prints carry `u:"canceled"`; a corrected print appears as `u:"incorrect"` plus a new `u:"corrected"` entry (simulator interpretation of Alpaca's `u`) |
| `GET /v2/stocks/{symbol}/bars` | `timeframe=1Min`/`1T` **only** (Alpaca has no 15 s bars), `adjustment=raw` only. Final 1m candles from the real chart engine (same rules as the chart); `vw` = VWAP of the prints the engine counted toward volume, net of cancels/corrections (Alpaca does not document its `vw` inclusion rules) |
| `GET /v2/stocks/snapshots?symbols=` | `latestTrade` (last print delivered, uncorrected), `latestQuote`, `minuteBar` (last final 1m bar), `dailyBar` (regular-session 1m candles so far, `t` = ET midnight), `prevDailyBar: null`; unknown symbols omitted |
| `GET /v2/assets` | From the manifest's symbols; `status`, `asset_class`, `exchange` filters; deterministic ids; fixture names say "simulator fixture, not a real security". Alpaca's endpoint has no price filter |
| WS `/v2/sip` | connected → auth (message or headers, 10 s timeout) → `subscribe`/`unsubscribe` for `trades` `quotes` `bars` `updatedBars` `dailyBars` `statuses` `lulds`. `corrections` and `cancelErrors` follow `trades`. Messages `t q b u s c x` in Alpaca's fields; frames are arrays and may coalesce. Errors 400/401/402/403/404/407 (slow client, then close) |
| WS `/v1beta1/boats`, `/v1beta1/overnight` | Same protocol; carry only overnight-session events (SIP carries the rest) |
| WS `/v1beta1/news` | Auth plus a `news` subscription; emits nothing (the contract has no news events yet) |
| `GET /stable/shares-float?symbol=&apikey=` | FMP shape `[{symbol,date,freeFloat,floatShares,outstandingShares}]`, **synthetic** values per symbol, header `x-stockwatcher-fixture: true`. FMP's docs refused automated fetching on 2026-10-17, so check the field list against a real response |
| `GET /sim/universe?min=&max=` | **Simulator-only**, not Alpaca: each symbol's latest price at the clock and whether it is inside $0.10–$25 (`market_events::PriceBand`) |

**Not implemented** (all answer 404 or are accepted but silent):
- quotes, latest-trade, latest-quote, latest-bar and multi-symbol `/v2/stocks/trades` and `/bars`;
- bars other than 1Min, adjustments, `asof` symbol mapping, `currency` other than USD;
- `/v2/assets/{symbol}`, the trading API beyond assets (orders, account, positions, clock,
  calendar), and screener/movers;
- IEX, `delayed_sip` and OTC feeds;
- `dailyBars`, `lulds` and `imbalances` stream messages;
- msgpack encoding;
- connection, symbol and entitlement limits (406, 405, 409);
- news content and FMP routes other than shares-float.

## Appendix D: server wiring, protocol and running it

**Ingest.** One task runs: provider → `provider-sim` → one `ChartEngine`, with per-symbol state
for every timeframe. Each item is applied under a short lock, and an engine sequence number is
stamped on it. The increments are serialized to JSON once and broadcast on a bounded channel of
16,384 items, which is each connection's queue.
- A client that lags gets a fresh `chart.snapshot` (resync) instead of more memory.
- Snapshots are taken under the same lock as the sequence number. A client drops increments with
  `seq` at or below its snapshot's, so a candle never moves backwards.
- During a seek's rebuild nothing is broadcast. The rebuild's closing `Clock` triggers a resync
  for every subscriber, and the visible clock does not jump until then.

**`/ws`.** The existing hello/token/heartbeat handshake is unchanged. After `welcome`:
- client `chart.subscribe {symbol, timeframe, profileBin}` → `chart.snapshot` (with `clock`), the
  last 40 `tape.trade` rows, then `chart.update` / `chart.remove` / `chart.gap` for that symbol and
  timeframe and `tape.trade` for the symbol;
- there is one subscription per connection. Switching timeframe or bin is a new subscribe on the
  same engine state;
- other bin widths are re-binned from the engine's exact per-price volume, with absolute totals;
- server `replay.clock {clock:{mode,now,speed,playing}}` goes to every client after `welcome`, on
  every change, and about 4 Hz while playing;
- client `replay.control {action, value}`: `play`, `pause`, `step`, `stepInterval` (`"15s"`…`"5m"`,
  default `"1m"`), `seek` (epoch ms or RFC 3339), `speed` (multiplier, 0 = max), `day`
  (`"YYYY-MM-DD"`, one of `/api/replay/state`'s `days`). There is no direct reply; the next
  `replay.clock` is the answer;
- a `day` switch resets exactly what a seek resets (engine, tape rings, quote board, paper
  account, the simulator's bar book) and every subscriber gets fresh snapshots; a subscribed
  symbol the new day lacks answers that resync with `unknown_symbol` (or `outside_price_band`);
- errors: `not_replay` (live mode), `bad_request`, `unknown_symbol`, `unknown_day` (not an
  offered day), `day_unavailable` (offered, but its files could not be opened).

**REST** (behind the bearer token when one is set):
- `GET /api/chart/{symbol}/snapshot?timeframe=1m&bin=0.01`;
- `GET /api/replay/state` returns mode, provider status (`running`, `no_data`, `not_configured`,
  `error`, `stopped`), source, day, `fixture`, symbols, `controllable`, clock, start/end, events,
  seq and `days`: every replayable archive day, ascending, as `{day, fixture, symbols}` (empty in
  live mode). Per-day facts come from the manifests, cached per manifest length and mtime, so the
  route stays cheap with real days of thousands of symbols. The clients' day menu fetches this
  when it opens; the list is not pushed on the socket because it changes a few times a month;
- `GET /api/symbols` returns mode, status, source, day, `fixture` and symbols;
- `GET /api/quotes` returns the quote board once (the same rows as `quotes.snapshot`, below);
- `GET /api/settings` returns the synced-settings snapshot, for debugging (`settings-sync.md`).

`/api/health` is unchanged and stays healthy when no provider runs.

**Quote board** (the ticker search's live rows; `crates/server/src/quotes.rs`, wire in
`packages/shared/src/protocol.ts` `QuoteRowWire`):
- client `quotes.subscribe` (no payload) → server `quotes.snapshot {t, quotes: [row]}` with every
  price-band candidate that has traded, then `quotes.update {t, quotes: [row], gone?: [symbol]}` with
  only the changed rows, at most once per wall-clock second. `gone` lists symbols that left the band.
  `quotes.unsubscribe` stops it; resubscribing after a reconnect is the client's job.
- Row `{s, p, pc?, o, v, n, pre?, name?, x?}`: symbol, last price, **previous close** (the change-%
  reference whenever present), the fallback reference (regular open, or the day's first price while
  `pre`), day volume, prints counted, and `name`/`x` (exchange) only in snapshots and for rows entering
  the band. Change % is `(p − ref) / ref`, computed by the client (`changeReference` in chart-core).
- After a seek, a day switch or a lag, a subscribed client gets a fresh snapshot.

**Synced settings** ride the same socket (`settings.*`); see `settings-sync.md`.

**Web.**
- `VITE_CHART_SOURCE` unset or `server` (the default): the app probes `GET /api/symbols` and uses
  the server.
- It falls back to the in-browser fixture **only** if the server is unreachable, refuses, or
  reports no symbols.
- The header always shows which source is active, with SERVER/BROWSER and FIXTURE badges and the
  reason.
- The play, pause, step and speed buttons send `replay.control`, and the displayed clock is the
  server's.
- `VITE_CHART_SOURCE=fixture` forces the in-browser fixture.

**Run it locally.**

```sh
SIM_AUTOFIXTURE=1 bun dev        # server :8790 (+ simulator :8791) and Vite :5173; writes a fixture day into ./data/archive on first run
# or separately:
SIM_AUTOFIXTURE=1 cargo run -p stockwatcher-server
bun run --filter @stockwatcher/web dev
# archive tools
cargo run -p market-archive --bin swarchive -- fixture 2026-10-15 SMPL FXLO
cargo run -p market-archive --bin swarchive -- verify 2026-10-15
# simulator smoke test (dummy keys)
curl -H 'APCA-API-KEY-ID: sim-key' -H 'APCA-API-SECRET-KEY: sim-secret' 'http://127.0.0.1:8791/v2/stocks/snapshots?symbols=SMPL'
```

Live mode is `MARKET_DATA_MODE=live` plus the Alpaca keys in the server's environment. Without the
keys it reports `not_configured` and the web app falls back to the labelled fixture. The production image sets `ARCHIVE_ROOT=/app/data/archive` and
does not auto-generate fixtures. Alpaca allows one stream connection per account on most plans, so run
live mode on one host only.

## Appendix E: the whole chart in the chart packages (2026-10-17)

Owner rule: everything chart-related is boxed into the chart, so any screen reuses all of it. Apps
pass data in and wire callbacks out, and nothing else.

| Package | Holds |
|---|---|
| `chart-core` (pure TS, no DOM, no RN) | Layout and display list, scales, `GestureController`, **indicators** (`indicators.ts`: SMA, EMA, session VWAP, Bollinger, Wilder RSI, MACD, aligned to slots, NaN on gaps), **drawings** (`drawings.ts`: time and price anchors, hit testing, drag maths, clipping), **trading** (`trading.ts`: paper-broker wire types, tick grid `snapPrice`, order and position lines, the place button, hit testing), and the shared `DEFAULT_CHART_THEME` |
| `chart-web` (React + Canvas 2D, shadcn on Base UI) | `CandleChart` with `toolbar`, `indicators`, `drawings`, `persistKey` and `trading` props; `ChartToolbar`, `OrderTicket`, `PriceAxisMenu` |
| `chart-native` (React Native + Skia + Gesture Handler + expo-haptics) | The same `CandleChart` API; Skia executes the display list (`drawSkia.ts`); touches go into the same `GestureController`; native toolbar and bottom sheets (indicators, price axis menu, order ticket) |

*(now)* Both packages have grown well past this table: chart types, price scales, zoom presets, the
Settings dialog/sheet, the drawing toolbox, levels, the trade menu, the timeframe wheel and more.
`chart-packages.md` is the current inventory and the embedding guide.

**Display list additions:**
- Lines and paths take an optional `clip` rect, so an indicator line never bleeds into another pane on
  a manual price range.
- Oscillators get panes stacked between the volume pane and the time axis (`indicatorPaneHeight`,
  hit region `"pane"`). The inspection line runs through all of them.

**Gesture controller additions** (constructor options, all off by default so the original recognizer
is what the existing tests pin):
- `momentum`: fling after a fast touch pan.
- `touchPricePan`: `"manual"` on the web, `"always"` on the phone.
- `haptics`: emits `haptic` actions.

Objects come in through `ctx.hitObject` and `ctx.tool`. The new actions are `draft`, `createDrawing`,
`select`, `objectDrag` (with `done` on release, so an order is modified once), `cancelOrder`,
`placeOrder` and `haptic`.

**Why Skia on the phone:** `@shopify/react-native-skia` is MIT, maintained by Shopify with many
contributors, supported by Expo, and widely used for charts. It draws the whole display list as one
picture per frame, natively, so the phone renders exactly what the browser renders. react-native-svg
(already in the app) would have meant hundreds of React elements per frame on a pan. It is a native
module, so the phone needs a new binary (the release pipeline builds an APK when the fingerprint
changes); OTA alone cannot deliver it.

**Indicator caps:** at most 3 overlays and 2 panes. The overlay colours are three fixed categorical
slots that the dataviz validator passes **all-pairs** on the dark surface (worst CVD ΔE 13.2), and lines
over candles are effectively all-pairs. A fourth colour would fail, so the menus never offer a fourth.

**Position tool (2026-10-17;** design canvas https://claude.ai/artifact/UkBuQdEMyZjAYmdMsTzatW**).**
`chart-core/src/position.ts` adds Long and Short as drawing kinds:
- Three anchors: `a` is the entry, `b` the target, `c` the stop. The zones run from `a.t` to `b.t`.
- A `risk` budget in dollars, and an optional `qty` override.

Behaviour:
- **Sizing:** qty = floor(risk ÷ |entry − stop|), and R:R = reward per share ÷ risk per share.
- **Default plan:** a tap places entry, stop and target at 2R. The stop distance is an eighth of the
  visible price range, but at least 1% of the price.
- **Handles:**
  - target and stop drag their price;
  - entry drags price and start time;
  - an end handle sets how far the zones reach;
  - the body moves the whole plan.
- **Editing limits:** prices stay on the broker's tick grid and in order, one tick apart at least, so
  the tool can never show a plan the broker would reject.
- **Plan card:** a selected position shows its plan: `PositionPlanCard` over the web chart, and
  `PositionPlanPanel` sliding up over the bottom of the native chart, so the chart never resizes.
  "Place bracket" sends one simulated bracket order: a limit entry plus a take-profit and a
  stop-loss that cancel each other (contract: `chart-trading-sim.md`, "Bracket orders"). The
  drawing then goes, and the order lines take its place.
- **Order lines:** bracket exits waiting for their entry draw faintly. Pills read "TP · …" and
  "SL · …", and two working exits of one group show an OCO link.
- **Keys:** L and S pick the tools on the web.

## Appendix F: provider history import (2026-10-17)

Code: `crates/market-providers/src/import/` (logic, network-free), `src/import/client.rs` and
`src/bin/swimport.rs` (feature `import`), `crates/market-archive/src/bars.rs`. Tests:
`crates/market-providers/tests/import.rs` (scripted HTTP, no credentials).

**Roles.**
- **Alpaca historical trades** (`GET /v2/stocks/{symbol}/trades`, `feed=sip`, `sort=asc`,
  `limit` ≤ 10 000, `next_page_token`) are the trade-faithful replay source. Each print becomes a
  contract `Trade` once, streamed page by page into the archive writer.
- **FMP 1-minute bars** (`GET /stable/historical-chart/1min`) are a separate comparison dataset in
  `bars/fmp-1min/<SYMBOL>.json`, listed in the day manifest's `bars` map with their own checksum.
  They are never events, never replayed and never turned into trades, sub-minute candles or a
  volume profile.

**Field mapping.** `t` becomes `event_ts` exactly (ns, UTC). No receive time is invented. `p`
becomes an exact decimal price; `s`, `x`, `z`, `c` and `i` become size, exchange, tape,
conditions and `source_seq`. The feed decides the source (`sip` → `alpaca_sip`, `iex` →
`alpaca_iex`, `boats` → `alpaca_boats`).

**Corrections and cancels.** Historical responses mark a print with `u` (`canceled`,
`incorrect`, `corrected`) but give no publication time and no link from a corrected print to the
original. That cannot become a `Correction` or `Cancel` event without guessing, so `u` is kept
raw in the new optional `Trade::update` (contract v1, additive). `Trade::effect` excludes
`canceled` / `incorrect` prints (tape reason `withdrawn_*`) and counts `corrected` ones. The
chart engine and the paper broker use it. Per-import counts are kept in the manifest.

**Completeness and idempotency.**
- Pages are followed until the token runs out, whatever a page's length. A repeated token is
  refused.
- Retries are bounded: 5 attempts, exponential from 0.5 s to a 30 s cap, `Retry-After` honoured.
  Only 408, 429, 5xx and network errors are retried; 401/403/400 fail at once.
- Any failure leaves `<SYMBOL>.swa.partial` and no manifest entry, so it is never labelled
  complete.
- `SymbolEntry.import` records the provider, dataset, endpoint, feed, requested range,
  `imported_at`, pages, `complete`, update counts and out-of-order count. Re-running the same
  request is a no-op; a different one needs `--force`; a re-run after an interruption starts over.

**Credentials.** `ALPACA_API_KEY` / `ALPACA_API_SECRET` (or `APCA_*`) and `FMP_API_KEY`, from the
environment only.
- Never printed: `Debug` is redacted, `swimport credentials` shows set / not set.
- FMP's `apikey` is shown as `REDACTED` in every log line and error. Error bodies are scrubbed of
  every secret. reqwest errors are taken `without_url`.

**Statuses and LULD.** Historical trades carry no halt, resume or LULD messages. An imported day
therefore has no authoritative halts, and the LULD bands on the chart remain the labelled model
built from trades.

**Running it** (a completed trading date only; SIP range = premarket open through the end of after
hours, from the session calendar):

```sh
cargo run -p market-providers --features import --bin swimport -- credentials
cargo run -p market-providers --features import --bin swimport -- alpaca-trades ABCD 2026-10-14
cargo run -p market-providers --features import --bin swimport -- fmp-bars ABCD 2026-10-14
cargo run -p market-providers --features import --bin swimport -- check ABCD 2026-10-14
```
