# Codex → Claude: StockWatcher detection handoff, 2026-10-21

## Final branch and scope

**Implementation complete locally; ready for Claude's review, full checks and owner-authorized
shipping. Nothing was pushed or merged into main by Codex.**

- Final branch: `feat/detection-final`, checked out in
  `H:/wavystack/sw-detection-panel-20261021`.
- Tested implementation: `22c4920183fc2f0d2b7ebb2dd78c4d91cec2c576`. The completion commit adds
  this handoff only; application code, policy bytes and dependencies are unchanged afterward.
- Base: current local `main`, `a3fa613dba18d226c03e92f3bae6bec767467c78`. Its history includes
  `8c4da12`, `2ab6288` (mobile ticker copy) and `a3fa613` (selective release/pruning). All are
  ancestors of the final branch. `git merge --ff-only main` reported already up to date.
- The final branch contains the earlier 21-commit reliability series through `c7252c2` **as
  well as** Detection Panel work. Review the entire `main..feat/detection-final` diff; it is
  broader than a new panel alone. Default-off detection does not disable those reliability fixes.
- Other branch refs remain at: core `102eebe`, transport `e5364cc`, panel `22c4920`, mobile
  `5bd8cb6`. Their work is integrated: Rust core/server trees and all 186 relevant mobile/shared
  committed blobs were checked for equality, with integration additions preserved.
- No provider acquisition, real provider probe, account/order operation, remote host access,
  workflow dispatch or production configuration change was performed. No credential files or
  values were inspected for this handoff.

## What was built, mapped to the brief

The owner Detection Panel handoff is preserved locally at
`H:/wavystack/outputs/detection-panel-20261021/owner-handoff.txt`.

| Brief sections | Delivered behavior / primary locations |
|---|---|
| §§1–3, reconciliation and research limits | Current-main baseline plus reviewed reliability work; Detection and Ross remain separate; existing authorized data only; experimental scope recorded in [reconciliation](../stock-context/detection-reconciliation.md) and [contract](../stock-context/detection-contract.md) |
| §4, time/data semantics | One admitted-print identity/ledger, correction/cancel handling, shared 15s bars, 2s settle, immutable feature cutoffs, source/epoch identity, explicit uncertainty; `crates/stock-context/src/engine.rs`, `detect/` |
| §5, precursor/expansion | Quiet-context momentum building, independent upward-acceleration descriptor, experimental ignition, separate 1.15× body/range diagnostics and range breakout; no prerequisite Ross membership |
| §6, micro-pullback | Causal impulse and retreat, frozen anchors, fixed armed trigger/invalidation, post-arm provisional crossing, subsequent full-bar trigger observation, invalidation/expiry/interruption and immutable retraction/correction history |
| §7, parameter discipline | Versioned `detection-v0` and current `detection-v1`; one development signal profile; v1 adds only 100bp proximity geometry. Original ignition sensitivities/trial budget not expanded; no outcome-driven tuning |
| §§8–9, engine/episodes/wire | Rust owns all decisions; server adds bounded transport/history/journaling; shared TypeScript projects raw Rust evidence without implementing another detector; `crates/server/src/detection.rs`, `packages/shared/src/detection.ts` |
| §10, panel/chart behavior | Web/native Detection Panel, stable event ordering, family/session/source/catalyst filters, active/recent history, evidence/timeline/provenance, related events, exact episode chart links, read-only causal levels and existing ticker-copy gesture |
| §§11–12, data/replay/resources | Existing-data replay, source/data hashes, deterministic repeat/reopen/prefix rebuild, measured disabled/enabled overhead, bounded queues and explicit drops; no fake quiet reference or original receipt times |
| §§13–16, collaboration/acceptance/handoff | Actual native Claude Code implemented core and reviewed Codex integration; Codex audited core and integrated server/web/native; adversarial fixtures, final actual server frames, local browser demonstration and concrete default-off shadow instructions |

Definitions and engineering defaults are in [detection-core.md](../stock-context/detection-core.md),
transport in [detection-server.md](../stock-context/detection-server.md), runnable walkthrough and
rollback in [detection-demo.md](../stock-context/detection-demo.md).

