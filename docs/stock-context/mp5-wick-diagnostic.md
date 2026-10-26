# Frozen MP5 candle observation

`mp5-wick-v1` is an observational diagnostic, not an arming condition or a
validated exhaustion rule. It does not change `detection-v2`, its policy hash,
episode stages, levels, notification text, delivery limits or first-setup
allowance. No blocking-filter switch is added.

When MP5 freezes H0 at the first observed retreat, it copies that exact candle's
OHLC, bar end and decision cutoff. It does not search the history again when
arming, crossing or publishing a snapshot. A later correction does not rewrite
the observation that was actually available at the cutoff.

The candidate is FLAGGED when upper wick >=50% of the range AND the close is in
the lower half of the range. Decisions use exact integer comparisons; displayed
ratios are thousandths, rounded down. CLEAR means only that this candidate rule
was not met; it is not a healthy-setup verdict. Missing, invalid, incomplete and
zero-range candles are UNKNOWN, never silently CLEAR or rejected. Different
cutoffs require a new rule ID and study.

## Records

- Detection transitions and episode snapshots: `anchors.wick`.
- Pullback setting-up and pullback-crossing payloads on desk/web and phone:
  `data.mp5_wick`.
- Both sent and held pullback notification decisions: `mp5_wick` in the
  notification ledger and `GET /api/notices` items.

The record contains `rule`, `status` (`flagged`, `clear`, `unknown`), `bar_end`,
`known_at`, `ohlc` ([open, high, low, close] in micro-dollars),
`upper_wick_milli`, `close_location_milli`, and a reason. Timestamps are epoch
nanoseconds. An absent/null observation in an older record is unavailable, not
a non-rejection. Pullback volume remains the existing separate measured ratio;
it is not used to tune this candidate.

The current clients continue showing the same notice text. Structured candle
evidence is available for inspection and future forward analysis; no new
warning or predictive performance claim is introduced in the UI.

## Evidence and replication

The predeclared EXPLORE study is `docs/research/mp5-wick-v1-protocol.md` and the
offline tooling is `tools/mp5-wick-replay`. The control excludes only the observer
and retains the same current pillar policy. Every replay step compares all
serialized transition fields except `anchors.wick`, including sequence, state,
time, anchors and levels. Any mismatch or dropped transition fails the job.

Unit/integration checks exercise threshold equality, unavailable evidence,
sub-dollar scaling, unchanged arming, frozen-anchor corrections and future
spikes, notification payload parity, held/sent ledger records and the read API.
Historical replay is not evidence of live feed completeness, executable returns
or a validated blocking filter. Runtime CPU, disk and client bandwidth effects
of the added metadata have not been benchmarked separately.
