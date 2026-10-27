# Runners exact post-alert highs

`rocket-momentum-15s-exact-after-v1` repairs the Runners momentum-high event. It is
separate from MP5, Signals range breakouts, and any proposed one-minute strategy.
No frozen model weights, feature precision, thresholds, entry bands, or MP5 policy
are changed.

## Price and time contract

High comparisons use original integer micro-dollar prices beside the existing
floating-point research features. Equal prices are never fresh highs; converting
an already rounded feature back to micros does not establish original evidence.
The live provisional state uses the same exact comparison reference.

A row starts its accumulator at the actual creation decision and admission
ordinal. An eligible high must execute **strictly after** that decision, be admitted
after the creation ordinal and by the current input cutoff, and pass the chart
engine's OHLC conditions. A print at an interval's end belongs to the next interval.
Pre-creation executions received later do not become post-creation highs. The
eligible suffix is retained even when a larger pre-creation spike exists in the
same candle. A later revision cannot backdate a new notification.

The event still uses completed **15-second** intervals. A genuine eligible spike
followed by a retreat can qualify; it does not require the close to remain above
the reference, the flag, or VWAP. Volume remains the **whole 15-second candle's**
volume, checked against the existing prior pace and multiple. The existing
positive-volume fallback at zero prior pace remains explicit; it is not evidence
of measured volume acceleration. Priority, hold and debounce rules are unchanged.

The scheduled bar end, actual evaluation, flag creation, source execution and
notification decision are distinct clocks. An ahead-of-observation barrier cannot
support fresh confirmed evidence. Known missing or unreconciled exact state is
shown as unavailable rather than invented.

## Evidence and presentation

New notices contain a versioned `breakout` object: the frozen previous comparison
reference before the candidate fold, high, original price units, bar bounds,
creation/evaluation/source clocks, admission cutoffs and volume measurements.
Nanoseconds and ordinals are strings; display clocks are explicitly milliseconds.
The optional candle close is unknown when the legacy last-arrival feature price
cannot establish the chronological candle close.
`entry_context` holds the original row reasons separately from the current cause.

The phone and desk share the title **15s new high since alert** and measured cause.
The ledger retains that evidence for sent and held decisions; the read API and
clients preserve it. Existing `at`, event IDs, notification switches, gaps and
delivery limits retain their meanings. Old records keep their original text and
unknown geometry; no historical reference or execution timestamp is fabricated.

Chart inspection carries bounded, temporary event context. It preserves the user's
selected timeframe and marks the same frozen reference/high on 15s, 1m or 5m views.
Reference lines start no earlier than their recorded known time. An event remains
historical if price later retreats; its record is not rewritten as a current quote.

## Restart and storage

Exact state has a versioned checkpoint. Replaceable per-row checkpoints are written
atomically as `breakout-state-<date>-<id>.json` beside the forward log, outside its
2 MiB append-only event budget. Each is bounded at 64 KiB, rejects unsafe IDs and
stale writers, and follows the existing retention window.

Legacy rounded gains cannot recover an exact high. Such rows remain displayable
with exact tracking unavailable. A persisted checkpoint also needs authoritative
history reconciliation before strict capability resumes. Unprovable creation-bar,
source-ahead, long-gap or incomplete-history cases retain explicit unknown state.
Checkpoint state does not establish feed completeness or what traded during an
unrecorded outage.

## Validation scope

The regression suite covers equality, micro-price collisions, pre-creation highs,
late admission, exact-time ties, interval edges, observation phase, valid spike and
retreat, volume-only exclusions, volume fallback, checkpoint recovery and unknown
legacy state. Feature-bit comparison uses an unchanged scorer observer linked
against baseline and repaired libraries. These are engineering checks. They do not
establish actual TDIC causation, device receipt, trading efficacy or profitability.
