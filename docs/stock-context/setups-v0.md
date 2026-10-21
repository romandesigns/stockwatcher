# Early setups: `bull_flag_v0` (setups-v0, EXPERIMENTAL)

> **Superseded for the Ross Panel on 2026-10-21 (owner decision).** The rows' cues now come from
> the detection policy's micro-pullback, `detection-v2` variant MP5, the same episodes the
> Detection Panel shows ([ross-panel-v1.md](ross-panel-v1.md) "Row cues",
> [detection-core.md](detection-core.md) "One pullback detector"). On the 2026-10-07..20 holdout
> BF1 (this family with a 5 % pole) scored 0.163 against MP5's 0.183 for "+3 % before −1.5 %",
> intervals overlapping. The code (`crates/stock-context/src/setup.rs`) and `setups-v0.json` are
> kept unchanged so the research harness (`research/setup-compare`) reproduces; no engine row
> runs it any more, and `tests/setups.rs` pins its behaviour directly. What follows describes it
> as it was shipped; "the engine" below means the pre-2026-10-21 engine.

Owner product decision (Roman, 2026-10-20): every Ross Panel member is monitored continuously for
an early bullish setup, before confirmation, so the UI can show cues. Implemented in
`crates/stock-context/src/setup.rs` and `engine.rs`; policy
`crates/stock-context/policy/setups-v0.json` (`"experimental": true`), tagged `setups-v0@<sha256>`
on every panel snapshot (`setup_policy`).

**Every number in the policy is a development starting value, not a researched, back-tested or
validated one.** Nothing here is a trading signal or a recommendation; the family names levels the
engine can measure and says where the price stands against them. A changed number is a new
policy file and id (`setups-v1`), never an edit of v0.

## Bars

The base timeframe is the engine's own 15 s grid (`bar_secs` must equal the pillar policy's
`grid.bar_secs`; otherwise every setup stays `watching` with `bar_secs_mismatch`). A bar
`[b − 15 s, b)` is built from the same admitted prints the pillars use: OHLC from OHLC-counted
prints (open and close by execution time), volume from volume-counted prints. It is completed and
read at the same barrier `b + 2 s` as the decisions, so the setup runs whether or not any chart is
open and does not depend on the timeframe a user views.

Ours: a print that executed in an earlier bar but was admitted after that bar's barrier (late)
never edits a completed bar; it is left out of the setup bars (it still counts for the pillars).
A 15 s slot with no print has no bar: nothing is interpolated.

## The family

Rules are evaluated at every barrier for every active member (members are few). Before a flag has
formed the detection is recomputed from the bars; once it forms the setup is tracked
statefully.

- **Impulse (pole).** Within the last `impulse_lookback_bars` (20 = 5 min) bar slots, the pole
  high `H` is the earliest bar with the window's highest high (a later, higher bar extends the
  pole) and the pole low `L` the lowest low at or before it. Needs `H ≥ L × (1 + 3 %)`
  (`impulse_min_bp` 300) and pole volume per bar slot `≥ 1.5 ×` (`impulse_vol_mult_milli` 1500) the
  symbol's average volume per bar slot today. Ours: the pole's volume is the sum over the slots
  from the low bar to the high bar divided by that slot count; today's average is today's counted
  volume divided by the slots since the bar of today's first counted print (empty slots count as
  zero).
- **Flag.** The bars with an OHLC print after the pole high: at least `flag_min_bars` (2), at most
  `flag_max_bars` (20), every low above `H − 0.5 × (H − L)` (`max_retrace_milli` 500). While fewer
  than two flag bars exist the member stays `watching` (`impulse_awaiting_flag`, pole shown, no
  levels). Ours: flag bars count bars that traded, so a quiet or halted stretch does not age the
  flag (the quiet rule below covers it).
