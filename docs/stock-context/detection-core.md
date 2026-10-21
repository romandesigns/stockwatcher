# Detection core: `detection-v0` (EXPERIMENTAL)

Owner handoff "StockWatcher Detection Panel" (Roman, 2026-10-21). Implemented in
`crates/stock-context/src/detect/` and wired into `engine.rs`; policy
`crates/stock-context/policy/detection-v0.json` (`"experimental": true`, `"alerts_enabled": false`,
tag `detection-v0@<sha256[..12]>` on every episode and snapshot).

**Nothing here is a trading signal, a probability or a validated edge.** `ignition_exp_v0`
reproduces the *unfrozen* research proposal `ignition-v0-proposal.md` (sha256 `6c435e0d…`, dev
values). Every other number is a StockWatcher engineering default, chosen from an existing
`setups-v0` value where one exists or from boundary fixtures, and marked **ours** below. A changed
number is a new policy file and id, never an edit of v0. Synthetic fixtures establish engineering
behaviour only.

## Where it runs

The detectors are part of `stock_context::Engine` and read exactly what the pillars read: the
admission ledger (identity, duplicates, corrections and cancels), the 15 s grid with its 2 s
settle, the completed bars, coverage and integrity flags, and observed halts. They run for **every
symbol the engine sees**, with or without a chart, Ross membership, news or float, and they never
change a pillar decision, the Ross Panel or `bull_flag_v0` (`tests/detection_core.rs`
`ross_membership_and_panels_are_untouched_by_detection`). Detection is off unless the caller
builds the engine with `.with_detection_policy(DetectionPolicy::v0())`. The engine is pure: no I/O
and no wall clock.

### API

| Call | What it does |
|---|---|
| `Engine::with_detection_policy(p)` | Turns detection on. A policy whose grid differs from the pillar grid stays disabled (`data_status.disabled_reason = "grid_mismatch"`). |
| `apply` / `apply_revision` / `advance_to` | Unchanged; they drive detection. |
| `detection_snapshot()` | Cursor, active episodes (with current distances), bounded recent history, data status, telemetry. |
| `take_detection_transitions()` | Drains the bounded transition journal (oldest dropped and counted at the bound). |
| `detection_telemetry()` | Logged / taken / dropped / held, episodes, corrections, interruptions. `complete()` is false once anything was dropped. |
| `latest_features(symbol)` | The latest candidate bar's feature snapshot (1.15× flags, baseline, window context), whether or not it opened an episode. |
| `add_quiet_reference(r)` / `prune_quiet_references(date)` | The prior-session quietness seam (below). |
| `interrupt_detection(symbol?, at, reason)` / `resume_detection(symbol?, at)` | Observation failures: link down/up and stream errors (`None` = every symbol), documented halts and per-symbol gaps. |
| `detect::fixture::{scenario_names, script, drive, run, engine}` | Deterministic adversarial fixtures (fictional symbols, `SyntheticFixture`, source `fixture`). |
| `cargo run -p stock-context --example detection_demo -- all [--summary]` | Runs every fixture through a real engine and prints JSON lines. |

Records serialize as snake_case like the rest of the crate. The server stamps actual emission time
and its own sequence outside the core. The core supplies the logical `observed_at`, `cutoff`,
`bar_end` and `as_of`.

## Timing

A bar `[b − 15 s, b)` is decided at `tau = b + 2 s`. A detection reads the prints with `exec < b`
that were admitted **before** `tau`. A print admitted exactly at `tau` reaches the next decision
as a late print and never edits the completed bar. This is the engine's inherited rule, pinned by
`tests/execution_boundary.rs`, and it is an intentional exception to the handoff prose "≤". It is
pinned for detection at `tau − 1 ns` (in), `tau` and `tau + 1 ns` (out) in
`a_print_admitted_one_nanosecond_before_the_cutoff_counts_and_at_or_after_it_does_not`.

Feature snapshots are taken at the barrier and frozen into the transitions that cite them. A late
print, a correction or a reference that arrives later can change current state. It never changes a
logged record: a correction appends a new record that references the earlier one.

Trade-driven events are micro-pullback crossings and invalidations. They are evaluated as each
print is admitted, against levels armed **before** the print executed. Their `observed_at` is the
admission time. Replay uses the recorded admission times, so live and replay agree for the same
input.

Windows never cross a session segment (pre / regular / after, early closes from
`market_events::session`; overnight excluded). An active episode is interrupted with
`segment_end` at the first barrier of the next segment.

