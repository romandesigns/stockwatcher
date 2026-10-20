# Daily baseline and checked previous close: acquisition plan

Status: **plan + code (2026-10-19, owner decisions 2026-10-20)**, branch `feat/daily-baseline`.
Raw daily bars are kept from the responses the nightly job already fetches and backfilled to the
retention window; `maintain` takes a split-adjusted snapshot of the watch list after each import
and fetches corporate-action announcements pre-market; the loader exists and is tested but is
**not wired into the server**. Nothing here has called Alpaca from a development machine: it runs
on the VPS after merge. Numbers marked *estimate* come from the code and the bar shape, not from a
measured run.

## 0. Owner decisions (2026-10-20)

All four open questions of the first draft were approved:

1. **Live corporate-action evidence: yes**, the runtime dependency on Alpaca's
   `/v1/corporate-actions` is accepted (§6, "Announcement coverage").
2. **Downloads approved**: `maintain` backfills missing raw dates to the 63-date retention (~120
   requests once), takes the watch list's split snapshot after the import (~72 requests a night,
   own 200/min limiter) and keeps 5 snapshot sets; the announcement fetch runs at 07:00 and 09:15
   ET on trading days.
3. **Compatibility thresholds accepted**: ±2 % for ≥ 95 % of symbol-days (and the median within
   ±2 %). If it fails, the fallback is the separately versioned `provider_daily_volume_v1` metric
   with a matching live numerator (§9). Harness: `swimport volume-compat`.
4. **Defaults accepted as chosen** (owner-accepted 2026-10-20, still engineering choices, not
   validated): lookback 45 trading dates, raw retention 63, same-basis tolerance 0.2 %,
   volume/price factor agreement 1 %, watch band $0.10–$25.00 over the last 5 dates.

What it serves (`pillars-v1.md` §4.2, §4.4, `crates/stock-context/src/evidence.rs`):

- RVOL pillar: `DailyBaseline { sessions: [SessionVolume { date, volume, complete, comparable }] }`
  with at least 30 complete, comparable sessions before the evaluated date D, on D's share basis.
- Gain pillar: `PrevClose { date = previous trading day P, basis, actions_checked_through ≥ D }`.

## 1. What already exists (and what was thrown away)

`swimport maintain` (VPS `archiver` service, daily at `MAINTAIN_AT` 21:15 ET) and
`swimport alpaca-daily-closes` call `GET /v2/stocks/bars?timeframe=1Day&feed=sip&adjustment=raw`
for every active US equity (`/v2/assets?status=active&asset_class=us_equity`, tradable or not), in
chunks of 200 symbols (`batch::SYMBOLS_PER_REQUEST`), `limit=10000`, following
`next_page_token`. Range: the 21-trading-day window plus the day before
(`maintain::refresh_closes`). Until this branch only `c` reached disk
(`$ARCHIVE_ROOT/daily-closes.json`); `o h l v n vw` were discarded. `plan_universe` makes a second,
tradable-only pass over the window for the trade import; it is not changed.

## 2. Watch universe

Two sets, deliberately different sizes:

