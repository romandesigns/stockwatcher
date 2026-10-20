# Daily baseline and checked previous close: acquisition plan

Status: **plan + code (2026-10-19; owner decisions 2026-10-20; rework per the consolidated brief
§1.2 A–D, 2026-10-20)**, branch `feat/daily-baseline`. Raw daily bars are kept from the responses
the nightly job already fetches and backfilled to the retention window; `maintain` takes a
split-adjusted snapshot of the watch list after each import, keeps a corporate-action
announcement cache populated before the 04:00 ET SIP start and refreshed during the day, and warms
up intraday band entrants on request. The loader is wired into the server since 2026-10-20
(`crates/server/src/evidence.rs`: rebuilt when a raw date file, the split snapshots or the session
date's announcement checks change; live as of the wall clock, a replay as of its day's first
check). The server does not file warmup requests yet. Nothing here has called Alpaca from a
development machine: it runs on the VPS after merge. Numbers marked *estimate* come from the code and the bar shape, not from a measured run.

## 0. Owner decisions (2026-10-20)

1. **Live corporate-action evidence: yes** (runtime dependency on Alpaca's
   `/v1/corporate-actions`), under a versioned coverage policy (§6).
2. **Downloads approved**: raw backfill to the 63-date retention (~120 requests once), the watch
   list's split snapshot after the import (~72 requests a night, own 200/min limiter, 5 sets kept),
   announcement fetches per the coverage policy.
3. **Volume study thresholds**: ±2 % for ≥ 95 % of symbol-days (median within ±2 %) as an
   **engineering diagnostic, not a certification**; outside it, the fallback is the separately
   versioned `provider_daily_volume_v1` metric with a matching live numerator (§9).
4. **Defaults owner-accepted 2026-10-20** (engineering choices, not validated): lookback 45
   trading dates, raw retention 63, same-basis tolerance 0.2 %, volume/price reciprocity 1 %, watch
   band $0.10–$25.00 over the last 5 dates.

What it serves (`pillars-v1.md` §4.2, §4.4, `crates/stock-context/src/evidence.rs`):

- RVOL pillar: `DailyBaseline { sessions: [SessionVolume { date, volume, complete, comparable }],
  basis }` with at least 30 complete, comparable sessions before the evaluated date D, on D's share
  basis.
- Gain pillar: `PrevClose { date = previous trading day P, basis, actions_checked_through ≥ D,
  actions }`.

## 1. What already exists (and what was thrown away)

`swimport maintain` (VPS `archiver` service, daily at `MAINTAIN_AT` 21:15 ET) and
`swimport alpaca-daily-closes` call `GET /v2/stocks/bars?timeframe=1Day&feed=sip&adjustment=raw`
for every active US equity (`/v2/assets?status=active&asset_class=us_equity`, tradable or not), in
chunks of 200 symbols, `limit=10000`, following `next_page_token`. Range: the 21-trading-day window
plus the day before. Until this branch only `c` reached disk (`daily-closes.json`); `o h l v n vw`
were discarded.

## 2. Watch universe and intraday entrants

| Set | Definition | Size (*estimate*) | Why |
|---|---|---|---|
| **Raw universe** | every active US equity the assets endpoint lists | ~12,000 requested; ~7,000–11,000 return a bar a day | free: same responses; covers any later band entrant's history |
| **Watch list** (split snapshots, announcements) | raw close within **$0.10–$25.00** on **any of the last 5 trading dates** | ~7,000–7,500 | a stock becomes interesting by moving into the band; its evidence must exist that morning |

**Intraday entrants** (brief C). A stock that enters the band intraday on D without an in-band
close in the last 5 dates is not on D's watch list. It gets a lazy, bounded warmup:

1. The server (later, increment 2b) calls `market_providers::warmup::WarmupQueue::request(symbol,
   D, reason, now)` on the market-event path: O(1), `try_lock` (never waits; `Busy` → try on a
   later print), bounded (`Full` is counted), deduplicated per symbol and date.
2. A background task calls `WarmupQueue::flush(&archive)`, filing each request as
   `$ARCHIVE_ROOT/warmup/<D>/<SYMBOL>.json` (idempotent; the server and the maintainer are separate
   processes, so the inbox is files).
