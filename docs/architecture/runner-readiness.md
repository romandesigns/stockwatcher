# Runners admission readiness and halts

The 2026-10-23 peer-reviewed candidate corrects admission prerequisites for the precursor
scorer (R0, R2 or placeholder) and the running entry. Scorer weights, frozen identifiers,
thresholds, policy bytes and the known-absent ADV fallback are unchanged. Fast move and the
other independent entry paths keep their existing contracts.

## History adoption

An archive-listed symbol's provisional prints still update its live tape, but the symbol is
excluded from precursor scoring, running inputs and adaptive threshold calibration until its
history and held suffix are atomically adopted. The pending flag survives a trading-date roll.
Adoption replaces the complete symbol state; an authoritative empty tape removes earlier
provisional bars instead of making them eligible. Existing logged rows remain visible.

Pending adoption and unavailable history are separate states. A structurally short rebuild
clears pending adoption, adopts the readable facts once with explicit partial coverage, and
marks the symbol unavailable for these Runners admissions on the adoption's trading date. It
removes the symbol from `unloaded`, so charts, scanner, zones and bursts immediately continue
their ordinary live input paths. Overflow, recorder drops and unreadable/truncated sources are
not automatically retried: rereading cannot recover lost deliveries, and repeated full-day
adoption would freeze other panels behind the hold. Existing catch-up deferrals retain their
original retry contract; a completed partial adoption is not such a deferral.

The Runners-only unavailable mark ends on a later real trading-date reset, successful explicit
authoritative adoption, or a restart followed by successful rebuilding. It is scoped to the
adoption date even when the rebuilt tape is empty: the first live print on that same date must
not clear it. Pending adoption continues to survive date resets. A restart by itself is not
proof of repaired history. Lost held/recorded events retain their partial provenance; no events
are fabricated. Each pending hold remains capped at 50,000 events. Unverified delivery order
remains explicit scanner provenance and does not by itself claim structural incompleteness or
complete feed coverage.

`LIVE_HISTORY_PRELOAD` remains unchanged. When disabled, symbols that nobody requests may stay
silent in these admission paths until an existing consumer requests history. Turning on preload
is a separate operational decision.

The running entry separately waits for the trading date's facts table. An explicitly installed
empty table means known absent; it uses the existing maximum move requirement and omits the
RVOL gate. With no running archive configured, the market explicitly installs that empty table
after the first facts request. A thread result from a different date cannot complete today's
readiness.

## Official trading status

Pressure is the shared source of the active official halt and latest matched official resume.
The market applies each chart halt update to pressure before syncing Runners. History adoption
also syncs that state. An active halt survives a trading-date roll and ends on a matching official
resume; no elapsed-time timeout is introduced.

Known halts prevent new precursor/running rows and their admission notices, including delayed
prints received during a halt. Existing rows keep their halted state. After resume, eligibility
requires a completed 15-second bar whose open is at or after the resume timestamp. The bar
containing a mid-bar resume is ineligible. The halt state known when the barrier is processed
wins an exact timestamp tie. Valid scoring and calibration during a halt remain unchanged; the
precursor's rolling windows can span a halt.

## Operational evidence

The existing live `history` status includes `runners_readiness`: symbol count, `unrestored`,
`unavailable`, `withheld` (the union), and `running_facts_ready`. The counts describe tracked
tapes, so unloaded symbols without any qualifying provisional tape are represented by the
separate history `pending` count. Following a partial adoption, later live prints are no longer
history-pending; only Runners stays unavailable for that date. Readiness is not an efficacy
metric or a claim that a restart gap was backfilled.

The sibling `pressure_expired_sided_prints_dropped` counter is pressure telemetry, not a readiness
count. It counts sided prints rejected as outside pressure's maintained execution-time windows.
It survives a day roll and symbol replacement without importing a rebuilder's diagnostic
counter. It resets with a fresh process/board and does not reconstruct prior-process drops.

Runners day log records include `admission_gate: restored-facts-halt-v1` and the release build SHA
under `build`, preserving frozen strategy identifiers while separating output cohorts across
the admission correction. Deployment evidence must record the SHA actually integrated into
`main`, not only the detached candidate's baseline SHA.
