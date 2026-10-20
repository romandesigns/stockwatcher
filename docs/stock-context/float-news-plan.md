# Float and news evidence: forward collection plan (float-news v1)

Status: **built and tested on recorded probe responses (2026-10-19), not deployed.** The
collector is `swimport evidence` (`crates/market-providers/src/import/evidence_collect.rs`).
The records live under `$ARCHIVE_ROOT/evidence/` (`crates/market-archive/src/evidence.rs`).
The loader into the context engine is `crates/market-archive/src/evidence_load.rs`. Nothing
calls a provider from the server.

Owner direction (Five Pillars brief §1.1 B):

- FMP float and Alpaca/Benzinga news are the initial **forward** sources.
- Store float observations and news versions from collection onward.
- Never backfill today's float into old episodes.
- Never attach a final edited story to its original publication time.

## 1. Endpoints and entitlement evidence

Every claim below comes from the account's own probes: research report 02
(`C:\research\stockwatcher\reports\02-endpoint-capabilities.md`, probes 2026-10-19 03:39–03:50 UTC).
The response bodies used by the tests are copied into
`crates/market-providers/tests/fixtures/evidence/`. They contain no credentials.

| Use | Endpoint | Probe | Result | What it establishes |
|---|---|---|---|---|
| Float, per symbol (fallback, `EVIDENCE_FLOAT_MODE=per-symbol`) | FMP `/stable/shares-float?symbol=` | #2, AAPL and SGRX | 200, one row | `floatShares`, `outstandingShares`, `freeFloat`, `source` (a SEC document URL), `date` = FMP's **refresh time in UTC** (SGRX refreshed 8 min before the probe) |
| Float, bulk (**used**, default) | FMP `/stable/shares-float-all?page=&limit=` | #3, page 0, limit 5; 2026-10-20, page 1, limit 1000 | 200; 1,000 rows | Same row shape; later pages are allowed on this account. Falls back to per-symbol on the first refusal |
| Float history | FMP `/api/v4/historical/shares_float` | #4 | **403** "legacy users only" | No point-in-time float. Stable guess `/stable/historical-shares-float`: 404 (#5). Finding F1 |
| News (**used**) | Alpaca `GET data.alpaca.markets/v1beta1/news` | #29 (2023), #30 (2025) | 200 | `created_at`/`updated_at` RFC 3339 UTC; 3+ years back; edits visible (`updated_at` later; body is the final state, F9); `next_page_token` pagination; data API limit header `X-Ratelimit-Limit: 10000`/min |
| News, FMP | FMP `/stable/news/stock` | #15–18 | 402 older than ~11 months | Not used. `publishedDate` is US/Eastern with no offset (F5), and there is no retrieval or edit time |
| News stream | Alpaca news WebSocket | 2026-10-20: connect, auth, subscribe `["*"]` | connected, authenticated, subscribed | Entitled on this account. Still not used: polling is what is built and tested (section 3) |

The probes did not establish:

- **FMP's request limit for this plan.** FMP sent no rate-limit header. The plan name was
  deliberately not inferred.
- **A symbol-less Alpaca news query.** Every probe passed `symbols=AAPL`. Alpaca documents
  `symbols` as optional.
- **Whether Alpaca's `start`/`end` filter on `created_at` or `updated_at`.**

The collector is built so that each of these is either a setting or harmless (sections 3 and 4).
**Before deploying**, run one manual request of each kind (section 9).

## 2. Universe

This is the same split the replay importer uses, in two parts:

1. **The watch universe.** Symbols whose official close was inside the production band
   ($0.10–$25, `PRICE_BAND_MIN`/`PRICE_BAND_MAX`) on any of the last `EVIDENCE_UNIVERSE_DAYS`
   (5) trading dates in `daily-closes.json`. The archiver already rewrites that file every
   evening. The archiver's 21-day window plans about 150k symbol-days, about 7,000 symbols a
   day, so this universe is roughly 7,000 symbols.
2. **Lazy warmup of intraday entrants.** A symbol trading inside the band today that the
   universe does not hold, and that was not fetched today, is fetched once.
   - Entrants come from today's live quotes (`<day>/live/quotes.json`). The recorder already
     writes that file.
   - The collector reads the file. It never asks the server, so the server makes no provider
     call and needs no new endpoint.
   - Warmup is capped at `EVIDENCE_FLOAT_WARMUP_MAX` (500) per ET date.
   - Warmups run before the 07:00 refresh too, so the first premarket movers get a float as
     soon as they show up.

