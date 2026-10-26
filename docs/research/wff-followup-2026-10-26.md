# WFF follow-up: evidence and proposed detector work

Inspected 2026-10-26. Source: `a7a89b597388cf8e69c0e2d7f65bf8317546a6d4`.
The public production health endpoint reported that same commit and `status: ok`.
This is a source audit and a local display fix. It does not establish a better trading rule.

**Owner follow-up, 2026-10-26:** all current live entry floors were subsequently
lowered to $0.10, including precursor scoring, Buying bursts, shelves and discovery
(`pillars-v3`). See `docs/reference/configuration.md`, "Current entry price minimum".
The audit below describes the baseline; its historical study ranges remain unchanged.
MP5's geometry and its wick-rejection behavior have not been changed.

**Subsequent diagnostic work:** `mp5-wick-v1-results.md` records the implemented
frozen-candle observation and the corrected exploratory comparison. Hard
rejection remains off. The recommendations and local validation below describe
the original baseline audit, not the later release state.

## Findings

### Tracking below $0.10 already exists

- `crates/server/src/rocket/features.rs`: `WIDE_BAND = (0.05, 30.0)` admits
  sub-dime prints into the 256-slot, 15-second rolling tape (64 minutes).
- `crates/server/src/rocket/mod.rs`: `FAST_WIDE_BAND = (0.05, 30.0)` admits
  sub-dime prints into the separate Fast tape. Its history is short, about a minute.
- Running uses the rolling tape's 60-minute low and money/volume windows, then
  applies its `$0.10–$25` price eligibility check. Fast also applies that band
  when deciding whether to fire. R2's feature/scoring band is separately `$0.15–$25`.
- History-pending and incomplete-history guards still apply; these constants do
  not prove that any particular live symbol had a complete restored prefix.
- The supplied Running notice itself reports `$0.1019 · +32.2% in 60 min`.
  That implies an approximately `$0.0771` reference low, already below the floor.
  This calculation uses the screenshot, not an independently recovered WFF tape.

Thus the alert floor explains why an eligible notice could not appear below
$0.10, but the claim that Runners had no sub-dime history is too broad. Adding
another $0.05 tape would duplicate existing machinery. Sub-dime setup
recognition, crossing-time explanation, and notice coalescing are separate work.
The supplied four notices alone also do not establish when every path first
satisfied its other conditions.

### The rejection failure mode is possible, but a wick-high trigger is normal

MP5 in `crates/stock-context/src/detect/micro.rs` freezes its trigger at the
impulse high (`H0`) and its invalidation at the pullback low. It checks impulse
size, coverage, pullback depth, formation time and price freshness. It does not
require the H0 candle to close strongly or reject a large upper wick. Pullback
volume is measured but is not a required arming condition.

`crates/server/src/cue_notices.rs` announces the first qualifying arming with
Runners/Ross context. It does not add a candle-rejection check. Consequently an
exhaustion-looking formation can receive a setting-up notice; a trigger at the
high alone does not distinguish it from a healthy pullback.

The existing `research/pullback-context` protocol admits triggers in
`$0.15–$25`. WFF's supplied `$0.1118` trigger is outside that study. Its results
cannot validate a new sub-dime rejection filter. Existing exports also lack the
frozen H0 candle's OHLC and volume evidence needed for that filter.

### The web log collapses the push body's newline

`crates/server/src/alerts/runners.rs::with_price_line` already puts the price
reference on its own line. The web log renders the body in a single-line span,
so normal HTML whitespace handling merges that line into the evidence sentence.

Local fix: `apps/web/src/insights/NotificationLog.tsx` converts body line breaks
to ` · ` for this inline display. The WFF example becomes
`…(33% of the move) · Alert $0.1051 · +5% = $0.1104`.
Web and the desktop shell share this component. Phone text supports actual
line breaks and does not need this web whitespace fix.

## Evidence limits

The WFF afternoon prices and four notice times come from the user's handoff and
screenshots. The available local live recording has only 35 records from
08:39:36 to 08:41:15 ET on October 26, at $0.0720–$0.0721. It cannot reproduce
the afternoon setup. Read-only requests for the live WFF chart and the day's
notification ledger returned HTTP 401; the documented SSH alias was unresolved
in this environment. No afternoon replay or historical rejection comparison was
completed. No conclusion about WFF's actual spread at notification time follows
from these sources.

## Proposed rejection experiment

This is an unvalidated candidate, not an enabled filter or a revision of MP5.

1. Export the **actual frozen H0 candle** and its availability time from the
   existing Rust detector. Never reconstruct an anchor by selecting the best
   matching wick after the outcome. Keep the original MP5 policy and transitions.
2. Before inspecting new outcome comparisons, freeze one candidate: the H0
   candle is complete by the arming decision, its upper wick is at least half
   its full range, and its close is in the lower half of that range. A zero
   range, missing anchor or incomplete coverage is UNKNOWN, not rejection.
   These numerical cutoffs are a research proposal and have no validation yet.
3. Record its classification beside the baseline setting-up notice; initially
   suppress nothing. Record pullback volume separately without adding another
   fitted condition. Use only facts admitted by the decision cutoff.
4. Compare false notices removed **and successful continuations lost**, counts
   per day, no-cross rate and timeouts. Keep the original continuation definition
   (+5% before -3% within 15 minutes of crossing) for comparability. Report
   `$0.05–<$0.10`, `$0.10–<$0.15`, and `$0.15–$25` separately, with sample counts.
   Missing/stale spread evidence stays UNKNOWN; price-only outcomes do not prove
   executable results. Replay and live classification must use the same code.
5. Work on EXPLORE only, with day-clustered uncertainty and coverage reporting.
   Previously read historical results are not a fresh holdout for this new idea.
   Freeze a new evaluation plan before using independent confirmation data.
6. If subsequently implemented, apply the decision before spending the daily
   first-setup allowance. Define how a suppressed setup's later crossing is
   treated across Pullback, Ross and Signals, and test restart/dedup behavior.
   Do not silently change the shared, frozen `detection-v2` policy.

## Proposed crossing-time context

Reuse the existing sub-dime tapes. At the first eligible in-band decision, attach
causal pre-crossing context with explicit coverage, freshness, and available
spread. A claimed base or consolidation must come from a defined causal
recognizer; WFF's hindsight-selected $0.083–$0.086 range cannot become a hardcoded
live explanation. Test threshold equality, oscillation around $0.10, stale or
missing quotes, restart/history adoption and incomplete prefixes.

One combined notice instead of several is a distinct delivery-policy choice.
Any design must preserve each client's enabled notice kinds, keep genuinely new
setup evidence, and retain the individual decisions in the log. A sub-dime
heads-up would expand alert eligibility and needs its own explicit policy and
evaluation; it is not required to obtain the already existing tracking history.

## Local validation

- Existing notification-log tests: 3 passed, 0 failed.
- Web lint: exit 0, with three existing warnings in unrelated files.
- TypeScript and production Vite build: passed; bundle-size warning remains.
- `git diff --check`: passed.
- Only the separator display behavior was changed. No detector threshold,
  notification policy, remote deployment or artifact distribution was changed.
