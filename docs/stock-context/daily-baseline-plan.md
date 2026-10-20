# Daily baseline and checked previous close: acquisition plan

Status: **plan + first code (2026-10-19)**, branch `feat/daily-baseline`. Raw daily bars are now
kept from the responses the nightly job already fetches; split-adjusted snapshots are an opt-in
command nobody has run; the loader exists and is tested but is **not wired into the server**.
Nothing here has called Alpaca. Numbers marked *estimate* come from the code and the bar shape, not
from a measured run.

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

**Lazy prefetch for new band entrants**: a symbol that enters the band during the session (first
in-band print) and is not in the latest snapshot is queued and fetched in one bounded call after the
session (≤ 200 symbols per request, so a day's entrants are one or two requests). It cannot help
that day's live decisions (§6: D's basis is unprovable before D's bar exists); it makes the symbol
complete for replay and research of that day onward. Raw bars need no prefetch: the raw universe
already includes every listed symbol. A new listing has no history and correctly stays
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
- **`adjustment=split` is a separate, opt-in snapshot.** Alpaca's split adjustment restates **both
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
| `swimport maintain` (unchanged schedule) | none new | + `daily-bars/raw/*`, prune to `DAILY_BARS_KEEP_DAYS` |
| `swimport alpaca-daily-closes --from --to` (existing) | none new | + `daily-bars/raw/*` (the one-off backfill) |
| `swimport alpaca-split-bars --from --to (--symbols A,B \| --symbols-file F) [--per-minute 200]` (**new, opt-in**) | ⌈symbols/200⌉ × pages | one `daily-bars/split/*.json` |

The split command is bounded in code (≤ 2,000 symbols, ≤ 120 calendar days, refused before any
request) and only accepts completed dates. It needs `ALPACA_API_KEY` / `ALPACA_API_SECRET` (or
`APCA_API_KEY_ID` / `APCA_API_SECRET_KEY`) like the others.

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

**Consequence, stated plainly:** daily bars can establish D's basis only after D's bar exists, in
a snapshot fetched after D. So these inputs make the gain and RVOL pillars **checkable for replay
and research of past dates, and still UNKNOWN live**. The night-of-P snapshot gives
`actions_checked_through = P` (truthful, insufficient). Closing that gap needs a source that can
vouch for "no split effective on D" before D opens (§10, question 1).

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
| Raw backfill, 63 trading dates (`alpaca-daily-closes`) | ~120 (60 chunks × 2 pages: 12,600 bars > 10,000) | ~60 MB | ~50 MB for 63 dates | once |
| Raw steady state | — | — | **~50 MB** (63 × ~0.8 MB) | pruned nightly |
| Split snapshot, watch list ~7,200 × 64 trading dates | ~72 (36 chunks × 2 pages), in 4 runs of ≤ 2,000 | ~46 MB | ~35 MB per full set | proposed nightly after `maintain`, **not scheduled** |
| Lazy prefetch of new entrants | 1–2 per day | < 1 MB | in the next snapshot | after the session |

Snapshot retention (not implemented): keep the latest full set plus any snapshot cited by a stored
decision log; ~5 sets ≈ 175 MB. One snapshot whose range covers D's 45-date lookback and D serves
every D in it, so the latest set covers the last ~19 dates.

Total new disk at steady state: **~50 MB raw + ~35–175 MB snapshots**, against ~0.6 GB per archived
trade day.

## 8. Rate limits

- The plan limit is 10,000 requests/min (`MAINTAIN_PER_MINUTE` comment); the import's workers
  share one `RateLimiter` at `MAINTAIN_PER_MINUTE` (default 9,000).
- The closes/raw fetch is sequential and unpaced (~60 requests, before the import starts).
- The split command paces itself with its **own** `RateLimiter` (`--per-minute`, default 200).
  Two processes share no limiter, so the operator keeps the sum under the plan: 9,000 + 200 leaves
  800/min of margin even if it overlaps the import. Recommended slot: after the nightly import
  finishes (it logs `maintain ok`), not 21:15–~23:00 ET. A full set at 200/min takes ~25 s.
- Scheduling the snapshot inside `maintain` (after the import, same limiter) is the clean end
  state; it was left out so nothing new runs against the API without the owner's decision.

## 9. Compatibility check (required before the strict metric is used)

The pillar's numerator is our own count: SIP prints 04:00 → session end, BOATS excluded, kept by
the chart engine's condition policy (`market_events::conditions`). The denominator from §6 is
Alpaca's daily-bar `v`. They are not the same definition by construction:

- Alpaca applies separate condition rules to minute bars and daily bars; some conditions update
  daily volume but not OHLC (extended-hours and some late or out-of-sequence prints).
- Daily bars are New York calendar-day buckets; our session day starts at 04:00 and files the
  20:00 → 04:00 overnight (BOATS, not SIP) to the next date.

**The check**, bounded and with no new requests: for symbol-days present both in the archive's
final import (21-day window) and in `daily-bars/raw/`, compute ours = Σ size of prints counted by
our policy, 04:00 → last session end, and compare with `v`.

- Sample: ~500 symbol-days over 10 dates, stratified by price ($0.10–$1, $1–$5, $5–$25), volume
  tercile and listing exchange, including an early-close date once one is in the window.
- Report **by session** (pre / regular / after: our volume per session vs the day's `v`) and **by
  condition code** (volume of prints carrying each code, and whether our policy counted it), with
  the distribution of `ours / v`: median, p5, p95, share within ±1 % and ±5 %.
- **Blocked on the trade-identity fix** (branch `fix/trade-identity`, another agent): distinct
  FINRA TRF prints sharing `(D, id)` are dropped as duplicates today, so our count is low and the
  comparison would measure that bug. Run it only on data imported after the fix lands.

Decision rule (proposed, the owner decides): **compatible** if ≥ 95 % of sampled symbol-days are
within ±2 % with no condition or session carrying a systematic bias; the tolerance is then
recorded in the policy. **Incompatible** otherwise, and then one of:

1. keep the strict metric unavailable (RVOL stays UNKNOWN), or
2. define an explicitly versioned **provider-volume metric** (`provider_daily_volume_v1`, the id
   already on these records) with a **consistent live numerator**: today's prints filtered by
   Alpaca's daily-bar condition rules, over Alpaca's day boundary, as a separate measure next to
   ours, never one unlabeled "RVOL".

## 10. What this cannot provide

- **Same-time and recent-window RVOL** need intraday cumulative curves of prior sessions. Daily
  bars have none: those diagnostics stay UNKNOWN (`no_intraday_baseline`). Daily volume is never
  distributed across the day. (A separate route: curves from the archive's own trades, 21 days of
  in-band symbols, ≥ 10 sessions for the diagnostic; not part of this plan.)
- **Live share basis of D** (§6): unknowable from daily bars before D's bar exists.
- **Corporate actions other than splits**: spin-offs, cash dividends, symbol changes and mergers
  are not covered; a rename breaks the symbol-keyed series (no instrument master yet).
- **Float, news**: separate sources.

## 11. Open questions for the owner

1. Live gain/RVOL: accept them as UNKNOWN live (replay/research only), or add an announcements
   source (Alpaca corporate-actions API, `research/foundation` `corporate.rs`) to vouch for "no
   split effective on D" before the open? That is a new runtime dependency and a new claim to
   validate.
2. Approve running the backfill (`alpaca-daily-closes` over ~63 trading dates, ~120 requests) and
   the split snapshot (~72 requests a night) on the VPS, and whether the snapshot moves into
   `maintain`.
3. The compatibility thresholds (±2 %, 95 %) and, if incompatible, option 1 or 2 of §9.
4. Engineering numbers chosen here, not validated: lookback 45, retention 63, factor tolerances
   (0.2 % same-basis, 1 % volume/price agreement), watch-list band $0.10–$25 over 5 dates.