The incorporated earlier five-pillar reliability work covers:

- **S1:** one current version per news story, provider revisions ordered without reviving stale
  versions, durable-before-seen writes and coverage that never outruns a failed story append.
- **S2:** corporate-action assumption expiry, fair bounded warmup filing/retries, actual derived
  intraday diagnostic curves and basis provenance, actionable Ross readiness and stale-state UI.
- **S3:** shared trade identity/admission/revision semantics, bounded output journals, original
  admission proof, day-facts adoption without another run's membership/setup decisions, honest
  truncated/unverified restore coverage and checkpoint/seal distinctions.
- **S4:** independent float/news workers, default-off optional news stream, bounded off-lock
  observation/shadow capture, source/build provenance and counted queue/write loss.

## Flags and environment defaults

Values below come from source defaults; deployed configuration was not read. Existing deployments
may already override defaults. **No new detector or news stream turns on merely by shipping this
branch.** Existing enabled Ross/evidence/maintainer services do pick up the reliability changes.
The new web/native panel still mounts and displays its disabled state when `DETECTION_PANEL=0`;
the additional subscription uses the existing socket and does not enable the engine.

| Variable / setting | Default | Effect and activation |
|---|---|---|
| `DETECTION_PANEL` | `0` | Explicit opt-in to experimental detector Engine/transport. No push/audio/orders. Independent of `STOCK_CONTEXT` |
| `DETECTION_QUIET_REFERENCES` | unset | Optional local JSONL references; no fetch. Size/identity/date/segment/basis/as-of guards; synthetic references refused outside fixture sources |
| `STOCK_CONTEXT` | `0` | Existing Ross/pillar switch. Existing deployment documentation enables it, but actual runtime values were not inspected. When enabled, reliability changes and Ross shadow capture run |
| `PRICE_BAND_MIN` / `PRICE_BAND_MAX` | `0.10` / `25` | Existing active band preserved. Research band is separately $0.15–$25; no silent production-band change |
| `CONTEXT_EVIDENCE_CHECK_SECS` | `15` | Existing stored-evidence refresh interval; local archive I/O, not a provider poll. `0` means startup/day-switch refresh only |
| `EVIDENCE_NEWS_STREAM` | `0` | New opt-in Alpaca news socket in continuously running `swimport evidence`; REST searches continue unchanged |
| `ALPACA_NEWS_STREAM_URL` | public Alpaca `/v1beta1/news` WebSocket | Optional stream URL override; no credential in URL. Authentication uses existing credential plumbing |
| `EVIDENCE_NEWS_STREAM_SYMBOLS` | empty → `*` | Stream symbol subscription. Queue 4096, reconnect backoff 1–60s, dial/send timeouts 10s are code defaults |
| `EVIDENCE_NEWS_POLL_SECS` / `EVIDENCE_NEWS_IDLE_POLL_SECS` | `60` / `300` | Existing all-symbol REST cadence: 04:00–20:00 ET trading days / otherwise |
| `EVIDENCE_NEWS_EDIT_EVERY_SECS` / `EVIDENCE_NEWS_EDIT_WINDOW_MINS` | `900` / `180` | Existing trailing edit search cadence/window |
| `EVIDENCE_NEWS_REPAIR_MINS` | `60` | Added full-freshness-horizon repair search; `0` disables. Protects against old-story edits outside the shorter edit window |
| `EVIDENCE_ALPACA_PER_MINUTE` | `60` | Existing collector news REST ceiling, including pagination/retries; not raised |
| `EVIDENCE_FLOAT_AT` / `EVIDENCE_FLOAT_MODE` | `07:00` ET / `bulk` | Existing daily float plan; bulk refusal falls back to per-symbol |
| `EVIDENCE_FMP_PER_MINUTE` / `EVIDENCE_FLOAT_WARMUP_MAX` | `120` / `500` | Existing FMP process ceiling and maximum daily intraday float entrants |
| `EVIDENCE_FLOAT_RETRY_ATTEMPTS` | `4` | Added transient retries per symbol/date after the initial attempt; durable append required before completion |
| `EVIDENCE_FLOAT_RETRY_BASE_SECS` / `EVIDENCE_FLOAT_RETRY_MAX_SECS` / `EVIDENCE_FLOAT_RETRY_QUEUE_MAX` | `30` / `900` / `2000` | Added exponential/jittered retry bounds; refusal/exhaustion/drop remain explicit |
| `EVIDENCE_UNIVERSE_DAYS` | `5` | Existing prior-close dates forming the float universe |
| `WARMUP_MAX_PER_RUN` / `WARMUP_MAX_ATTEMPTS` | `50` / `4` | Maintainer warmup batches and bounded attempts; failed work stays open until its allowance is spent |
| `INTRADAY_CURVES` / `INTRADAY_MAX_PER_RUN` | `1` / `400` | Added maintainer derivation from local archived days; these diagnostics are enabled when the existing maintainer runs |

