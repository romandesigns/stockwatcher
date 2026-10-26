# Early ignition, protocol v1 (pre-registered)

Owner, 2026-10-26, on JAGX: a ~1 h dead base near $2.10, three small green 1-min candles, then the
ignition candle (~$2.25–2.30) one minute before the explosion to $2.71; Runners' Fast move fired
on the explosion candle at $2.50. Question: **can a causal detector flag the ignition of a move
from a quiet base early enough to matter, at a fixed alert budget, better than the baselines, and
earlier than the current Runners paths?**

**Committed before any outcome is read.** Later changes are amendments at the end (reason,
commit), never motivated by a TEST result. Research only: production is unchanged. Nothing here is
a trading signal.

## 1. Data

- Rows: the burst-onset exports (`research/burst-onset`, harness `crates/setup-research`, protocol
  in that branch), already computed causally from the archives through the production
  `ChartEngine`: `C:\research\stockwatcher\reports\burst\{export,export-test}\{rows5,rows15,pool}-<day>.csv`.
  One row per gate pass (burst-onset §4: ≥ 5 prints and ≥ $5k in the last 15 s, `ret15` ≥ ln 1.015,
  15 s dollar rate ≥ 3× the 30-min base rate) with its 34 features, alert time, alert price and
  outcomes; `pool` = the volatility-matched random barriers; `b3` = the plain price-spike rule.
  Using these rows instead of a new export is a disclosed choice: the gate is the candidate
  generator here too, and it fired inside the leg of every +30 % rocket in its pilot.
- **DEV** = 2026-03-30 .. 2026-07-31 (archive-hist; 87 days). Days before 2026-03-30 are excluded:
  until 03-27 FINRA's trade-report batch arrived at 08:00 ET and dominates that window
  (`research/fast-0800`).
- **TEST** = 2026-08-03 .. 2026-10-20, scored **once** after `frozen.json` is committed, reported in
  two parts: **TEST-H** 2026-08-03..09-21 (35 days, out of sample for every scorer incl. Burst) and
  **TEST-V** 2026-09-22..10-20 (21 days; Burst V3-5 was fitted on these, so its baseline numbers
  there are in-sample and flattered). 10-21..10-23 have no export.
- **FORWARD** = live sessions from 2026-10-26 (incl. JAGX): pending; needs a live export, not done
  here.
- **Trade-report batch window**: rows with alert time 04:00:00–04:03:00 ET are excluded for every
  scorer and baseline. (The exports cannot drop venue-D Form-T prints from features without a
  re-export; excluding the window is the conservative equivalent.)
- Band $0.15–$25 (the exports' band; the owner's band starts at $0.10), sessions 04:00–20:00 ET.

## 2. Ignition definitions (causal; from the row's own features)

- **Onset**: `age` ≤ 60 s (the first minute of a burst episode; an episode starts at a gate pass with
  none in the previous 10 min).
- **Quiet base**: the 30-min base ending 30 s before the row is quiet: `idle30` ≥ 0.5 (half its
  15 s bars without a trade) **or** `rng30` ≤ 0.03 (high/low within 3 %).
- **Base breakout**: `brk` > 0 (last above the base high).

## 3. Candidate family (6 configurations; target CONT10)

| id | G | rows scored | score |
|---|---|---|---|
| I1-5 | 5 s | onset ∧ quiet ∧ breakout | `rd15` (hand rule, no fit) |
| I1-15 | 15 s | onset ∧ quiet ∧ breakout | `rd15` (hand rule, no fit) |
| I2-5 | 5 s | onset | logistic regression (L2, C=1, lbfgs, 2000 iter) on the 34 features, clipped 0.5/99.5 pct, median-imputed, z-scored |
| I3-5 | 5 s | onset | LightGBM, the Burst V3 recipe (leaves 15, depth 4, lr 0.05, 300 trees, min child 200, subsample/colsample 0.8, λ 1, seed 7, deterministic) |
| I3-15 | 15 s | onset | as I3-5 |
| I4-5 | 5 s | onset ∧ quiet | as I3-5, fitted and scored on quiet-base onset rows only |

Rows outside a configuration's set never alert.

## 4. Alert policy and budgets

Score ≥ θ, at most one alert per symbol per 10 min (burst-onset §5). Budgets **5, 10, 20 alerts/day**
market-wide.

## 5. Outcomes

From the exports (burst-onset §3, unchanged): **CONT10 (primary)** = +10 % before −5 % within
30 min from the alert price; CONT20, CONT30 (+30 %, the "rocket" outcome), MFE, MAE.

Also reported:
- **Runners timing**: against all Runners entries replayed for these days
  (`C:\research\stockwatcher\work-pat\triggers-explore.jsonl`, research/post-alert-timing): share of
  alerts with a Runners entry on the symbol in the next 60 min and none in the previous 60 min
  ("ahead of Runners") and their median lead; share with a Runners entry in the previous 60 min
  ("after Runners"); share with none within ±60 min.
- **Move share at the alert** (proxy, same for every scorer): with base low
  `L = last / ((1 + rng30) · e^brk)` and peak `P = pa · (1 + mfe)`, share = `(pa − L) / (P − L)` when
  `P > L`. A 30-min-base, 30-min-forward window: comparable across scorers here, not with the
  lateness study's 60/60-min number.
- **Rocket recall**: share of canonical +30 % events (`reports\rockets\events\<day>.jsonl`) with an
  alert on the symbol between `start_bar_open − 10 min` and the +30 % touch.

## 6. Baselines (same budgets, dedup and exclusions)

- **B1** volatility-matched random (burst-onset §6: 5 draws per alert from the same day/session/price
  bucket's pool with `range5` ≥ max(300 bp, the alert's `range5`)).
- **B3** plain price spike (the export's `b3` rule, ranked by its value): the 15 s analogue of the
  lateness study's naive 1-min flag, which the exports do not carry.
- **VA** top volume acceleration: all gate rows at 5 s ranked by `rd15`.
- **BU** Burst V3-5, the frozen production model (`reports\burst\frozen\V3-5.lgb.txt`) on all 5 s
  gate rows.

## 7. Selection (DEV only, single criterion)

DEV days in time order → 6 consecutive blocks; fold k = 1..5 fits on blocks 1..k, validates on
block k+1 (hand rules: no fit). On each validation day, alerts = the top 10 by score after the dedup.
**Criterion = pooled CONT10 precision at 10 alerts/day over the 5 validation blocks.** Ties → simpler
(I1 < I2 < I3 < I4), then coarser G. The winner is refitted on all DEV; θ per budget = the value giving
a mean of 5/10/20 alerts/day over DEV in-sample (each baseline calibrated the same way on DEV).
`frozen.json` is committed before any TEST row is read.

## 8. Verdict (TEST-H, budget 10/day primary; day-clustered bootstrap, 2,000 resamples)

Δ = CONT10 precision(candidate) − precision(baseline).
- **CANDIDATE FOR FORWARD SHADOW**: lower bound of Δ > 0 against B1, B3, VA **and** BU.
- **PROMISING BUT INCONCLUSIVE**: lower bound > 0 against B1, B3 and VA but not BU, or point Δ > 0
  against all four with an interval crossing 0.
- **BASELINE PREFERRED**: otherwise.
"Earlier than Runners" is claimed only if the candidate's alerts are ahead of Runners more often
than Burst's (difference with interval) and its median move share is lower.
TEST-V and the 5/20 budgets are reported beside it, not verdict-bearing.

## Amendments

(none)