3. `swimport maintain`, on every announcement tick (§6), takes up to `WARMUP_MAX_PER_RUN` (50) open
   requests and fetches their raw daily bars over the retention window, a split snapshot (its own
   set) and an announcement check, then closes each request with its outcome
   (`<SYMBOL>.done.json`).

Until it is done, and if the history is short anyway, the symbol's evidence stays explicit:
`unavailable_coverage` / `insufficient_baseline` / `fewer_than_30_comparable`, never guessed. A
new listing correctly stays insufficient until it has 30 sessions.

## 3. Dates

The engine needs 30 complete, comparable sessions strictly before D, counted in **trading dates**
from `market_events::session`:

- Lookback: **45 trading dates** before D (`baseline::DEFAULT_LOOKBACK`), leaving 15 for early
  closes (not comparable), dates without a bar and uncovered dates. ~45 calendar days is only ~31
  trading dates and would fail the first holiday month.
- Raw retention: **63 trading dates** (`DAILY_BARS_KEEP_DAYS`), backfilled when missing.
- Calendar coverage: 2025–2027. A date outside it is never `comparable`.

## 4. Feed and adjustment

- **Feed `sip`** only.
- **`adjustment=raw` is always kept**: history as printed, mergeable across fetches.
- **`adjustment=split` is a separate, immutable snapshot**: Alpaca restates **prices and volumes**
  of every bar before a split onto the share basis at the moment of the request, so two fetches on
  either side of a split are on different bases and are never merged.
- **`asof` is not a corporate-action knowledge cutoff** (it maps renamed symbols). Never sent.
- **Never `adjustment=all`**: it folds dividends into the factor, and the volume would no longer be
  the reciprocal of the price.

## 5. Storage and commands

```text
$ARCHIVE_ROOT/
  daily-bars/raw/2026-10-15.json                      # RawDayBars, merged per date
  daily-bars/split/set-<cycle>-<retrieved>-<h>.json   # nightly SplitSnapshot set (5 kept)
  daily-bars/split/<retrieved>-<h>.json               # manual or warmup snapshot (own set)
  corporate-actions/<D>/<retrieved>-<h>.json          # ActionRetrieval for session D, append-only
  corporate-actions/ledger.json                       # every action id: first_observed, raw versions
  corporate-actions/late.jsonl                        # late discoveries, append-only
  warmup/<D>/<SYMBOL>[.done].json                     # warmup requests and outcomes
```

All files are schema 1 and written atomically. Raw `no_bar` = requested, nothing returned:
**covered, not zero**; a symbol in neither list is unknown. Split retention: nightly sets by count;
a manual/warmup snapshot is kept while not older than the oldest nightly set kept.

| Command | Requests | Writes |
|---|---|---|
| `swimport maintain`, nightly cycle | raw: none new; backfill: missing dates only (~120 once); split set: ~72 | raw files, one split set |
| `swimport maintain`, announcement ticks (§6) | ~72 per fetch + rename follow-ups; warmups bounded | one retrieval per fetch, ledger, late log, warmup outcomes |
| `swimport maintain --actions-once` | one fetch | as above |
| `swimport alpaca-daily-closes --from --to` | none new | raw files (manual backfill) |
| `swimport alpaca-split-bars …` (manual) | ⌈symbols/200⌉ × pages | one snapshot |
| `swimport volume-compat --from --to …` (offline) | none | `plans/volume-compat-<from>-<to>.json` |

Settings: `DAILY_BARS_KEEP_DAYS` (63), `MAINTAIN_SPLIT_SNAPSHOTS` (1), `MAINTAIN_SPLIT_PER_MINUTE`
(200), `SPLIT_SETS_KEEP` (5), `ACTIONS_FETCH` (1), `WARMUP_MAX_PER_RUN` (50). The announcement
schedule, window, pagination, retries and freshness are **not** settings: they are the versioned
coverage policy.

## 6. From bars and announcements to evidence (`market_archive::baseline::load`)

For symbol S and evaluated date D, P = the trading day before D, `as_of` = the decision time the
record is built for (nothing retrieved later is used).

### 6.1 Split consistency of the daily bars (brief B)