Ross observation/shadow capture has no separate new environment opt-in: it follows an enabled
Ross board. ~~With an archive it writes separate per-boot segments under the archive's
`shadow/`~~ — **corrected in review**: a production server enables the board before the archive
is set, so the capture is memory-only and writes no `shadow/` files (see "Review amendments"
below); without durable storage it reports insufficiency. Detection has its own separate per-source/boot
journal when enabled, with unverified EOF tail and explicit loss counters. Neither is a complete
forward cohort or power-loss durability guarantee. Preserve all originals/journals on rollback.

## Data, API and request-rate impact

New authenticated HTTP endpoints are `GET /api/detection?source=live|replay` and
`GET /api/detection/export?source=live|replay`. Export is bounded retained state/history, not a
full-run corpus. New socket messages are `detection.subscribe`, `detection.unsubscribe`,
`detection.snapshot` and `detection.update` on the **existing source-specific socket**.

| Activity | Estimate at defaults / change from main |
|---|---|
| Detector provider traffic | **0 new provider HTTP calls/day.** Uses already admitted data and local optional references. Lazy history loads are off-core archive reads |
| Detector client HTTP polling | **0 automatic HTTP polls/day.** One subscription per existing feed plus reconnect/resync requests; no dedicated detection socket |
| Detection publication | At most 4 replacement frames/s per subscribed feed, if state changes: upper bound 345,600 frames/24h. These are WebSocket messages, not provider requests |
| Evidence file refresh | At 15s, about 5760 local checks/24h per running loader; no provider calls from server event/refresh paths |
| News REST, trading weekday | About 960 active + 96 idle forward searches, 96 edit searches and **24 added repair searches** = approximately **1176 searches/day** for a continuously running day. One page each would be 1176 HTTP requests; real pagination, catchup/restarts and retries add requests. Weekend baseline is about 288 forward + 96 edit + 24 repair searches/day |
| Optional news stream | Default **0 connections/requests**. Enabled: one persistent authenticated provider WebSocket plus reconnect/auth/subscription attempts; no fixed requests/day estimate. Does **not** replace or reduce the above REST load; requires account entitlement/connection-budget review |
| Float REST | Typical bulk plan approximately 10 page requests/trading day for the existing market-sized universe, plus up to 500 one-off intraday entrants; source-dependent, not a measured new daily quota. Per-symbol fallback is roughly one per universe member (about 7000 at that universe size). New transient retries can add up to four attempts per symbol/date, still using the existing 120/min FMP pacer |
| Baseline warmups | Newly filed/fair requests can cause the existing maintainer to do work previously skipped: raw daily-bar pages, split snapshots and announcement checks, up to 50 symbols/refresh and four attempts/date. Daily requests depend on entrants, batch pagination and existing refresh cadence; not a fixed 50 requests/day or a new unbounded poll. Existing split/announcement pacing remains in force |
| Intraday diagnostic curves | Derived locally from existing archive, up to 400 symbols/cycle; **0 provider requests for the derivation itself** |