| Set | Definition | Size (*estimate*) | Why |
|---|---|---|---|
| **Raw universe** | every active US equity the assets endpoint lists (what the closes fetch already requests) | ~12,000 requested; ~7,000–11,000 return a bar on a given date (OTC and dormant listings return none) | free: same responses. Covers any symbol that later enters a band, with no prefetch |
| **Split watch list** | symbols whose raw close was within **$0.10–$25.00** (`PriceBand::DEFAULT`, the import band, wider than the pillars' $2–$20 and discovery $0.15–$25) on **any of the last 5 trading dates**, from `daily-bars/raw/` | ~7,000–7,500 (the window plan imports ~150k symbol-days over 21 days ≈ 7,100 a day) | not only stocks already meeting the pillars: a stock becomes interesting by moving into the band, and its baseline must already exist that morning |

**New band entrants** need no separate prefetch: raw bars already cover every listed symbol, and
the watch list is recomputed from the raw closes every night, so a symbol that closed in the band
today is in tonight's split set and in tomorrow's 07:00 announcement fetch. One that enters the
band intraday on D without having closed in it on any of the last 5 dates has no announcement
coverage on D (UNKNOWN that day). A new listing has no history and correctly stays
`insufficient_baseline` until it has 30 sessions.

## 3. Dates

The engine needs 30 complete, comparable sessions strictly before D. Counted in **trading dates**
from `market_events::session`, not calendar days:

- Lookback: **45 trading dates** before D (`baseline::DEFAULT_LOOKBACK`), leaving 15 for early
  closes (not comparable: 2026-11-27, 2026-12-24, …), dates without a bar (halts, no trades) and
  uncovered dates. ~45 calendar days (the brief's estimate) is only ~31 trading dates and would
  fail the first holiday month.
- Raw retention: **63 trading dates** (`DAILY_BARS_KEEP_DAYS`, 0 = all, else 46–504; pruned by
  `maintain` after each refresh). 45 + D + room to evaluate the last ~17 dates of history.
- Calendar coverage: the session table covers 2025–2027. A date outside it is never `comparable`
  (its early closes are unknown), so a 2028 baseline fails loudly as `fewer_than_30_comparable`
  rather than quietly mixing half days.

## 4. Feed and adjustment

- **Feed `sip`** only: the archive's prints are SIP; IEX volume is a fraction of the tape.
- **`adjustment=raw` is always kept**: history as printed, mergeable across fetches (a refetch of
  a date returns the same numbers or a correction). One file per date.
- **`adjustment=split` is a separate snapshot** (nightly for the watch list, or manual).
  Alpaca's split adjustment restates **both
  prices and volumes** of every bar before a split onto the share basis at the moment of the
  request. Two split fetches on either side of a split are on different bases, so they are never
  merged: each fetch is one immutable file, and a reader uses one snapshot at a time.
- **`asof` is not a corporate-action knowledge cutoff.** It is Alpaca's symbol-mapping date (which
  ticker a renamed instrument had). We never send it and record `asof: null` so nobody reads a
  fetch as "adjusted as known on date X".
- **Never `adjustment=all`** for this: it folds dividends into the same factor, so a price ratio
  stops meaning "split" and the volume would not follow the price. The rule here is split-only.

## 5. Storage (code on this branch)

```text
$ARCHIVE_ROOT/daily-bars/
  raw/2026-10-15.json                 # RawDayBars: every symbol's raw bar on one trading date
  split/20261019T231502Z-1a2b3c4d.json # SplitSnapshot: one split-adjusted fetch, never merged
```

Both are `"schema": 1` (a reader refuses anything else) and written atomically (temp file, fsync,
rename), like `daily-closes.json`. Neither is a date-named directory, so replay and retention never
mistake them for a day.

- `RawDayBars { date, adjustment: raw, bars: {SYM: {o,h,l,c,v,n,vw}}, no_bar: [SYM…], fetches }`.
  `no_bar` = requested, nothing returned: **covered, not zero**. A symbol in neither list was never
  requested (unknown). A refetch replaces changed bars and never deletes one (a provider that
  drops a bar it once served is more likely a partial answer). The file is rewritten only when
  something changed, so the identical nightly refetch of 22 dates writes nothing (tested).
- **Why per date** and not one file or per month: the nightly refresh touches ~22 dates; per-date
  files rewrite only what changed and retention deletes whole files. One file would be rewritten
  nightly at ~50 MB; per-month files would still rewrite ~20 dates to change one.
- `SplitSnapshot { fetch, symbols: [requested, sorted], bars: {SYM: {date: bar}} }`. Coverage is
  requested symbols × trading dates in `from..=to`; a symbol the provider returned unasked is
  dropped.
- **Provenance** (`BarFetch`, on every file): `retrieved_at`, provider `alpaca`, feed, adjustment,
  timeframe `1Day`, `from`, `to`, symbols requested, symbols per request, requests made, `asof`,
  and the command. A raw date file keeps the last 8 fetches that changed it.

Commands:

| Command | Requests | Writes |
|---|---|---|
| `swimport maintain`, nightly cycle at `MAINTAIN_AT` | raw: none new; backfill: only dates missing from the retention window (~120 once, then 0); split set: ⌈watch list/200⌉ × pages (~72) | `daily-bars/raw/*` (pruned to `DAILY_BARS_KEEP_DAYS`), one `daily-bars/split/set-*` set (5 kept) |
| `swimport maintain`, at `ACTIONS_AT` (07:00, 09:15 ET, trading days) | ⌈watch list/100⌉ × pages (~72) | one `corporate-actions/<D>/*.json` per fetch, append-only |
| `swimport maintain --actions-once` | as above, once | as above |
| `swimport alpaca-daily-closes --from --to` (existing) | none new | + `daily-bars/raw/*` (a manual backfill) |
| `swimport alpaca-split-bars --from --to (--symbols A,B \| --symbols-file F) [--per-minute 200]` (manual) | ⌈symbols/200⌉ × pages | one `daily-bars/split/*.json` (a set of its own) |
| `swimport volume-compat --from --to [--sample 500]` (offline) | none | `plans/volume-compat-<from>-<to>.json` |

Every split fetch is bounded in code (≤ 2,000 symbols per snapshot, ≤ 120 calendar days, refused
before any request) and only covers completed dates. Credentials: `ALPACA_API_KEY` /
`ALPACA_API_SECRET` (or `APCA_API_KEY_ID` / `APCA_API_SECRET_KEY`). New settings:
`MAINTAIN_SPLIT_SNAPSHOTS` (1), `MAINTAIN_SPLIT_PER_MINUTE` (200), `SPLIT_SETS_KEEP` (5),
`ACTIONS_AT` (`07:00,09:15`, or `off`).

Announcement retrievals (`market_archive::corporate_actions`, schema 1):
`corporate-actions/<request end D>/<retrieved>-<hash>.json` holding `retrieved_at`, endpoint,
types, `start`/`end`, symbols requested, **symbols covered** (their request chunk completed, every
page parsed), `success`, errors, requests, and the parsed forward/reverse splits (id, symbol in
force, `ex_date`, `old_rate`, `new_rate`), unit splits and name changes. A failed fetch is stored
too, so a gap is visible rather than read as "no actions". Shapes are the ones the research probes
recorded (`reports/probes/alp-corp-actions-{BTOG,SGRX}.json`, copied to
`crates/market-providers/tests/fixtures/`); unit-split fields follow Alpaca's documentation and
have not been seen in a recorded response yet.

## 6. From bars to evidence (`market_archive::baseline::load`)

For symbol S and evaluated date D, P = the trading day before D. Raw `R(x)`, split-adjusted
`A(x)` from **one** snapshot, `f(x) = R(x).c / A(x).c` (the split factor from x to the snapshot).

**Coverage.** Raw `(S, x)` is covered when the date file lists S (bar or `no_bar`) and its first
fetch happened after x's last session ended. A snapshot covers `(S, x)` when S was requested, x is
in its range, and it was retrieved after x's last session ended.

**A date x is verified** when both sides cover it, agree on presence (both bars or neither), and
with bars: closes finite and positive, trade counts equal when both present, and the volume moved
by the price factor (`|A.v − R.v·f| ≤ max(1, 1 %·R.v·f)`) — what a split adjustment does and a
dividend-style, price-only adjustment does not.

**`actions_checked_through`** = E, the latest date in P..=D such that every trading date from P
to E is verified and both sides have a bar on E; `None` when P is not verified with a bar on both
sides. The close is restated onto E's basis, `R(P).c · f(E)/f(P)`: basis `Raw` when that ratio is
1 within 0.2 % (the raw close is used exactly), else `SplitAdjusted`. Without a snapshot, the raw
close is still emitted with `None`, so the engine keeps showing `gain_vs_stored_close_bp` and the
pillar stays UNKNOWN (`adjustment_coverage_unknown`).

**Baseline.** Sessions = the 45 trading dates before D (never D or later). For each: skipped
(missing, never zero) unless raw has a covered bar with a volume and the date is verified; volume
on D's basis = `R.v · f(x)/f(D)` (exactly `R.v` when the ratio is 1); `complete = true`;
`comparable` = a full day in the calendar. **No baseline at all** unless D itself is verified with
bars on both sides (`share_basis_unverified`): a 1:10 reverse split on D makes every raw prior
volume 10× too big.

**Bars alone:** daily bars can establish D's basis only after D's bar exists, in a snapshot
fetched after D: replay and research of past dates. The night-of-P snapshot gives
`actions_checked_through = P` (truthful, insufficient for a live decision on D).

### Announcement coverage (live; a different, weaker kind of evidence)

For a live decision on D, `market_archive::baseline::announced` reads the announcement
retrievals stored for D and uses the **latest** one that (a) covered the symbol, (b) requested a
range containing `(P, D]` (`start ≤ P+1`, `end ≥ D`), and (c) was made **after P's last session
ended and before D's 09:30 open**. Then:

| Announcements in `(P, D]` for the symbol | Result |
|---|---|
| no covering retrieval (none made, its chunk failed, made too late or too early) | unknown (`no_announcement_coverage`) |
| a name change into or out of the symbol | unknown (`symbol_changed`): Alpaca files actions under the ticker in force (report 02, F7), so a rename makes the lookup ambiguous |
| a unit split touching it (or an undated one) | unknown (`unit_split`) |
| no forward/reverse split | **no split**: P's raw close (on P's basis by definition) is D's reference; `actions_checked_through = D` |
| exactly one, integral rates | close × `old_rate/new_rate`, prior volumes × `new_rate/old_rate`; ratio and action id recorded |
| several, or a fractional rate | unknown |

