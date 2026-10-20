# Stock context: five-pillar measurements (pillars-v1)

Status: **increment 1–2 of the Five Pillars brief (2026-10-20)**: definitions, evidence
records and the deterministic engine (`crates/stock-context`). Not yet wired into the server,
the wire protocol or the dashboard. Nothing here is a validated trading rule.

## 1. Reconciliation (repository as of `origin/main` 82a5b3b, 2026-10-20)

| Brief item | Found | Consequence here |
|---|---|---|
| AGENTS.md / handbook | No AGENTS.md; the handbook is `docs/CONVENTIONS.md`. Every push to `main` releases (server, desktop, OTA) | Work on `feat/*` branches, draft PRs, nothing merged by the agent |
| Quiet-to-ignition detector | **Does not exist in code** on any branch; `C:\research\stockwatcher\protocol\ignition-v0-proposal.md` is unfrozen | Discovery panel (brief §9A) is blocked on a frozen ignition protocol; pillars do not wait for it and never gate it |
| Previous close | `$ARCHIVE_ROOT/daily-closes.json`, Alpaca SIP `1Day` with `adjustment=raw`, no corporate-action coverage | Gain pillar is UNKNOWN (`adjustment_coverage_unknown`) until a checked close exists; the raw comparison is shown only as `gain_vs_stored_close_bp` |
| Prior-session volumes (RVOL) | None server-side (no daily bars, no intraday curves) | RVOL pillar UNKNOWN (`no_daily_baseline`), diagnostics UNKNOWN (`no_intraday_baseline`) until acquired (manifest needed, §7) |
| Float | No client; FMP float is current-only (research report 02, F1) | Float UNKNOWN (`no_float_evidence`); historical float can never be certified from it |
| News | No client; Alpaca/Benzinga news has `created_at` 3y+, FMP ~11 months | News UNKNOWN (`news_coverage_unknown`) until an adapter writes `NewsItem`/`NewsCoverage` |
| Corporate actions | Only on `research/foundation` (`crates/research-features/src/corporate.rs`), not on main | Engine takes a close plus `actions_checked_through`; no adjustment logic is duplicated here |
| Eligibility / staleness | `eligibility-research-v1` on `research/foundation` (band $0.15–$25, stale 10 min regular / 30 min extended) | Same numbers recorded in `pillars-v1.json` with their source; production's $0.10–$25 quote band is untouched |
| Recorder coverage | No coverage/gap records; nightly import **replaces** `live/` | Numerator coverage of a live run is `unverified` (§4.3); original arrival history is not preserved (risk, §8) |
| Exchange-D trade ids | Distinct FINRA TRF prints share `(D, id)` (`research/acquisition` 7c91eb5). **Fixed** (2026-10-19): the print identity adds the execution time, and a correction/cancel that cannot be pinned to one print is counted `*Ambiguous`, not applied (`docs/architecture/trade-identity.md`) | These prints are no longer dropped. An ambiguous revision is left unapplied, so a withdrawn `D` print can stay in volume; rare (4 candidate pairs in the pilot) and counted per session |
| Quotes / NBBO | Live stream subscribes `trades` + `statuses` only | Liquidity context (brief §5.5) not implemented |

## 2. What the engine is

