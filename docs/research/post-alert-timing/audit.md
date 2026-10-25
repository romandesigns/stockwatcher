# Post-alert timing — baseline audit (deliverable A)

2026-10-25, read-only, before any new code or scoring. Independent recomputation from the
per-entry rows (`C:\research\stockwatcher\reports\pullback\test-entries.jsonl`, 14,912 rows),
not from the prior summary. Script: scratchpad `recon.py` (reproduced in `py/recon.py`).

## Repository and artefacts

| Item | State |
|---|---|
| `main` HEAD | f4b8862, unchanged since the prior study; uncommitted: only untracked `.claude/`, `target-memold/`, `target-memwt/` (build/worktree dirs) |
| `research/pullback-after-alert` | c608a68 protocol → 980c01b harness + A1 → b9ac39a frozen → d9af68a A2 → 8f73957 TEST → 7d63b4a; not pushed, not merged |
| Prior report.md | **never written** (the run's harness blocked it); results exist only as `dev-scored.json`, `test-scored.json`, and the per-entry rows |
| Trades | `archive-hist` 126 days 2026-03-23..09-21 (~0.6–0.8 GB/day, ~6,000 symbols/day), `archive-vps` 23 days 2026-09-22..10-23 (10-21 absent) |
| Quotes | `quotes-paa` present, 152 entries (day dirs + manifest), windows `[entry − 2 min, entry + 30 min]` per rule entry only |
| Runners replay | `crates/server/src/rocket/running/paa.rs` drives the unchanged burst / Run +20% / fast / running code; R2 excluded (no per-symbol replay) |

## Reconciliation (TEST, 58 days, 4,773 first-alert triggers)

All reported headline numbers reproduce:

| Measure | Reported | Recomputed |
|---|---|---|
| Frozen C2 entries | 736 | 736 |
| C2 − B0 (quote costs) | −0.42 pp [−0.94, +0.08] | −0.42 pp [−0.93, +0.07] |
| C2 − B1 | −1.48 pp [−1.92, −1.04] | −1.48 pp [−1.93, −1.04] |
| B1 − B0 | +1.86 pp [+1.67, +2.07] | +1.86 pp [+1.65, +2.07] |
| B1 − B0 gross (no costs) | +0.64 pp | +0.64 pp |
| Median half-spread at entry, B0 / B1 | 0.96% / 0.41% | 0.96% / 0.41% — **on quote-complete rows only** |
| 04:00–04:02 triggers | ~14% | 666 / 4,773 = 13.95% |
| Absolute mean net, B0 / B1 | — | −3.83% / −1.94% per trade (both negative) |

## Discrepancies and biases found

1. **Missing-quote fallback favours waiting.** A trade with no valid quote was costed at a flat
   0.5% per side. B1 lacks an entry quote far more often than B0 (1,137 vs 386 rows; quotes
   go stale >10 s on thin names minutes after the alert), and 0.5% is *below* B1's own mean quoted
   half-spread (0.83%; B0's is 1.81%). On quote-complete pairs only, B1 − B0 is
   **+1.55 pp [+1.27, +1.85]** (gross +0.41 pp), versus +2.48 pp on the pairs where B1 used
   the fallback. The waiting effect survives but is ~0.3 pp smaller than reported.
2. **Horizon reset favours waiting.** Outcomes ran 30 min *from entry*, so a 504 s wait held to
   alert + 38.4 min and 38% of B1 trades timed out (B0: 15%). The new study anchors every strategy's
   lifecycle to the alert (§ protocol 5).
3. **Selection by survival.** 50 triggers had no print after 504 s and got no B1 trade; their
   B0 mean was −6.8%. Small (1%), but per-alert accounting (no-trade = 0, reported) is required.
4. **"Median half-spread later 0.41%"** compares different entry moments on different subsets
   (quote-complete only). Medians understate: mean half-spread at the alert is 1.81%, driven by a
   long tail (p75 2.19%).
5. **Holdout status.** The waiting hypothesis was formed by looking at DEV *and* TEST; all 149
   days are now exploration data for it. A genuinely untouched confirmation set is required.

## Implications

The waiting advantage is real in the prior data but overstated by about a fifth and partly an
artefact of horizon and cost handling. Every rule remained loss-making in absolute terms under
the +10 % / −5 % bracket, which stays the headline.
