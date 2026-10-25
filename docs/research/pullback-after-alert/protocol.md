# Pullback after alert (PAA) — protocol v1

Pre-registered 2026-10-25, committed before any trigger, entry or outcome is computed. Later
changes are amendments at the end (date, reason, commit), never motivated by a TEST result.

Source brief: owner's "Multi-Timeframe Micro-Pullback Research Specification" (2026-10-25),
narrowed with the owner to an **augmentation** of the live Runners paths, not a replacement.

## 1. Question

The live Runners paths find most in-band movers (10-23 scorecard: 53/65), but often late or at
a price that is hard to fill (Running v2 enters ~90% into the leg; YMAT fill window ~30 s).
**After a Runners entry fires, does waiting for a structured pullback and entering on its
continuation give a better trade than entering at the alert, and better than simply waiting?**

Secondary questions (descriptive unless stated):
- Q2: does the lower-timeframe shape of the pullback (brief §6 scenarios A–D) change the outcome?
- Q3: do 15 s bars add anything over 1-min structure (C3 vs C2)?
- Q4: does the answer differ between pre-market (04:00–09:30 ET) and regular hours (09:30–16:00)?
- Q5: how often is a pre-market entry untradable on spread (NBBO quotes)?

## 2. Data and split

- Trades: `C:\research\stockwatcher\archive-hist` (126 days, 2026-03-23..09-21) and
  `archive-vps` (23 days, 2026-09-22..10-23; 10-21 absent), read only through `market-archive`
  / `ReplayCore`. Daily closes/bars as in the burst-onset protocol.
