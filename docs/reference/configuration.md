# Configuration reference

Every environment variable the server, the archiver and the clients read, in one place. The
source of truth is the doc table next to each reader (linked per section); update both when a
variable changes. Secrets go in the gitignored `.env` on each host, never in the repo, a URL or a
public build variable (`docs/CONVENTIONS.md`).

Production (`~/apps/stockwatcher/.env` on the VPS, shared by the `server` and `archiver` compose
services) sets `STOCKWATCHER_API_TOKEN`, `RUST_LOG`, `MARKET_DATA_MODE`, `REPLAY_START=09:30`,
`SIM_AUTOFIXTURE=0` and the Alpaca keys. The image adds `HOST=0.0.0.0`, `STATIC_DIR=/app/web`,
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
| `LIVE_RECORD_DAYS` | `5` | live recordings kept when no import replaced them (at least 2; never the day being recorded). Shares `ARCHIVE_MIN_FREE_MB` with the archive |
| `LIVE_HISTORY_PRELOAD` | `1` | after a start, load every symbol's day from the archive in the background, busiest first; `0` loads a symbol only when a chart asks for it (`crates/server/src/history.rs`) |

Alpaca allows one stream connection per account on most plans: run live on one host only.

## Archive (`crates/market-archive/src/config.rs`)

| Variable | Default | Meaning |
|---|---|---|
| `ARCHIVE_ROOT` | `./data/archive` | `/app/data/archive` in the image |
| `ARCHIVE_RETENTION_DAYS` | `0` (keep forever) | the server prunes older days at startup in replay mode. Keep `0` in production: the archiver owns the window |
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
| `ACTIONS_AT` | `07:00,09:15` | ET times on trading days of the corporate-action announcement fetch (`off` disables) |
| `MAINTAIN_AT` | `21:15` | ET wall time of the daily cycle |
| `MAINTAIN_JOBS` | `12` | parallel import workers |
| `MAINTAIN_PER_MINUTE` | `9000` | request ceiling across workers (the plan allows 10,000) |
| `MAINTAIN_RETRIES` | `3` | extra passes over failed symbol-days per cycle |
| `MAINTAIN_RETRY_DELAY_SECS` | `120` | delay before the first extra pass; doubles each pass |
| `STOCKWATCHER_SERVER_URL` | `http://127.0.0.1:8790` | the server it asks which day is being replayed (never deleted) |
| `ALPACA_DATA_BASE` / `ALPACA_TRADING_BASE` | `https://data.alpaca.markets` / `https://paper-api.alpaca.markets` | API bases |

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
