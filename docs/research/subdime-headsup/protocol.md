# Sub-dime breakout heads-up — protocol v1

Pre-registered 2026-10-28, committed before any candidate, outcome or quote is computed.
Research only: nothing here changes production. Amendments go at the end with date, reason and
commit, and may not be motivated by a TEST result.

## Question

All Runners entry paths alert only from $0.10 (`docs/research/entry-floor-2026-10-26.md`). The
rolling tapes already track prints from $0.05 (`docs/research/wff-followup-2026-10-26.md`). Would
a **heads-up** — the same production entry rules, allowed to fire at $0.05–<$0.10 — be worth
sending? Sub-dime names are thin and wide-spread, so "worth it" needs both: follow-through net of
the spread that is not worse than in-band alerts, and a material lead over the in-band alert the
owner would get anyway.

## Data

- Trades: `C:\research\stockwatcher\archive-hist` and `archive-vps`, read through
  `market-archive` exactly as the earlier harnesses. Only days from **2026-03-30** (FINRA's TRF
  opening moved from 08:00 to 04:00 that day, `research/fast-0800`).
- **DEV** = 2026-03-30..2026-07-31. **TEST** = 2026-08-03..2026-10-23 (10-21 absent), scored once
  after `frozen.json` is committed. DEV is used only to check the pipeline and to fix the volatility
  buckets (below); no rule parameter is chosen on it.
- TRF batch: every venue-`D` print with condition `T` executed 04:00:00–04:03:00 ET is dropped from
  all tapes (`research/fast-0800`), in both runs below.
- Quotes: Alpaca historical quotes for each unit's window `[t − 2 min, t + 31 min]`, fetched with
  the existing `quotes-pat` tooling into `C:\research\stockwatcher\quotes-sd\`, manifest with sha256.

## Candidates (production code, floor changed only)

Two replays of every day with the unchanged production rules of Running v2, Fast move, Run +20%
(early fire) and Buying burst, in the research harness pattern of
`crates/server/src/rocket/running/paa.rs` (`research/fast-0800`), first fire per path per
symbol-day:

- **PROD** run: today's bands (floor $0.10).
- **SUB** run: the same build with every path's price floor at **$0.05** (a `subdime` cargo feature
  on this branch: Running and Fast `BAND`, the Run +20% start-price band and its state band, Burst's
  entry band). Everything else (liquidity, dollar and trade minimums, gates, models, thresholds,
  refractories) is unchanged. Burst's model was trained on $0.15+; its sub-dime scores are out of
  that range and are reported separately.

Units:

- **Heads-up** (SUB): the first fire per symbol-day, any path, at a price in [$0.05, $0.10).
- **In-band reference** (PROD): the first fire per symbol-day, any path, at a price in
  [$0.10, $0.50].
- **Production alert of the same move**: the first PROD fire (any path, any in-band price) on the
  heads-up's symbol at or after the heads-up time.

## Outcomes

- **Bracket** (scorecard rule): from the unit's price, +10% before −5% within 30 minutes on
  OHLC-counted prints after the fire; neither = timeout, exit at the last print by 30 min.
- **Net of spread**: bracket return − half-spread at entry − half-spread at the exit time, from the
  NBBO prevailing at that moment (the last quote within 60 s; locked/crossed or ask ≤ bid = invalid).
  A unit is **quote-complete** when both are valid.
- **Lead**: whether the heads-up's stock prints ≥ $0.10 within 60 minutes, and the time from the
  heads-up to the production alert of the same move (none within 60 min = no lead).
- Counts per day (notification burden).

## Comparisons and verdict (TEST)

1. **Matched budget**: on each day, `n` = heads-ups that day; the reference is that day's first `n`
   in-band units by time (all of them if fewer). Primary metric: net-of-spread win rate on
   quote-complete units; also mean net bracket return. Day-clustered bootstrap, 5,000 resamples.
2. **Random baseline**: per symbol-day with ≥ 30 OHLC prints in [$0.05, $0.10), three seeded random
   print times (04:03–15:30 ET) at a sub-dime price; outcome as above (gross). Matched to heads-ups
   by quintile of the trailing 15-minute high/low range (quintile edges fixed on DEV, in
   `frozen.json`).

**Verdict**:

- **Supported** — all of: heads-up net win rate − reference net win rate has a lower 95% bound
  above **−3 pp** (non-inferiority); heads-up gross win rate − matched random has a lower bound above
  0; median lead over the production alert ≥ **5 minutes** among heads-ups whose stock reaches
  $0.10 within 60 min; ≥ 100 quote-complete TEST heads-ups.
- **Promising but inconclusive** — point estimates meet all three but an interval or the count does not.
- **Unsupported** — otherwise.
- **Inconclusive due to data** — quote-complete share of heads-ups < 70%.

A ship recommendation follows only from **Supported**.

## Amendments

**A1 (2026-10-28, before any outcome; reason: data gap found in the pipeline smoke test).** The
existing archives were imported by **that day's close in $0.10–$25**
(`market-providers/src/import/batch.rs`), so a stock that traded below $0.10 is present only on days
it closed at or above $0.10: the sub-dime population would be winners only. A second root,
`C:esearch\stockwatcherrchive-sd`, is imported with the same importer for every symbol-day of
2026-03-30..2026-10-23 whose close was in [$0.02, $0.0999] (plan: 13,346 symbol-days, 290 symbols).
Both runs (PROD, SUB) replay every day on both roots; together they hold every symbol-day that closed
at or above $0.02, so neither population is chosen by its outcome. Known remaining bias: the
importer plans from currently active assets (delisted sub-dime names are missing), and a symbol-day
closing below $0.02 is absent. The smoke-test day 2026-05-01 (DEV) was run once on archive-hist for
the pipeline check only; its sub-dime fire count (0) is what exposed the gap.
