# Detection Panel local demonstration and shadow handoff

This is experimental event discovery. Rust owns the decisions; web and native render the same
episodes, checks, levels and transition history. No audio, push, orders or probability estimates
are enabled. Ross membership and ordering remain independent.

## Run the isolated browser demonstration

From this integration checkout, with the existing locked dependencies available:

```powershell
bun install --frozen-lockfile --ignore-scripts
bun run --filter @stockwatcher/web build
cargo run -p stockwatcher-server --example detection_fixture --locked -- micro_retracted --until-event crossing_retracted --serve --static apps/web/dist
```

Open `http://127.0.0.1:8792/#/chart` (the practice page: the dashboard reads live only since
2026-11-23, so the replayed fixture shows there).
The live slot remains idle. The fixture server binds loopback and starts no live provider.
Its synthetic logical clock is paused after the requested real core transition; later expiry
inputs have not been applied. Every detector view is labeled fixture / synthetic.

1. MPXC is armed, below its $3.40 trigger, with fixed $3.32 invalidation. Select its row to load
   that exact episode on the practice chart. Its first recognition and latest
   material transition have separate millisecond timestamps.
2. Open Evidence & timeline. Inspect the frozen $3.00 → $3.40 impulse anchors, 20% depth,
   mandatory PASS checks, optional participation measurement, arming time and level version.
   The provisional crossing and its subsequent retraction remain in history. Restored history
   is a separate label from provisional/confirmed/retracted certainty.
3. Change the chart between 15 seconds, 1 minute and 5 minutes. Engine IDs, state, arming time
   and structural levels stay the same. Reference lines begin when known, never at an earlier
   pivot or larger candle's opening. They do not alter autoscaling or become editable drawings.
4. Exercise family, session, source, catalyst and active/history filters. Filtering/pagination
   does not delete monitoring state. Tap then hold a ticker to use the existing copy gesture.

To inspect other completed scenarios, stop only this local example and replace its scenario
argument (omit `--until-event`): `quiet_build_ignition`, `ignition_no_precursor`, `wick_only`,
`sparse_prints`, `micro_valid`, `micro_invalidated`, `micro_depth_exceeded`,
`micro_crossing_cancelled`, `micro_crossing_before_arming`, `micro_halt_interrupt`.
Completed scenarios may have no active episode; use **Recent history**. The quiet-building
scenario's reference is explicitly synthetic. The separate body/range 1.15× flags are
diagnostics; the experimental ignition uses its own stricter rules.

Without `--serve`, the same example prints an actual Market JSON envelope. The pure-core
equivalent is `cargo run -p stock-context --example detection_demo -- all --summary`.
The HTTP research view is `/api/detection/export?source=replay`. It is bounded history,
not a declaration that an entire feed or run was captured.

## Reviewable forward shadow configuration

The configuration in [detection-shadow.env.example](detection-shadow.env.example) is prepared
for a separately authorized runtime transition. It has **not** been applied to production.
Preserve existing provider, archive, authentication, Ross and other feature settings. Use the
existing authorized data; this increment requests no new subscription or acquisition.

`DETECTION_PANEL=1` enables the current experimental profile, `detection-v2` (detection-v1 with
the micro-pullback as variant MP5, owner decision 2026-10-21; its episodes are also the Ross
Panel's cues, with or without this flag). It preserves the active $0.10–$25 band and reports the
$0.15–$25 research band separately. v1 added the 100bp near-trigger geometry cue to the
unchanged v0 signal profile. See the exact hashed
policies and [core definitions](detection-core.md). Near-trigger is not a probability.

When an archive handle is available, the server writes separate create-new
`shadow/detection-<source>-<boot>.jsonl` journal segments under that configured archive.
If no durable target is available, the frame discloses it. Do not treat an in-memory export
as a complete forward research record. Preserve originals and the per-run header, exact
code/policy identity, data-status records, sequence/drop counters and unverified-tail status.
This does not establish power-loss durability.

Record a forward cohort without tuning thresholds against its outcomes. Check current-state
recovery separately from complete transition coverage. A detected history hole remains
disclosed through resynchronization for that run. Quietness remains unavailable without an
admissible prior-session reference; price-only ignition and micro-pullbacks remain independent.
Historical data without original receipt times does not establish observer-time parity.

While a symbol's archive is loading, the panel reports **Assessment unavailable: history pending**
even if other symbols have active rows. RVOL and momentum ranking remain unavailable for that
symbol; inputs continue accumulating. Successful history adoption ends this temporary guard
without erasing genuine coverage or revision doubts. A failed load remains unavailable.

Disable by restoring the prior `DETECTION_PANEL` value (default `0`) and restarting the same
authorized runtime. Keep the journal and prior configuration; no data deletion or policy
rewrite is part of rollback. No main merge, PR, push or deployment is needed for this local demo.

Native Android/iOS exports are bundle validation, not installed-device or distribution proof.
Engineering replay and synthetic performance measurements do not establish predictive value,
profitability or competitive live latency.