## Shared quantities

All prices are integer micro-dollars and every comparison is exact integer arithmetic. Log returns
are `round(10⁶·ln)` computed with a fixed sequence of IEEE `+ − × ÷` (no `libm`), so they are
identical on every machine (`detect/math.rs`).

Each bar adds the following to the existing OHLC and volume, without changing any existing
behaviour:
- price-qualifying trade count `N_j` (OHLC-counted prints);
- dollar volume `DV_j` (volume-counted prints);
- the execution times of the bar's high and low.

| Quantity | Definition | Source |
|---|---|---|
| `W` | the 20 slots immediately before the candidate, same segment; candidate excluded | proposal §2 |
| valid slot | a slot with ≥ 1 price-qualifying print; empty slots stay empty | proposal |
| `C_prev` | close of the last traded bar before the candidate in the segment | proposal §2 |
| return class | `adjacent` (previous slot traded) / `gap_event` (`elapsed_secs` recorded) / `first_in_segment` | proposal §2 gap rule |
| return baseline | newest ≤ 80 same-segment adjacent returns, min 30, candidate excluded; a bounded ring of 80 per symbol, independent of the 64-bar buffer | proposal §2 |
| `Z` | `(r − median) / max(1.4826·MAD, s_floor)`, thousandths; adjacent returns only | proposal §2 |
| `s_floor` | 1 increment / `C_prev` | proposal §2 |
| increment | $0.0001 below $1, $0.01 at or above (`resolution_proxy = proposal_v0_increment_proxy`): a versioned conservative proxy, **not** a claim about the exchange's trade increment | proposal §2 |
| `E` (primary) | `R / max(median_W R, 2 increments)`, unavailable unless ≥ 10 valid window bars **and** a nonzero raw median; floor binding flagged | proposal §2; ambiguity resolved in definition-agreement-v1 item 2 |
| `E_R`, `E_B` | literal `R / median R`, `B / median B`; only for a nonzero median | handoff §5.3 |
| 1.15× flags | `range_expansion_15pct`, `body_expansion_15pct`: PASS / FAIL / UNKNOWN on the literal ratios; **features only**, never an episode or alert | handoff §5.3 |
| `Q` | `(max_W H − min_W L) / last W close`, ppm | proposal §4 |
| `D` | `|ln(last W close / first W open)|`, ppm (**ours**: first price in `W` is its first open) | proposal §4 |
| `A` | valid slots / 20 | proposal §4 |

## Quiet context

Precedence is fixed (handoff §5.1, proposal §4):

1. `insufficient`: fewer than 10 valid slots (`segment_start` / `few_valid_slots`), or the
   window overlaps a recorded data gap, halt or requalification boundary (`data_gap`).
2. `sparse`: `A < 0.6` or `N_base < 20`.
3. `active_consolidation`: `Q ≤ q_low` and `D ≤ d_low`, from an admissible `QuietReference`.
4. `already_moving`: an admissible reference exists and the window is wider or drifting.
5. `unavailable`: data sufficient, but no admissible reference. Quietness is not known. It is never
   assumed quiet or moving.

`QuietReference` carries:
- the symbol's own past same-segment 30th percentiles (`q_low_ppm`, `d_low_ppm`);
- `prior_sessions`, `prior_through`, `basis`, `short_history` (pooled fallback), `synthetic`,
  `provenance` and `observed_at`.

The engine never derives percentiles; a loader computes them from prior sessions only. A reference
is admissible only if all of these hold:
- it was observed at or before the decision cutoff;
- `prior_through` is before the trading date (no in-progress or future session);
- its segment matches.

Fixture references are `synthetic: true`, and every window that read one says so.

The core also refuses, with `unavailable` and a specific reason, any reference that:
- has fewer than 10 prior sessions (`quiet_reference_too_few_sessions`), unless it is the
  proposal's explicitly flagged pooled fallback (`short_history`, then labelled
  `quiet_short_history`);
- uses a percentile other than the policy's 30 (`quiet_reference_wrong_percentile`);
- has a basis other than `split_adjusted` (`quiet_reference_basis_unsupported`; only fixtures may
  use `synthetic`);
- is synthetic in a run that is not a fixture (`quiet_reference_synthetic_outside_fixture`);
- has a negative threshold.

The latest version observed by the cutoff decides. An inadmissible latest version is never
replaced by an older one.

Quietness never gates the parent ignition; it is a stratum label on it.

## Families

