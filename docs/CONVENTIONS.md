# stockwatcher: project conventions

Stockwatcher’s goal is to help you, Roman, identify high-quality bullish stock opportunities and judge whether the price action offers a worthwhile entry.
We’ll build around your trading approach:
- 5-minute chart: understand market structure and the broader setup.
- 1-minute chart: assess momentum and potential entries.
- 30-second chart: inspect execution timing.
- Key context: the first meaningful expansion candle, volume, VWAP, moving averages, pullbacks, and nearby resistance.
The application should explain why a stock deserves attention, how the setup is developing in one intuitive, web, desktop and mobile-friendly view.

## Product direction (owner, 2026-10-18)

- **The main page consumes only live data.** It is the Dashboard (2026-10-18): a panel of 1–4 charts
  with a live "Most active" list; select a chart, pick a ticker to load it there
  (`docs/architecture/chart-packages.md` "The dashboard"). It shows whatever the server streams, so
  it is still replay while production runs replay.
- **Tap, then press and hold a ticker to copy it** (owner, 2026-10-18), on every client, with a
  fading "Copied …" toast. New ticker labels use `useTickerPress` / `TickerLabel` so they all behave
  alike.
- **Replay is a practice feature**, like TradingView's Bar Replay or Webull's paper trading: pick a
  recorded day and trade it with the simulated broker. It is not a mode the whole product switches to.
  The server runs **both markets side by side** (2026-10-18): the dashboard reads `live`, the chart
  page `replay` (`hello.source` / `?source=`). A new page that shows market data names its source;
  it never relies on the server default.

## Layout (monorepo, Bun workspaces + Cargo workspace)

```
stockwatcher/
  Cargo.toml                 # Rust workspace (members: crates/*, apps/desktop/src-tauri)
  package.json               # Bun workspaces: apps/*, packages/*
  crates/
    market-events/           # the normalized event contract, ET session calendar, trade-condition policy
    chart-engine/            # server-side aggregation: 15s/30s/1m/5m candles, gaps, profile, halts, LULD
    market-archive/          # on-disk day/symbol archive (+ swarchive CLI), assets.json, daily-closes.json
    market-replay/           # deterministic virtual clock over the archive
    market-providers/        # Replay and Live (Alpaca stream) providers; swimport + the rolling-window archiver
    paper-broker/            # simulated order matching, positions, account
    provider-sim/            # Alpaca/FMP-compatible local simulator (replay mode, :8791)
    server/                  # axum: /api/*, /ws, /updates/*; serves apps/web/dist
  apps/web/                  # React + Vite + TypeScript + Tailwind v4 + vendored shadcn/ui (Base UI)
  apps/desktop/              # Tauri v2 shell around apps/web (src-tauri/)
  apps/mobile/               # Expo SDK 58 React Native app (development build)
  packages/
    shared/                  # protocol types, HTTP/WS clients, settings sync engine, ticker search
    chart-core/              # framework-free chart logic and display list
    chart-web/               # the chart for React DOM (Canvas 2D) + its components and UI primitives
    chart-native/            # the chart for React Native (Skia) + its sheets and controls
    app-state/               # the clients' Zustand store (synced + device + session state)
  infra/docker/Dockerfile    # one image: builds web + server (+ swimport), runs the server
  infra/vps/                 # compose file (server + archiver), deploy/publish scripts, Caddy snippet, SETUP.md
  .github/workflows/         # ci.yml (checks, per part), release.yml (every push to main: ships what changed; reuses ci.yml)
  docs/                      # CONVENTIONS, architecture/, reference/, specs/, research/
```

