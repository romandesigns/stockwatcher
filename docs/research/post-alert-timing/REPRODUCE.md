# Post-alert timing — reproduction (deliverable H, draft)

Branch `research/post-alert-timing` (worktree `H:\wavystack\stockwatcher\.claude\worktrees\timing`).
Commits: b99474b audit + protocol · eb43fc9 engine + A1 · e58d87e frozen S2(0.5) · e67cf8a A2 ·
f75c024 matched/P2/examples · f1b3c56 A3 · (CONFIRM scoring commit: see report).
Python: `C:\research\stockwatcher\venv` (3.13, numpy 2.5.4, pandas 3.0.6, tzdata via zoneinfo).
Rust: workspace at the branch; release builds in `C:\research\stockwatcher\work-pat\target`.
`P = docs/research/post-alert-timing/py`, `W = C:\research\stockwatcher\work-pat`.

## 1. Data

- EXPLORE trades: `archive-hist` (2026-03-23..09-21), `archive-vps` (09-22..10-23), unchanged.
- CONFIRM-PAST trades: `C:\research\stockwatcher\archive-past`, imported one day at a time:
  `W/sw.sh alpaca-batch --from D --to D --jobs 16 --per-minute 5500` (`W/import_loop.sh`), each
  day accepted only after `swarchive verify D` (every file's sha256 against the day manifest).
  `sw.sh` loads only `ALPACA_API_KEY`/`ALPACA_API_SECRET` from the repo `.env`. Daily bars/closes:
  `sw.sh alpaca-daily-closes --from 2025-08-15 --to 2026-03-20`. Plans: `archive-past/plans/`.
  (A first all-days import was lost to an unclean reboot that zero-filled every manifest; the day
  directories were deleted and re-imported from scratch.)
- Triggers: EXPLORE reuses `work-paa/{dev,test}/<day>/triggers.jsonl` (pullback study, unchanged
  Runners code); CONFIRM: `work-paa/paa2.exe running::paa` per day (`W/process_loop.sh`) →
  `W/trig-confirm/<day>/triggers.jsonl`.
- `python -I P/trig.py build <trig root> <series root> W/triggers-<set>.jsonl` (P1/P2, A3.8 cutoffs).
- Per-second series: `pat.exe running::pat` (`crates/server/src/rocket/running/pat.rs`) via
  `W/run_series.sh` → `W/series{,-confirm}/<day>/<SYM>.bin`.
- Quotes: `python -I P/trig.py windows …` then `python -I P/fetch.py <csv> quotes-pat/<set> <set>`;
  sha256 per file in `quotes-pat/<set>/sha256.jsonl`, request log in `manifest.jsonl`.

## 2. Scoring

```
python -I P/pat.py run W/triggers-explore.jsonl W/series quotes-pat/explore W/rows-explore --jobs 16
python -I P/agg.py select W/rows-explore reports/post-alert-timing/selection-explore-a3.json
python -I P/agg.py tables W/rows-explore reports/post-alert-timing/frozen.json reports/post-alert-timing/explore-p1
python -I P/agg.py tables W/rows-explore reports/post-alert-timing/frozen.json reports/post-alert-timing/explore-p2 --pop p2
PAT_SENS=a|b|c python -I P/pat.py run … --strats B0,D8,S2_0.5,S2_0.7   (A3.7 sensitivities)
python -I P/pat.py run … --lat 3 …                                       (latency 3 s)
python -I P/feed.py W/triggers-<set>.jsonl <series> quotes-pat/<set> reports/…/feed-<set>.json
python -I P/oprint.py W/triggers-<set>.jsonl <series> reports/…/oprint-<set>.json
python -I P/ex.py W/rows-<set> reports/…/examples-<set>.json
python -I P/md.py <tables>.json <tables>.md
```
CONFIRM: the same commands with `triggers-confirm.jsonl`, `series-confirm`, `quotes-pat/confirm`,
`rows-confirm`, run once after f1b3c56.

## 3. Checksums

`sha256.txt` in this directory lists every derived table (written at the end of the run).
