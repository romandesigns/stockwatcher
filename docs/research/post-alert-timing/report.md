# Post-alert timing, liquidity normalisation and alert lateness — report

2026-10-25. Owner brief "Research R2: Post-Alert Liquidity Normalization, Entry Timing, and Early
Momentum Detection". Research only: nothing here changes production, and nothing is recommended
for integration yet (see Recommendation).

Branch `research/post-alert-timing` (not pushed): audit + protocol b99474b → engine + A1 eb43fc9 →
frozen S2(0.5) e58d87e → A2 e67cf8a → A3 f1b3c56 → CONFIRM scored once b215193 → lateness dbfccba,
8e7209a, 8f1a79d. Deliverables: A `audit.md`, B `protocol.md` (with amendments A1–A3), C–G this
file, H `REPRODUCE.md` + `results/` + `C:\research\stockwatcher\reports\post-alert-timing\`.

## Data

| Set | Days | First alerts (P1) | Status |
|---|---|---|---|
| EXPLORE | 149 (2026-03-23..10-23) | 12,734 | Seen by the earlier pullback study; every choice was made here |
| CONFIRM-PAST | 118 (2025-10-01..2026-03-20) | 11,035 eligible | Downloaded for this study, never read before; scored once after the freeze |
| CONFIRM-FWD | live sessions from 2026-10-26 | — | Not yet available |

Every lifecycle is anchored to the alert (entry window 15 min, every trade ends by alert + 45 min);
an alert a rule never enters counts 0. Fills at the NBBO ask/bid at decision + 1 s. Bracket +10% /
−5%. Day-clustered bootstrap, 5,000 resamples.

## The five decision questions

**1. Is the advantage from waiting caused by lower costs, price movement, or both?** Both, mostly
cost. Waiting 8 minutes (D8) beats entering at the alert (B0) by +1.51 pp per alert on CONFIRM
[+1.37, +1.66] and +1.75 on EXPLORE. Of that, price movement is +0.63 pp on CONFIRM (+0.24 on
EXPLORE); the rest is spread: the median half-spread is 0.95% at the alert and 0.44% eight
minutes later. The gain rises to about +1.4 pp by 5 minutes and is flat from 8 to 15 minutes.

**2. Can StockWatcher detect improving execution conditions dynamically?** Not with the rules
tested. The frozen spread-normalisation rule S2(0.5) times entries no better than the fixed wait
(matched gain vs D8 +0.007 pp [−0.15, +0.17] on CONFIRM). Its apparent per-alert advantage came
from the alerts it skipped, and random skipping at the same rate earns almost as much. Its sibling
flipped sign and its coverage fell below 60% on CONFIRM.

**3. Does any approach improve outcomes enough to justify further independent validation?** Only
the fixed wait, and only once costs can be measured properly (see Recommendation). **No rule is
profitable**: per trade on CONFIRM, B0 −3.96%, D8 −2.45%, S2(0.5) −2.18%; all 18 rules are
negative on both sets. Waiting loses less; it is not an edge.

**4. Are 04:00 opening prints distorting the detector's apparent performance?** No. They carry
0.2% of the waiting gain on CONFIRM, and follow-through on them was lower than other alerts on
EXPLORE (15.6% vs 20.8%) but higher on CONFIRM (25.1% vs 17.3%). A larger cluster was found instead
(below).

**5. Does Runners catch ignition, or momentum after significant appreciation?** After
appreciation. At the median first alert, 74% (EXPLORE) / 77% (CONFIRM) of the hour's move has
already happened; the stock is 13–15% above its 60-minute low and has a median 3.5–4.7% left to
its 60-minute high. Burst is the exception (46–57% of the move before the alert, 12–17% left);
Running is the latest (about 80%, 40 minutes after the low). A simple real-time flag fires
inside the move 5–6 minutes earlier for 30–40% of alerts, but it fires about 1,200–1,360 times a
day and only 7–8% of fires lead to a Runners alert: earlier information exists, and nothing tested
here separates it from noise.

## Hypothesis classification

| Hypothesis | Classification | Basis |
|---|---|---|
| HA — a fixed wait beats immediate entry | **Inconclusive due to data limitations** (promising) | Every statistical condition passes on CONFIRM (interval > 0, ≥ +0.5 pp, 4/4 quarters, top 1% removed +1.37, strict quote-complete +0.69 [+0.45, +0.93]), but only 63.8% of D8's fills are on real quotes, under the pre-registered 70% floor |
| HB — dynamic liquidity readiness beats a fixed wait | **Unsupported** | Timing gain ≈ 0 on both sets; sibling flips sign; coverage 56.9% |
| H0 — any rule profitable under the +10/−5 bracket | **Unsupported** | All 18 rules negative on both sets |
| HC — sessions differ; opening prints explain the gain | **Unsupported** for opening prints; the gain is present in every segment except open_print (+0.07, thin) | Segment table below |
| HD — later events differ from first alerts | **Promising but unconfirmed** for after-failed-breakout events only | +2.02 pp [+1.51, +2.55] vs +1.51 for first alerts; other later types thin |
| HE — Runners is late | **Supported** (descriptive, both sets) | Lateness table below |

## Results

Fixed delays, per-alert gain over B0 (pp):

| Delay | 30 s | 1 m | 2 m | 3 m | 5 m | 8 m | 10 m | 12 m | 15 m |
|---|---|---|---|---|---|---|---|---|---|
| EXPLORE | +0.67 | +0.93 | +1.27 | +1.38 | +1.54 | +1.75 | +1.74 | +1.74 | +1.75 |
| CONFIRM | +0.55 | +0.80 | +1.11 | +1.24 | +1.38 | +1.51 | +1.55 | +1.58 | +1.62 |
| CONFIRM, price part | +0.20 | +0.27 | +0.41 | +0.46 | +0.55 | +0.63 | +0.65 | +0.63 | +0.60 |
| CONFIRM median half-spread at entry | 0.77% | 0.65% | 0.57% | 0.50% | 0.46% | 0.44% | 0.42% | 0.41% | 0.41% |

Sessions (CONFIRM, D8 − B0): open_print n 371 +0.07 [−0.85, +1.02]; early pre-market 815 +1.69;
late pre-market 5,754 +1.68; 09:30–10:00 1,761 +1.17; regular session 2,334 +1.52.

**The 08:00 cluster (post-hoc, descriptive).** 36% of CONFIRM first alerts (3,978) fire between
08:00:00 and 08:02:00, almost all Fast move, on all 118 days: these stocks trade from 04:00, but the
07:59 bar has a median of 1 trade and the 08:00 bar 407. Their follow-through is 12.3% vs 20.5% for
other CONFIRM alerts. On EXPLORE the same cluster is 3.8% of alerts with normal follow-through. It
explains most of the population shift between the sets (late pre-market 52% of CONFIRM alerts vs
24%). The waiting result is unaffected: D8 − B0 is +1.35 [+1.17, +1.55] inside the cluster and
+1.60 [+1.40, +1.82] without it (EXPLORE +1.79 / +1.75). The likely cause is a step change in
extended-hours participation at 08:00 in that period; venue codes were not checked.

Lateness (first alerts, median; share = (alert − 60-min low) / (60-min high after − low)):

| | EXPLORE | CONFIRM |
|---|---|---|
| Share of the move before the alert | 73.7% | 77.2% |
| Alerts with ≥ half the move behind them | 74.6% | 80.1% |
| Rise left to the 60-min high | +4.7% | +3.5% |
| No new high within 60 min | 13.5% | 17.1% |
| Share before, by path (burst / run / fast / running) | 46 / 75 / 71 / 81% | 57 / 77 / 77 / 80% |
| Half-spread at the move's low / at the alert | 1.2% / 1.2% | 0.8% / 0.9% |

Spreads are no tighter at the start of the move than at the alert, so detecting earlier would not
by itself make entries cheaper.

## Failure analysis (CONFIRM, both fills on real quotes; full lists in `results/examples-*.json`)

- Waiting helps: NXL 2025-10-08 (B0 −28.5%, D8 +25.4%), VIVO 2025-12-12 (−25.5% / +11.9%),
  ASPC 2026-01-08 (−17.8% / +17.4%).
- Waiting hurts: CYPH 2025-10-06 (+105% / −7.3%), PCSA 2025-12-17 (+28.8% / −5.6%).
- Waiting misses the breakout: VMET 2026-03-06 (B0 won by 04:16; D8 entered at 20.33, −26.2%),
  FOXX 2025-11-21 (+8.4% / −12.5%).
- Waiting enters just before a reversal: TOPP 2026-01-20 (D8 −22%), WSHP 2026-02-25 (−18.2%).
- Frequency: D8 beat B0 on 6,787 of 11,038 alerts; B0 had already won within 8 minutes 1,325 times
  (D8 then lost 670 of them); D8 was stopped out within 3 minutes 1,854 times.

## Limitations

- Delayed entries often fall on stale (> 10 s) or locked/crossed quotes, especially pre-market
  (locked/crossed 16–18% of updates); imputed costs probably understate what waiting costs on thin
  names. This is why HA stays inconclusive.
- CONFIRM-PAST was planned from symbols listed today, so delisted names are under-represented.
- Execution is modelled at 1-second resolution with no depth or partial fills; a stop and target in
  the same second counts as a stop.
- Later events are capped at 4 per symbol-day by how triggers were exported (first entry per path).
- The 60-minute lateness window truncates slow climbs; a day-level window still gives 64–66%.
- The 08:00 finding was not pre-registered and is descriptive only.

## Recommendation

1. **Do not change alerts or add an entry recommendation now.** Nothing tested makes money, and the
   one robust effect cannot pass its own cost-data bar.
2. **Settle HA with live-recorded quotes (CONFIRM-FWD).** The live recorder keeps every session;
   recording the full NBBO for each alerted symbol from the alert to alert + 45 min removes the
   stale-quote problem. Score the frozen D8 once ≥ 20 sessions exist (about four weeks). If it
   passes, the product form is information, not a trade call: an alert note such as "spread 1.9% now
   — typically about 0.4% within 8 minutes".
3. **Investigate the 08:00 Fast-move cluster** as its own question: it was a third of all alerts
   in the older period with weak follow-through. Confirm the cause at venue level and check whether
   it recurs in current live sessions before deciding anything about Fast move.
4. **Earlier detection needs its own detector study.** The room is real (about 74–77% of the move
   happens before the alert, and Burst shows earlier is possible), but the simple flag is 92% noise.
   A pre-registered precursor study judged at a fixed alert budget, building on Burst, is the path.