One variant per family. One active episode per family per symbol. Episode ids are
`<run>:<date>:<symbol>:<family>:<n>`; transition ids are `<episode>:t<n>`. The `seq` is engine-wide.

### `ignition` — `ignition_exp_v0` (proposal §3, dev values, NOT frozen)

All of these must hold at `tau`:
- an adjacent 15 s return;
- close inside the active band $0.10–$25;
- `C > C_prev` and `C > O`;
- `Z ≥ 4.0`;
- `C ≥ 1.01·C_prev` (exact);
- primary `E ≥ 3.0`.

It is recognized at `tau` with certainty `confirmed`, plus a `range_high` level (the `W` high)
for context. The research band $0.15–$25 is recorded (`in_research_band`), never a gate.

Cooldown (proposal §5): no new episode until the rule has been false for 4 consecutive bars **and**
5 min have passed since `t0`.

Display (**ours**): the episode stays `recognized` for 8 slots, then becomes `completed`.

Sensitivities S1 (30 s) and S2 (`Z ≥ 3`) are **not** implemented, so the original trial budget is
untouched. No precursor is required, and a quiet-to-ignition is just the `active_consolidation`
stratum of this parent cohort.

### `ignition_gap_event` (proposal §2 gap rule; details-only)

The same shape across empty slots: upward, `≥ 1 %`, `E ≥ 3`. `Z` is unknown because this is not
a 15 s return. Recorded as `observed` (certainty `provisional`) and reported separately; never
confirmed. Same cooldown as ignition.

### `momentum_building` / `upward_acceleration` (**ours**, handoff §5.2)

Endpoints are the closes of the traded bars ending at `b`, `b − 30 s` and `b − 60 s`. All three
must exist, and every slot of the 30 s buildup must have traded.

Rule:
- `r30 = ln(P_b/P_{b−30}) ≥ 5000 ppm`;
- `a30 = r30(b) − r30(b−30) ≥ 2500 ppm`;
- displacement `≥ 2` increments.

The quiet window is the 20 slots ending at `b − 60 s`, strictly before both comparison windows.

| Context of that window | Result |
|---|---|
| `active_consolidation` | `momentum_building` |
| `already_moving` or `unavailable` | the separately named `upward_acceleration`: details-only (`default_visible: false`), never labelled quiet-to-ignition |
| `sparse` or `insufficient` | nothing |

The context window's high and low are frozen at first recognition as `range_high` / `range_low`
version 1, and later windows never move them. The following are measured, not required:
- dollar-volume and trade-count rates over equal durations (per slot);
- the distance below the range high.

A precursor never starts on a bar that is itself an ignition, or while an ignition episode is
active (a precursor must precede).

Lifecycle:
- `building` → `ignited` when an ignition is recognized; the two are linked both ways, and the
  ignition's `parent_episode_id` is the precursor.
- `faded` (`expired_without_ignition` after 8 slots, or `returned_below_range` on a close under the
  frozen low).
- Cooldown 4 slots after resolution.

### `range_breakout` — `range_breakout_v0` (**ours**, handoff §5.5)

Only from a `W` classified `active_consolidation`: a defined range, not a trend's recent high. The
level is `W`'s high (`range_high` v1).

| Candidate bar | Result |
|---|---|
| close above the level | `confirmed` |
| high above, close at or below | `rejected` (`crossed_and_closed_inside`, certainty `retracted`) |

Retention over the next 4 slots:
- any close at or below the level → `rejected` (`returned_into_range`);
- otherwise → `retained`;
- no follow-through print at all → `expired`.

A close more than 300 bp above the level adds a descriptive `extended_above_range_high` check, not
a state. Cooldown 4 slots.

v0 breakout tests are completed-bar classifications against the prior window. There is no
trade-driven breakout crossing (see limits).

### `micro_pullback` — `micro_pullback_v0` (**ours**, values from `setups-v0` where one exists)

Independent of every other family, membership, news and float (handoff §6). Each stage applies
only while no micro episode is active.

**Anchors.** The lookback is 20 slots (setups-v0 `impulse_lookback_bars`), traded bars only, in the
segment, after any requalification boundary.
- `H0` is the **latest** bar holding the lookback's highest high.
- `L0` is the lowest low in a **strictly earlier** bar (latest on ties). Distinct bars mean
  intrabar order is never assumed.
- `U = H0 − L0` must be at least 300 bp of `L0` (setups-v0 `impulse_min_bp`) and at least 3
  increments.
