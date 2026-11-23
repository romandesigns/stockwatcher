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
| `DATA_DIR` | `./data` | `updates/` (desktop installers, served at `/updates/*`), `settings/` (synced settings) and `alerts/` (price alerts' Activity `history.jsonl`, the phones registered for push with their Runners opt-ins, `push-devices.json`, and today's Runners push count and per-stock cooldowns, `runners-push.json`) live here |
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
| `BURST_ALERTS` | `1` | with `ROCKET_WATCH`, LIVE market only: **buying bursts** (`crates/server/src/rocket/burst/`), the burst-onset research's frozen detector `V3-5` (branch `research/burst-onset`, `d0d2156`). Per print O(1) (~110 ns measured on whole-market TEST days in release); every 5 s boundary is checked 1 s after it for the symbols that printed in the last 15 s; only a gate pass (≥ 5 prints, ≥ $5k and ≥ +1.5 % in 15 s, dollar rate ≥ 3× its 30-minute base, $0.10–$25, pre/regular/after) computes the 34 features and scores the 300 LightGBM trees (pure Rust, bit-identical to LightGBM, missing values handled natively). An alert when the score ≥ the threshold and the stock had none in 10 minutes. It confirms heavy buying about 5–9 minutes into a move (roughly halfway), it does not call the first print: in the original $0.15–$25 study, on 35 unseen days at 20 a day, 23 % reached +20 % before −5 % within 30 minutes (37 % +10 %), median 30-minute max gain +13 %, median drawdown before it −2.8 %. Each alert is a "Buying burst" row at the top of the Runners panel, tracked like a flag (momentum state, day line, later outcomes), with its own strict outcome (CONT10/20/30, MFE/MAE over 30 minutes) in the forward log (`burst` and `burst_outcome` lines). ADV30 is read from the archive's raw daily bars once a day in the background (`radv30`/`rvol` are missing until then). Replay parity: three whole-market TEST days replayed through the live detector match the research's gate passes, every feature and its alert files exactly (`rocket::burst::replay_parity`, needs the research disk). A replay never runs it. `0`: off |
| `N1_LIVE` | `1` | with `ROCKET_WATCH`, LIVE market only: the **N1 "early leg" SHADOW detector** (`crates/server/src/n1_live/`, owner approval 2026-11-17, judge ruling J8: labelled alerts beside production, never instead of it). Engine A round 2's entrant N1, since 2026-11-18 as frozen in T2-c4 (`X0.08-W3-k10`; T2-c6's `X0.06-W3-k10` before), branch `research/engine-a`: on the Runners' 15 s bars of every counted print, a bar decided 2 s after it ends fires when its close is in $0.10–$25, the stock has not run +20 % off its day low yet (R20), and the 3 minutes ending there gained ≥ 8 % from their low with the close ≥ 0.98 × their high, ≥ 20 prints, ≥ $50k and ≥ 10× the stock's usual dollars a minute (DADV20 / 390; a missing DADV20 passes); a previous close is required and ETFs, warrants, units and preferreds never fire (`universe-v1`); at most 2 a stock a day, 20 minutes apart, premarket candidates included; only 09:30–11:30 ET is sent. C1 / DADV20 come from the archive's raw daily bars and the types from `assets.json`, read at 03:30 ET. It only ADDS notices: kind `shadow.n1` ("SYM · N1 · shadow · $price") through the Runners delivery (ledger with the server's decision time, dedupe, quiet hours, stale drop), its own switch `n1` (phone `n1Shadow`) ON by default since 5350cad3, and a Shadow section on the Runners board. A day the process did not watch from 04:00 ET (or whose facts were late) never notifies. Research at X 8 %: 12.4 a day on Validation and 10.9 on 234 forward days at ~13 % precision (12.2 on the live-ledger days) (the research tape held only end-of-day $0.05–$30 symbols). Replay parity: `C:/wavystack/outputs/stockwatcher-n1-shadow/README.md`. `0`: off |
| `C4_LIVE` | `1` | with `ROCKET_WATCH`, LIVE market only: **Engine A T2-c4's additions** (`crates/server/src/c4_live/`, owner approval 2026-11-18; frozen candidate `research/engine-a/r2/T2/frozen/T2-c4.json`, branch `research/engine-a-forward`). (1) **F1u**, first pullback after ignition: entrant E4 (`ign=0.1;depth=0.382;bar=60;win=10;trig=bar_high`) on the Runners' prints, `universe-v1` at the fire, filter 10 (`ext_base ≤ 0.20`), E4's frozen budget with the day cap removed (θ20 0.3499, 2 a stock, 30 min apart, from 04:00), only 09:30–11:30 ET sent; kind `shadow.f1u`, switch `f1u`, ON by default; facts shared with N1; a day not watched from 04:00 ET never notifies. (2) **HALT add-on**: a Halt watcher gauge appearing on the buying side, in `universe-v1`, 09:30–11:30 ET, as kind `halt.bull` (switch `haltBull`, ON by default; not sent again to a device whose `halt` gauge switch is on). (3) **Prune POST<1**: no Runners notice for Run +20 %, a shelf entry, or a follow-up whose first or new path is Run +20 %, under $1 (the board's rows stay). The armed micro-pullback is kept (owner, 2026-11-18). Forward (234 days): F1u 15.0 a day at 6.6 %, HALT 20.0 at 18.3 % (live volume uncertain ±2×, review M2). Parity: `C:/wavystack/outputs/stockwatcher-c4-live/`. `0`: all three off |
| `XHALT_LIVE` | `1` | with `ROCKET_WATCH`, LIVE market only: **research v2's x-halt-1, resumed up after a halt** (`crates/server/src/xhalt_live/`, owner approval 2026-11-22; frozen finalist `30-w2/finalists/x-halt-1.json`, research `research/v2-x-halt` 860e7af1, pick `RU-u0.05-open0-Kany`). An LULD pause or "News pending" halt (the status's own reason code) whose last 15 s close before the halt is at least 5 % over the 15-minute low and within 1 % of the 60 s high, that reopens (the first priced 1 s bar from the resume status; 15 s without 1 s rows) at or above its halt price in $0.10–$25 and is not halted again before the decision, 1 s after the reopening bar ends; kind `halt.resumed_up`, device switch `resumedUp`, ON by default; facts (C1, types) shared with N1; needs the 15 minutes before the halt watched and both statuses seen. Research: 4.4 a day at 25.5 % GOOD vs 20.3 % (Train), Validation +0.07 pp recall, +0.10 pp precision. Parity: every Train day, 2,668/2,668 rows (`C:/wavystack/outputs/stockwatcher-research-v2/50-ship/run-halt/`). No-restart switch: the synced `detectors` namespace, entry `x-halt-1` (`crates/server/src/alerts/detectors.rs`), which also holds the cut `run` kind by default. `0`: off |
| `XFLOAT_LIVE` | `1` | with `ROCKET_WATCH`, LIVE market only: **research v2's x-float-1 "rotation ignition"** (`crates/server/src/xfloat_live/`, owner 2026-11-22: ship the proven v2 wins; finalist `outputs/stockwatcher-research-v2/30-w2/finalists/x-float-1.json`, branch `research/v2-x-float` d4bb58c0): a float of at most 50 M shares (FMP, via `STOCK_CONTEXT`; the research used the SEC float), a quarter of it traded since 04:00 ET, the close +10 % on the previous close and above every earlier high of the day; the first such 15 s bar of a symbol-day, decided at bar end + 50 ms. Kind `v2.xfloat`, switch `xFloat`, ON by default; runtime switch `detectors."x-float-1".on` (synced settings, read live, no restart: a switched-off fire is held `switched_off` and still ledgered). A day not watched from 04:00 ET never notifies. Train 6.7 a day at 25 % GOOD; Validation +1.1 pp recall, +0.15 pp precision over B0. Parity: `C:/wavystack/outputs/stockwatcher-research-v2/50-ship/float/PARITY.md`. `0`: off |
| `GAP_CTX_LIVE` | `1` | with `ROCKET_WATCH`, LIVE market only: **the v2 research finalists x-gap-1 and x-ctx-1** (`crates/server/src/gap_ctx/`, owner-approved ship set 2026-11-22, `stockwatcher-research-v2/DEPLOY-PLAN.md`). (1) **x-gap-1 "Pre-market high"** (kind `v2.xgap`, switch `xGap`): once a symbol-day, the first pre-market 15 s bar whose close is ≥ 10 % over the split-restated prior close, a new session high, with ≥ $100k traded since 04:00 and 5-minute dollars ≥ 3× the time-of-day baseline. (2) **x-ctx-1 "Day 2"** (kind `v2.xctx`, switch `xCtx`): on a symbol whose prior session's high was ≥ 1.3× its own prior close, the first bar 04:00–20:00 closing above that prior high on the same volume rules. Both decide at the bar's end + 50 ms, ON by default, and are held for everyone WITHOUT A RESTART by the synced settings namespace `detectors` (`{"x-gap-1": {"on": false}}`, ledgered as `held: switched_off`). They write the per-session time-of-day baselines `ARCHIVE_ROOT/tod/<date>.json` (kept 120 days) and need 5 of the 20 prior sessions' files before a day can notify (a one-time backfill gives full fidelity at once); facts from `daily-bars/raw/` (set `DAILY_BARS_KEEP_DAYS` ≥ 368 for the research's exact prev_ran) and the corporate-actions ledger. A day not watched from 04:00 ET, with stale daily bars or too few ToD sessions never notifies. Parity: `C:/wavystack/outputs/stockwatcher-research-v2/50-ship/gap-ctx/PARITY.md`. `0`: both removed |
| `ALERT_SPREAD` | `1` | LIVE market only: the **fire-time bid-ask spread** on every Runners-pipeline notice (`crates/server/src/spread.rs`, owner 2026-11-19; display only, no detector or limit reads it). When a notice fires, the server asks Alpaca's historical quotes endpoint (`GET /v2/stocks/{symbol}/quotes`, the live stream's `ALPACA_STREAM_FEED` when it is `sip` or `iex`, the same keys, base `ALPACA_DATA_BASE`) for the minute up to the fire time, newest first, and takes the newest market quote (bid > 0, ask > bid, spread ≤ 50 % of the mid): `round((ask − bid) / mid × 10 000)` bp, else `null`. Each fetch is bounded at 1.5 s, so a slow answer delays a push at most that much, then goes without. It rides on `runners.notice` (`spreadBp`), the push text ("Alert $2.41 · +5% = $2.53 · spread 1.6%"), the ledger's `decision` lines (`spread_bp`) and, for a row's own entry, the Runners row (`spread_bp`, fire time only, not restored after a restart). The live stream itself still subscribes to trades and statuses only. When the request is answered but no market quote qualifies, the notice carries `no_quote: true` (owner 2026-11-22: the **"No live quote"** warning chip on Runners rows and in the notification log; research v2: such alerts were GOOD 11.5 % vs 21.5 %); a failed or timed-out request is unknown and never sets it (`spread::Priced`). `0`, no Alpaca keys or another feed: no quote requests, no spread, no warning |
| `BURST_ALERTS_PER_DAY` | `20` | the frozen threshold to alert at: `10` (0.2618), `20` (0.2334) or `40` (0.2028), the scores that gave that many alerts a day on the research's DEV days. A runaway guard caps a day at max(5 × it, 50) |
| `BURST_THRESHOLD` | unset | a model probability in (0, 1) to alert at instead (overrides `BURST_ALERTS_PER_DAY`'s) |
| (no variable) Runners phone push | on | the LIVE market's Runners board sends an Expo push (the price alerts' pipeline, `crates/server/src/alerts/runners.rs`) for each new flag ("Runners: CJMB $2.45", the top two reasons) and, for phones that turned it on, each flagged row turning *breaking out* ("CJMB breaking out $3.10", % from the flag). Buying bursts (`BURST_ALERTS`) push too, "CJMB buying burst $2.10", body "+18.2% in 6 min · $ rate 6.4× base" (gain since the burst started, its age, the top reason). Data `{symbol, kind: "runners.new" \| "runners.breakout" \| "runners.burst", alert_id}`: a tap opens that ticker's chart. Opt-in per phone, sent in `push.register` (`runnersNew`, default on; `runnersBreakout`, default off; `runnersBurst`, default on; Settings → Runners on the phone). Limits: at most one Runners push per stock per 10 minutes (either kind), 30 a day (ET; kept across restarts, separate from the price alerts' phone budget), the price alerts' quiet hours, nothing older than 3 minutes. Off the ingest path (a bounded `try_send` after the core lock); a failed push is logged and costs nothing else. A replay never pushes. Needs `ROCKET_WATCH` and a registered phone; tune by editing the constants, not the environment |

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
| `EDGAR_USER_AGENT` | **unset: the EDGAR worker is off** | SEC's required descriptive User-Agent with a contact (placeholder: `ExampleCo market-data admin@example.com`). Set, a third worker collects SEC filings into `ARCHIVE_ROOT/evidence/filings/` (`crates/market-providers/src/import/edgar.rs`; pillars-v6 reads 8-K and 6-K as news, offerings as a row flag). Unset, the start log says `SEC EDGAR filings OFF` once |
| `EDGAR_POLL_SECS` / `EDGAR_IDLE_POLL_SECS` | `60` / `600` | EDGAR feed poll interval in filing hours (06:00–22:00 ET weekdays) / otherwise (min 10) |
| `EDGAR_PER_MINUTE` | `60` | EDGAR request ceiling, retries included (SEC's fair-access limit is 10 a second; this is 1) |
| `EDGAR_POLL_MAX_PAGES` / `EDGAR_BACKFILL_MAX_PAGES` | `10` / `60` | feed pages (100 filings each) a forward poll / the first poll after a start may read |
| `EDGAR_BACKFILL_DETAIL_MAX` | `600` | filing index pages a start's daily-index back-fill may read for acceptance times |
| `EDGAR_TICKERS_REFRESH_HOURS` | `24` | how often SEC's `company_tickers.json` is fetched again |
| `EDGAR_BASE` | `https://www.sec.gov` | API base |

`swimport`'s one-off commands (`crates/market-providers/src/bin/swimport.rs`) also read
`FMP_API_KEY` and `FMP_BASE` (default `https://financialmodelingprep.com`) for FMP comparison bars.

## Press-release wires (`crates/server/src/wires/`)

Read by the server, live market with `STOCK_CONTEXT=1` only. **Off, and with no built-in feed**:
the 2026-11-23 source check approved none of GlobeNewswire, PR Newswire, Business Wire,
Accesswire or Newsfile (terms forbid automated retrieval, feeds deactivated, robots.txt
disallows `/rss`, or the client is refused). Set a feed only once its terms are cleared.

| Variable | Default | Meaning |
|---|---|---|
| `WIRES_NEWS` | `0` | `1` polls the `WIRES_FEEDS` feeds; the start line lists them |
| `WIRES_FEEDS` | unset | `id=https-url` pairs, comma-separated (`id` is `[a-z0-9_]+`, the item's sub-source) |
| `WIRES_POLL_SECS` | `60` | per-feed poll period; never under 60 (backs off to 30 min on errors) |

## Clients

| Variable | Where | Meaning |
|---|---|---|
| `STOCKWATCHER_DEV_SERVER` | `apps/web/vite.config.ts` (dev only) | where Vite proxies `/api`, `/ws`, `/updates`; default `http://127.0.0.1:8790`. `https://stockwatcher.wavystyle.io` runs the local web app against production (sign in with the production token) |
| `EXPO_PUBLIC_SERVER_URL` | `apps/mobile/src/feed/server.ts` | the server the phone talks to; default `http://10.0.2.2:8790` (Android emulator → host) in dev builds, production otherwise |

The clients never hold provider credentials. The access token is typed into the sign-in screen and
kept in `localStorage` (web) or the keystore (phone, `expo-secure-store`).


### Float window (owner, 2026-10-31)

`pillars-v4` uses a float record for **4 days** after it was retrieved (v1–v3:
24 h). Float barely changes from day to day, and a late or missed daily refresh
had made the float pillar UNKNOWN and dropped otherwise qualifying symbols from
the Ross Panel. The server keeps float records from 5 days before the session
(`FLOAT_LOOKBACK` in `crates/server/src/evidence.rs`), and a Ross row whose
figure is a day old or more shows its age (`Float 3.1M · 2d old`).

### Float fallback on shares outstanding (owner, 2026-11-22)

`pillars-v5` is `pillars-v4` plus one rule: where no usable float is visible (no
record, a null or zero figure, or one older than 4 days) but a usable
shares-outstanding figure is, and it is **below** 20M, the float pillar PASSes
with reason `below_max_via_outstanding`. The public float can never exceed the
shares outstanding, so this is a proof, not an estimate; an outstanding figure at
or above 20M proves nothing and the pillar stays UNKNOWN, and a null or zero never
passes. The figure comes from the same FMP retrieval (`outstandingShares`), obeys
the same 4-day window, and a Ross row shows it as `Float ≤ 8.0M (shares out)`,
never as the float. On 2026-11-20 FMP returned a null float for 1,026 of 3,409
symbols. `pillars-v1` to `pillars-v4` remain frozen.

### Near-qualifying section (owner, 2026-11-22)

The readiness summary in every `ross.*` frame carries `near` (at most 50) and
`near_total`: symbols passing price, gain and RVOL with no pillar failing and float
and/or news UNKNOWN. The web and phone Ross panels list them under the members
with a `Float ?` / `News ?` badge; the first three show and the rest open from a
disclosure. Descriptive only: no membership, no notice.

### Current entry price minimum (owner, 2026-10-26)

All current live entry paths use an inclusive **$0.10–$25** band: precursor
scoring (R0/R2/placeholder), Buying bursts, volume shelves, Running, Run +20%,
Fast and stock-context discovery. Discovery uses `pillars-v5` (`pillars-v3` with
the float window above and the shares-outstanding fallback); `pillars-v1` to
`pillars-v4` remain unchanged historical policies. MP5's active band was already
$0.10–$25, so no pullback geometry or wick filter changed.

Precursor, burst and shelf eligibility use `market_events::PriceBand::DEFAULT`.
Their liquidity, history/readiness, model, score, cooldown and budget rules still
apply; eligibility alone does not produce an alert. The precursor, Fast and burst
tapes still admit prices from $0.05 to retain pre-crossing context.

Existing research protocols and the canonical precursor rocket outcome retain
their original $0.15–$25 definitions. The added $0.10–<$0.15 range has not been
validated by those studies. Burst pushes omit the historical odds below $0.15;
the shared web/desktop/mobile section labels the original study's range. New
precursor/burst alert logs and shelf run logs record `entry_band_micros` to make
the active eligibility explicit alongside the unchanged model identity.

MP5 now also records the frozen impulse candle's experimental `mp5-wick-v1`
observation, including in held notification ledger decisions. This is not a
blocking filter or a new alert gate. Its contract and exploratory assessment
are in `docs/stock-context/mp5-wick-diagnostic.md` and
`docs/research/mp5-wick-v1-results.md`.