The search estimates count scheduled windows, not proof of completed provider coverage. Pagers,
HTTP retries, startup catchup and multiple independently running processes affect actual totals;
existing per-process limits are not account-wide budget guarantees. No real provider call was
made to validate these estimates. Float/news workers now run independently so an FMP retry no
longer delays news, while each retains its own provider pacer.

Local data changes are additive evidence/coverage/version records, derived curves, bounded warmup
requests and admission/shadow/detection journals. Original data was not rewritten for this work:
all 146 manifest files (77,417,280 bytes) remained hash-identical. No dataset download or import
was run. Replay used only the existing SIP archive; BOATS/overnight were excluded from v1.

## Changes to existing live behavior

| Existing surface | Actual effect of the integrated branch |
|---|---|
| Ross admission/ranking | Frozen `pillars-v1`, `pillars-v2`, `ross-panel-v1`, `setups-v0` bytes unchanged. Still simultaneous 5/5 admission, retain at least 4/5 and rank by existing momentum policy. Corrected version selection, basis expiry, cancellation/identity, volume restoration and coverage can change factual PASS/UNKNOWN results. A held/unrestored tail no longer qualifies/ranks optimistically; pending RVOL is UNKNOWN |
| Evidence collector (`swimport evidence`) | Durable-before-seen float/news writes; bounded float retries; ordered news revisions; hourly repair; separate supervised float/news workers. Optional stream off by default, REST fallback unchanged. These reliability changes apply to an already running collector after shipping, even with Detection Panel off |
| Halt watcher | `pressure.rs` detector, thresholds, admission and default ordering unchanged. Detection does not gate it or consume its signals. It continues receiving the common normalized market/chart input; shared identity/revision fixes are foundation changes, not a new LULD/pressure strategy |
| `ross.*` wire | Message names/source opt-in unchanged; additive `readiness` summary can update even when rows do not. Bounded viewer-independent output draining, explicit stale/reconnecting display and freshness reasons replace silently stale/empty presentation. Core policy identity is unchanged |
| `pressure.*` wire | Existing subscribe/unsubscribe/snapshot/update schema and source handling unchanged. New `detection.*` frames coexist; no pressure payload migration |
| Phone connection handling | Source/client/attempt guard prevents the previous connection from being rendered under a newly selected source or sign-in attempt. Ross/Detection retain last-known rows but remain stale until a frame arrives on the current socket; a reopened socket alone is insufficient. Detection subscription uses existing reconnect machinery and skips redundant chart/paper JSON parsing. Chart modal shares its dashboard feed; replay retains its existing separate source connection |
| Recording/restoration | Shared execution-aware identity/revision ledger; original admissions/clock proof (now opt-in, `ADMISSION_JOURNAL`) or explicit uncertainty. Restore stops global clocks before the first omitted symbol input and never trusts condition-only held verdicts as chart-authoritative. Successful adoption clears only temporary history pending. Refused/empty adoption names its failure and (since review) ends the pending guard; no historical alert burst |

## Checks and evidence

The exact owner-requested checks were re-run on `feat/detection-final`, with `GIT_SHA` explicitly
set to the tested implementation commit. **All 21 requested commands passed: Rust tests
838 passed / 0 failed / 4 ignored across 58 binaries, formatting and all-target Clippy clean,
and all 18 JavaScript/mobile commands passed.**

```text
cargo fmt --all --check
cargo clippy -p stockwatcher-server -p market-events -p chart-engine -p market-archive -p market-replay -p market-providers -p provider-sim -p paper-broker -p stock-context --all-targets --locked -- -D warnings
cargo test -p stockwatcher-server -p market-events -p chart-engine -p market-archive -p market-replay -p market-providers -p provider-sim -p paper-broker -p stock-context --locked --lib --bins --tests
```

For each `shared`, `app-state`, `chart-core`, `chart-web`: all of
`bun run --filter @stockwatcher/<package> typecheck`, `lint`, `test` passed.
Web `lint`, `test`, `build` passed. In `apps/mobile`, `bun run typecheck`, `lint`, `test` passed
(18 tests / 105 assertions), including `src/noScrollbars.test.ts`: both indicators hidden on
all scrolling views. Mobile `tsconfig.json` was not changed. Existing lint and bundle-size
warnings remain; no warning was disabled or gate weakened.

