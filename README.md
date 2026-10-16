# stockwatcher

Private monorepo for stockwatcher: a Rust server, a React web app, and,
next, a Tauri desktop shell and an Expo mobile app. All of them share one
protocol package. It's served at <https://stockwatcher.wavystyle.io>.

So far it is **boilerplate**: server, auth, realtime connection, a Hello world web page and the
deploy pipeline. It has no UI beyond that and no domain features. Fixed decisions
(names, ports, routes, auth, release pipeline) are in
[`docs/CONVENTIONS.md`](docs/CONVENTIONS.md).

## Structure

```
crates/server/       Rust (axum): /api/health, /api/version, /ws, /updates/*, serves apps/web/dist
apps/web/            Vite + React + TS + Tailwind v4 + shadcn/ui setup (no components yet); one Hello world page
apps/desktop/        Tauri v2 shell (to be added)
apps/mobile/         Expo app (to be added)
packages/shared/     protocol types, authenticatedFetch, WebSocket client (reconnect + hello)
infra/docker/        Dockerfile: one image, web + server, distroless, non-root
infra/vps/           compose file, deploy.sh, publish-updates.sh, Caddy snippet, SETUP.md
.github/workflows/   ci.yml (pull requests), release.yml (every push to main)
```

## Develop

Requirements: Bun 1.4, Rust 1.98 (pinned in `rust-toolchain.toml`).

```sh
bun install
bun dev            # server on :8790 + Vite on :5173 (proxies /api, /ws, /updates)
```

Open <http://localhost:5173>. Without `STOCKWATCHER_API_TOKEN` the server runs
with auth off and logs a warning. To try the sign-in screen, set a token of at
least 32 characters in a root `.env` (copy `.env.example`) or in your shell,
then restart `bun dev`.

```sh
bun run test       # bun tests (shared, web) + cargo test
bun run lint       # oxlint + cargo fmt --check + clippy -D warnings
bun run typecheck  # tsc in every workspace
bun run build      # web build (tsc -b + vite) + release server binary
```

Run the production image locally:

```sh
docker build -f infra/docker/Dockerfile --build-arg GIT_SHA=$(git rev-parse HEAD) -t stockwatcher-server:dev .
docker run --rm -p 127.0.0.1:8790:8790 stockwatcher-server:dev
```

## Server configuration

| Env | Default | Notes |
|---|---|---|
| `STOCKWATCHER_API_TOKEN` | unset (auth off) | ≥32 chars. Guards `/api/*` (not `/api/health`) and `/ws` (token in the `hello` message) |
| `PORT` | `8790` | |
| `HOST` | `127.0.0.1` | the image sets `0.0.0.0` |
| `STATIC_DIR` | `./apps/web/dist` | `/app/web` in the image |
| `DATA_DIR` | `./data` | `/app/data` in the image (volume `stockwatcher_data`); `updates/` under it is served at `/updates/*` |
| `RUST_LOG` | `info` | tracing filter |
| `LOG_FORMAT` | pretty | `json` in the image |

## Deploy

Every push to `main` runs `release.yml`:

1. `checks` (the same workflow PRs run).
2. `server` builds `ghcr.io/romandesigns/stockwatcher-server:<sha>`, then SSHes to
   the VPS with a key whose forced command is `deploy.sh`. That script pulls the
   image, restarts it, waits for `/api/health` to report the sha, and rolls back
   if it doesn't.

The desktop and mobile jobs are added alongside `server`. One-time host setup is
in [`infra/vps/SETUP.md`](infra/vps/SETUP.md).
