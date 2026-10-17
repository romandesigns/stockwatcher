# Architecture: simulator and chart (spec 01)

Status: proposed 2026-10-17. It implements `docs/specs/01-market-simulator-and-chart.md`. The UI direction is
**C · Session timeline** (design canvas https://claude.ai/artifact/ECxnETgKWmPAEBmPdkvnrY).
Work happens on the branch `feat/simulator-chart` and merges to `main` after validation: the spec rules out
deployment in this phase, and every push to `main` deploys.

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
| `market-providers` | A `MarketDataProvider` trait with **`ReplayProvider`** and **`LiveProvider`** (the Alpaca SIP + BOATS WS client and its normalizer; FMP float lookup); `ArchiveRecorder`, which captures live sessions for parity tests | above |
| `provider-sim` | The local, provider-compatible surface: the Alpaca REST/WS subset and the FMP float route that Stockwatcher uses, served from replay. Dummy auth; no external calls | `market-providers` |
| `server` (exists) | Wires provider, engine and client WS (`/ws` chart subscriptions and replay control) plus REST (`/api/chart/...`) | all |

## Client: a modular, reusable chart

| Package | Responsibility |
|---|---|
| `packages/chart-core` (TS, framework-free) | Candle and profile store (applies the server's snapshot and increments), time and price scales, viewport (zoom/pan), hit testing (candle inspection), and a **display list**: rects, lines and text with no rendering API calls. Fully unit-tested |
| `packages/chart-web` (React + Canvas 2D) | Draws the display list on `<canvas>` with DPR handling, plus React components: `<Chart>`, `<TimeframeSelector>`, `<SessionTimeline>`, `<Tape>`, `<VolumeProfile>`. Composable, so any future feature can embed just the chart |
| RN later | The same `chart-core` with a Skia renderer (`@shopify/react-native-skia`) executing the same display list. One rendering logic for two platforms |

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

- Archives go on the Pi (`/storage`, about 6.7 TB free) **only after** reachability, throughput and compute are
  verified. Until then they stay local (configurable `ARCHIVE_ROOT`), behind a free-space floor and a retention
  limit in days.
- No database, broker or microservices. One server process per mode. Bounded memory per symbol: candle ring
  buffers per timeframe; the profile is per session.

## Credentials

- Simulation needs none (dummy auth).
- Live mode reads `ALPACA_API_KEY` / `ALPACA_API_SECRET` / `FMP_API_KEY` from the environment (a local
  gitignored `.env`, or a secret store). Credentials previously shared in chat are **never** used.

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

**`swarchive`** (`cargo run -p market-archive --bin swarchive -- …`):
- `days`;
- `fixture <day> [SYMBOL…] [--seed N]` writes a deterministic **fixture** day (default SMPL, FXLO,
  FXMD, FXHI, seed 7);
- `inspect <day> [--symbol S] [--records N]`;
- `verify <day>` recomputes checksums and decodes every record; it exits non-zero on mismatch;
- `prune [--today D]`;
- `import alpaca …` is a skeleton. It refuses to run unless `SWARCHIVE_IMPORT=alpaca`, and even
  then reports "not implemented". It never reads credentials and never calls the network.

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

One tokio task drives everything, with no per-symbol tasks. A 50 ms pacing tick while playing,
`virtual = anchor + wall_elapsed × speed`, re-anchored on every control. Output goes through a
bounded channel (8192), so a slow consumer slows replay rather than growing memory; at max speed,
batches of 2048 events keep commands responsive. A seek over a whole fixture day (~48k events by
10:40) takes about 2–3 s in a debug build.

**Providers.** `MarketDataProvider::start() → ProviderSession { info, items, clock, control }`.
- `MARKET_DATA_MODE=replay|live` (default `replay`).
- Replay settings: `REPLAY_DAY` (default: the latest archived day), `REPLAY_START` (ET `HH:MM[:SS]`,
  default 10:40, matching the browser fixture), `REPLAY_SPEED` (default 10), `REPLAY_AUTOPLAY`
  (default 1).
- `LiveProvider` is the boundary only. Without a `LiveFeed` it fails with **not configured**: the
  Alpaca SIP and BOATS WebSocket client waits for entitlement verification. It reads no
  credentials in this build.
- With a feed, `LiveProvider` stamps `receive_ts` and applies the same mark rule, plus a 1 s idle
  tick.
- Parity test: the fixture archive through `ReplayProvider`, and the same events through a
  `ChannelFeed` into `LiveProvider`, give identical tape rows, snapshots (all timeframes), profiles
  and quality counters. The late, cancel and correction paths are exercised.

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
| `GET /sim/universe?min=&max=` | **Simulator-only**, not Alpaca: each symbol's latest price at the clock and whether it is inside $0.15–$25 (`provider_sim::universe::in_price_band`) |

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
  default `"1m"`), `seek` (epoch ms or RFC 3339), `speed` (multiplier, 0 = max). There is no
  direct reply; the next `replay.clock` is the answer;
- errors: `not_replay` (live mode), `bad_request`, `unknown_symbol`.

**REST** (behind the bearer token when one is set):
- `GET /api/chart/{symbol}/snapshot?timeframe=1m&bin=0.01`;
- `GET /api/replay/state` returns mode, provider status (`running`, `no_data`, `not_configured`,
  `error`, `stopped`), source, day, `fixture`, symbols, `controllable`, clock, start/end, events
  and seq;
- `GET /api/symbols` returns mode, status, source, day, `fixture` and symbols.

`/api/health` is unchanged and stays healthy when no provider runs.

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

Live mode is `MARKET_DATA_MODE=live`. In this build it reports `not_configured` and the web app
falls back to the labelled fixture. The production image sets `ARCHIVE_ROOT=/app/data/archive` and
does not auto-generate fixtures.

Known dev-only issue: in `vite dev`, React StrictMode leaves chart canvases blank. `useCanvas` in
`packages/chart-web` cancels its pending frame in cleanup without resetting `frame.current`, so it
never schedules again. Production builds are unaffected. The fix is one line
(`frame.current = 0` after `cancelAnimationFrame`).