The prior broader named-build workspace run also passed 841 tests / 0 failures / 4 ignored,
plus the fixture example test, all-target workspace Clippy and JS checks. No doctests were run
because this PC lacks rustdoc. Provider-key/live-stream and existing manual measurement tests
remain ignored; they were not counted as passed or turned into real provider probes.

Additional completed evidence:

- Eleven **final-source actual Rust/Market** fixture frames passed shared decoding and every
  filter/history/session projection. Independent transport/channel audit: 17/17.
- Offline cleared-cache Android and iOS exports passed; 186 relevant committed source blobs
  matched integration. These are bundles, not installed-device/store/EAS verification. The native
  consumer fixture corpus is an accurately identified earlier Rust compatibility capture.
- Final real replay: 72,036 SIP trades / 12 symbol-days / 5 dates, 68 transitions / 29 episodes,
  zero recorded transition/recent drops; exact repeat, reopen and fresh-engine prefix rebuild;
  61 barrier, 22 feature and seven trade/level record invariants with zero violations.
- Final source/data hashes stayed unchanged during replay. Original data has no receipt times:
  this is reconstructed-history engineering validation, not original-observer parity.
- Actual Claude Code approved final server repair (`named-restore-closure-v2.txt`) and reviewed
  measurement scope (`final-evidence-review.txt`). Codex reviewed Claude's core; findings were
  repaired without weakening the original overcount/truncated-archive assertions.

Local receipts/logs are under
`H:/wavystack/outputs/detection-panel-20261021/owner-handoff-checks/` (this protocol) and
`H:/wavystack/outputs/detection-panel-20261021/` (full report, checks, immutable replay/benchmark
receipts, reviewer findings and screenshot). These paths are local evidence, not runtime
dependencies. The earlier failed named-build logs are retained alongside successful final runs.

## Known gaps, thresholds and owner decisions

- **No validated trading edge, calibrated probability, optimal-threshold or competitive latency
  claim.** Robust surprise Z≥4, return≥1%, range expansion≥3, impulse≥3%, depth≤50%, two pullback
  bars and bounded durations are versioned experimental engineering choices, not researched
  production optima. The 1.15× diagnostics and 100bp near cue are distinct from ignition rules.
- Real admissible quiet references were absent. Momentum-building/range-breakout behavior is
  synthetic-tested but **not real-data validated**. Gap-event observations were absent. The
  small, selection-biased real sample includes hindsight/reverse-split symbols; no outcomes or
  precision/recall study was performed.
- Inherited strict `admit < bar_end + settle` with clock-first equality is an explicit legacy
  exception to the brief's inclusive cutoff prose. No frozen pillar policy bytes were changed.
- Micro `price_fresh` is a signed offset from bar end: a negative value can mean a print after
  bar end but admitted before cutoff, not future leakage. Freshness therefore allows the two-second
  settle beyond 60s at the cutoff; current price age is separately positive/as-of based. Clarify
  naming and consider cutoff-based freshness in a later version.
- ~~Conservative restore residuals accepted in local review: a genuinely empty archive can remain
  terminal `history_restore_empty` even if it later trades; recovery requires a source/process
  restart rebuilding History.~~ **Superseded in review** (see "Review amendments"): a refused or
  empty restore is no longer terminal. Reconstructed held verdicts remain
  `journal_verdict_unproven`. The existing no-Tape condition fallback remains; the review found
  no overcount path with the admission ledger's dedup.
- Journals/readers/queues are bounded; EOF is unverified and loss is counted. No whole-feed/run
  completeness, power-loss durability or full unseen-input causality proof is claimed.
- Pure Engine median on 30,434 trades: 29.40ms off → 36.03ms on (~22.5%). Actual Market synthetic
  maximum-speed batch: 36.97ms off → 174.81ms on (~4.73×), with 16 forced publications. Enabled
  publication p50/p95/p99: 6.87/14.41/15.83ms; about 675KB/full frame in that state. Four such
  frames/s would be ~2.7MB/s/subscriber before overhead (arithmetic estimate, not measured live
  bandwidth). Core-lock share, disk/network/client fan-out and rendering remain unmeasured.
  **Profile publication lock hold/payload before live enablement.**
