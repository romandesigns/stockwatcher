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
 "candles":[{"t":1760535600000,"o":3.18,"h":3.21,"l":3.17,"c":3.20,"v":41200,"n":87,"final":true}],
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