- At least 50 % of the impulse's slots must have traded.
- A new `H0` must lie after the previous episode's.

**Retreat and formation.** The episode opens as `pullback_forming` at the first barrier with a bar
after `H0`; every such bar has a lower high by construction. `L0` and `H0` are frozen at that
cutoff (`frozen_at`). The pullback low `Lp` evolves with each new bar, and the episode records:
- `d = (H0 − Lp)/U` and the stock-price retreat `(H0 − Lp)/H0`;
- pullback bars and slots;
- dollar-volume rate per traded bar versus the impulse (`dv_rate_ratio_milli`), measured, not
  required.

A negative return during the retreat never erases the impulse. Before arming:

| Condition | Outcome |
|---|---|
| `Lp ≤ L0` | `invalidated` (`impulse_origin_breached`) |
| `d > 0.5` (setups-v0 `max_retrace_milli`) | `invalidated` (`depth_exceeded`) |
| a bar with a higher high | `superseded` (`new_high_before_arming`) |
| more than 8 slots since `H0` | `expired` (`formation_timeout`) |

**Arming.** Requires at least 2 pullback bars (setups-v0 `flag_min_bars`), the latest admitted
price no more than 60 s old, and that price inside `[I, T]`. Arming fixes:
- `T = H0` (`trigger` v1) and `I = Lp` (`invalidation` v1);
- `known_at = armed_at` = the cutoff.

If the latest price is already above `T` at arming, the episode is `superseded`
(`crossed_before_arming`). A crossing before the level existed never becomes a trigger. **v0 never
revises an armed level.**

**After arming.** Only prints executed at or after `armed_at` count.

| Event | Outcome |
|---|---|
| a print above `T` | `crossing_provisional` (once per armed spell; oscillation at `T` is not a new setup) |
| a print below `I` | `invalidated` (`print_below_invalidation`) |
| a crossing and a breach at the same execution nanosecond | `ordering_ambiguous` PASS; the invalidation stands (conservative) |
| the first **full** bar starting at or after `armed_at` that closes above `T` with low ≥ `I` | `trigger_observed` (`extended` check at 300 bp, setups-v0 `extended_bp`) |

- A bar straddling `armed_at` never confirms.
- A full bar with low below `I` invalidates whatever its close (backstop). It is flagged ambiguous
  when its high and low share an execution time.
- A completed bar closing at or below `T` while provisional: `crossing_retracted` back to `armed`
  (certainty `retracted`, `correction_of` = the crossing transition).
- A revision of the crossing print does the same (`crossing_print_revised`).
- A revision of the print behind an invalidation appends a `correction` record. The outcome stands.
- Armed for more than 8 slots: `expired` (`armed_timeout`).

Volume and indicators are measured only. Missing quotes and indicators never block the family.

## Data status, interruptions and restores

| Situation | Effect |
|---|---|
| Observed halt (`set_halted`) or `interrupt_detection(Some(sym), …, "halt")` | Active episodes → `interrupted`; nothing new starts. On resume the gap is recorded, and windows overlapping it are `insufficient` / `data_gap`. |
| `interrupt_detection(None, …, "link_down")` | The same for every symbol, plus `data_status.global_interruption`. |
| Coverage `Partial` | Interrupted `coverage_partial`; nothing new. |
| Integrity: window or volume untrusted (an unprovable revision) | Interrupted `inputs_untrusted`; nothing new. |
| No print for 60 s while a micro episode is active | That episode → `interrupted` (`stale_price`). This is **not** a data gap: no baseline is reset and no global state changes. |

After a real interruption, `requalify_from` moves to the resume time. New anchors, windows and
return baselines use only bars after it. This is conservative: ignition needs 30 fresh adjacent
returns again.

**Restores.** `adopt_facts` adopts the return ring and the latest print as facts and never the
restore's episodes (`episodes_discarded` counts them). It refuses a restore under another
detection policy (`Refused("detection_policy")`). A recognition whose window reaches into a
restored prefix (`set_restored_through`) carries `warmup: true`, and is never a fresh notification.

## Policy versions

- `detection-v0` (sha256 `9ee03f45…`): the families above. The first real-data engineering replay
  ran on it.
- `detection-v1`: identical rules, plus `assessment.near_trigger_bp = 100`. That value is reused
  from setups-v0 `near_bp`. `DetectionPolicy::current()` returns v1.