The baseline uses an announcement only **on top of bar-verified history through P** (factors from
a snapshot that verifies P), so 44 of the 45 dates stay bar-verified and only the last step is
announced. When bars verify D itself (replay of a past date), bar verification wins.

**Why it is weaker, and how it is labelled.** Bar verification observes the restatement itself:
raw and adjusted bars disagree exactly by the split. An announcement is a claim from a feed about
the future (or the morning): it can be late, missing, or filed under another ticker, and "nothing
announced" is an inference from absence. The two are never merged into one flag. Every record
carries `actions: ActionsEvidence { source: unchecked | bar_verified | announced,
bar_verified_through, announcement (retrieval id), applied_split (ratio, action id) }`
(`PrevClose.actions`, `DailyBaseline.basis`), and `observed_at` includes the retrieval time, so a
decision before the 07:00 fetch cannot see it. The engine does not yet grade the two kinds
differently; a consumer that needs the strong kind filters on `source`.

Each record's `observed_at` is the latest retrieval among the files it used (ns), so the engine's
leakage rule keeps a snapshot fetched after D away from a live decision on D. Ids carry the metric:
`provider_daily_volume_v1:{baseline|prev_close}:{SYM}:{D}`.

## 7. Requests, bytes, storage, cadence (*estimates*)

