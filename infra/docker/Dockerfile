# syntax=docker/dockerfile:1.7
# One image: builds apps/web and crates/server, runs the server, which serves
# the web app, /api, /ws and /updates. It also carries `swimport` (the
# importer, feature `import`), which the VPS runs as a second compose service
# (`archiver`: `swimport maintain`, the rolling one-month replay archive) on
# the same volume. Same image so the two can never disagree about the archive
# format; the server stays the ENTRYPOINT and the archiver overrides it.
# Build from the repo root:
#   docker build -f infra/docker/Dockerfile --build-arg GIT_SHA=$(git rev-parse HEAD) .
#
# Cache layout: dependency layers (bun install, cargo chef cook) depend only
# on manifests/lockfiles, so source-only changes rebuild just the app code.
# That holds for registry/GHA layer caches too, not only BuildKit cache mounts.

# --- inputs: split the context into a JS-manifest tree and a Rust tree ---------
FROM rust:1.98-slim-trixie AS inputs
WORKDIR /ctx
COPY . .
# bun install --frozen-lockfile needs EVERY workspace manifest (apps/mobile,
# apps/desktop included) or the lockfile looks out of date. The Rust tree
# must include every Cargo workspace member (apps/desktop/src-tauri once it
# is one), or cargo refuses to load the workspace.
RUN set -eu; \
    mkdir -p /js /rs; \
    cp package.json bun.lock /js/; \
    find apps packages -maxdepth 2 -name package.json -exec cp --parents {} /js/ \; ; \
    cp Cargo.toml Cargo.lock /rs/; \
    cp -r crates /rs/; \
    if [ -d apps/desktop/src-tauri ]; then \
      mkdir -p /rs/apps/desktop/src-tauri; \
      cp apps/desktop/src-tauri/Cargo.toml /rs/apps/desktop/src-tauri/; \
      cp -r apps/desktop/src-tauri/src /rs/apps/desktop/src-tauri/; \
      if [ -f apps/desktop/src-tauri/build.rs ]; then cp apps/desktop/src-tauri/build.rs /rs/apps/desktop/src-tauri/; fi; \
    fi; \
    mkdir -p /data/updates

# --- web ---------------------------------------------------------------------
FROM oven/bun:1.4.0-slim AS web
WORKDIR /src
COPY --from=inputs /js/ ./
RUN --mount=type=cache,target=/root/.bun/install/cache \
    bun install --frozen-lockfile --filter @stockwatcher/web
# Every package: apps/web imports chart-core and chart-web too (node_modules
# stay out via .dockerignore).
COPY packages packages
COPY apps/web apps/web
RUN bun run --filter @stockwatcher/web build

# --- server ------------------------------------------------------------------
FROM lukemathwalker/cargo-chef:latest-rust-1.98-slim-trixie AS chef
WORKDIR /src

FROM chef AS planner
COPY --from=inputs /rs/ ./
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS server
COPY --from=planner /src/recipe.json recipe.json
# Two cooks, two builds: built in ONE cargo invocation, feature unification
# would switch market-providers' `import` feature on for the server too and
# link an HTTP client into it that it never uses. Separately, the server
# binary is exactly what it was before swimport joined the image; the second
# cook only adds the importer's own dependencies (reqwest, flate2).
RUN cargo chef cook --release --locked -p stockwatcher-server --recipe-path recipe.json \
 && cargo chef cook --release --locked -p market-providers --features import --recipe-path recipe.json
COPY --from=inputs /rs/ ./
# Declared here, after the cook, so a new commit doesn't bust the deps layer.
ARG GIT_SHA=unknown
RUN GIT_SHA="${GIT_SHA}" cargo build --release --locked -p stockwatcher-server \
 && cargo build --release --locked -p market-providers --features import --bin swimport \
 && cp target/release/stockwatcher-server /stockwatcher-server \
 && cp target/release/swimport /swimport

# --- runtime -----------------------------------------------------------------
# distroless/cc: glibc + libgcc, no shell, no package manager; runs as
# uid 65532 ("nonroot"). Debian 13, matching the trixie build stages.
FROM gcr.io/distroless/cc-debian13:nonroot
ARG GIT_SHA=unknown
LABEL org.opencontainers.image.source="https://github.com/romandesigns/stockwatcher" \
      org.opencontainers.image.revision="${GIT_SHA}" \
      org.opencontainers.image.title="stockwatcher-server"
WORKDIR /app
COPY --from=server /stockwatcher-server /app/stockwatcher-server
COPY --from=server /swimport /app/swimport
COPY --from=web /src/apps/web/dist /app/web
# Pre-created and owned by nonroot: an empty named volume mounted here
# copies this ownership, so the server can write without running as root.
COPY --from=inputs --chown=65532:65532 /data /app/data
ENV HOST=0.0.0.0 \
    PORT=8790 \
    STATIC_DIR=/app/web \
    DATA_DIR=/app/data \
    ARCHIVE_ROOT=/app/data/archive \
    LOG_FORMAT=json \
    RUST_LOG=info
USER nonroot
EXPOSE 8790
# No curl in distroless: the binary checks itself (GET /api/health on loopback).
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["/app/stockwatcher-server", "--healthcheck"]
ENTRYPOINT ["/app/stockwatcher-server"]
