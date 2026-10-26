# Lateness on CONFIRM-PAST: post-hoc observation (not a protocol quantity)

Found after reading the CONFIRM tables. Definitions are unchanged; this note only describes
a cluster that the protocol did not anticipate.

**An "08:00 opening" dominates CONFIRM-PAST (2025-10-01..2026-03-20).**

- **The cluster:** 3,978 of 11,038 first alerts (36.0%) fire between 08:00:00 and 08:02:00 ET, on all 118 days. 4,073 of the 4,276 alerts in 08:00–08:10 come from the fast path.
- **What happens at 08:00:** these symbols were already trading from 04:00. Over 07:00–07:59 they print a median 5.4 trades/min. The 07:59 bar has a median of 1 trade; the 08:00 bar has 407.
- **The alert:** the alert price is a median +5% over the last close before 08:00.
- **EXPLORE has the same cluster, much smaller:** 485 alerts, 3.8%.
- **Follow-through** (+10% before −5% in 30 min, no costs):
  - The 08:00–08:02 cluster: 12.3%.
  - All other CONFIRM first alerts: 20.5% (fast only: 21.4%).
  - EXPLORE: 20.4% for its 08:00 cluster, 20.1% for the rest.
- **Share of the move before the alert** (median): 82.6% for the cluster; 73.6% for the rest of CONFIRM, the same as EXPLORE's 73.6%.

The likely cause is a step change in participation at 08:00 ET in the older period, such as
broker or venue extended-hours access, which acts like a second opening print. This was not
verified against venue codes.

Survivorship is also present: the importer planned the older days from symbols active today.

Script: inline in the run log (`rows.jsonl` filtered on ET alert time 08:00:00–08:02:00).