Assumptions: ~12,000 assets requested; ~10,000 bars per date; one Alpaca daily bar ≈ 100 B of
JSON (`{"c":1.25,"h":1.31,"l":1.18,"n":812,"o":1.2,"t":"2026-10-14T04:00:00Z","v":120500,"vw":1.2471}`
is 94 B); `limit=10000` bars per page across the request's symbols; stored compactly ≈ 75 B per bar
plus ~8 B per `no_bar` symbol.

| Item | Requests | Over the wire (JSON) | On disk | Cadence |
|---|---|---|---|---|
| Nightly raw (existing closes fetch, 22 dates) | **0 new** (~60 today: 60 chunks × 1 page, 200 × 22 = 4,400 bars) | 0 new (~22 MB today) | ~0.8 MB per new date written; usually 1–2 dates a night | daily, `maintain` |
| Raw backfill to 63 trading dates (inside `maintain`) | ~120 the first night (60 chunks × 2 pages: 12,600 bars > 10,000), then 0 | ~60 MB once | ~50 MB for 63 dates | automatic, only for missing dates |
| Raw steady state | — | — | **~50 MB** (63 × ~0.8 MB) | pruned nightly |
| Split snapshot set, watch list ~7,200 × 64 trading dates | ~72 (36 chunks × 2 pages), in 4 snapshots of ≤ 2,000 symbols | ~46 MB | ~35 MB per set; **5 sets kept ≈ 175 MB** | nightly, after the import |
| Announcements, watch list ~7,200 | ~72 per fetch (100 symbols per request, 1 page: a 2-day window holds a handful of actions) | < 1 MB | a few KB to ~100 KB per fetch (the full requested-symbol list dominates) | 07:00 and 09:15 ET, trading days |
| Lazy prefetch of new entrants | 0 (raw already covers every listed symbol; the next nightly set includes them) | — | — | — |

One snapshot whose range covers D's 45-date lookback and D serves every D in it, so the latest set
covers the last ~19 dates for replay. Older sets are deleted by whole set (`SPLIT_SETS_KEEP`).

Total new disk at steady state: **~50 MB raw + ~175 MB snapshots + ~50 MB a year of announcement
retrievals**, against ~0.6 GB per archived trade day.

## 8. Rate limits