For a date x with raw bar R and split-adjusted bar A from **one** snapshot, the adjustment
multiplier is `f = A.c / R.c`, so `P_adj = P_raw · f` and, for a split adjustment, `V_adj =
V_raw / f` (`P·V` preserved). For a split of `old → new` shares `f = old/new`: a 1-for-10 reverse
split gives f = 10, $0.08 → $0.80, 1,000,000 → 100,000 shares; a 2-for-1 forward split f = ½; two
splits after x multiply. `split_consistency` checks exactly that: positive closes, equal trade
counts, `|A.v − R.v/f| ≤ max(1 share, 1 %)`; never exact float equality (adjusted prices are
rounded; same-basis ratios within 0.2 % snap to 1). Tested: forward, reverse, two intervening
splits, unchanged dates, zero-volume edges, rounding.

**This checks the consistency of the adjustment Alpaca supplied. It is not proof of coverage**
(an action the provider never applied is invisible to it), and it exists only after the fact:
**a final daily-bar adjustment never certifies an earlier live intraday decision.** Records keep
their basis date (`actions.basis_date`), `observed_at` (the retrieval time; a snapshot fetched
after D is invisible to a live decision on D) and source, so a replay labels them
`reconstructed_history`.

Bars give E = the latest date in P..=D with every date from P to E consistent and bars on E. When
E = D (replay of a past date) the record is `bar_verified`.

### 6.2 Announcement coverage (live; policy `corporate-actions-coverage-v1`, brief A)

**Endpoint.** `GET https://data.alpaca.markets/v1/corporate-actions` (the market-data host, the same
one as the bars; confirmed against the probes' recorded requests).

**Provider facts that shape the rules.** Arrival and processing can be delayed; requests are
organized by **process date**, not by ex/effective date; the default `data_quality=complete` can
omit early incomplete records; actions are filed under the **symbol in force** (report 02, F7); a
reverse split can carry an optional `new_symbol`. **An empty response is never proof of no
action.**

**The policy** (`crates/market-archive/policy/corporate-actions-coverage-v1.json`, embedded,
identified as `corporate-actions-coverage-v1@<sha256 prefix>` on every record; a change is a new
file and id):

| Rule | v1 value |
|---|---|
| request | `types=forward_split,reverse_split,unit_split,name_change`, `data_quality=all`, 100 symbols, `limit=1000`, every page (a chunk counts only when its last page had no token) |
| process-date window | D − 30 days … D + 10 days; actions selected by their own ex/effective date |
| retries | 5 attempts, 0.5 s → 30 s backoff (transient errors only) |
| schedule | at process startup; **03:30 ET** pre-session (retried every 5 min until the **04:00** SIP start if it failed); then every **30 min** from 04:00 to 20:00 ET, trading days |
| freshness | a check older than **90 min** at decision time is stale |
| rename follow-ups | counterpart tickers not requested are fetched in a second pass, ≤ 200 |
| absence inference | `no_known_action_provider_checked` **permits unchanged-share-basis calculations**, labelled as an assumption |

Each retrieval stores its request, per-chunk pages and completeness, the symbols covered, and every
record normalized **and verbatim** (`raw`), with the provider's quality flag when present (no
recorded response has one yet; the parser looks under `data_quality`, `quality`, `status`) and a
`complete` flag (every field the rule needs is present and not flagged incomplete). The ledger
keeps every action id's `first_observed` time and each raw version.

**Per-symbol coverage state** (`coverage_for`, `stock_context::CoverageState`). Usable retrievals
cover S (its chunk completed), contain the policy window around D, were made after P's last
session ended and at or before `as_of`. Then:

| State | When | Effect |
|---|---|---|
| `unavailable_coverage` | no usable retrieval (none made, chunk failed, wrong window, made later than the decision) | not checked through D |
| `stale_coverage` | latest usable one older than 90 min | not checked through D |
| `unresolved_action` | a relevant action that is incomplete, undated, a unit split, fractional, conflicting on the same date, or listed earlier but missing from the latest check; an incomplete rename; a rename counterpart not covered | not checked through D |
| `confirmed_action` | complete, consistent split terms in `(P, D]` (duplicates of the same terms count once; several dates multiply), or a complete rename | close × Π(old/new), prior volumes × Π(new/old); `source = announced_action`, ratios and action ids recorded; history read under the old ticker after a rename |
| `no_known_action_provider_checked` | a fresh, fully paginated check covering S (and every alias) with nothing relevant | **unchanged basis assumed**: `source = provider_checked_no_known_action`, `assumption = unchanged_share_basis_assumed`; **not verified absence** |