- No installed Android/iOS walkthrough, native gesture/accessibility/device-network validation,
  installer distribution or production verification was performed.

Roman/Claude decisions before shipping or enabling new paths:

1. Review the entire combined reliability + detection diff; choose rollout/rollback for existing
   enabled Ross/collector behavior, not only the default-off detector.
2. Keep `DETECTION_PANEL=0` initially or explicitly authorize a forward shadow cohort with durable
   recording, admissible references and measured publication cost. Preserve existing auth/provider/
   archive/Ross configuration; disable by restoring the prior flag and restarting, retaining logs.
3. Keep `EVIDENCE_NEWS_STREAM=0` unless the account entitlement, connection allocation and provider
   budget are approved. Account for unchanged REST polling plus new repair pages/retries; changing
   workers does not authorize a larger account-wide budget.
4. Select real quiet-reference provenance and separately freeze adequately powered precursor,
   ignition and micro-pullback outcome studies. Resolve the documented device/performance gaps
   before making corresponding readiness or effectiveness claims.

The paused fixture demonstration currently uses `127.0.0.1:8792`, source `fixture`, replay slot,
with live idle and no alerts. Its executable names implementation `22c4920`, not the later
documentation-only completion commit. [Demo instructions](../stock-context/detection-demo.md)
show how to rebuild it; it is not a production server.

Codex's completion signal is the **last commit** on `feat/detection-final`, subject beginning
`Codex: done`. Claude owns subsequent review, merge into local main and any authorized shipping.

## Review amendments (Claude, 2026-10-21, branch `review/ship-fixes`)

Pre-ship review against a whole-market live SIP server (`STOCK_CONTEXT=1`, `DETECTION_PANEL=0`).
Measurements are local and synthetic (tests named below); no provider or host was contacted.

- **Admission journal is now opt-in (`ADMISSION_JOURNAL`, default `0`).** Measured 187–268 B per
  journalled print (`admit::tests::measure_bytes_per_record`): ~7.5–10.7 GB for a ~40M-print day,
  beside a ~0.8 GB sealed recording, and its reader stops at 4M records, so a whole-market
  restore after mid-morning could not use it anyway. Without it a restore replays the recording's
  order labelled `no_admission_journal`, as before this branch.
- **Restore merges unjournalled deliveries by time.** A recording that began before its journal
  (the first deploy of a journalling build; any boot with the journal off) had its whole morning
  replayed after this boot's clocks and excluded as late everywhere — charts, quotes, Halt
  watcher, scanner. Fixed; labelled `journal_starts_after_recording`.
- **Scanner ledger bounded for a whole market.** Was ~285 B/entry and an hour per symbol: ~1.9 GB
  for an 8M-print hour, never released for an idle symbol. Now ~146 B/entry, 20 minutes
  (`REVISION_WINDOW_SECS`), a 1,000-entry bound for symbols far outside the band, a once-a-minute
  clock sweep that returns allocations, and barrier scans over the queued tail only
  (`stock-context/tests/ledger_memory.rs`).
- **Unplaceable corrections/cancels are bounded**, not fatal: RVOL passes on the count net of
  their sizes (`at_or_above_min_net_of_unplaced`) instead of UNKNOWN for the rest of the date.
- **Coverage is dated**: Partial from a short recording, refused restore or overflow ends with
  its trading date. A refused old print is `Unverified`, not `Partial`, and only if it would
  have counted.
- **Failed restores are not terminal**; the restore engine is advanced to the live clock before
  adoption; deferred loads are retried; with the preload on, a printing symbol loads first.
- **Shadow capture** is memory-only in production (it never wrote `shadow/`); its recency bound
  is O(1) and its periodic report no longer waits on the fold.
- **Intraday curves**: band symbols first, pruned with the maintainer window, a corrupt day
  skipped and re-derived.