`stock_context::Engine` takes prints (with the chart engine's verdict) and evidence records,
and emits append-only `Decision`s. Pure Rust, no I/O and no clock: the server feeds it, a provider
outage only means fewer records (UNKNOWN), never a blocked print.

Policy: `crates/stock-context/policy/pillars-v1.json`, embedded, identified as
`pillars-v1@<sha256 prefix>` on every decision. A rule change is a new file and id.

## 3. Reference profile `ross_public_reference_v1`

A reproducible StockWatcher reading of publicly described criteria (Warrior Trading tools
article, S2; Stock Selection PDF, S1). **Not** Warrior Trading's scanner, not a clone, not
validated. Other published variants (S1 $1–$20, S3 2× RVOL / $2–$10 preference, S4 worksheets)
are documented here and deliberately not offered as switchable variants.

| Pillar | Rule | Unit | Our choice (not attributed) |
|---|---|---|---|
| Price | last qualifying price in [$2.00, $20.00] inclusive | micro-dollars | price = latest OHLC-counted print before the bar end; stale after 10 min (regular) / 30 min (extended) → UNKNOWN |
| Gain | `P / C_prev_adj − 1 ≥ 10 %` | basis points | `C_prev_adj` must be the previous trading day's close with corporate actions checked through today; exact integer comparison `P·10000 ≥ C·11000` |
| RVOL | `V(04:00→b) / mean(prior 30 full sessions 04:00→end) ≥ 5.0` | thousandths | 30 covered, comparable, earlier sessions or UNKNOWN; early closes excluded; BOATS excluded |
| Float | public float `< 20,000,000`, positive | shares | stale 24 h after retrieval → UNKNOWN; shares outstanding never substitutes |
| News | relevant, substantive, credible-source news published within 24 h, first observed by the cutoff | — | 24 h is our window; FAIL only when covered sources were searched over the whole window |

Three-valued: PASS / FAIL / UNKNOWN per pillar; QUALIFIED only if all pass, NOT_QUALIFIED if any
fails, else UNDETERMINED. Counts ("3 pass / 1 fail / 1 unknown") are counts, never a score.

Discovery context: every symbol that trades is measured, whatever its price; `in_discovery_band`
says whether its fresh price is inside $0.15–$25. A $0.40 stock is never "reference qualified".

## 4. Measurements

### 4.1 Timing

Decisions sit on completed 15 s bars. The bar ending at `b` is decided at `tau = b + 2 s`
(settle) and sees exactly the prints that executed before `b` **and** were admitted by `tau`.
A print admitted after `tau` is late: it joins the next decision, never rewrites the past one.
Evidence joins as of `tau` (latest record with `observed_at ≤ tau`). First-seen times
(`timeline`) are cutoffs, never bar ends. The engine runs on the run's clock (`advance_to` with
each item's admission time), so a linear replay and a rebuild after a seek give identical
decisions (tested).

Each decision stores `bar_end` (volume endpoint), `cutoff`, and per pillar `as_of` (the print's
execution, the float's retrieval, …).

### 4.2 Three volume measures, never one unlabeled "RVOL"

- **Daily-denominator RVOL** (the pillar): today 04:00 → `b` over the mean full-session volume of
  the 30 prior covered sessions.
- **Same-time RVOL** (diagnostic): today 04:00 → `b` over the mean of prior sessions' cumulative
  volume at the same offset from 04:00. Minimum 10 sessions (engineering starting point,
  `validated: false`).
- **Recent-window RVOL** (diagnostic): the last 5 minutes over the mean of the same 5-minute
  window in prior sessions; UNKNOWN when today's window start was not observed.

Example (tested): 1,000,000 shares by 08:00 against a normal 100,000 by 08:00 and 2,000,000 per
day gives same-time 10.0× and daily-denominator 0.5×.

Rules: only sessions before the evaluated date; incomplete sessions are missing, not zero; a
zero denominator is unavailable (`zero_denominator`), never infinite; fewer than required
sessions is `insufficient_baseline` with the count shown.

### 4.3 Numerator coverage

A run states how completely it collects today's prints: `complete`, `partial` or `unverified`
(today's live recorder keeps no coverage record, so a live run is `unverified`). Missing prints
can only lower the numerator, so an incomplete count already at or above 5.0× is a valid PASS;
one below it is UNKNOWN (`numerator_coverage_unverified`), never FAIL.

### 4.4 Gain and corporate actions

The pillar needs `PrevClose { date = previous trading day, basis, actions_checked_through ≥
today }`. Today's `daily-closes.json` is raw and unchecked, so the pillar is UNKNOWN and the raw
comparison is reported as `gain_vs_stored_close_bp`. On a 1:10 reverse-split day a raw $0.50
close against a $5.00 print would read +900 %: tested to stay UNKNOWN.

### 4.5 Float and news

Float and news are evidence records with ids and versions. A later retrieval or edit is a new
record; an earlier decision can never see it (tested: float retrieved at 10:00, news edited at
11:00, future evidence appended). News reports three things separately: fresh news present (the
pillar), primary-source verified (issuer or regulator), favorability (favorable, adverse, mixed,
undetermined). Adverse news still passes the news pillar.

## 5. Decision record

`Decision` (serde JSON) carries run id, source (`live`/`replay`), fidelity (`recorded_live`,
`reconstructed_history`, `synthetic_fixture`), policy tag, symbol, instrument id, trading date,
bar end, cutoff, the five pillar results (state, value, unit, rule, reason, evidence ids, as-of),
qualification and counts, discovery band, volume facts (shares, dollars, trades, coverage), RVOL
diagnostics, news summary, the day's timeline (first eligible, first pass per pillar, first
qualified, lost at, requalified count, qualified earlier) and which parts changed. A symbol emits
only when something changed; the engine keeps the latest per symbol for snapshots.

Instrument identity is the symbol (`sym:XYZ`) until an instrument master exists: a rename or a
reused ticker cannot be told apart yet.

## 6. Tests and benchmark (run 2026-10-20)

`cargo test -p stock-context`: 6 unit + 17 acceptance tests, covering brief §12 cases 1, 2, 3,
4, 6, 7, 8, 9, 10, 11, the leakage test, stale price without a new print, loss and
requalification, overnight exclusion and the 20:00 rollover. Not covered here because they need
the server or detectors that do not exist: 12 (timeframe/reconnect), 13 (live/replay isolation:
separate engines per market), 14–15 (setup and ignition), 16 (source outage), 17 (dedup fix).

`cargo run --release -p stock-context --example bench -- 3000 1000000`, synthetic, Windows
desktop, engine only: 1,000,000 prints over 3,000 symbols in 3.6 s (~277k prints/s); `apply`
p50 200 ns, p95 400 ns, p99 500 ns, max 3.8 ms (a barrier deciding the whole board). Not a VPS
figure and not the integrated server; the with/without comparison comes with increment 2b.

## 7. Next increments and what blocks them

1. **2b, server integration**: a `context` board in the market `Core` beside `pressure`
   (feed next to `market.rs` `apply`, `replace_from` in history load, `OutKind::Context`,
   `context.*` subscribe / snapshot / delta on the existing seq), live and replay engines kept
   separate, decisions to an append-only per-run log. Benchmark with and without.
2. **3, evidence sources**, each needing a decision from Roman before any download:
   - Daily baseline: Alpaca SIP `1Day` bars (volume) for the eligible universe, ~45 calendar
     days, split-adjusted plus raw; a manifest with symbols, requests and storage comes first.
   - Checked previous close: the same request, `adjustment=split` vs `raw`, with corporate
     actions (`research/foundation` `corporate.rs`) to set `actions_checked_through`.
   - Float: FMP float, cached once per trading day, forward use only.
   - News: Alpaca/Benzinga news, first-seen and version history, coverage records.
3. **4, dashboard**: pillar evidence strip and timeline on the existing Halt watcher and chart.
4. **Discovery**: needs the ignition protocol frozen (ignition-v1.json) and implemented.

## 8. Unresolved risks (gates for specific claims)

- Exchange-D trade-id collisions undercount volume (not fixed).
- No preserved original arrival history once the nightly import replaces `live/`.
- No recorder coverage records: live numerator coverage is `unverified`.
- No instrument master: renames and reused tickers are not distinguished.
- Float history cannot be reconstructed from a current-only endpoint.
- Paper fills omit execution frictions; nothing here measures profitability.

## Sources (checked by the brief's authors 19–20 Oct 2026)

S1 Warrior Trading, Stock Selection PDF · S2 Ross Cameron, The Best Tools and Software For Day
Trading (2025-01-11) · S3 How to Day Trade: The Plain Truth, sample · S4 2025 Small Account Tool
Kit · S5 Warrior Trading Support, Scanners · S6 Relative Volume Definition · S8 Float Definition ·
S9 Bull Flag · S11 Alpaca bid/ask size display change · S12 SEC webmaster FAQ. Full URLs in the
brief (Five Pillars Research and Implementation Brief, 20 Oct 2026).