Incomplete evidence for one symbol never affects another (coverage is per symbol, per chunk).

**Identity.** The alias set starts with S; a name change into or out of S, or a reverse split whose
`new_symbol` is S, effective in `(P, D]`, adds the other ticker (the history symbol when it is the
old one). Splits filed under any alias count. Every alias must be covered by the latest check
(`rename_chain_not_covered` otherwise), which is why the fetch follows renames up: an old-symbol
query alone would lose the chain.

**What the engine can tell apart.** Every record carries `ActionsEvidence { source:
unchecked | bar_verified | announced_action | provider_checked_no_known_action, basis_date,
bar_verified_through, coverage, coverage_policy, announcement, applied_splits, assumption,
history_symbol }`. The engine still only checks `actions_checked_through ≥ D`; admission or a
stricter profile can require `source = bar_verified` or reject the assumption label.

The baseline uses an announcement only **on top of bars consistent through P** (44 of 45 dates
stay bar-checked; only the last step is announced). Bar verification through D wins when present.

**Late discoveries.** An action first seen (or seen with changed terms) after its effective date's
04:00 start is appended to `late.jsonl` with the ids of the evidence records of the effective date
through the discovery date (`provider_daily_volume_v1:{prev_close|baseline}:{SYM}:{D}`). Nothing is
rewritten: earlier records keep their `observed_at` and coverage state. Decision ids proper join
when decisions are logged (increment 2b).

### 6.3 Sessions

Only dates before D; missing is missing; a raw file fetched before the date's last session ended
is not covered; `comparable` = a full trading day in the calendar.

## 7. Requests, bytes, storage, cadence (*estimates*)

| Item | Requests | Over the wire | On disk | Cadence |
|---|---|---|---|---|
| Nightly raw (existing closes fetch) | 0 new | 0 new | ~0.8 MB per new date | nightly |
| Raw backfill to 63 dates | ~120 once | ~60 MB once | ~50 MB total | when dates are missing |
| Split set, ~7,200 × 64 dates | ~72 | ~46 MB | ~35 MB per set, 5 kept ≈ 175 MB | nightly after the import |
| Announcements, ~7,200 symbols | ~72 + follow-ups per fetch; ~34 fetches a trading day (03:30, then 04:00–20:00 every 30 min) ≈ 2,500/day | small (a 40-day process window holds a few hundred actions market-wide) | ~100–300 KB per retrieval (the covered-symbol list dominates) ≈ 5–10 MB a day | policy schedule |
| Warmups | ≤ 50 symbols per tick: ~1 raw + 1 split + 1 announcement request | < 1 MB | small | announcement ticks |

Announcement retrievals grow ~2–3 GB a year at that cadence; pruning retrievals older than the
raw retention (keeping the ledger and the late log) is a follow-up.

## 8. Rate limits

The plan limit is 10,000 requests/min; the import's workers share one limiter at 9,000/min. The
split set, the announcement fetches and warmups each pace themselves at
`MAINTAIN_SPLIT_PER_MINUTE` (200/min). The maintainer runs one thing at a time, so announcement
ticks never overlap the import; even if they did, 9,200 < 10,000. A full announcement fetch at
200/min takes ~25 s.

## 9. Volume study (brief D): `swimport volume-compat`

