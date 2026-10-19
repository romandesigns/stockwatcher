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
  .github/workflows/         # ci.yml (checks; pull requests), release.yml (every push to main; reuses ci.yml)
  docs/                      # CONVENTIONS, architecture/, reference/, specs/, research/
```

## Names, ports, URLs
- Repo: `github.com/romandesigns/stockwatcher` (**private**). Default branch `main`.
- Public URL: `https://stockwatcher.wavystyle.io` (Caddy on the VPS → `127.0.0.1:8790`).
- Server port: **8790** (env `PORT`, default 8790; bind `0.0.0.0` in the container, published as `127.0.0.1:8790` on the host).
- Routes: `GET /api/health` → `{"status":"ok","version","commit"}`; `GET /api/version`; `GET /api/chart/{symbol}/snapshot`, `/api/replay/state`, `/api/symbols`, `/api/quotes`, `/api/settings`; `/updates/*` (desktop installers, no auth); WebSocket `/ws` (JSON: `hello`/`welcome`/`ping`/`pong` with a 15 s heartbeat, then `chart.*`, `tape.trade`, `replay.*`, `order.*`/`paper.*`, `quotes.*`, `settings.*`); everything else → the web app (SPA fallback to index.html). Message reference: `docs/architecture/simulator-and-chart.md` Appendix D and the header of `crates/server/src/ws.rs`. Environment variables: `docs/reference/configuration.md`.
- Auth: env `STOCKWATCHER_API_TOKEN` (≥32 chars). When set, `/api/*` (except `/api/health`) and `/ws` require `Authorization: Bearer <token>` (WS: token in the hello message). When unset in dev, auth is off and the server logs a warning. Never put the token in a URL or a public build variable.
- Web dev server: Vite on 5173, proxying `/api`, `/ws` and `/updates` to 8790 (or to `STOCKWATCHER_DEV_SERVER`). Production web build: `apps/web/dist`.
- Image: `ghcr.io/romandesigns/stockwatcher-server:<git sha>` (+ `:main`). Container user non-root.
- VPS: host alias `stockspotter-vps` (srv1170872), user `wavystack` (docker group, no passwordless sudo). App dir `~/apps/stockwatcher` (compose file + `.env` + deploy log). Data volume `stockwatcher_data` → `/app/data`.
- Mobile: Expo app slug `stockwatcher`, EAS owner `creativecoder87`, **EAS project id `b43c471a-3d47-4904-bd31-a7f5797ee45a`** (created by the owner; link with `eas init --id`), Android package `io.wavystyle.stockwatcher`, iOS bundle id `io.wavystyle.stockwatcher`. Channels: `preview`, `production`. runtimeVersion policy `fingerprint`.
- Desktop: Tauri identifier `io.wavystyle.stockwatcher`, product name `stockwatcher`. Windows installers (NSIS + MSI), updater signed with a minisign key held only in GitHub secrets.

## Release pipeline: every push to `main` ships everything (owner decision, 2026-10-16)

No release branches and no manual dispatch: a push to `main` runs one
workflow, `.github/workflows/release.yml`:

1. `checks`: all tests, lints and builds (Rust, web, shared, mobile tsc,
   desktop type/compile check). Every later job `needs` it. A red check
   ships nothing.
2. In parallel after checks:
   - `server` builds and pushes `ghcr.io/romandesigns/stockwatcher-server:<sha>`,
     which already contains the web app. It then deploys to the VPS with the
     forced-command key (`VPS_DEPLOY_KEY`, environment `production`), with a
     health check and automatic rollback. The web app is live when this job
     ends.
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
     - Always publishes an OTA update (`eas update --channel preview
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
- **Alerts:** `expo-notifications` (push token registration, Android channel setup, tap opens the right screen) together with `expo-device`; `expo-haptics`.
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