## Names, ports, URLs
- Repo: `github.com/romandesigns/stockwatcher` (**private**). Default branch `main`.
- Public URL: `https://stockwatcher.wavystyle.io` (Caddy on the VPS → `127.0.0.1:8790`).
- Server port: **8790** (env `PORT`, default 8790; bind `0.0.0.0` in the container, published as `127.0.0.1:8790` on the host).
- Routes: `GET /api/health` → `{"status":"ok","version","commit"}`; `GET /api/version`; `GET /api/chart/{symbol}/snapshot`, `/api/replay/state`, `/api/symbols`, `/api/quotes`, `/api/settings`; `/updates/*` (desktop installers, no auth); WebSocket `/ws` (JSON: `hello`/`welcome`/`ping`/`pong` with a 15 s heartbeat, then `chart.*`, `tape.trade`, `replay.*`, `order.*`/`paper.*`, `quotes.*`, `pressure.*`, `settings.*`, `alerts.*`/`alert.*` and `push.register`); everything else → the web app (SPA fallback to index.html). Message reference: `docs/architecture/simulator-and-chart.md` Appendix D and the header of `crates/server/src/ws.rs`. Environment variables: `docs/reference/configuration.md`.
- Auth: env `STOCKWATCHER_API_TOKEN` (≥32 chars). When set, `/api/*` (except `/api/health`) and `/ws` require `Authorization: Bearer <token>` (WS: token in the hello message). When unset in dev, auth is off and the server logs a warning. Never put the token in a URL or a public build variable.
- Web dev server: Vite on 5173, proxying `/api`, `/ws` and `/updates` to 8790 (or to `STOCKWATCHER_DEV_SERVER`). Production web build: `apps/web/dist`.
- Image: `ghcr.io/romandesigns/stockwatcher-server:<git sha>` (+ `:main`). Container user non-root.
- VPS: host alias `stockspotter-vps` (srv1170872), user `wavystack` (docker group, no passwordless sudo). App dir `~/apps/stockwatcher` (compose file + `.env` + deploy log). Data volume `stockwatcher_data` → `/app/data`.
- Mobile: Expo app slug `stockwatcher`, EAS owner `creativecoder87`, **EAS project id `b43c471a-3d47-4904-bd31-a7f5797ee45a`** (created by the owner; link with `eas init --id`), Android package `io.wavystyle.stockwatcher`, iOS bundle id `io.wavystyle.stockwatcher`. Channels: `preview`, `production`. runtimeVersion policy `fingerprint`.
- Desktop: Tauri identifier `io.wavystyle.stockwatcher`, product name `stockwatcher`. Windows installers (NSIS + MSI), updater signed with a minisign key held only in GitHub secrets.

## Release pipeline: every push to `main` ships what changed (owner decisions, 2026-10-16 and 2026-10-20)

No release branches, no feature PRs (since 2026-10-20, to keep GitHub
Actions spend under its budget: work is committed straight to `main`, checked
locally with the same commands as `ci.yml`, and pushed in batches). A push
to `main` runs one workflow, `.github/workflows/release.yml` (it also runs on
a schedule after 20:00 ET, and by hand only for the emergency override; see
"Deploy window" below):

0. `changes` diffs the push against the commit of the **last fully
   successful, not deferred** release (not the previous push: a run a
   billing block stopped leaves its changes unshipped, and the next push
   picks them up) and decides which parts ship: `server` (crates, the
   Docker image, the web app it embeds, Rust manifests), `desktop` (the
   desktop app, the web app, Rust manifests), `mobile` (the phone app and
   the packages it uses), `web`. The server's diff starts from the commit
   the live server reports at `/api/health` (what deploy.sh verified),
   falling back to the last full release when it cannot be reached. A
   change to a workflow, `scripts/release/`, `package.json` or `bun.lock`
   ships everything. A docs-only push ships nothing.
1. `checks`: the tests, lints and builds of the parts that ship (`ci.yml`
   with inputs; the Windows desktop job, billed at twice the Linux rate,
   runs only when desktop ships). Every later job `needs` it. A red check
   ships nothing.
