# Indicator definitions and chart parity

Date: 2026-10-18. Base: origin/main e14e9ab. Code: `crates/research-features/src/indicators.rs`
(the port, forming/completed readings, `vwap_trade`), `parity.rs` + `bin/rf.rs` (comparison),
`parity/ts-indicators.ts` (the TypeScript half), `tests/parity.rs` and
`parity/ts-indicators.test.ts` (checked-in fixtures).

## What is ported, and the rule for names

The chart's indicators live only in `packages/chart-core/src/indicators.ts` (client-side). Research
needs them in Rust, **exactly**: a feature called "EMA 9 on 1m" must be the number Roman sees on the
1m chart. Every port is named `chart_*`; anything that is not the chart's definition gets a
different name and is never substituted.

| Rust | TS | Definition (as on origin/main e14e9ab) |
|---|---|---|
| `chart_build_slots` | `buildSlots` | Merge candles and gap markers by `t`; a candle supersedes a gap at the same `t`. Inputs to every indicator are these **slots**. |
| `chart_closes` | `closes` | Candle close per slot; **NaN on gap slots**. |
| `chart_sma(v, n)` | `sma` | Mean of the last `n` **candles**: running sum, oldest subtracted. Gap slots are skipped (not counted), output NaN there. Preset `sma-50`. |
| `chart_ema(v, n)` | `ema` | Seeded with the SMA of the first `n` closes, then `v·k + prev·(1−k)`, `k = 2/(n+1)`. **No session or day reset.** NaN skipped; state carries across gaps. Presets `ema-9`, `ema-20`. |
| `chart_vwap_hlc3(slots)` | `vwap` | `Σ((h+l+c)/3 · v) / Σ v` over **candles**, reset when `tradingDayOf` changes (ET date, +1 day from 20:00 ET: the overnight session belongs to the next day). Gap slots are skipped **including `volume_only_trades` gaps and their volume**. Candle `v` includes volume-only prints inside a candle (odd lots, `W`, `4`, ...), weighted at that candle's HLC/3. Zero cumulative volume shows the close. Preset `vwap`. |
| `chart_bollinger(v, n, m)` | `bollinger` | SMA ± `m` × **population** SD of the last `n` candles. Preset `bb-20` (2σ). |
| `chart_rsi(v, n)` | `rsi` | Wilder: simple mean of the first `n` changes, then `(avg·(n−1) + x)/n`; 100 if avg loss is 0 (50 if avg gain is 0 too). Preset `rsi-14`. |
| `chart_macd(v, f, s, g)` | `macd` | Line = EMA f − EMA s where both exist; signal = EMA g of the line (NaN skipped); hist = line − signal. Preset `macd` 12/26/9. |
| `chart_trading_day_of`, `chart_session_of`, `chart_session_slots` | `tradingDayOf`, `sessionOf`, `sessionSlots` | With the chart setting **Extended hours off**, slots outside 09:30-16:00 ET (by wall clock; the display classifier keeps 13:00-16:00 as "regular" even on early-close days) are removed **before** indicators run, so EMA/VWAP/... become regular-hours-only series. Default is on. |
| `chart_presets(slots)` | `INDICATOR_PRESETS` | All of the above with the dashboard's preset parameters, keyed `"<preset id>/<line>"`. |

Not a chart indicator, named separately:

- **`VwapTrade` (`vwap_trade`)**: trade-weighted VWAP, `Σ price·size / Σ size` over individual
  prints in exact integers (micro-dollars × shares). A print counts iff `conditions-research-v1`
  lets it update **both high/low and volume** (Alpaca's own rule for its bar VWAP field), so odd
  lots, average-price, prior-reference, derivatively priced, official open/close and every
  unresolved print are out; the caller chooses the `u`-marker mode (features: `AsDelivered`).
  Resets with `chart_trading_day_of`. It differs from `chart_vwap_hlc3` by construction (no HLC/3
  approximation, odd lots excluded, volume-only intervals irrelevant) and must never be reported as
  "the chart's VWAP".

## Forming candles

**What the chart does.** The chart's slot list includes the forming candle (the newest, `final:
false`) with its *current* OHLCV, and `computeIndicators` ignores `final`: the last indicator value
on screen is computed from an unfinished interval and moves with every print until the interval
closes. (The forming state is only visible in the inspector "Forming" label, the dashed/outlined
candle and the countdown.)

**What research exposes.** `completed_and_forming(slots, f)` returns two readings at decision time:

- `completed` (+ `completed_t`): `f` evaluated on the prefix ending at the **last final candle**, so
  nothing from the forming interval can reach it;
- `forming` (+ `forming_t`): `f` on all slots, i.e. exactly what the chart shows now.

Neither is ever the value the forming interval will finally have; that value only exists once the
candle is final, which is after the decision. `slots` must be the engine's state **as of** the
decision time (only the newest candle may be non-final; checked in debug builds). All indicators
are causal (the value at slot `i` depends on slots `≤ i`), so `completed` also equals the chart's
own value at that candle; the integration test checks both readings against the TypeScript on real
MDXH candles with the newest marked forming.

## Interval boundaries (15s / 30s / 1m / 5m)

Intervals are half-open `[start, start + tf)` and **epoch-aligned in UTC**
(`chart_engine::Timeframe::start_of`; `interval_start_ms` is the same function). ET offsets are
whole hours and every session boundary (04:00, 09:30, 13:00, 16:00, 17:00, 20:00 ET) is a multiple
of 5 minutes, so epoch alignment **is** session anchoring for all four timeframes: the 09:30
regular open always starts a 15s, 30s, 1m and 5m interval, and no interval straddles a session
boundary. `intervals_are_session_anchored_on_every_boundary` checks every session start and end on
DST-start week (Fri 2026-03-06 EST, Mon 2026-03-09 EDT), a normal day (2026-10-15), DST-end Monday
(2026-11-02) and an early-close day (2026-11-27), for all four timeframes, including that one
nanosecond before a boundary falls in the previous interval.

## Parity test

**Pipeline.** `rf candles <archive> <day> <symbol> <out>` replays the archived day through a
default `ChartEngine` (the same way as `market-providers`' `replay_day`: apply every event, then
advance the clock 10 minutes past the last print so every candle is final) and writes each
timeframe's candles + gaps. `bun crates/research-features/parity/ts-indicators.ts <candles.json>
<ts.json>` runs chart-core's own `buildSlots` → `sessionSlots` → `computeIndicators` on each
`INDICATOR_PRESETS` entry (one at a time, so the 3-overlay cap drops none), for extended hours on
(`ext`) and off (`rth`). `rf parity <candles.json> <ts.json>` computes the Rust port on the same
file and compares every value; a NaN on one side and a number on the other is a failure.

**Tolerance: 1e-9 absolute.** Observed:

- **Bit-exact** (max |diff| = 0.0 on every line of all 20 series) when Rust parses the JSON with
  serde_json's `float_roundtrip` feature (tried locally, not committed: enabling a serde_json
  feature unifies into production crates whenever the workspace is built together, and production
  must not change for a research crate).
- With the default parser (committed), serde_json may round a decimal 1 ULP away from JavaScript;
  the residual is exactly 1 ULP of each value's magnitude, at most **1.4e-14**:

Max |Rust − TS| per indicator and timeframe, worst of `ext` and `rth` (and, for the fixture day, of
its four symbols); default parser:

| Series | Timeframe | VWAP | EMA 9 | EMA 20 | SMA 50 | BB basis/upper/lower | RSI 14 | MACD line / signal / hist |
|---|---|---|---|---|---|---|---|---|
| Fixture 2026-10-15 (SMPL, FXHI, FXLO, FXMD) | 15s | 7.1e-15 | 7.1e-15 | 7.1e-15 | 7.1e-15 | 7.1e-15 | 7.1e-15 | 5.6e-17 / 5.6e-17 / 1.4e-17 |
| | 30s | 7.1e-15 | 7.1e-15 | 7.1e-15 | 7.1e-15 | 7.1e-15 | 7.1e-15 | 5.6e-17 / 5.6e-17 / 1.4e-17 |
| | 1m | 7.1e-15 | 7.1e-15 | 7.1e-15 | 7.1e-15 | 7.1e-15 | 7.1e-15 | 5.6e-17 / 5.6e-17 / 2.8e-17 |
| | 5m | 7.1e-15 | 7.1e-15 | 7.1e-15 | 7.1e-15 | 7.1e-15 | 7.1e-15 | 5.6e-17 / 5.6e-17 / 2.8e-17 |
| Real MDXH 2026-10-16 | 15s | 5.6e-17 | 5.6e-17 | 5.6e-17 | 5.6e-17 | 5.6e-17 | 7.1e-15 | 3.5e-18 / 3.5e-18 / 4.3e-19 |
| | 30s | 5.6e-17 | 5.6e-17 | 5.6e-17 | 5.6e-17 | 5.6e-17 | 1.4e-14 | 3.5e-18 / 3.5e-18 / 4.3e-19 |
| | 1m | 5.6e-17 | 5.6e-17 | 5.6e-17 | 5.6e-17 | 5.6e-17 | 1.4e-14 | 3.5e-18 / 3.5e-18 / 4.3e-19 |
| | 5m | 5.6e-17 | 5.6e-17 | 5.6e-17 | 5.6e-17 | 5.6e-17 | 7.1e-15 | 6.9e-18 / 6.9e-18 / 1.7e-18 |

522,198 defined values compared, 0 NaN-pattern mismatches. Series sizes (candles + gaps): fixture
symbols 288 (5m) to 4,591 (15s) slots; MDXH 180 (5m), 764 (1m), 1,388 (30s), 2,429 (15s).

**Checked in** (runs in `cargo test -p research-features`): `tests/fixtures/*.candles.json` and
their `*.ts.json`, trimmed windows of the same engine output: MDXH 2026-10-16 08:30-11:00 ET for
15s/30s/1m/5m (spans the pre → regular boundary, so `rth` differs from `ext`), SMPL 2026-10-15
08:30-11:00 1m and the full 5m day (long enough for SMA 50 and MACD's signal). `tests/parity.rs`
asserts all 22 lines (11 per variant) of each within 1e-9 with identical NaN patterns, and the
forming/completed readings. `bun test crates/research-features/parity` re-runs chart-core on the
fixture candles and requires **exact** equality with the stored TS output, so a change to
chart-core's indicators fails here and forces the Rust port and fixtures to be updated together.
Neither runs in the repo's CI (which lists its Rust packages explicitly); run them when touching
either side.

**Exactly what parity covers.** Calculations: `chart_build_slots`, `chart_closes`,
`chart_session_slots` (both settings), `chart_trading_day_of` (through VWAP), `chart_sma` (50),
`chart_ema` (9, 20, and 12/26/9 inside MACD), `chart_vwap_hlc3`, `chart_bollinger` (20, 2),
`chart_rsi` (14), `chart_macd` (12, 26, 9), `completed_and_forming` (EMA 9, VWAP). Inputs:
chart-engine default config (`EngineConfig::default()`: 2 s settle, production condition policy =
`market_events::conditions`, **not** `conditions-research-v1`), all candles final except in the
forming test. Not covered: `VwapTrade` (unit-tested only; it has no TS counterpart), non-preset
periods (the code is parameter-generic and unit-tested against the TS unit tests), drawing/levels
code, and the chart's history depth (below).

**Reproduce:**

```sh
cargo run -p research-features --bin rf -- candles data/archive-real 2026-10-16 MDXH <dir>
bun crates/research-features/parity/ts-indicators.ts <dir>/MDXH-2026-10-16-1m.json <dir>/MDXH-1m.ts.json
cargo run -p research-features --bin rf -- parity <dir>/MDXH-2026-10-16-1m.json <dir>/MDXH-1m.ts.json
```

## Caveats for research use

1. **History depth decides EMA/MACD/RSI.** None of them resets, so their value depends on where the
   series starts. In replay the chart's series starts at the replay day's first print; live, at
   whatever the server's engine retained (a ring of 4,000 intervals per timeframe since the server
   started, plus volume-only gaps). Research anchors every series at the trading date's 04:00 ET
   (`eligibility::history_start`), which equals the replay chart; a live chart that has been running
   since an earlier day shows different EMA values for the first ~3·n candles of a day. Record the
   anchor with every feature.
2. **Overnight.** The chart's VWAP day starts at 20:00 ET the evening before; the SIP archive has
   no overnight (BOATS) prints, so research VWAP starts at 04:00. A live chart with overnight prints
   shows a different VWAP until volume swamps the overnight part.
3. **Candles come from production's condition policy**, not the research one: a parity feature is
   "what the chart shows". A research feature built on `conditions-research-v1` candles is a
   different series whenever a print carries an unresolved code (none on the real day tested,
   `conditions.md`), and must say which.
4. `chart_vwap_hlc3` drops volume-only intervals' volume and weights volume-only prints inside a
   candle at the candle's HLC/3; that is the chart's definition, kept as is.
