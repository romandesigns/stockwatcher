# Post-alert timing, liquidity normalisation and alert lateness — protocol v1

Pre-registered 2026-10-25 before any new scoring. Follows `audit.md` (deliverable A). Owner brief:
"Research R2: Post-Alert Liquidity Normalization, Entry Timing, and Early Momentum Detection"
(called the *post-alert timing study* here; "R2" is already a Runners scorer). Research only:
no production change, no merge, no deploy, prior-study artefacts untouched. Amendments go at the
end with date, reason and commit; none may be motivated by a CONFIRM result.

## 1. Hypotheses

- **HA (fixed waiting):** entering a fixed delay after the alert beats entering at the alert,
  per original alert, after costs. Decompose into cost and price components.
- **HB (dynamic readiness):** entering when liquidity has normalised (and structure is intact)
  beats the fixed 8-min delay, per original alert, after costs.
- **HC (sessions):** the HA/HB effects differ by session segment; 04:00–04:02 opening prints
  explain a disproportionate share.
- **HD (later events):** later distinct momentum events of a symbol-day differ from first alerts.
- **HE (lateness):** Runners fires after a large share of the move has already happened.
- **H0 profitability:** every rule's absolute net return is reported with its interval; a rule
  is "profitable" only if the lower bound of its mean net per trade is > 0.

## 2. Data

| Set | Days | Status |
|---|---|---|
| **EXPLORE** | 149: 2026-03-23..10-23 (archive-hist + archive-vps; 10-21 absent) | Seen by the prior study; all results here are labelled exploratory. Every selection in §7 uses EXPLORE only |
| **CONFIRM-PAST** | Every trading day 2025-10-01..2026-03-20 (≈ 117), acquired fresh with the same importer (`swimport`) and universe rules as archive-hist | Never read by anyone. Acquired and replayed, but no outcome is computed until `frozen.json` is committed |
| **CONFIRM-FWD** | Live-recorded sessions from 2026-10-26 on | Scored with the frozen rules once ≥ 20 sessions exist; reported as a second confirmation, not part of the first verdict |