2. In parallel after checks, each only when its part ships:
   - `server` builds and pushes `ghcr.io/romandesigns/stockwatcher-server:<sha>`,
     which already contains the web app. It then deploys to the VPS with the
     forced-command key (`VPS_DEPLOY_KEY`, environment `production`), with a
     health check and automatic rollback. The web app is live when this job
     ends. It then deletes all but the newest 10 image versions (Packages
     storage has a $0 budget; 10 leave room for a rollback).
   - `desktop` (windows-latest) sets the version to
     `0.1.<github.run_number>` (the committed version stays `0.1.0`). It
     builds signed NSIS and MSI installers and writes `latest.json` with URLs
     `https://stockwatcher.wavystyle.io/updates/desktop/<file>`. It then
     uploads them to the VPS with a second forced-command key
     (`VPS_PUBLISH_KEY`, only able to run `publish-updates.sh`, which
     extracts a validated tar from stdin into the server's updates
     directory). The server serves `/updates/*` without auth (installers and
     manifests only; the app itself still needs the access token). The
     updater pubkey is in `tauri.conf.json`; the private key and password
     are the secrets `TAURI_SIGNING_PRIVATE_KEY` and
     `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Installed apps update themselves.
     A private repo's GitHub Releases are not used, because the updater
     cannot authenticate to them.
   - `mobile`:
     - Publishes an OTA update (`eas update --channel preview
       --platform android`, using `EXPO_TOKEN`).
     - It also queues an EAS Android preview **build** when the native
       fingerprint differs from the latest finished preview build's
       `runtime.version`, so a native change produces a new APK and a JS
       change reaches phones over the air.
     - It doesn't build on every push, because EAS build quota is limited
       and an unchanged native layer would make an identical APK.
3. `summary` records versions, image tag, OTA group and any APK build link
   in the job summary.

Concurrency: group `release-main` with `cancel-in-progress: false`, so a
newer push waits instead of interrupting a deploy halfway.

### Deploy window: no server restart during the trading day (2026-10-31)

**The live server is never deployed between 04:00 and 20:00
America/New_York on an NYSE trading day.** A restart mid-session loses
state a restore cannot rebuild exactly: on 2026-10-30 a 16:38 ET deploy
restarted the server and two winning INO micro-pullback setups were never
announced (the warm-up and notice-memory fixes since then make a restart
safer, but a deploy is still a gap in the live feed).

- `changes` asks `scripts/release/deploy-window.ts` whether now is inside
  the window. It reads the holidays from the server's own table
  (`crates/market-events/src/session.rs`, `HOLIDAYS`), so the two never
  disagree; past the years that table covers, every weekday counts as a
  trading day (as the server assumes). Early-close days are trading days.
  Time zones come from Intl (the IANA database), so DST needs no rules of
  our own: 04:00 ET is 08:00 UTC in summer, 09:00 UTC in winter.
  `bun test scripts/release` covers both clock changes, holidays, weekends
  and the boundaries (in `ci.yml`'s web job).
- **The hold starts at 03:00 ET**, an hour before the window: the window is
  about when the server restarts, but the check runs when the release
  starts, and checks plus the image build and deploy take up to ~45 minutes.
  A run started at 03:55 would otherwise restart the server in premarket.
- **A push inside the window still runs its checks**, but when the server
  has something to ship, the run **holds**: the server deploy, the desktop
  release and the phone OTA/APK are all skipped, so no client ships a
  protocol the live server does not speak yet. A client-only push (the
  live server already runs the server code) ships at once. A held run ends
  green with a step named "Deploy deferred until after 20:00 ET"; that step
  is the marker `changes` uses to never take a held run as the base.
- **The scheduled run** (`cron: 7 0,1 * * *`: 20:07 and 21:07 EDT, or 19:07
  and 20:07 EST) deploys whatever is pending once the window has closed. A
  scheduled run still inside the window (19:07 EST) skips even the checks;
  one with nothing pending stops after `changes`.
- **Emergency override:** run the workflow by hand (Actions, Release, Run
  workflow) with `force` ticked. It ships main now, window or not. Without
  `force` a manual run behaves like a push.
- Weekends, NYSE holidays and 20:00–03:00 ET deploy immediately, as before.

## Mobile (Expo) modules and config plugins

The target is Expo SDK 58 (latest when this was written). Install every package with
`bunx expo install` so versions match the SDK. Anything native needs a
**development build** (`expo-dev-client`): the screen-transitions library
does not run in Expo Go.

- **Over-the-air updates:** `expo-updates`.
  - `updates.url` points to the EAS project.
  - `runtimeVersion: { policy: "fingerprint" }`.
  - `checkAutomatically: "ON_LOAD"`.
  - The app also checks on return to foreground with `Updates.checkForUpdateAsync`, downloads in the background, and offers "Restart to update" (or applies on the next cold start). It must never block launch for long.
  - Channels: `preview` and `production`.
- **Builds and native config:** `expo-dev-client`; `expo-build-properties` for Android minSdk/targetSdk and Kotlin, plus iOS deployment target as SDK 58 requires.
- **Navigation and motion** (CLAUDE.md "Screen transitions and motion"):
  - `@react-navigation/native` and `@react-navigation/bottom-tabs`;
  - `react-native-screen-transitions` 4.x with its peers `react-native-reanimated` 4, `react-native-worklets`, `react-native-gesture-handler`, `react-native-safe-area-context` and `react-native-teleport`;
  - `react-native-screens`.
  - One motion module (`src/motion/transitions.ts`) defines the named specs. iOS edge-swipe back, Android back and predictive back go through the same spec. Reduced motion collapses to a cross-fade.
- **Alerts:** `expo-notifications` (push token registration, Android channel setup, tap opens the right screen) together with `expo-device`; `expo-haptics`. Built 2026-10-19 (`src/notifications/push.ts`, the Alerts tab): the token is registered with the server (`push.register`) on every connection; permission is asked only from the Alerts tab (or from Settings → Runners, whose two switches are the Runners push opt-ins, sent with `push.register`; a tapped push about a ticker, Runners or price alert, opens THAT ticker's chart modal, replacing any open one (owner, 2026-10-23), anything else the Alerts tab: `src/notifications/route.ts`, through the one `openTickerChart` in `src/navigation/ref.ts`). **Android needs FCM credentials in the build** (a Firebase project's `google-services.json` and its FCM v1 key uploaded to EAS) before Expo can issue a token: a native change, so a new EAS build, done by the owner.
- **Storage:** `expo-secure-store` for the access token (never AsyncStorage); `@react-native-async-storage/async-storage` for non-secret preferences.
- **System:** `expo-constants`, `expo-application`, `expo-linking` (deep links with scheme `stockwatcher`), `expo-splash-screen`, `expo-status-bar`, `expo-system-ui`, `expo-font` (an open font bundled locally, no runtime CDN), `expo-keep-awake` (opt-in keep screen on), `expo-localization`.
- **Network:** `@react-native-community/netinfo` for the offline/online indicator and reconnecting the WebSocket.
- **Graphics:** `@shopify/react-native-skia` draws the chart (chart-core's display list, one picture per frame; why: `simulator-and-chart.md` Appendix E); `react-native-svg` for icons, gauges and sparklines.
- **Styling:** React Native `StyleSheet` with a `src/theme.ts` token module mirroring the web tokens. No NativeWind and no web-only UI kits.
- **Screens (2026-10-18):** a sign-in screen (the access token, kept in the keystore) and the chart screen (`src/screens/ChartScreen.tsx`: header, chart, replay bar, and the ticker search, settings, tape and replay-day sheets). The boilerplate's "one Hello world screen" stage is over; the motion module, update check, notification channel and secure-store wrapper from that stage are still the wiring new screens use.

## Desktop (Tauri v2) plugins

- **Over-the-air updates:** `tauri-plugin-updater` and `tauri-plugin-process`, used for relaunch.
  - The endpoint is `https://stockwatcher.wavystyle.io/updates/desktop/latest.json`, signed with the pubkey in `tauri.conf.json`.
  - **Fully automatic (owner decision, 2026-10-16):** every push to `main` reaches installed apps with no clicks. The app checks on start and every 2 minutes. When a version is newer, it downloads it, verifies the signature, then installs immediately in Windows `passive` mode (a progress bar, no input) and relaunches. There is no prompt and no dialog plugin.
  - The only manual step is the very first install on a machine. CI cannot reach into a PC that has no stockwatcher on it.
