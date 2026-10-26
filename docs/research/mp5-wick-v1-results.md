# MP5 wick diagnostic: retain observation, leave hard rejection off

2026-10-26. The fixed `mp5-wick-v1` candidate did not improve this exploratory
comparison. Ship the frozen-candle observation; do not add a blocking filter.
No alternative wick cutoff, volume gate or date selection was fitted afterward.

## Corrected comparison

Success means an armed setup crosses, then reaches trigger +5% before -3%
within 15 minutes. These are gross price outcomes from every first setup notice
opportunity, including those that never cross, not trading returns or actual
delivered pushes.

| Measure | Current first-setup rule | Hypothetical wick filter |
|---|---:|---:|
| Notice opportunities | 5,585 | 4,747 |
| Opportunities per recorded day | 37.48 | 31.86 |
| Successful continuations | 650 | 534 |
| Continuation rate | 11.64% | 11.25% |

The filter removes 838 notices (15.0%). It discards 179 originally successful
setups. Later replacement setups recover 13 of those stock-days, leaving 166 of
the 650 original successes lost (25.5%). Replacements also add 50 successes on
other stock-days, so the total falls by 116, from 650 to 534. The paired rate
difference is -0.389 percentage points; the 5,000-resample day-cluster bootstrap
95% interval is [-0.959, +0.170] percentage points. This does not establish an
improvement or a statistically proven general deterioration.

The point difference is negative in both chronological halves (-0.601 and
-0.162 percentage points). The $0.10–<$0.15 subgroup has only 90 baseline
notices: 14 successes versus 9 on 80 filtered notices. Its interval is wide
[-10.90, +1.59] percentage points. Do not extrapolate validation into that range.

## Population, controls and limits

- 149 EXPLORE sessions, 2026-03-23..2026-10-23; 12,609 replayable symbol-days.
  Another 125 historical context symbol-days had no replayable file and remain
  missing coverage. No CONFIRM-PAST observations were used.
- Fixed historical first-Runners contexts, admitted to the watched scope at
  their recorded first trigger. This does not reconstruct all new discoveries
  under the widened floors, Ross-only membership, or device delivery limits.
- 979,075,475 archived events; 447,052,144 OHLC-counted prints. No receive
  timestamps or source corrections were present. Admission is reconstructed;
  live completeness, execution costs and actual spread are unknown.
- 299,540 transitions matched the pre-observer control exactly after removing
  only the new `anchors.wick` field: no state, sequence, time, level, anchor or
  other transition-field differences; zero journal drops and zero replay errors.
- 7,024 context symbol-days had no arming in the eligible window. All 5,585
  remaining first-notice pairs are retained in the corrected comparison.
- The baseline first notices contain 1,487 flagged candles, 3,879 clear candles
  and 219 UNKNOWN observations. UNKNOWN is retained, never silently rejected.

The research package uses its own recorded Cargo.lock. The comparison proves
core-transition parity within this harness, not identity with a running server
binary. The sampled Runners context export is historical, and this is not an
independent confirmation study or a performance claim for the widened universe.
WFF's October 26 afternoon event is outside this historical population.

## Measurement correction disclosed

The first analysis incorrectly treated every `crossing_retracted` event as a
source-print revision. Normal `bar_closed_at_or_below_trigger` events were
therefore censored, excluding 750 pairs. Those are observed price paths, not
missing evidence. Protocol amendment A2 distinguishes them from an actual
`crossing_print_revised` event and adds regression cases.

The entire identical population was replayed again into a fresh directory with
the corrected outcome code. Neither the wick candidate nor the notice-selection
rule changed. The first output and comparison remain as pre-audit artifacts.
The corrected numbers above supersede the initial analysis. The correction
followed reading that initial result and is not presented as a fresh holdout.

## Implemented behavior and verification

The diagnostic freezes the actual H0 candle and carries its OHLC, cutoff, rule,
status and geometry through episode transitions, pullback payloads, sent/held
notification ledger entries and the read API. It does not change MP5 arming,
crossing, levels, notice text, eligibility, cooldowns or the first-setup budget.
See `docs/stock-context/mp5-wick-diagnostic.md` for the record contract.

Local checks: 1,060 Rust tests passed, CI's nine-crate Clippy command and Rustfmt
passed, 212 shared tests, 87 web tests, 87 mobile tests, client typechecks, web
build, Expo doctor under the repository policy and a clear Android bundle export
passed. The offline harness also passes its settlement regressions and Clippy.
The Android export is a bundle check, not installed-device verification.

Replay code and protocol are in `tools/mp5-wick-replay` and
`docs/research/mp5-wick-v1-protocol.md`. Detailed manifests, source/binary hashes,
comparison JSON and both runs are retained under
`H:/wavystack/outputs/mp5-wick-20261026`. Publication and runtime verification are
recorded separately by the release workflow and deployment receipts.
