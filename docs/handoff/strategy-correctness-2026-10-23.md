# Strategy correctness candidate, 2026-10-23

The workspace strategy audit identified admission and presentation defects that can make a
panel display a stock without its prerequisites being satisfied. Codex and Claude agreed on
this bounded correctness candidate after independent read-only source review. The original
audit has sixteen findings; this candidate does not claim to resolve every finding or improve
trading returns. Effectiveness requires a separate chronological, market-wide evaluation.

## Changes

| Area | Resulting contract |
| --- | --- |
| MP5 | A newly discovered setup older than eight formation slots silently declines before opening. Eight slots remains eligible; nine does not. Arming has the same backstop, including delayed history and late watch admission. |
| Runners precursor and running | Archive-listed provisional tapes wait for atomic history adoption. Pending and structurally unavailable are distinct and both suppress history-dependent scoring and calibration. An empty authoritative tape replaces provisional facts. Unavailable adoption releases live events immediately and blocks only these Runners admissions for the affected trading date. Running also waits for the date's facts table; explicit known-absent facts preserve the existing fallback. |
| Official halts | New precursor/running admissions wait through a known halt and until a completed bar whose open is at or after the matching official resume. Pressure and Runners share restored halt state. Existing rows remain. Open halts survive date rolls; there is no invented timeout. |
| Pressure | Execution windows have a monotone clock and sorted retained prints, stable equal-time ordering, a timestamped anchor, and a separate latest execution price. Truly expired sided prints are rejected and counted. Tick-rule side inference retains arrival-order semantics. |
| Clients | Pressure keeps last-known rows during reconnect, uses neutral stale gauges without animation, and becomes current only on the first replacement frame. Wire timestamps label the last frame; there is no wall-clock expiry. Runners and Signals attention excludes disconnected/stale evidence. |

Frozen model/policy identifiers, thresholds, policy bytes, scorer selection, flags,
notification preferences, Ross membership, original research and recorded data are unchanged.
Production inspection selected R2 explicitly; release must preserve that configuration even
though the source default is R0. No model promotion is part of this patch.

## Evidence and provenance

Baseline: `95abfaaa3c9d0c5377642d00289675d06d7358ed`. Preparation used a detached worktree.
Claude's first final-diff review found that retaining failed adoptions in the held queue could
freeze other panels and replay a permanently incomplete day repeatedly. The corrected design
removes that retry path, releases the live stream, and retains the Runners-only unavailable
mark until the symbol's next trading-date reset or a successful authoritative adoption
(including a successful rebuild after restart).

The final release receipt must record the SHA actually integrated into `main`, the remote
release run, the running service image identities, and readiness after preload. A candidate
baseline SHA alone does not identify a deployed build.

Runners day-log records now include `admission_gate: restored-facts-halt-v1` and `build`
(the release SHA), allowing pre-fix and post-fix cohorts to remain distinguishable without
renaming frozen strategies. Full readiness/retry semantics are in
[runner-readiness.md](../architecture/runner-readiness.md).

Local evidence is preserved outside source under
`H:/wavystack/outputs/stockwatcher-peer-review-20261023`: independent Claude review and scope
agreement, baseline-failing/candidate-passing regressions, test logs, pressure reproductions,
benchmark sources and results, and release receipts. Final-diff review and all release checks
must pass before the authorized push. The release pipeline gates shipping on its checks.

Pressure permutation evidence applies to distinct timestamps with fixed sides, not arbitrary
tick-rule arrival permutations. Synthetic timing measures the pressure portion under the real
Core lock, excludes lock acquisition/other consumers, and does not establish production
contention or end-to-end ingest latency. Browser reconnect fixtures are local synthetic data,
not market observations. Build and publication receipts do not verify installed devices.

## Deferred research and operational limits

The full revision/retraction contract across detectors, R2 feature/model parity, quiet reference
acquisition, universe/coverage bias, calibration/outcome efficacy, unknown-ADV fallback policy,
and a global readiness display remain separate work. Pressure can still consume provisional
prints until adoption. No exponential improvement is measured or promised.

`LIVE_HISTORY_PRELOAD` remains unchanged. If disabled, unrequested archive symbols can remain
silent until an existing consumer requests their history. Structural readiness does not mean
global complete feed coverage or repair a restart gap. Lost held/source events remain partial;
the date reset does not repair their historical coverage. The telemetry counter is process-local and counts only rejected
expired sided prints.

Release verification must distinguish source integration, CI, server/web runtime, desktop
installer publication, Android preview OTA publication, and installed desktop/mobile use.
The existing workflow does not distribute iOS. Its dependency-advisory step is explicitly
non-blocking; a green release does not certify dependency advisory clearance.
