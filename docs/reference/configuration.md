# Configuration reference

Every environment variable the server, the archiver and the clients read, in one place. The
source of truth is the doc table next to each reader (linked per section); update both when a
variable changes. Secrets go in the gitignored `.env` on each host, never in the repo, a URL or a
public build variable (`docs/CONVENTIONS.md`).

Production (`~/apps/stockwatcher/.env` on the VPS, shared by the `server`, `archiver` and
`evidence` compose services) sets `STOCKWATCHER_API_TOKEN`, `RUST_LOG`, `MARKET_DATA_MODE`,
`REPLAY_START=09:30`, `SIM_AUTOFIXTURE=0` and the Alpaca keys, and for the Ross Panel
`FMP_API_KEY` and `STOCK_CONTEXT=1`. The image adds `HOST=0.0.0.0`, `STATIC_DIR=/app/web`,
`DATA_DIR=/app/data`, `ARCHIVE_ROOT=/app/data/archive` and `LOG_FORMAT=json`.

## Server basics (`crates/server/src/config.rs`, `main.rs`)

| Variable | Default | Meaning |
|---|---|---|
| `STOCKWATCHER_API_TOKEN` | unset (auth off, logged as a warning) | ≥ 32 chars. Guards `/api/*` (not `/api/health`) and `/ws` (token in `hello`) |
| `PORT` | `8790` | HTTP port |
| `HOST` | `127.0.0.1` | bind address; the image sets `0.0.0.0` |
| `STATIC_DIR` | `./apps/web/dist` | the built web app the server serves |
| `DATA_DIR` | `./data` | `updates/` (desktop installers, served at `/updates/*`), `settings/` (synced settings) and `alerts/` (price alerts' Activity `history.jsonl` and the phones registered for push, `push-devices.json`) live here |
| `RUST_LOG` | `info` | tracing filter |
| `LOG_FORMAT` | pretty | `json` in the image |

## Price alerts: phone push (`crates/server/src/alerts/push.rs`)

| Variable | Default | Meaning |
|---|---|---|
| `EXPO_ACCESS_TOKEN` | unset | **Secret.** Sent as a bearer token to Expo's push service; needed only when the EAS project has "enhanced push security" on. Never logged. Unset works for a project without it |
| `EXPO_PUSH_BASE` | `https://exp.host/--/api/v2` | Expo's push API (`/push/send`, `/push/getReceipts`). Point it at a stub to test locally; an unreachable one only costs the phone notification (the alert still reaches the app, and Activity says `push_failed`) |

## Market data (`crates/market-providers/src/config.rs`, `crates/server/src/market.rs`)

| Variable | Default | Meaning |
|---|---|---|
| `MARKET_DATA_MODE` | `replay` | The market a client gets when it names none (`hello.source`, `?source=`). Both markets always run: replay on the archive, live whenever the Alpaca keys are set (`infra/vps/SETUP.md` §8) |
| `PRICE_BAND_MIN` / `PRICE_BAND_MAX` | `0.10` / `25.00` | the candidate price band (inclusive); the archiver uses the same pair for its import |
| `ARCHIVE_META_CHECK_SECS` | `60` | how often `assets.json` / `daily-closes.json` are re-checked (0 = only on a day switch) |

**Replay only**

| Variable | Default | Meaning |
|---|---|---|
| `REPLAY_DAY` | latest ready archived day | `YYYY-MM-DD`; unset in production so a restart lands on the newest day |
| `REPLAY_START` | `10:40` | ET wall time the clock starts at (`HH:MM[:SS]`); production uses `09:30` |
| `REPLAY_SPEED` | `10` | `1`, `2`, `10`, `60`, … or `max` |
| `REPLAY_AUTOPLAY` | `1` | start playing immediately |
| `REPLAY_FOLLOW_LATEST` | `0` | switch the running replay to a newly imported latest day (resets the paper account) |
| `SIM_AUTOFIXTURE` | `0` (`bun dev` sets `1`) | write a labelled FIXTURE day when the archive is empty |
| `SIM_FIXTURE_DAY` | `2026-10-15` | that day |
| `SIM_FIXTURE_SYMBOLS` | `SMPL,FXLO,FXMD,FXHI` | its symbols |

**Live only**

| Variable | Default | Meaning |
|---|---|---|
| `ALPACA_API_KEY` / `ALPACA_API_SECRET` | unset | also accepted as `APCA_API_KEY_ID` / `APCA_API_SECRET_KEY`. Without them live mode reports `not_configured` |
| `ALPACA_STREAM_BASE` | `wss://stream.data.alpaca.markets` | provider-sim is `ws://127.0.0.1:8791` |
| `ALPACA_STREAM_FEED` | `sip` | `sip`, `iex`, `boats`, `overnight`, or `test` (Alpaca's fake stream, labelled fixture) |
| `ALPACA_STREAM_NIGHT_FEED` | `boats` | With the `sip` day feed: the feed during the overnight session (20:00–04:00 ET before a trading day), when SIP carries nothing. The stream reconnects on the other feed when the session changes (one connection, taking turns). `overnight`, or `none` for SIP around the clock. Needs the account's BOATS entitlement; without it the night connection is refused and the link shows down until 04:00 |
| `LIVE_SYMBOLS` | `*` | `*` (whole market) or a comma list |
| `LIVE_RECORD` | `1` | record every event the live stream delivers into the archive itself, `ARCHIVE_ROOT/<trading date>/live/` (a provisional day: `crates/market-archive/src/live.rs`, `crates/market-providers/src/recorder.rs`). The nightly import replaces it with the complete day. `0` turns recording off; history already in the archive is still loaded |
| `LIVE_RECORD_DAYS` | `5` | days **with a recording** kept in `live/` when no import replaced them (newest N such directories, at least 2; never the day being recorded). Older ones are moved to `live-original/` (not deleted) and fall under `LIVE_ORIGINAL_*`. Shares `ARCHIVE_MIN_FREE_MB` with the archive |
| `LIVE_ORIGINAL_DAYS` | `5` | **completed trading dates** of preserved originals (`ARCHIVE_ROOT/live-original/<day>/`, receipt-order recordings the import replaced) always kept locally: the last N trading dates before the day being recorded, plus the newest N originals. At least 5 (fewer is refused at startup). Past the window an original is deleted only when unpinned **and** durably copied (`crates/market-archive/src/retention.rs`); otherwise it stays. ~0.8 GB a sealed whole-market day (estimate), ~4 GB for five. Not touched by the rolling window or `ARCHIVE_RETENTION_DAYS`. Changed 2026-10-19 within PR #52: it first counted original directories and allowed `0` |
| `LIVE_ORIGINAL_DURABLE_DIR` | unset (off) | durable research store. Each sealed original is copied to `<dir>/<day>/` with its coverage records and a verified `durable-copy.json`. The directory must already exist; a missing one is "unavailable" (originals kept, `retention_at_risk`). Unset: nothing past the window is ever deleted. Production: `/durable/originals`, the Pi's NFS share (set in `infra/vps/docker-compose.yml`; setup in `infra/vps/SETUP.md` section 9). Point it at a subdirectory that exists only on the mounted share: an unmounted mount point is an empty directory, which would otherwise count as available |
| `LIVE_ORIGINAL_MIN_HEADROOM_MB` | `10240` | free space under this marks `plans/retention-status.json` `retention_at_risk` (with a warning log). Reports only: protected originals are never deleted to make room |
| `LIVE_HISTORY_PRELOAD` | `1` | after a start, load every symbol's day from the archive in the background, busiest first; `0` loads a symbol only when a chart asks for it (`crates/server/src/history.rs`). With `STOCK_CONTEXT=1` a symbol that prints before its turn moves to the front; a load the live tail outpaces is retried (2–60 s backoff) |
| `ADMISSION_JOURNAL` | `0` | live with a recorder: also write `live/admissions-<epoch>.jsonl`, the order the market admitted each print in, so a restart can prove it replayed the same inputs (`crates/server/src/admit.rs`). **Leave off for whole-market SIP**: measured 187–268 B per print (~7.5–10.7 GB for a ~40M-print day beside a ~0.8 GB sealed recording, moved to `live-original/` and the Pi with it), and its reader stops at 4M records, so past mid-morning a restore cannot use it anyway. Without it a restore replays the recording's own order and labels the order unverified (`no_admission_journal`) |
| `STOCK_CONTEXT` | `0` | five-pillar stock context and the Ross Panel (`crates/server/src/context.rs`, docs/stock-context/), under `pillars-v2`, `ross-panel-v1` and `detection-v2` (each row's pullback cue is the micro-pullback MP5, run for the panel's members only while `DETECTION_PANEL` is off: docs/stock-context/ross-panel-v1.md); off: `ross.snapshot` says `enabled: false`. Needs `PRICE_BAND_*` to match pillars-v2's price band and the panel policy's platform band (both $0.10–$25), else it stays off with an error in the log. Its float, news, baseline and checked-close evidence comes from the archive (the `evidence` and `archiver` services); without them the panel stays empty (`infra/vps/SETUP.md` section 10) |
| `CONTEXT_EVIDENCE_CHECK_SECS` | `15` | with `STOCK_CONTEXT=1`: how often the server reads newly stored evidence from `ARCHIVE_ROOT` (`crates/server/src/evidence.rs`, which explains the 15 s against the news pillar's 120 s coverage lag). `0` = only at startup and on a replay day switch |
| `ROCKET_WATCH` | `1` | the EXPERIMENTAL Rocket watch (`crates/server/src/rocket/`, `rocket.*` on /ws): symbols a strategy flags as likely to start a +30 % leg soon. Read-only, needs no provider or evidence: O(1) per print (measured ~0.2 µs in release) and one pass over the symbols active in the last hour per 15 s barrier (~16 ms for 7,000 in release, under the core lock). Every flagged row is then followed until 20:00 ET (`rocket/track.rs`): the later-move outcomes (max gain / drawdown over 2 h, max gain to 20:00, a +30 % mark any time later; the panel says "ran later +43%" at ≥ +15 % in 2 h) and a live momentum state (watching, building, breaking out, running, pullback, fading, halted; documented thresholds), plus the micro-pullback levels when `STOCK_CONTEXT=1` (the stock-context engine runs MP5 for the flagged symbols as for Ross members) and the day line. The live market appends every alert, its strict and later outcomes and every state transition to `ARCHIVE_ROOT/rocket-watch/<date>.jsonl` (~30–70 KB a day at 10 alerts, capped at 2 MB, pruned with `ARCHIVE_RETENTION_DAYS`) and reads today's back after a restart. `0`: `rocket.snapshot` says `enabled: false`. Shown on screen as "Runners". The strategy is `RUNNERS_SCORER` (below) |
| `ROCKET_ALERTS_PER_DAY` | `10` | Rocket watch's daily budget: the adaptive threshold aims for it and never exceeds it (1–200) |
| `ROCKET_THRESHOLD` | unset | a fixed Rocket watch threshold in the scorer's own units (overrides its frozen one; capped at max(5 × budget, 50) alerts a day), or `budget` to force the adaptive one. Unset: the scorer's frozen threshold if it has one, else the budget |
| `RUNNERS_SCORER` | `r0` | which strategy flags Runners (`crates/server/src/rocket/scorer.rs` `ScorerChoice`): `r0` = `precursors-R0`, the bake-off's round-1 rule (frozen B10 threshold); `r2` = `precursors-R2`, the round-2 LightGBM model (300 trees on 26 research features, frozen B10 raw-score threshold 3.0595, embedded as `rocket/r2_model.txt` and evaluated in Rust bit-identically to LightGBM; ~10 ms per barrier for 7,000 symbols in release; `pdh`/`rvol` are not wired yet and take their training medians); `placeholder` = `placeholder-b2`, the volume-acceleration baseline (adaptive budget). Anything else refuses to start. Every alert and log line carries the strategy id and version, so switching never mixes track records |

Alpaca allows one stream connection per account on most plans: run live on one host only.

## Archive (`crates/market-archive/src/config.rs`)

| Variable | Default | Meaning |
|---|---|---|
| `ARCHIVE_ROOT` | `./data/archive` | `/app/data/archive` in the image |
| `ARCHIVE_RETENTION_DAYS` | `0` (keep forever) | the server prunes older days at startup in replay mode. Counts **calendar** days (unlike `ARCHIVE_WINDOW_DAYS` and `LIVE_ORIGINAL_DAYS`, which count trading days). Never touches `live-original/` or `coverage/`. Keep `0` in production: the archiver owns the window |
| `ARCHIVE_MIN_FREE_MB` | `1024` | writers refuse to write below this much free space |

## Provider simulator (`crates/provider-sim/src/lib.rs`; replay mode only)

| Variable | Default | Meaning |
|---|---|---|
| `SIM_ENABLED` | `1` | `0` turns the Alpaca/FMP-compatible simulator off |
| `SIM_HOST` / `SIM_PORT` | `127.0.0.1` / `8791` | its listener |
| `SIM_ALPACA_KEY` / `SIM_ALPACA_SECRET` | `sim-key` / `sim-secret` | dummy credentials it accepts |
| `SIM_FMP_KEY` | `sim-fmp-key` | dummy FMP key |

## Archiver: `swimport maintain` (`crates/market-providers/src/import/maintain.rs`)

Runs as the `archiver` compose service on the same `.env`; also reads the Alpaca keys and
`ARCHIVE_*` above.

| Variable | Default | Meaning |
|---|---|---|
| `ARCHIVE_WINDOW_DAYS` | `21` | rolling window in trading days (1–504) |
| `DAILY_BARS_KEEP_DAYS` | `63` | trading dates of raw daily bars kept in `daily-bars/raw/` (0 = all, else 46–504); missing dates are backfilled (`docs/stock-context/daily-baseline-plan.md`) |
| `MAINTAIN_SPLIT_SNAPSHOTS` | `1` | split-adjusted snapshot of the watch list after each import (`0` = off) |
| `MAINTAIN_SPLIT_PER_MINUTE` | `200` | pace of the split snapshot and announcement fetches, each its own limiter (1–1000) |
| `SPLIT_SETS_KEEP` | `5` | split snapshot sets kept in `daily-bars/split/` |
| `ACTIONS_FETCH` | `1` | corporate-action announcement fetches (`0` = off); schedule, window, retries and freshness are the versioned policy `corporate-actions-coverage-v1`, not settings |
| `WARMUP_MAX_PER_RUN` | `50` | intraday band entrants warmed up per announcement tick (1–500) |
| `MAINTAIN_AT` | `21:15` | ET wall time of the daily cycle |
| `MAINTAIN_JOBS` | `12` | parallel import workers |
| `MAINTAIN_PER_MINUTE` | `9000` | request ceiling across workers (the plan allows 10,000) |
| `MAINTAIN_RETRIES` | `3` | extra passes over failed symbol-days per cycle |
| `MAINTAIN_RETRY_DELAY_SECS` | `120` | delay before the first extra pass; doubles each pass |
| `STOCKWATCHER_SERVER_URL` | `http://127.0.0.1:8790` | the server it asks which day is being replayed (never deleted) |
| `ALPACA_DATA_BASE` / `ALPACA_TRADING_BASE` | `https://data.alpaca.markets` / `https://paper-api.alpaca.markets` | API bases |

## Evidence collector: `swimport evidence` (`crates/market-providers/src/import/evidence_collect.rs`)

Runs as the `evidence` compose service on the same `.env` (`infra/vps/SETUP.md` section 10). It
writes float and news records under `ARCHIVE_ROOT/evidence/`, which the server reads with
`STOCK_CONTEXT=1`; plan and reasons: `docs/stock-context/float-news-plan.md`. Needs
`FMP_API_KEY` (float) and the Alpaca keys above (news); a missing key only switches that half
off, and the start line says `NOT SET`. Also reads `PRICE_BAND_*` (its watch universe) and
`ARCHIVE_*`.

| Variable | Default | Meaning |
|---|---|---|
| `FMP_API_KEY` | unset | FMP key for the float refresh; unset: no float is collected |
| `EVIDENCE_FLOAT_AT` | `07:00` | ET time of the daily float refresh (`HH:MM`), trading days |
| `EVIDENCE_FLOAT_MODE` | `bulk` | `bulk`: `shares-float-all` pages (~10 requests for the whole market; page 1 probed 2026-10-20), falling back to per-symbol on the first refusal. `per-symbol`: one request per universe symbol (~7,000 a day) |
| `EVIDENCE_FMP_PER_MINUTE` | `120` | FMP request ceiling for this process |
| `EVIDENCE_FLOAT_WARMUP_MAX` | `500` | intraday band entrants given a float per ET date, at most |
| `EVIDENCE_UNIVERSE_DAYS` | `5` | trading dates of `daily-closes.json` whose in-band symbols form the float universe |
| `EVIDENCE_NEWS_POLL_SECS` | `60` | news poll interval 04:00–20:00 ET on trading days (min 10) |
| `EVIDENCE_NEWS_IDLE_POLL_SECS` | `300` | news poll interval otherwise (min 10) |
| `EVIDENCE_NEWS_EDIT_EVERY_SECS` | `900` | how often the trailing window is searched again for edited stories (min 60) |
| `EVIDENCE_NEWS_EDIT_WINDOW_MINS` | `180` | that trailing window |
| `EVIDENCE_ALPACA_PER_MINUTE` | `60` | Alpaca news request ceiling for this process |
| `ALPACA_DATA_BASE` / `FMP_BASE` | `https://data.alpaca.markets` / `https://financialmodelingprep.com` | API bases |

`swimport`'s one-off commands (`crates/market-providers/src/bin/swimport.rs`) also read
`FMP_API_KEY` and `FMP_BASE` (default `https://financialmodelingprep.com`) for FMP comparison bars.

## Clients

| Variable | Where | Meaning |
|---|---|---|
| `STOCKWATCHER_DEV_SERVER` | `apps/web/vite.config.ts` (dev only) | where Vite proxies `/api`, `/ws`, `/updates`; default `http://127.0.0.1:8790`. `https://stockwatcher.wavystyle.io` runs the local web app against production (sign in with the production token) |
| `VITE_CHART_SOURCE` | `apps/web` | unset/`server`: use the server, falling back to the in-browser fixture only when it is unreachable or empty; `fixture` forces the fixture |
| `EXPO_PUBLIC_SERVER_URL` | `apps/mobile/src/feed/server.ts` | the server the phone talks to; default `http://10.0.2.2:8790` (Android emulator → host) in dev builds, production otherwise |

The clients never hold provider credentials. The access token is typed into the sign-in screen and
kept in `localStorage` (web) or the keystore (phone, `expo-secure-store`).