- The plan limit is 10,000 requests/min (`MAINTAIN_PER_MINUTE` comment); the import's workers
  share one `RateLimiter` at `MAINTAIN_PER_MINUTE` (default 9,000).
- The closes/raw fetch and the backfill are sequential and unpaced (~60 and ~120 requests).
- The split set and the announcement fetches each pace themselves with their **own**
  `RateLimiter` at `MAINTAIN_SPLIT_PER_MINUTE` (default 200). Inside `maintain` they run after the
  import (split set) or at 07:00/09:15 ET when no import runs (announcements), so they never
  overlap the 9,000/min import; even if they did, 9,200 < 10,000. A set at 200/min takes ~22 s.
- The manual `alpaca-split-bars` is a separate process with its own limiter (`--per-minute`,
  default 200): the operator keeps it outside 21:15–~23:00 ET.

## 9. Compatibility check (required before the strict metric is used)

The pillar's numerator is our own count: SIP prints 04:00 → session end, BOATS excluded, kept by
the chart engine's condition policy (`market_events::conditions`). The denominator from §6 is
Alpaca's daily-bar `v`. They are not the same definition by construction:

- Alpaca applies separate condition rules to minute bars and daily bars; some conditions update
  daily volume but not OHLC (extended-hours and some late or out-of-sequence prints).
- Daily bars are New York calendar-day buckets; our session day starts at 04:00 and files the
  20:00 → 04:00 overnight (BOATS, not SIP) to the next date.

**The harness** (`market_providers::compat`, `swimport volume-compat --from --to [--sample 500]`,
offline, no credentials, no request): for symbol-days present both in an imported (final, never
fixture) archive day and in `daily-bars/raw/`, ours = Σ size of prints with `Trade::effect()`
updating volume whose session is that date's pre/regular/after; compared with `v`.

- Sample: `--sample` symbol-days spread over the dates, stratified by price (< $1, $1–$5, $5–$25)
  × volume tercile, taken round-robin across strata in a fixed hash order (re-runs pick the same
  symbol-days). Listing exchange is not a stratum yet (it needs `assets.json`; a follow-up).
- Report (JSON in `plans/volume-compat-<from>-<to>.json`): per symbol-day and in total, our volume
  **by session**, volume **by condition code** counted and not counted by our policy, volume
  outside the window, the distribution of `ours / v` (median, p5, p95, share within ±1/±2/±5 %),
  median per stratum, and the verdict. Exit code 0 only when compatible.
- **Run it only after the trade-identity fix (PR #51) has merged and the compared days were
  re-imported with it.** Before that, distinct FINRA TRF prints sharing `(D, id)` are dropped as
  duplicates, our count is low, and the check measures that bug. Every report carries this note.
- Include an early-close date once one is in the window (11-27).

**Decision rule (owner-accepted 2026-10-20):** **compatible** when ≥ 95 % of sampled symbol-days
are within ±2 % and the median is within ±2 %; fewer than 100 symbol-days is
`insufficient_sample`. **Incompatible** → the fallback the owner chose: the separately versioned
**provider-volume metric** (`provider_daily_volume_v1`, the id already on these records) with a
**matching live numerator** (today's prints filtered by Alpaca's daily-bar condition rules, over
Alpaca's day boundary), shown as its own measure, never one unlabeled "RVOL"; the strict metric
stays unavailable.

## 10. What this cannot provide

- **Same-time and recent-window RVOL** need intraday cumulative curves of prior sessions. Daily
  bars have none: those diagnostics stay UNKNOWN (`no_intraday_baseline`). Daily volume is never
  distributed across the day. (A separate route: curves from the archive's own trades, 21 days of
  in-band symbols, ≥ 10 sessions for the diagnostic; not part of this plan.)
- **Bar-verified live basis of D**: unknowable from daily bars before D's bar exists; live rests
  on the weaker announcement kind (§6).
- **Corporate actions other than splits**: spin-offs, cash dividends and mergers are not covered;
  name changes are recorded but only used to refuse (no rename chain walk; no instrument master).
- **Float, news**: separate sources.

## 11. Remaining open items

- The engine treats `bar_verified` and `announced` coverage alike; whether the dashboard should
  show the kind (it is on every record) is a UI decision for increment 4.
- Server wiring (increment 2b) reads these files; not done here.
- Unit-split response fields are unverified against a recorded response (none in the probes).
