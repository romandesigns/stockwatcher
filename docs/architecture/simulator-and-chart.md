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