- **Levels.** Trigger = highest high, invalidation = lowest low of the COMPLETED flag bars. At
  barrier `b` the bar ending at `b` is under test and is never part of the level it is tested
  against. After the test a non-breakout bar joins the flag; if it moves a level the level gets a
  new version `<setup id>:trigger:<n>` / `:invalidation:<n>` with its time and reason
  (`flag_formed`, `flag_high_raised`, `flag_low_lowered`). Ours: once the flag has formed, a bar
  that trades above `H` without confirming is absorbed this way too (it raises the trigger) rather
  than restarting the pole.

## States (one primary per member)

| State | When |
|---|---|
| `watching` | No setup; `reason` says why (`no_impulse`, `impulse_volume_low`, `impulse_awaiting_flag`, `retrace_beyond_max`, `flag_exceeded_max_bars`, `no_recent_prints`) |
| `forming` | Valid flag, price below the trigger by more than `near_bp` (1 %) |
| `near` | Price at most 1 % below the trigger |
| `crossing` | The bar under test traded above the trigger (or the price is above it) but the confirmation is incomplete; `pending` lists `close_above_trigger` and `volume`, each PASS / FAIL / UNKNOWN, `crossed_version` names the trigger version |
| `triggered` | The bar under test CLOSED above the trigger AND its volume `≥ 1.0 ×` (`confirm_vol_mult_milli` 1000) the average flag-bar volume |
| `invalidated` | The bar under test closed below the invalidation level, the price is below it, a flag bar retraced beyond 50 % of the pole, or the flag exceeded 20 bars |
| `halted` | The symbol is halted: paused, nothing is tested, levels are kept |

Because decisions are made at barriers, `crossing` is visible for one barrier at a time: a bar
that spikes through the trigger and closes back below is `crossing` (close FAIL) at its barrier,
then joins the flag (raising the trigger) and the member returns to `near` or `forming`.

`triggered` and `invalidated` freeze an `outcome` (decision cutoff `at`, `bar_end`, the level
version crossed, the deciding close and bar volume) that never changes. They stay visible for
`display_hold_secs` (120 s); then the family re-arms (cooldown) and only looks for a new impulse in
bars after the spent one.

**Missing data.** No OHLC print for `max_quiet_secs` (60 s, ours) drops the setup: the member
shows `watching` / `no_recent_prints` with no levels. Levels are never carried over a gap or
invented.

**Halts** use the engine's observed trading status (`Engine::set_halted`).

**Scope.** A member that leaves the panel forgets its setup; on re-entry it starts at `watching`.
Setup and transition counters run per symbol per trading date, so ids never repeat within a day;
a new trading date starts from nothing.

## Output

`PanelRow.setup` (`SetupView`, snake_case JSON): `family`, `policy`, `experimental`, `state`,
`state_since` (cutoff ns), `reason`, `transition_id`, `setup_id`, `pole {low, low_bar_end, high,
high_bar_end}`, `trigger` and `invalidation` (`{price, version, set_at, reason}`), `flag_bars`,
`price`, `distance_to_trigger_bp` (`(trigger − price) / price`, positive below), `meter_milli`
(`(price − invalidation) / (trigger − invalidation)` clamped to 0..1000 thousandths),
`risk_to_stop_bp` (`(price − invalidation) / price`), `pending [{condition, state}]`,
`crossed_version`, `extended` (price more than 3 % over the trigger, `extended_bp` 300), `outcome`.
Prices are micro-dollars, times UTC nanoseconds; bp and milli values are truncated for display and
classify nothing (every comparison is an exact integer cross-multiplication).

`PanelSnapshot.attention` counts members in `near`, `crossing` or `triggered`.

`Engine::take_setup_log()` returns the append-only log: every transition (`id`, `from`, `to`,
`reason`, `at`, `bar_end`, the level versions current at that moment) and every level version.

## Not here yet

No research on any of the numbers, no other families (flat-top breakout, VWAP reclaim, …), no
server channel or UI. The setup is not part of ranking or membership.