News needs no universe. One unfiltered query covers every symbol Benzinga tags, so its cost is
independent of the watch list.

## 3. Cadence

| Source | When | Bound |
|---|---|---|
| Float refresh | Once per trading day at `EVIDENCE_FLOAT_AT` (07:00 ET), universe minus symbols already fetched that ET date | Each symbol at most once per ET date, restarts included: the day's stored records say what is done. Spread over ticks at half a minute's budget per 15 s tick, so news never waits on it |
| Float warmup | Every tick (15 s) | ≤ 500 per ET date; queued ahead of the refresh |
| News, forward | Every `EVIDENCE_NEWS_POLL_SECS` (60 s) 04:00–20:00 ET on trading days; every `EVIDENCE_NEWS_IDLE_POLL_SECS` (300 s) otherwise | Each search runs from the previous complete search's end minus a 5 min overlap to now minus 30 s (stories indexed a little late are still found) |
| News, edits | Every `EVIDENCE_NEWS_EDIT_EVERY_SECS` (15 min), a trailing `EVIDENCE_NEWS_EDIT_WINDOW_MINS` (3 h) | Edits seen in the probes came 1–2 min after creation (09:13:42 → 09:15:03; 17:45:36 → 17:46:53); 3 h is generous. An edit made after the window is missed until a stream exists |
| News, first start or gap > 3 days | One search of the last 25 h | Covers the pillar's 24 h freshness window at once. Stories found are observed now, not at publication |

**Streaming.** Alpaca's news WebSocket would replace the forward poll and catch every edit as
an update message. It is not used because no probe opened it on this account. It is also a
second stream connection beside the live market-data stream, and the per-account connection
limit for it was not checked. The next step is one manual connect-and-subscribe check. The
records would not change: the stream would write the same `NewsVersionRecord`s and a coverage
record per connected interval.

**Not per event.** Nothing here is triggered by a print. The server only ever loads records.
The heaviest trigger, a symbol entering the band, costs one FMP request per symbol per day.

## 4. Requests per day (estimates) and rate coordination

| Item | Requests per trading day | Peak per minute |
|---|---|---|
| Float refresh, per-symbol | ≈ 7,000 (one per universe symbol) | 120 (`EVIDENCE_FMP_PER_MINUTE`): about 60 min, done by about 08:00 ET |
| Float refresh, bulk (if pages > 0 are allowed) | all symbols ÷ 1,000 per page, about 25–80 | same pacer |
| Float warmup | ≤ 500, typically tens | same pacer |
| News forward polls | 960 active + 96 idle ≈ 1,060, plus about 50 extra pages (about 2,500 stories ÷ 50 per page, an unverified volume) | ≤ 60 (`EVIDENCE_ALPACA_PER_MINUTE`) |
| News edit re-polls | 96 × (3 h of stories ÷ 50) ≈ 96 × 7 ≈ 650 | same pacer |

**Alpaca.** The account limit is 10,000 requests a minute for the data API (header on every
probe).

- The nightly importer (`swimport maintain`, 21:15 ET) takes up to 9,000 a minute.
- This collector takes at most 60 a minute, so together they stay under 10,000.
- The live market-data WebSocket does not count against REST.
- No shared limiter is needed while the sum stays below the account limit. If the importer's
  ceiling is raised past 9,940, the two must share one budget.
- Integration point: `MAINTAIN_PER_MINUTE` + `EVIDENCE_ALPACA_PER_MINUTE` ≤ 10,000.
  `batch::RateLimiter` is process-local, so sharing it across processes would need a
  file-based token or both jobs in one process.

**FMP.** No other production job calls FMP: `fmp-bars` is a manual comparison tool.

- The plan's per-minute and per-day caps are unknown (section 1).
- 120 a minute is a conservative default. A plan with a daily cap below about 7,500 cannot run
  the per-symbol refresh: that needs bulk mode, or a smaller universe.

**Backoff.** Both clients use the importers' `get_with_retry`:

- Retries: 5 attempts, exponential from 0.5 s up to 30 s, honouring `Retry-After`, for 408,
  429, 500, 502, 503 and 504.
- A 401, 402 or 403 is a plan refusal and is never retried. FMP float pauses for the rest of
  the ET date. A news search reports it and claims no coverage.