- Quotes: Alpaca historical NBBO, fetched for entry windows only (§7), stored under
  `C:\research\stockwatcher\quotes-paa\`, same format and manifest style as `quotes\`.
- **DEV** = 2026-03-23..2026-07-31 (91 days). Every choice in §5 is made on DEV only.
- **TEST** = 2026-08-03..2026-10-23 (58 days), scored **once** after `frozen.json` is committed.
  Disclosure: these dates were read by earlier, different studies (burst-onset TEST, rocket-quotes,
  scorecards); this study has not read them and no choice here depends on them.
- Owner reference cases (SHFS, XLAB, MLTX, CJMB, FPSX) are reported as named checks, never tuned on.

## 3. Triggers (the alerts we augment)

Production Runners logic **unchanged**, replayed over each day: Buying burst (V3-5), Run +20%,
Fast move, Running v2, R2 flag — the code on `main` at f4b8862. If a path cannot be replayed
faithfully from the archive it is excluded and the reason recorded; no path is re-implemented.

Trigger = the **first** Runners entry per symbol-day (any path), price $0.10–$25, time
04:00–15:30 ET (so a 30-min outcome window fits before 16:00 for RTH; pre-market triggers may
run into RTH). Its path, time `t0`, price `p0` and session are recorded. Later entries on the same
symbol-day do not create new triggers.

## 4. Entry rules (all causal; completed bars only; one entry per trigger per rule)

Every rule searches from `t0` to `t0 + 30 min`. Entry price = the first trade that satisfies the
rule's entry condition, plus cost (§7). Bars are built from trades exactly as production builds
them (chart-engine semantics, `conditions-research-v1` eligibility); empty bars stay empty.

Baselines:
- **B0 chase**: enter at the first trade at or after `t0`.
- **B1 wait**: enter at the first trade at or after `t0 + d`, where `d` = the median entry delay of
  the frozen candidate on DEV, applied to **every** trigger (does the pattern beat just waiting?).
- **B2 red candle**: first completed red 1-min bar after `t0`; enter on the first trade above that
  bar's high (the brief's naive candle-colour pullback).
- **B3 new high**: after at least 2 min, enter on the first trade above the post-`t0` high.

Candidates:
- **C1 MP5**: the existing `micro_pullback` detector (`stock-context/src/detect/micro.rs`), unchanged;
  enter on its confirmed trigger crossing after `t0`.
- **C2 structured pullback (1-min)**: `H` = high since `t0 - 5 min`, `L` = low of the 5 min before `t0`.
  A pullback is ≥ `k` completed 1-min bars whose highs are all below `H`, with
  depth `(H - Lp)/(H - L)` in `[dlo, dhi]` (`Lp` = pullback low), mean pullback bar volume < mean
  volume of the 1-min bars from `t0 - 5 min` to the bar holding `H`, and `Lp` ≥ the support line `S`.
  Enter on the first trade above the highest high of the pullback bars. Stop = `Lp`.
- **C3 = C2 + 15 s confirmation**: additionally, the last 4 completed 15 s bars before entry each
  contain ≥ 1 trade and their mean range ≤ the median 15 s range inside the pullback.

## 5. DEV choices (declared grid, single criterion)

Grid for C2/C3: `k` ∈ {2, 3, 4}; `[dlo, dhi]` ∈ {[0.15, 0.50], [0.15, 0.65], [0.25, 0.65]};
`S` ∈ {day VWAP, 1-min EMA-9, none}. That is 27 configurations each for C2 and C3, plus C1
(no parameters) — 55 candidates.

DEV days in time order form 6 consecutive blocks; fold j = 1..5 validates on block j+1 (no fitting
happens, so "training" is only the choice). **Criterion = pooled paired net expectancy gain over
B0 on the same triggers (§6), over the 5 validation blocks, among configurations with ≥ 8 entries
per validation day on average across the market.** The highest wins; ties → fewer parameters /
shallower depth band. The winner, `d` for B1, and every threshold are written to `frozen.json`
and committed before TEST is exported.

## 6. Outcomes and estimand

From entry price `e` (after entry cost), over the next 30 min of trades:
- **Primary — bracket outcome**: `+10%` reached before `−5%` → win (+10), `−5%` first → loss (−5),
  neither → exit at the last trade at 30 min (mark-to-market). Same bracket as the live scorecard.
  Net return = bracket return − exit cost. A trade with no prints after entry is a timeout at 0
  before costs, never dropped.
- Secondary: forward return at 1, 3, 5, 10, 15 min; MFE/MAE over 30 min; R-multiple with stop `Lp`
  and 2R target (C2/C3/B2 only); entry delay `t_e − t0`; coverage (share of triggers that get an
  entry); entries per day.

**Primary estimand** (paired, same triggers): for triggers where the candidate entered,
`Δ = mean net return(candidate) − mean net return(B0 on those triggers)`, and the same against B1.
Inference: cluster bootstrap over days, 5,000 resamples, 95% percentile intervals.

**Verdict on TEST (frozen candidate only):**
- **MEANINGFUL** — lower bound of Δ vs B0 > 0 **and** lower bound of Δ vs B1 > 0 **and** point
  Δ vs B0 ≥ +1.0 percentage point per trade **and** ≥ 150 TEST entries.
- **PROMISING BUT INCONCLUSIVE** — point estimates meet the +1.0 pp bar but an interval crosses 0,
  or fewer than 150 entries.
- **NO GAIN** — otherwise.

Only MEANINGFUL leads to production integration (§9). Q2–Q5 are descriptive and cannot by
themselves trigger integration; a session gate (Q4) is adopted only if pre-market Δ vs B0 has its
own interval below 0 while RTH meets the bar.

## 7. Costs and quotes

Quotes are fetched for every DEV and TEST entry window `[entry − 2 min, entry + 30 min]` of the
frozen candidate and all baselines (one request series per symbol-day, union of windows), before
TEST outcomes are read. Cost per side = half the NBBO spread at that moment, as a fraction of
the midpoint; a quote older than 10 s or a crossed/locked NBBO counts as **no quote**.
Where no usable quote exists, the trade is costed under three fixed scenarios — 0.5%, 1% and 2%
round trip — and the primary verdict uses **1%**; results are also reported quote-only.
Q5: share of entries whose half-spread at entry > 1%, by session.

## 8. Reporting

Report at `C:\research\stockwatcher\reports\pullback\` and `docs/research/pullback-after-alert/
report.md`: DEV selection table, the frozen config, TEST table (every rule: n, entries/day,
coverage, win/loss/timeout rates, mean/median net, Δ with intervals), session split, scenario
split (A directional, B horizontal, C compression, D sparse — definitions below), price-band split
($0.10–1, 1–5, 5–25), per-path split, cost scenarios, named owner cases, and the verdict.

Scenario labels at entry time (descriptive, from the pullback bars): **D sparse** if any 15 s bar
in the 2 min before entry is empty; else **A directional** if ≥ 2 of the last 3 pullback 1-min
highs are lower highs and the pullback's net close-to-close decline ≥ 50% of its range; else
**C compression** if the last 4 15 s ranges are below the pullback's median 15 s range; else
**B horizontal**.

## 9. If MEANINGFUL: integration

A new Runners follow-up notice, "Pullback entry", on the symbol's existing Runners row (not a new
row, not a new path): fires when the frozen rule enters, with the rule's level and stop. Behind a
config flag (`RUNNERS_PULLBACK`), wired through the same notification kinds, dedupe and text
builder (`runners::message()`) as the other follow-ups; replay and live share the code. Existing
paths and their alerts are untouched. Unit tests replay the named owner cases.

## Amendments

(none)