**Question.** How far is provider volume `Vp` (raw daily bar `v`) from `Ve`, the sum of eligible
trade sizes under the corrected engine policy (SIP prints 04:00 → session end, overnight/BOATS
excluded, one print per `PrintId` (PR #51), counted when `Trade::effect()` updates volume)? Trade
counts (`n` vs eligible prints) are compared separately.

**Report** (`market_providers::compat`, JSON in `plans/`, fixed seed `--seed`, default 20261020):

- `relative_error = |Vp − Ve| / Ve` for Ve > 0; zero-volume pairs kept apart by pattern; signed
  bias `(Vp − Ve)/Ve` (mean, median); absolute share error; median / p95 / p99 / max; share within
  ±1/±2/±5 %; trade-count ratio.
- **Per stratum**: price (< $1, $1–$2, $2–$20, $20–$25), activity tercile, relevance (day gain
  ≥ 10 %; in the pillars' $2–$20 band and up 10 % stands in for "small-cap gainer": float is not in
  the archive), session composition (extended-hours share > 25 %), sparse trading (< 100 eligible
  prints), early-close dates, split/rename examples (symbols with a ledger action in the range).
- By session and by condition code (volume counted and not counted by our policy), duplicates
  dropped.
- **Excluded / damaged sessions with reasons**: provisional live recording, no archived trades,
  fixture day, import incomplete (provenance `complete: false`), provider returned no bar, not in
  the raw bars, bar without volume, archive read error.
- **Effect on denominators and 5× classifications**: for `--denominator-symbols` (100) seeded
  symbols followed over every date, leave-one-out: each session's full-day `Ve` against the mean
  of the other sessions, once with our volumes and once with the provider's; the distribution of
  `mean(Vp)/mean(Ve)`, sessions near 5× (4–6× on our denominator), and **how many flip** at 5×.
  Full-day numerators and ≤ 21 sessions stand in for the intraday numerator and 30 sessions.

**Verdict** (`within_engineering_tolerance` / `outside_engineering_tolerance` /
`insufficient_sample` below 100 symbol-days): ≥ 95 % within ±2 % and the median signed error within
±2 % — an **engineering diagnostic, not a certification**. Outside → `provider_daily_volume_v1` as
its own versioned metric with a matching live numerator.

**What it needs.** Imported (final) archive days with every print stored **and** raw daily bars of
the same dates. The VPS holds the rolling ~21 trading days of trades (raw bars accumulate there once
this branch deploys; the backfill covers the window's dates); a development machine has only the
acquisition pilot (12 symbol-days under `C:\research\stockwatcher\data\archive`, no raw daily bars).
The nightly import stores every provider print (the identity bug dropped prints in the engine, not
in the archive), and the harness applies the corrected `PrintId` rule itself, so imported days are
valid whether imported before or after #51; live recordings are excluded (a recorder that
discarded prints before storage would be incomplete). Run on the VPS:
`swimport volume-compat --from <first window day> --to <last>`.

## 10. Corporate-action SSE feed: evaluation only (not implemented)

Alpaca also offers corporate-action events as a server-sent-events stream. Not used here; what
would have to be established first, none of it verified from this session:

- **Entitlement**: whether the stream is part of the market-data subscription in use or of another
  Alpaca product (Alpaca's server-sent event streams are documented mainly for its Broker API); a
  probe with the existing keys answers it.
- **Replay limits**: how far back a reconnect can resume (`since` / `until`, or an event id), and
  whether events older than the limit are lost; the REST cache above would stay the source of truth
  and the stream only shorten latency.
- **Cursor and dedup**: resume from the last event id after a disconnect; events would be deduped by
  action id and version (the ledger already keeps raw versions), and a gap in the cursor would
  mark coverage stale rather than "no action".
- **Benefit**: minutes instead of ≤ 30 min to see a late-processed action; the cost is a second
  long-lived connection beside the trade stream. Worth a probe once the REST cache has run for a
  few weeks and the late log shows how often actions arrive after their effective date.

## 11. What this cannot provide

- Same-time and recent-window RVOL (intraday curves): UNKNOWN (`no_intraday_baseline`); daily
  volume is never distributed across the day.
- Verified absence of an action: `no_known_action_provider_checked` is an assumption by design.
- Spin-offs, cash dividends, mergers: not covered.
- Float, news: separate sources.

## 12. Remaining open items

- Server wiring (increment 2b): call `WarmupQueue::request` on band entry and `flush` from a task;
  read the evidence files; log decisions so late actions can name decision ids.
- The engine does not yet grade the evidence kinds; admission policy to decide.
- Unit-split fields and the provider quality flag are unverified against a recorded response.
- Retention of announcement retrievals.
