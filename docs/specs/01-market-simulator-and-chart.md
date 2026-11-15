# Spec 01: Market simulator and chart

Status: **implemented and deployed** (brief received 2026-10-16, built 2026-10-17/18; the "no deployment in this
phase" rule has been lifted and archives now live on the VPS, not the Pi). This file is the historical record:
the owner's brief is reproduced verbatim in section A; section B records the agreed reading of it and the
decisions or open questions that followed. The current design is `docs/architecture/simulator-and-chart.md`.

---

## A. Brief (verbatim)

# Stockwatcher: Build the Market Simulator and Chart First

We are starting the application implementation from zero. The first working system should be a market-session simulator and chart pipeline that we can use to develop and test Stockwatcher before relying on a live market opening.

Use Rust for the performance-critical market-data ingestion, replay, normalization, and chart aggregation. Inspect the workspace before choosing the rest of the stack; do not assume existing application services or crates exist. Keep the implementation focused and resource-efficient.

## Product context

Stockwatcher is an intraday bullish-event detection and setup-intelligence application, not a stock screener. It will monitor stocks priced from **$0.15 through $25 per share**.

The user's chart workflow is:

- **5-minute:** market structure and context
- **1-minute:** decision framing
- **30-second:** execution context
- **15-second:** finer execution view

For this phase, implement and validate the simulator and chart. Do not yet implement bullish strategies, ignition detection, ranking, machine learning, automated trading, or live order execution. The central-versus-specialist ML discussion is parked until after the simulator is validated.

## Main objective

Build two interchangeable market-data providers:

1. A **historical replay provider** that emits archived market events on a controllable virtual clock.
2. A **live provider** that emits current market events using the same normalized event contract.

The application should be able to switch between them through configuration. The chart component, aggregation rules, and downstream application logic must remain the same in replay and live modes.

The simulator should expose local REST and WebSocket interfaces compatible with the subset of Alpaca and FMP behavior the application actually uses. Do not try to clone either provider's entire API.

## Potential resources

The user currently has:

- An Alpaca subscription with SIP-related and BOATS overnight settings
- An FMP subscription currently used for float-share data
- A Pi server with approximately 8 TB of storage

Verify the current subscription entitlements, feed coverage, and server accessibility before relying on them. Check the Pi's actual compute, network, and storage configuration before assigning it work.

**Never include, use, log, or commit the API credentials previously shared in chat.** Use local dummy credentials for the simulator. Keep any replacement credentials in local environment variables or a secret store, outside source control.

## Build sequence

1. **Inspect the workspace.** Confirm its current state and any project instructions. Propose a concise architecture, then implement the simulator foundation.
2. **Verify available data.** Determine which historical and live trades, quotes, bars, overnight events, news, and float-share data the current Alpaca and FMP plans actually allow. Do not assume a subscription includes every endpoint or history range.
3. **Define a normalized event contract.** Include symbol, event timestamp, receive timestamp when available, price, size, session identifier, source sequence, exchange, source, and relevant trade conditions. Add quote, correction/cancel, and market-status events where supported and needed.
4. **Build the historical archive and replay provider.** Preserve source timestamps and event detail. Replay must be deterministic and support fast-forward, pause, step, and selectable replay speed.
5. **Build the live-provider boundary.** Normalize live provider events into the same contract. Do not enable live order execution.
6. **Implement the shared chart engine.** It must consume normalized events in both replay and live modes and produce all four chart timeframes.
7. **Implement a minimal responsive chart view** so candle and profile behavior can be reviewed before adding strategy features.
8. **Test replay, aggregation, and provider parity.** Use deterministic fixtures for software tests and real archived events for market-data validation. Never treat fabricated test data as market evidence.

## Required chart implementation

Build a single chart component that lets the user select **15s, 30s, 1m, or 5m** for the currently selected stock.

The user must be able to switch timeframes while replay is running or while live data is arriving. The selected timeframe must update from the same underlying event stream and chart rules. Do not request or construct a different, provider-specific candle path for each mode.

For every timeframe, build candles from timestamped trades using consistent rules:

- Open: first qualifying trade in the interval
- High: highest qualifying trade
- Low: lowest qualifying trade
- Close: last qualifying trade
- Volume: sum of qualifying traded share size

Display candle bodies, upper and lower wicks, and volume in a readable chart. Update the currently forming candle as new trades arrive, then finalize it when the interval ends. Handle boundary timestamps consistently.

Include a **session volume profile** that aggregates actual traded volume by price bin. Make the bin width configurable. The profile should use the same trade inclusion and session rules in replay and live modes. Changing candle timeframe should not silently change the profile's session anchor or price-bin rules.

The chart should include, at minimum:

- A timeframe selector for 15s, 30s, 1m, and 5m
- A responsive layout usable on mobile and desktop
- A visible price axis and time axis
- Clearly rendered candle bodies and wicks
- A volume display and session volume profile
- A way to inspect a candle's timestamp, OHLC, and volume

Use the existing UI framework if one is present. If none exists, choose a minimal maintainable client approach and document the choice before adding unnecessary infrastructure.

Define and document how the chart handles:

- Session boundaries and Eastern Time, including daylight-saving changes
- Premarket, regular, after-hours, and supported overnight sessions
- Trades with special conditions, corrections, cancels, and late arrival
- Missing data and out-of-order events
- Intervals with no trades

Do not fabricate zero-volume candles or interpolate prices unless the chosen chart convention explicitly requires it; mark data gaps clearly.

## Data fidelity requirements

Faithful 15-second and 30-second candles require trade events with timestamps finer than one minute. A true volume profile requires trade-level price and size information. One-minute OHLCV bars cannot reconstruct the order of trades inside the minute, genuine 15-second candles, or actual traded volume by price.

Verify that the available Alpaca historical data supports the required trade granularity and is compatible with the live feed. FMP minute bars may be useful for comparison or fallback, but they are not sufficient by themselves to prove 15-second or trade-based profile fidelity.

If suitable history is unavailable:

- Report the limitation clearly.
- Do not present minute-bar-derived 15-second candles or estimated price-volume profiles as exact.
- Do not purchase or upgrade data plans without user approval.
- Keep the provider boundary ready for a compatible historical source once the user authorizes it.

## Provider-compatible simulator

First identify the exact Alpaca and FMP routes, subscription messages, authentication shapes, and event formats that Stockwatcher needs. Implement only that subset locally.

Where consumed by the application, account for:

- REST-based symbol, asset, universe, or scanner lookups
- SIP trade, quote, bar, and status events
- BOATS overnight events
- News events
- FMP float-share lookup

In simulation mode:

- Point provider base URLs and WebSocket URLs to localhost through configuration.
- Serve deterministic replay data using the expected local request and event formats.
- Make REST snapshots correspond to the replay clock.
- Emit WebSocket events in replay timestamp order at the selected playback speed.
- Do not call external market-data services.
- Use dummy local authentication only if the application requires authentication.

In live mode, select the real provider URLs and load credentials only from local environment variables or a secret store. Switching modes must not require changes to the chart engine or strategy logic.

## Resource-efficient design

The system should be suitable for monitoring thousands of symbols without creating a thread or polling loop for every symbol.

- Use event-driven ingestion and incremental per-symbol state updates.
- Normalize each incoming event once and route it to the required consumers.
- Keep compute bounded and avoid unnecessary full-history recalculation on every update.
- Store historical data in a practical, documented format on the Pi only after confirming it is reachable and suitable.
- Set configurable storage locations, data-retention rules, and disk-space safeguards.
- Do not introduce microservices, a message broker, Kubernetes, or a separate database unless measured needs justify them.

## Verification and acceptance criteria

At minimum, test:

- Deterministic replay of the same event file
- Timestamp ordering and same-timestamp sequence behavior
- Correct OHLC, wicks, and volume for each timeframe
- Exact 15-second, 30-second, 1-minute, and 5-minute boundaries
- Switching timeframes during replay without changing source data or candle rules
- Current-candle updates and interval finalization
- Profile bins, cumulative volume, configurable bin width, and session separation
- Session boundaries and supported extended/overnight periods
- Missing, duplicate, late, corrected, cancelled, and out-of-order events
- Application consumption of both providers through the same normalized contract
- Switching simulation/live mode through configuration without changing chart logic
- Replay parity: capture a live session, replay the same archived event log, and compare the resulting candles and profile
- Comparison with a trusted data or chart reference where feed coverage and chart rules match

The simulator must never claim exact chart parity if its historical feed differs in coverage, trade-condition handling, corrections, or session rules from the live feed. Document any differences.

## Scope and delivery

Keep this phase focused on the simulator, provider boundary, and chart implementation. Do not implement strategy rules, model training, ranking, auto-trading, live order execution, or deployment in this phase.

Aim to produce a validated simulator and chart foundation before Monday's market open. Do not claim the full application is production-ready unless the relevant data, reliability, and operational checks have passed. Report blockers plainly.

When finished, report:

- The architecture and files implemented
- How to run the simulator and chart
- How to select simulation versus live mode
- Verified Alpaca and FMP data access, with unresolved entitlement questions
- Tests and replay comparisons performed
- Whether 15s, 30s, 1m, and 5m chart outputs are trade-faithful or limited by available data
- Known fidelity, session, and operational limitations
- The next smallest step after simulator validation

---

## B. Agreed reading, decisions and open questions

**Restated essentials**

1. **Providers:** two interchangeable providers, historical replay and live, behind **one normalized event contract**. Mode is configuration only. The chart engine and everything downstream are identical in both modes.
2. **Rust core:** ingestion, normalization, replay and aggregation in Rust. Event-driven, with incremental per-symbol state; no per-symbol threads or polling, and no broker, microservices or separate database without measured need.
3. **Local provider surface:** only the Alpaca/FMP REST + WS subset Stockwatcher uses, on localhost:
   - REST snapshots follow the virtual clock;
   - WS events are emitted in replay order at the chosen speed;
   - dummy auth;
   - no external calls in simulation.
4. **One chart component:**
   - **15s / 30s / 1m / 5m**, switchable live, all built from the same trade stream (O/H/L/C = first/high/low/last qualifying trade, V = sum of qualifying size);
   - a forming candle that finalizes at the interval end, with exact boundaries;
   - candle inspection (time, OHLC, V);
   - a session volume profile with configurable bin width and a session anchor independent of the timeframe;
   - responsive on phone and desktop.
5. **Fidelity first:**
   - 15s/30s candles and a real volume profile need trade-level history; one-minute bars cannot produce them.
   - Entitlements are verified, never assumed. Limits are reported plainly, and nothing estimated is presented as exact.
   - No plan upgrades without the owner's approval.
6. **Credentials:** never use the credentials previously shared in chat. Simulator credentials are dummies; live credentials come only from local env vars or a secret store.

**Rules to define and document in the implementation:**
- Eastern Time and DST.
- The pre, regular, after-hours and overnight (BOATS) sessions.
- Trade conditions, corrections, cancels, late and out-of-order events, duplicates.
- Gaps: no fabricated zero-volume candles and no interpolation; gaps are marked.

**Carried over from an earlier trading app (lessons, re-verified before use):**
- Alpaca's own bars follow a trade-condition table. Odd lots count toward volume but not price; average-price, late/out-of-sequence and similar prints are excluded from OHLC. The chart must apply the same table, or replay will not match reference charts.
- A late print must never reset a forming candle. Only a strictly newer interval opens a new candle.
- Market-hours logic must use an explicit America/New_York timezone database. On the owner's Windows machine, Git Bash ignores `TZ=America/New_York`.
- A research copy's Alpaca keys did serve SIP history on 2026-10-16. Entitlements and history range still have to be re-verified for this project.

**Pi (as found 2026-10-16):** `/storage` md RAID, 7.3 TB, about 6.7 TB free after an earlier cleanup. Compute and network are still to be checked before it is assigned archive work.

**Price universe (decided 2026-10-17, supersedes the brief's $0.15 floor):** only stocks priced **$0.10 through $25.00** (inclusive) are candidates. The Rust server lets nothing else through: `/api/symbols` lists candidates only, snapshots and subscriptions for other stocks are refused (`outside_price_band`), and a live subscription ends when its stock trades out of the band. Membership uses the latest price-qualifying trade at the clock, so stocks enter and leave as they trade; the engine keeps tracking every stock, so a stock entering the band arrives with full chart history. The band is defined once (`market_events::PriceBand`) and is configurable (`PRICE_BAND_MIN` / `PRICE_BAND_MAX`).

**Phase scope vs. the pipeline (decided 2026-10-17):** work happens on the branch `feat/simulator-chart` and merges to `main` only after validation, because the spec rules out deployment in this phase. This is the safer default; the owner can change it.

**Design (decided 2026-10-17):** the owner chose **C · Session timeline** (design canvas https://claude.ai/artifact/ECxnETgKWmPAEBmPdkvnrY): the chart above a full-day session timeline with a volume minimap, plus a side column with the session profile and a trade tape showing how each trade was counted. Owner priorities: **the simulator, which feeds future features, and a modular, reusable chart**.

**Chart scope widened (decided 2026-10-17):** drawings, indicators and **simulated** chart trading move into this phase, and the mobile app gets the chart. Trading is **simulator only** (owner's choice): orders fill inside our own server against the replay or live tape, with no broker, no keys and no money, so the brief's "no live order execution" still holds. Everything chart-related lives in the chart packages (`chart-core` logic, `chart-web` and `chart-native` renderers and controls) so any screen reuses the whole chart; apps only pass data and wire callbacks. Contract: `docs/architecture/chart-trading-sim.md`.

**Mobile gestures (decided 2026-10-17):** the items the web cannot do are done on the phone: a haptic cue on entering a scrub (plus a tick per candle, grabs, placements and fills), fling momentum, and vertical price panning (the chart screen does not scroll, so a mostly vertical swipe pans price). Details in `docs/research/robinhood-chart-gestures.md`.

**Architecture:** `docs/architecture/simulator-and-chart.md`.
