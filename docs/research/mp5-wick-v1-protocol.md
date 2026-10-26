# MP5 frozen-candle diagnostic: protocol v1

Registered 2026-10-26 before reading this diagnostic's replay outcome comparisons.
This is an EXPLORE study and a local observational implementation. It does not
change `detection-v2`, arming, crossings, invalidation or notice eligibility.

## Candidate fixed before outcomes

`mp5-wick-v1`: freeze the actual selected H0 candle with the impulse anchors.
FLAGGED iff its upper wick is at least half its range AND its close is in the
lower half of its range, with exact integer comparisons. Record the raw OHLC,
bar end, decision cutoff and both ratios. Missing/invalid/incomplete/zero-range
candles are UNKNOWN; UNKNOWN is never rejection. No volume condition is added.
Keep the current measured pullback/impulse dollar-volume ratio separately.
Never look up a revised H0 candle later and substitute it into a frozen episode.

## Population and baseline

Use all available EXPLORE dates and symbols in the existing historical Runners
trigger export (`C:/research/stockwatcher/work-pat/triggers-explore.jsonl`), within
2026-03-23..2026-10-23. Read only the existing `archive-hist` and `archive-vps`
copies. Write a manifest of selected date/symbol/first-trigger tuples, hashes,
source identities and archive versions before replay. No CONFIRM-PAST data is
used. No provider acquisition or production access is needed for this study.

Primary replay is the current Rust MP5 in Runners-watched scope: feed the full
available symbol-day history, admit the watched symbol when its first historical
Runners trigger becomes known, and drain transitions every step. Do not require
Ross membership or invent news/float evidence. Record the Detection-Panel-on
scope separately if it is run; never mix its episodes into the primary estimate.

The trigger export is a fixed historical context population, not a recomputation
of the newly widened R2/burst/shelf universe. Consequently this study cannot prove
parity with every future live discovery, nor evaluate Ross-only notifications.
Reconstructed archives without receive times use execution-time admission and
cannot establish live feed completeness or latency.

## Matched comparisons

Baseline: first qualifying arming per symbol/day within 30 minutes of that first
trigger. Candidate: ignore FLAGGED armings before spending that allowance and
select the first remaining arming in the same window. Preserve UNKNOWN. Also
report the paired classification of the baseline notice itself, to expose how
many baseline winners would be discarded, even when a replacement is selected.
No device cooldown, desk/phone opt-in or daily delivery cap is simulated; these
are detector-notice opportunities, not delivered push counts.

Use the existing stage-A outcome: continuation only if the episode crosses and
then a counted print reaches trigger +5% before trigger -3%, within 15 minutes
of crossing; otherwise failure, timeout, or no-cross, each reported separately.
Only prints admitted after crossing and executed within the horizon can settle
it. Record provisional crossing retractions and data/journal loss separately;
do not silently count missing or censored outcomes as ordinary failures.
No spread/fees/slippage adjustment is invented. Gross outcomes are not executable
returns or evidence of a trading edge.

Report counts, notices/day, flagged/clear/unknown, baseline successes discarded,
candidate replacements, continuation rate and notice reduction. Report
$0.10–<$0.15 and $0.15–$25 separately, plus the two chronological halves and
session segments. There are no current MP5 episodes below $0.10: report that
coverage limit rather than silently widening the detector's active universe.
Use 5,000 paired day-cluster bootstrap resamples, seed 20261026, for the
candidate-minus-baseline continuation-rate interval. Zero denominators remain
undefined. Include all recorded days, including zero-notice days.

## Decision

This exploratory comparison can support shipping an observation and can expose
obvious harm; it cannot validate a blocking filter. A hard filter remains off
until a separately frozen independent confirmation study supports it. In
particular, sparse sub-$0.15 evidence must not inherit the higher-price verdict.
Do not choose alternative wick cutoffs, add volume gates, or select dates after
seeing results. Any such proposal is a new rule and study.

## A1: outcome-clock clarification before reading comparisons

Recorded after the adapter smoke test (8 symbol-days, 0 transition mismatches)
and before inspecting its episode outcomes or any group comparisons. Both
execution AND admission of a settling print must lie within 15 minutes of the
crossing; an old print received after the deadline cannot settle the outcome.
Execution must also be at or after crossing, so a delayed pre-crossing print
cannot count as a subsequent gain or loss. No candle cutoff or population was
changed. The official comparison uses a fresh output directory; smoke results
are not mixed into it. Preserve the original protocol hash and record this
amendment's hash in the run receipt.

## A2: correct retraction categorization after the first comparison audit

The first comparison was read, then audited. Its adapter conflated
`bar_closed_at_or_below_trigger` with `crossing_print_revised`, excluding 750
paired cases as if the source crossing print had been corrected. That exclusion
was a measurement bug: an ordinary failed candle close does not erase the
observed crossing or censor its predefined price outcome.

Preserve the first run and comparison as pre-audit artifacts. Rerun the identical
frozen population and exact same wick candidate, watch times, notice allowance
and price target/stop. Record ordinary retractions separately while scoring the
original first-crossing price window. Only a genuinely revised source crossing
print is treated as retracted/censored. Add regression cases for ordinary failed
closes, target-first, stop-first, timeout and actual source-print revision.
Use a fresh output directory and binary receipt for this corrected run. This
post-result measurement correction is disclosed; it is not a new threshold
selection or independent confirmation. Hard rejection remains off regardless.