- **Also:** `tauri-plugin-single-instance`, `tauri-plugin-window-state` (remembers size and position), `tauri-plugin-notification` (native alerts), `tauri-plugin-log` (a rotating file log for support), `tauri-plugin-os` and `tauri-plugin-opener` (open links in the browser).
- **Least-privilege capabilities:** `src-tauri/capabilities/` grants only what's used. The updater and process permissions are scoped.
- **Version:** `0.1.0` in the repo; CI sets `0.1.<run_number>` for each release.

## Workspace rules that apply (from H:\wavystack\CLAUDE.md and owner preferences)
- Compact design approach. Use balanced padding and marging just enough to allow breathing room between UI components.
- Latest stable versions when first added; Bun as package manager; Rust for the backend.
- Web UI: vendored shadcn/ui **on Base UI primitives — never Radix** (no `@radix-ui/*` package anywhere; owner rule 2026-10-16), 60/30/10 color tokens on `:root` (dominant neutrals / structural / one accent), dark-first, responsive (works at 375 px), every visible layout change animates and respects reduced motion.
- Layout: the boilerplate stage ("no UI yet", 2026-10-16) ended with the simulator and chart (2026-10-17/18). For new screens the stockspotter layout strategy (fixed-viewport resizable panel grid; rail + workspace; stacked below 1400 px; a Home hub with tabs on phones) is the default proposal. Owner UI rules that apply everywhere: no borders or dividers (surface contrast, fills and gaps instead), light layouts (regions sit on the page background, fills only for stages, samples, code and controls), minimal and compact, duotone icons (accent duotone when active).
- Reusable UI: a new component a second screen could use also goes into `wavykit/` (workspace rule).
- Mobile motion: one motion vocabulary for iOS and Android (CLAUDE.md "Screen transitions and motion").
- Open source first; no paid runtime APIs without a reason recorded in the commit message.
- App state: one Zustand store per client from `@stockwatcher/app-state` (owner, 2026-10-18; Zustand over Redux Toolkit). Components read it through `useApp(selector)`, never a new top-level `useState` for state another screen or device could need. See `docs/architecture/app-state.md`.
- Commits explain why. Comments explain non-obvious constraints, concisely.
- Secrets only in gitignored `.env` files or GitHub secrets; never committed, never printed.
