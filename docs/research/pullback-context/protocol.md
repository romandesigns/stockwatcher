# Context-gated micro-pullbacks — protocol v1

Pre-registered 2026-10-25, committed before any episode outcome is computed. Research only: no
production change. Amendments go at the end with date, reason and commit.

## Question

Does restricting MP5 micro-pullback episodes to stocks that already have context make them worth
the owner's attention (a "pullback setting up" notification), compared with every MP5 episode?

## Data

- Trades: `archive-hist` + `archive-vps` (**EXPLORE**, 149 days 2026-03-23..10-23) and
  `archive-past` (**CONFIRM-PAST**, 118 days 2025-10-01..2026-03-20), read through
  `market-archive` / `ReplayCore`.
- Runners entries: `C:\research\stockwatcher\work-pat\triggers-{explore,confirm}.jsonl` (every
  capped-in Runners entry, in band, 04:00–15:15 ET), unchanged.
- Daily bars (raw) and daily closes from each archive.

## Episodes

MP5 = the production policy `detection-v2` (`micro_pullback_mp5`), unchanged, run by the
production `stock_context::Engine` per symbol-day through the `setup-research` replay glue (the
same glue as the server). Every symbol whose daily bar does not prove it stayed out of the band.

- **Stage A (armed):** the episode's transition to `armed`; notice time `tA`; trigger `T` = the
  armed trigger level, stop `I` = the pullback low.
- **Stage B (crossing):** the transition to `crossing_provisional`; notice time `tB`.

An episode counts if `T` is in the $0.15–$25 band and `tA` is in 04:00–15:45 ET.

## Gates (decided at the notice time, causally)

- **G0:** every MP5 episode.
- **G1:** the symbol had a Runners entry (any path) at or before the notice time that day.
- **G2:** G1 and the point-in-time Ross pillars that can be computed historically:
  - price $0.10–$25 (pillars-v2);
  - day gain ≥ +10% (last price at the notice vs the previous official close, `daily-closes.json`);
  - RVOL ≥ 5×: counted day volume from 04:00 to the notice ÷ the mean full-day volume of the 30
    prior sessions (raw daily bars; fewer than 20 prior sessions → UNKNOWN);
  - float and news are not available historically: UNKNOWN, never imputed, never a pass.
  G2 requires every computable pillar to pass; an UNKNOWN RVOL fails G2 (reported separately).

## Outcome

From the crossing (`tB`), using chart-counted prints executed after `tB`:
**continuation** = a print ≥ `T × 1.05` before a print ≤ `T × 0.97`, within 15 minutes; else
failure (stop first) or timeout. Stage A's outcome is continuation only if it crosses and then
continues; an armed episode that never crosses scores 0. Also: crossing rate, time from arm to
crossing, MFE/MAE over 15 minutes from `T`.

Costs: the existing quote windows cover only the minutes around Runners alerts, so G1 episodes
would have quotes far more often than G0 episodes. The **primary comparison is gross**
(price-only, both gates measured the same way); half-spread-adjusted results are reported on the
episodes whose crossing has a valid quote, labelled as a biased subset.

## Budgets

Notices per day are capped at **10 / 20 / 40**: on each day the first N notices of the gate in time
order (a daily cap, as a live notifier would apply). Primary budget: **20**.

## Primary estimand and verdict

Stage A, budget 20: `Δ = continuation rate(G1) − continuation rate(G0)`, day-clustered bootstrap
(5,000), on CONFIRM-PAST.

- **Supported** — Δ lower bound > 0, Δ ≥ +3 pp, same sign on EXPLORE, and positive in ≥ 3 of 4
  chronological quarters of CONFIRM-PAST.
- **Promising but unconfirmed** — Δ ≥ +3 pp with exactly one condition failing.
- **Unsupported** — otherwise.
- **Inconclusive** — fewer than 300 G1 stage-A notices on CONFIRM-PAST.

The only choice made on EXPLORE is which gate (G1 or G2) is carried as the "context" candidate:
the one with the higher stage-A budget-20 continuation rate on EXPLORE, provided it averages ≥ 5
notices per day; ties → G1. It is frozen (`frozen.json`, committed) before CONFIRM-PAST outcomes
are computed. The primary Δ is then that gate vs G0.

## Also reported

Notices per day each gate produces (stage A and B, uncapped), by session segment (04:00–04:02,
04:02–07:00, 07:00–09:30, 09:30–10:00, 10:00–16:00), with and without the 08:00–08:02 cluster;
stage B results; price bands; continuation by path of the preceding Runners entry.

## Amendments

**A1** (2026-10-25, before any EXPLORE outcome is read; implementation details):
1. Harness: `crates/setup-research` (from research/setup-compare) with a `PBC=1` mode
   (`src/pbc.rs`). Main has one pullback detector (614ffe0), so the engine is built without a
   separate setup policy; MP5 = `DetectionPolicy::v2()` unpatched (variant id `V2`). The crate and
   its read-only engine accessor were staged by the checkout and landed in the protocol commit
   106773e: code only, nothing was run before it.
2. Trigger `T` and stop `I` are the armed transition's anchors (`H0`, pullback low). The crossing
   time is the `crossing_provisional` transition's observation (admission) time; the outcome reads
   prints admitted after it, in execution order, executed within 15 minutes of it.
3. `armed_vol` = counted shares of the completed pre/regular 15 s bars at arming (lags the arming
   by at most one bar).
4. Stage B's G1 uses Runners entries at or before the crossing; G2 is decided at arming.
5. G2's gain uses the raw previous official close (`daily-closes.json`), not split-adjusted.
6. CONFIRM-PAST is exported only after `frozen.json` is committed (the export computes outcomes).
7. Analysis: `docs/research/pullback-context/py/pbc.py` (`select`, then `confirm`).