- A float symbol that exhausts its retries is not asked again that ET date.
- The pacers apply to every page, retries included.

## 5. Storage and retention

`$ARCHIVE_ROOT/evidence/{float,news,news-coverage}/YYYY-MM.jsonl`, append-only JSON lines (schema
`v: 1`). Writes are whole lines followed by `sync_data`. A torn final line from a crash is skipped
on read, and the next append starts a new line.

| Record | One per | Size (approx.) | Per day | Per year |
|---|---|---|---|---|
| `FloatRecord` | retrieval | 450 B | ≈ 3.2 MB | ≈ 0.8 GB |
| `NewsVersionRecord` | new version (first sighting) | 700 B | ≈ 2 MB at 2,500 stories (unverified volume) | ≈ 0.5 GB |
| `NewsCoverageRecord` | complete search | 350 B | ≈ 0.4 MB | ≈ 0.1 GB |

**Retention.** These records are kept forever. They are the only history of what was known
when, and none can be refetched: float is current-only, and news returns the final state only.

- The rolling 21-day window never prunes them: `Archive::days` lists only date-named
  directories.
- Closed months can later be zstd-compressed in place (about 10× smaller). That is not done
  yet.
- Each record keeps:
  - **Float:** the SHA-256 of the raw response body. The body itself is not stored.
  - **News:** the headline (a title, needed by the classification rules) and SHA-256 hashes of
    headline and summary. Never summary or body text: `include_content=false`.

**Versions.**

- **Float:** every retrieval is a record. An unchanged figure is still a new record: it is a
  new observation time.
- **News:** a version is a distinct (`updated_at`, headline hash, summary hash, symbols).
  - A re-fetch of the same version adds nothing.
  - An edit adds a record with a later `first_observed_at`.
  - The same headline under a new story id (a recycled story) is its own story.
  - Nothing is deduplicated across stories.

## 6. What each record can and cannot establish (loader rules)

The loader keeps one invariant: **`observed_at` is our first-seen time, never publication
time.** The engine joins evidence as of each cutoff, so neither a later edit nor today's float
can reach an earlier decision. Both are tested.

### Float (`float-rules-v1`)

- **Can establish:** what FMP stated as the public float at our retrieval time, with FMP's
  cited document when given.
- **Cannot establish:**
  - The date the float was true. FMP's `date` is its refresh time. `effective_as_of` stays
    empty, flag `no_effective_date`.
  - That the issuer's float changed on a given day. A changed figure between two daily
    retrievals is flagged `changed_since_previous_retrieval`, and that is all it means.
  - Any historical float. Never backfilled: old episodes stay `no_float_evidence`.
- **Never used as float:**
  - `outstandingShares`.
  - A row whose float equals outstanding (`float_equals_outstanding`, e.g. SGRX 799,914 and
    799,914), because it cannot be told apart from a substitution.
  - A float above outstanding (`float_exceeds_outstanding`).
  - An empty response (`no_row`).
- **Kept:** a zero float. The engine reports it as `nonpositive_float`.
- **Freshness:** the pillar goes stale 24 h after the retrieval (pillars-v1). A fresh retrieval
  does not prove a fresh provider figure.

### News (`news-rules-v1`)

One `NewsItem` per tagged symbol per version:

- `id` = `news:alpaca:<story id>`.
- `version` = order of first sighting. Consecutive identical versions collapse.
- `published_at` = the story's `created_at`, for freshness only.
- `observed_at` = `first_observed_at`.

| Field | Rule |
|---|---|
| `event_type` | First category whose phrase occurs as whole words in the lowercased headline, in this order: `regulatory_decision` (fda, approval, clearance, crl, …), `clinical_result` (phase 1/2/3, topline, trial, …), `earnings`, `guidance`, `financing` (offering, private placement, registered direct, warrants, …), `corporate_action` (reverse split, merger, acquisition, delisting, …), `commercial_agreement` (agreement, partnership, contract, license, …); else `other`. Lists in `evidence_load::EVENT_RULES` |
| `relevant` | The story tags the symbol and either tags at most 3 symbols, or names the ticker in its headline. Roundups ("Top 10 Trending Stocks", 10 symbols) are not relevant |
| `substantive` | `event_type ≠ other` and the headline matches no screen/commentary pattern (`NOISE_RULES`: whale alerts, movers, price target, …) |
| `credible_source` | Provider `alpaca`, source `benzinga`: credible for reporting that a story was published, not for its claim |
| `authenticity` | `ReportedUnverified` always. Issuer or regulator confirmation (8-K, press wire, FDA) is a later enrichment |
| `favorability` | `Undetermined` always. No NLP or LLM. Adverse news still passes the pillar |

