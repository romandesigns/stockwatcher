# stockwatcher

Private monorepo for stockwatcher, a tool for finding bullish stock setups in the $0.10–$25 range and
judging the entry: a Rust market server and one chart that runs on the web, the desktop (Tauri) and
Android (Expo). Live at <https://stockwatcher.wavystyle.io> (access token required).

## What exists (2026-10-18)

- **Market data, aggregated once on the server.** Live Alpaca SIP streaming or a replay of recorded SIP
  days, through one pipeline: the same chart engine builds 15s/30s/1m/5m candles, gaps, the session
  volume profile, halts and LULD bands either way. Replay has play/pause/step/seek/speed and a day
  picker over a rolling archive of the last 21 trading days, kept current every evening by the
  `archiver` service.
- **One reusable chart** for every client (`packages/chart-*`): seven chart types, three price scales,
  indicators, drawings and position plans, levels (profile, halts, LULD, key levels, opening range,
  previous close, fills), and simulated paper trading with order lines, a ticket and brackets.
- **Clients:** the web app (also the desktop app's content), the Android app, a ticker search with
  live quotes, and settings. A single **Zustand app store** per client syncs the person's ticker,
  timeframe, preferences and drawings across every device live.
- **The Dashboard** is the main page: up to 4 charts on one connection, a live "Most active" list,
  select a chart and pick a ticker to load it there; tap, then hold any ticker to copy it. Next,
  replay becomes a practice feature beside the live feed ("Product direction" in
  `docs/CONVENTIONS.md`).

## Structure

```
crates/       Rust: market-events, chart-engine, market-archive, market-replay, market-providers,
              paper-broker, provider-sim, server (axum: /api/*, /ws, /updates/*, serves the web app)
apps/web/     React + Vite + Tailwind v4 + vendored shadcn/ui on Base UI
apps/desktop/ Tauri v2 shell around apps/web, auto-updating
apps/mobile/  Expo SDK 58 (development build; the chart draws with Skia)
packages/     shared (protocol, WS client, settings sync), chart-core, chart-web, chart-native, app-state
infra/        Dockerfile (one image), VPS compose (server + archiver), deploy scripts, SETUP.md
docs/         conventions, architecture, configuration reference, specs, research
```

## Develop

Requirements: Bun 1.4, Rust 1.98 (pinned in `rust-toolchain.toml`).

```sh
bun install
bun dev            # server :8790 (+ simulator :8791) and Vite :5173; writes a labelled fixture day on first run
```

Open <http://localhost:5173>. Without `STOCKWATCHER_API_TOKEN` the server runs with auth off and logs a
warning. `.env.example` lists the common variables; all of them are in
[`docs/reference/configuration.md`](docs/reference/configuration.md).

- **Against production:** `STOCKWATCHER_DEV_SERVER=https://stockwatcher.wavystyle.io bun run --filter @stockwatcher/web dev`,
  then sign in with the production token.
- **Phone:** a development build on the Android emulator; Expo Go cannot run the app. Recipe and traps:
  [`docs/reference/android-dev-build.md`](docs/reference/android-dev-build.md).

```sh
bun run test       # bun tests in every workspace + cargo test
bun run lint       # oxlint + cargo fmt --check + clippy -D warnings
bun run typecheck  # tsc in every workspace
bun run build      # web build (tsc -b + vite) + release server binary
```

Release builds of the server are about 6× faster than `bun dev`'s debug server for a real replay day.

## Deploy

Work is committed straight to `main` (no feature PRs since 2026-10-20) and pushed in batches. Every push
runs `.github/workflows/release.yml`, which ships only the parts changed since the last successful
release: their checks, then in parallel the server image (deployed to the VPS with health check and
rollback), the signed Windows desktop installers (installed apps update themselves), and an Expo OTA
update for Android (a new APK only when native code changed). Details: `docs/CONVENTIONS.md` "Release pipeline"; one-time host
setup and operations (including switching replay/live): [`infra/vps/SETUP.md`](infra/vps/SETUP.md).

## Docs

| Read | For |
|---|---|
| [`docs/CONVENTIONS.md`](docs/CONVENTIONS.md) | product direction, names, ports, routes, release pipeline, mobile/desktop modules, rules |
| [`docs/architecture/chart-packages.md`](docs/architecture/chart-packages.md) | **start here for UI work**: what the chart packages hold, how to embed the chart, dashboard constraints |
| [`docs/architecture/app-state.md`](docs/architecture/app-state.md) | the Zustand store: synced, device and session state |
| [`docs/architecture/settings-sync.md`](docs/architecture/settings-sync.md) | how settings and drawings sync across devices |
| [`docs/architecture/simulator-and-chart.md`](docs/architecture/simulator-and-chart.md) | the market pipeline: engine policies, archive, replay, providers, simulator, protocol |
| [`docs/architecture/chart-trading-sim.md`](docs/architecture/chart-trading-sim.md) | simulated trading: broker rules, orders, brackets |
| [`docs/architecture/halts-luld-fills.md`](docs/architecture/halts-luld-fills.md) | halts, LULD bands, fills on the chart |
| [`docs/reference/configuration.md`](docs/reference/configuration.md) | every environment variable |
| [`docs/reference/android-dev-build.md`](docs/reference/android-dev-build.md) | running the phone app on the Android emulator |
| `docs/specs/`, `docs/research/` | the original brief and the research behind chart decisions (history) |