v1 adds one field to `current`: `near_trigger` is `Some(true)` when the latest fresh price is below
the trigger by at most 100 bp of the price, using the exact integer test
`(T − P)·10⁴ ≤ 100·P`. Its other values:
- `Some(false)` when the price is further away, or at or above the trigger.
- `None` when there is no trigger, the price is stale, or the policy has no near rule (v0).

It is geometry, not a probability. It is never sequenced, and it never changes a state or an
order.

## Checks

Every `Check` carries `required`:
- `true`: part of the family's rule (PASS / FAIL / UNKNOWN decides it).
- `false`: a measured descriptor, which is never pending and never blocks. These are participation
  (`dollar_volume_rate`, `trade_rate`, `volume_condition`), `below_range_high`, `extended`,
  `extended_above_range_high` and `ordering_ambiguous`.

Clients read `required`; they never infer it from a check's name or reason. An episode's `checks` are the checks of its latest transition that carried any (for example, the
rules at arming). A transition without checks, such as an interruption, leaves them as they were.

## Snapshot behaviour

- **One row per symbol.** One dominant episode per symbol, with this precedence:
  1. armed or crossing micro;
  2. ignition;
  3. breakout;
  4. forming micro;
  5. momentum building;
  6. gap event;
  7. acceleration.

  Ties go to the larger `last_material_seq`, then the episode id.
- **Revisions of the latest print.** A correction or cancel recomputes the latest admitted price
  from the prints still standing, in admission order, or leaves it unavailable. A revised print is
  never shown again.
- **Ordering.** Only transitions are sequenced. Price and proximity changes update `current`
  (distance to trigger/invalidation in micros and bp, `at_or_above_trigger`, `stale`) and never
  the sequence, so ordinary ticks cannot re-order rows.
- **Monitored counts.** `monitored` counts the symbols the detectors watch, whether or not they have
  an episode:
  - `symbols_seen`: at least one admitted price-qualifying print in this run;
  - `symbols_today`: evaluated on the latest barrier's trading date;
  - `symbols_active`;
  - `symbols_interrupted`.

  `symbols` and `active` list only symbols with episodes, so a panel's denominator is
  `monitored.symbols_today`.
- **Bounds.** Recent terminal history is bounded at 256, the transition journal at 8192, and data
  gaps at 64. Every drop is counted.

## Tests

`cargo test -p stock-context --lib --tests` runs the policy and math unit tests plus three
integration files:

- `tests/detection_core.rs` (family A, timing, causality, Ross independence, policy identity);
- `tests/detection_micro.rs` (anchors, formation, arming, crossings, failures, ambiguity,
  corrections, halts, the "every trigger cites a level known before its crossing" sweep);
- `tests/detection_lifecycle.rs` (replay determinism and run isolation, bounded journal, clock-only
  expiry, link gaps and requalification, coverage loss, segment end at 16:00, restore guards,
  warmup, dominance and proximity).

Doc-tests are not run (Rust 1.98 on Windows); the API examples above are not executed as tests.

## Cost

`cargo run --release -p stock-context --example bench -- 3000 500000 [--detection]` replays the
same synthetic stream twice: 3,000 symbols and 500,000 prints spread over the regular session.
That makes about 320 fresh candidate bars per barrier, a worst case in which nearly every print
opens a new bar. On the development machine (Windows, synthetic, not a VPS figure, 2026-10-21):

| Detection | Throughput | Largest single barrier |
|---|---|---|
| off | ≈ 122k prints/s | ≈ 4.4 ms |
| on | ≈ 81k prints/s | ≈ 5.8 ms |

That is about 3.7 µs per candidate bar. Most of it is the per-bar feature snapshot (two 80-sample
medians, window scans, record construction). Session bounds are resolved once per step, and the
policy is shared, not cloned.

## Known limits (v0)

- No validated edge, probability, latency or outcome claim. Fixtures prove definitions, not
  usefulness.
- Quietness is unavailable until a loader supplies real prior-session references. That is a
  scoped data limit, and no dataset is edited here.
- Only the 15 s canonical base. There is no 30 s sensitivity, and no 1 m / 5 m context (optional,
  reported as absent rather than invented). VWAP/EMA, quotes and catalysts are not features yet.
- Breakouts are bar-level only (no trade-driven provisional breakout crossing). The micro v0 has no
  post-arm level revision and no local pullback-pivot variant.
- A same-bar `L0`/`H0` impulse is never used, even when trade order would establish it.
- A revision older than the 64 retained bars leaves older ring returns as they were (the engine
  marks such windows untrusted when it cannot rebuild them).