- **Can establish:** that a Benzinga story tagged with the symbol existed by our first
  sighting, with its stated creation time.
- **Cannot establish:**
  - When a backfilled story was first knowable. Our first search of a past window observes
    its stories *now*.
  - Any version we never saw. Alpaca returns the final state only (F9).
  - That the reported event happened.

### Coverage

A complete search (every page fetched) writes one coverage record: provider, sources, scope
(`all` or a symbol list), query window, completion time.

- The loader turns records into one `NewsCoverage` per symbol.
- Overlapping searches join into a run: from the run's start through the furthest end seen,
  observed at each completion.
- `coverage_min_interval_secs` thins this for memory. It only ever shortens what a cutoff
  sees, never lengthens it.
- A failed or partial search claims nothing, so the news pillar stays UNKNOWN for that window.
- Writes are ordered stories first, then coverage. A crash in between leaves UNKNOWN, never a
  false FAIL.

## 7. The coverage lag (needs a decision)

pillars-v1 FAILs the news pillar only when a coverage record observed by the cutoff has
`covered_through ≥ cutoff`. An honest record cannot meet that:

- A search covers through its request time and completes after it.
- A stream is likewise always behind its last message.

So with honest records the v1 news pillar can PASS or be UNKNOWN, never FAIL. The acceptance
test that FAILs uses a record that claims coverage until 17:00 while observed at 09:31.

This branch adds an **optional** `news.coverage_max_lag_secs` to the policy schema (default 0):

- `pillars-v1.json` is byte-identical, so its hash, tag and behaviour are unchanged.
- The engine schedules re-decisions when a coverage window opens (start + 24 h) and when it
  closes (end + lag).

With a lag of 120 s and a 60 s poll, a symbol with no story is:

- FAIL (`no_matching_news_in_covered_sources`) while the latest search ended at most 120 s
  before the cutoff and began at least 24 h before it;
- UNKNOWN before that search completed, and again once its end is older than the lag (tested).

**Decision for Roman:** adopt a `pillars-v2` with a lag (120 s is the proposal: two missed polls
before UNKNOWN), or keep v1 and accept that "no news" stays UNKNOWN in forward runs.

## 8. Integration points (not done here)

- **Deployment.** A compose service `evidence` (same image, same volume, same `.env`) running
  `swimport evidence`. `infra/vps/` is not edited on this branch.
- **Server (increment 2b).** Load `LoadedEvidence::load(archive, symbols, since, 300)` into each
  market's `Engine`. Reload by mtime like `daily-closes.json`, appending only records newer than
  the last load. Use separate engines for live and replay.
  - Memory: per-symbol coverage for 3,000 symbols at a 300 s thinning is about 600k small
    records a day.
  - A market-wide coverage list in `EvidenceStore` would make this one list. That is a
    stock-context change for later.
- **Configuration reference.** The `EVIDENCE_*` variables are documented on `EvidenceConfig`.
  Copy them to `docs/reference/configuration.md` when the service is deployed.
- **Concurrent work.** No edits to `import/maintain.rs`, `batch.rs` or `corporate.rs` (PR #53),
  or to the recorder and archive coverage code (PR #52). `price_band_from_lookup` and
  `DailyCloses` are only read.

## 9. Before deploying

Done 2026-10-20 (owner-approved, one request each, keys never printed):
page 1 of `shares-float-all` returned 1,000 rows, so `bulk` is now the default; FMP exposes no
rate-limit headers, so its plan limits stay unknown, which bulk mode (~10 requests a day for the
whole market) makes moot; a news request without `symbols` returned 50 Benzinga stories for 62
symbols with a next page; the news stream connected, authenticated and subscribed to `*`. The
coverage lag is pillars-v2's 120 s (PR #58).

Original checklist:

1. One FMP request to `shares-float-all?page=1&limit=1000`. If it answers 200, set
   `EVIDENCE_FLOAT_MODE=bulk`.
2. Confirm the FMP plan's per-minute and per-day limits.
3. One Alpaca news request without `symbols`, to confirm it returns stories for many symbols.
4. One manual connect to Alpaca's news stream, to decide on streaming (section 3).
5. Decide on the coverage lag (section 7).