Quotes: Alpaca historical quotes for each alert window `[t0 − 60 min, t0 + 45 min]` (the −60 min side
serves §9), stored under `C:\research\stockwatcher\quotes-pat\` with a manifest of request,
counts and sha256 per file. `quotes-paa` is reused read-only where it covers a window.
Feed check (deliverable): compare quote timestamps against trade prints per symbol-day (share of
trades with a valid quote within 1 s; share of locked/crossed quotes) and report by session; the
quotes are treated as Alpaca's consolidated SIP feed, and depth beyond top of book is never inferred.

## 3. Alert populations

Runners replay exactly as in the prior study (unchanged production code at f4b8862 via `paa.rs`,
burst / Run +20% / fast / running, daily caps applied in time order; R2 excluded). Band $0.10–$25,
alerts 04:00–15:15 ET (lifecycle fits before 16:00 for RTH alerts).

- **P1 first alert**: the first Runners entry per symbol-day. Used for HA, HB, HC, HE.
- **P2 events**: all Runners entries per symbol-day grouped into events. A new event starts at an
  entry made ≥ 15 min after the previous entry on that symbol (any path); entries inside 15 min of
  the previous one belong to the same event and only its first entry is traded. Event types,
  decided causally at the event's first entry:
  - `first` — first event of the day;
  - `after_failed` — since the previous event's first entry, price traded ≥ 10% below that
    event's high (fixed before this entry);
  - `after_consolidation` — not after_failed, and the 15 min before this entry have high/low ≤ 1.08;
  - `later_other` — the rest.

## 4. Session segments (America/New_York via tzdata, DST-aware)

`open_print` 04:00:00–04:02:00 · `early_pre` 04:02–07:00 · `late_pre` 07:00–09:30 ·
`mkt_open` 09:30–10:00 · `rth` 10:00–16:00. Segment = the alert time's segment.

## 5. Lifecycle, fills and costs (identical for every strategy)

- **Alert-anchored lifecycle.** Each strategy may enter from `t0` to `t0 + 15 min`. Every trade
  ends at the earlier of bracket hit or **`t0 + 45 min`** (exit at the last print), regardless of
  entry time. No horizon reset.
- **Bracket**: +10% target / −5% stop from the entry price, checked on prints (primary). Secondary:
  forward return to `t0` + {5, 15, 30, 45} min, MFE, MAE, all measured from the entry price.
- **No entry** by `t0 + 15 min` = no trade. **Per-alert** metric counts it as 0; **per-trade**
  metric excludes it. Both are reported for every strategy; the verdicts use per-alert.
  For each strategy we also report what B0 earned on the alerts it skipped (selection effect).
- **Fills.** Decision at time τ; latency 1 s (sensitivity 3 s): entry at the first ask quote at or
  after τ + latency, fill price = that ask; exit at the bid at the bracket print time + latency
  (stop/target) or at `t0 + 45 min`. A quote is valid if ≤ 10 s old, bid > 0, ask > bid.
- **Missing quotes** (no valid quote at the fill moment): the last valid quote within 60 s
  before; else that symbol-day's median quoted half-spread over the alert window applied to the
  trade price; else the trade is quote-less. The **primary** analysis uses all alerts with this
  imputation; the **quote-complete** analysis (both fills on valid quotes, for both strategies of a
  pair) is reported beside it and must agree in sign for a "Supported" call.
- **Size.** Small orders are assumed (≤ displayed size). Reported: share of entries whose
  displayed ask size × price < $2,000. Partial fills are not modelled (no depth data).
- **Fees**: commission 0; SEC/TAF fees on sells (~0.003%) ignored as immaterial; stated.
- **Halts / gaps**: a gap ≥ 5 min without prints inside a lifecycle flags the trade
  `possible_halt`; results are reported with and without those trades.
- **Decomposition** for each pair X vs B0: gross Δ (mid-to-mid, no spread) = price component;
  net Δ − gross Δ = cost component.

## 6. Strategies (all causal; entry window `t0`..`t0 + 15 min`)

Reference and baselines:
- **B0** immediate (delay 0).
- **D(x)** fixed delay, x ∈ {30 s, 1, 2, 3, 5, 8, 10, 12, 15 min}. All nine are reported; none is
  selected. **D8** is the pre-registered timing baseline because 8.4 min is the prior result.
- **RC** first red completed 1-min bar after `t0`, enter above its high (prior B2).
- **PB** frozen C2 from b9ac39a (`C2_k2_d25-65_none`), unchanged, under this lifecycle.

Dynamic candidates (the full family; nothing else is tried):
- **S1(h)** spread threshold: enter at the first valid quote with half-spread ≤ h, h ∈ {0.5%, 1.0%}.
- **S2(r)** spread normalisation: enter when the half-spread has been ≤ r × the half-spread at
  `t0` on 3 consecutive quote updates spanning ≥ 10 s, r ∈ {0.5, 0.7}. If `t0` has no valid quote,
  S2 waits for the first valid quote and uses it as the reference.
- **L(s)** = S(s) **and** structure intact **and** liquidity adequate, for s ∈ {S1(1.0%), S2(0.7)}.
  Structure intact: last trade ≥ day VWAP, last trade ≥ 1-min EMA-9 (completed bars), and the last
  completed 1-min low ≥ the lowest low since `t0` (higher low kept). Liquidity adequate: ≥ 5 trades
  and ≥ $10,000 traded in the previous 60 s.

That is 6 dynamic candidates. Feature diagnostics (descriptive only, never used to select):
spread change since `t0`, contraction rate, spread vs the symbol-day's prior 30-min median, quote
update rate, quoted sizes, trade frequency, price displacement from `p0`, 1-min range, EMA-9/20
alignment, VWAP distance — logged at every decision for the failure analysis.

## 7. Selection (EXPLORE only, single criterion)

Among the 6 dynamic candidates, the winner is the one with the highest **per-alert mean net Δ vs
D8** on EXPLORE, among those with coverage ≥ 60% of P1 alerts. Ties → the simpler rule (S before L,
lower h / higher r). Written to `frozen.json` with the code commit and committed **before** any
CONFIRM-PAST outcome is computed. If no candidate reaches 60% coverage, the highest-coverage one
is frozen and that is stated.

## 8. Verdicts (CONFIRM-PAST, P1, per-alert, day-clustered bootstrap 5,000)

- **HA — fixed waiting:** *Supported* if D8 − B0 has lower bound > 0 and point ≥ +0.5 pp, the
  quote-complete analysis agrees in sign, and it is positive in ≥ 3 of 4 chronological quarters of
  CONFIRM-PAST, and with the top 1% of D8 trades by net return removed. *Promising but unconfirmed*
  if the point estimate is ≥ +0.5 pp but one condition fails. Else *Unsupported*. Reported with the
  cost/price decomposition (answers decision question 1).
- **HB — dynamic readiness:** the frozen candidate − D8 judged by the same conditions, plus
  coverage ≥ 60% and same sign for its sibling parameter (the other h or r of its family).
- **H0 — profitability:** absolute per-trade net mean of B0, D8 and the frozen candidate with
  intervals; stated separately, never folded into HA/HB.
- **HC, HD, HE** are descriptive (intervals given, no promotion). Cells with < 100 trades are
  flagged *thin* and get no claim.
- *Inconclusive due to data limitations* is used when quote coverage on CONFIRM-PAST is < 70% of
  fills or the feed check (§2) shows systematic gaps.

The same tables are produced on EXPLORE (labelled exploratory) to show chronological stability.

## 9. Lateness (HE) — EXPLORE + CONFIRM-PAST, P1

For each first alert, from trades in `[t0 − 60 min, t0 + 60 min]`:
- **Retrospective move** (hindsight, descriptive only): `L*` = lowest price in the 60 min before
  `t0`, at time `t_L`; `P*` = highest price in the 60 min after `t0`. Displacement before alert
  `p0/L* − 1`; share of the move before the alert `(p0 − L*)/(P* − L*)` (only when `P* > L*`);
  minutes `t0 − t_L`.
- **Retrospective ignition**: the first completed 1-min bar after `t_L` whose range ≥ 3× and
  volume ≥ 3× the medians of the 30 bars before it (zero-trade bars count as range 0, volume 0).
- **Causal early flag** (a deliberately simple, real-time-computable reference, not a candidate
  detector): a completed 1-min bar with volume ≥ 5× the trailing-30-bar median, close ≥ +3% over the
  previous close and ≥ 5 trades. Reported: lead time over the Runners alert when it fires inside the
  same move, and its market-wide fire count per day with the share followed by a Runners alert on
  that symbol within 30 min (its false-positive burden).
- Spread and trade rate at `t_L`, ignition, causal flag and `t0` (quotes from §2 windows).
- Split by path, segment and price band. Stratified gallery of 40 events (10 per path) with charts.

## 10. Opening prints (HC detail)

For `open_print` alerts: share whose trigger print is the session's first print; B0/D8 outcomes
with and without them; scorecard-style follow-through (+10% before −5%, 30 min) per path with and
without them.

## 11. Deliverables and reproducibility

`docs/research/post-alert-timing/`: `audit.md` (A), this protocol (B), `report.md` (C–G, written by
the lead from the run's tables), `REPRODUCE.md` (H: commands, commits, data manifests with sha256 of
every derived table, versions). Tables and rows under `C:\research\stockwatcher\reports\post-alert-timing\`.

## Amendments

(none)
