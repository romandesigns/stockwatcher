# VPS setup: one time

Host `stockspotter-vps` (srv1170872), user `wavystack` (docker group, no
passwordless sudo). The steps marked **sudo** need Roman. Everything else
runs as `wavystack`. Other tenants on this box (stockspotter, Convex,
scout-ntfy) are unrelated and are not touched.

After this setup a push to `main` deploys with no manual step.

## 1. App directory

```sh
mkdir -p ~/apps/stockwatcher && cd ~/apps/stockwatcher
# From a checkout of this repo, copy the four infra/vps files:
#   docker-compose.yml  deploy.sh  publish-updates.sh  (and keep SETUP.md for reference)
chmod 755 deploy.sh publish-updates.sh
```

Check that the host has what the scripts call: `docker compose version`,
`curl --version`, `flock --version` (util-linux) and GNU `tar`.

## 2. `.env` (secrets, never committed)

```sh
cd ~/apps/stockwatcher
umask 077
{
  printf 'STOCKWATCHER_API_TOKEN=%s\n' "$(openssl rand -hex 32)"
  printf 'RUST_LOG=info\n'
} > .env
```

Clients enter the token on their sign-in screen. Read it with
`grep STOCKWATCHER_API_TOKEN .env` on the host when you need it. Don't
paste it into chats, URLs or build variables.

## 3. SSH keys for CI (two keys, two forced commands)

Generate both keys on your own machine, not the VPS. Each key can only run
its script: the `command=` option ignores whatever command the client asks
for and passes it in `SSH_ORIGINAL_COMMAND` instead.

```sh
ssh-keygen -t ed25519 -N '' -C stockwatcher-deploy  -f stockwatcher_deploy
ssh-keygen -t ed25519 -N '' -C stockwatcher-publish -f stockwatcher_publish
```

Append the two public keys to `/home/wavystack/.ssh/authorized_keys`, one
line each (the `ssh-ed25519 AAAA...` parts come from the `.pub` files):

```
command="/home/wavystack/apps/stockwatcher/deploy.sh",no-port-forwarding,no-X11-forwarding,no-agent-forwarding,no-pty ssh-ed25519 AAAA... stockwatcher-deploy
command="/home/wavystack/apps/stockwatcher/publish-updates.sh",no-port-forwarding,no-X11-forwarding,no-agent-forwarding,no-pty ssh-ed25519 AAAA... stockwatcher-publish
```

GitHub repository secrets (environment `production` or repo level):

| Secret | Value |
|---|---|
| `VPS_DEPLOY_KEY` | contents of `stockwatcher_deploy` (private key) |
| `VPS_PUBLISH_KEY` | contents of `stockwatcher_publish` (private key; used by the desktop job) |
| `VPS_HOST` | the VPS's public hostname or IP (GitHub runners aren't on the tailnet) |
| `VPS_KNOWN_HOSTS` | output of `ssh-keyscan -t ed25519 <VPS_HOST>`, checked against the host's real key (`ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub` on the VPS) |

Then delete the local private key files.

## 4. Caddy (**sudo**)

Append `Caddyfile.snippet` to `/etc/caddy/Caddyfile`, then:

```sh
sudo rm -f /var/log/caddy/stockwatcher.access.log   # a root-owned log file makes the reload fail
sudo -u caddy caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile && sudo systemctl reload caddy
```

## 5. DNS

An `A` record `stockwatcher.wavystyle.io` pointing to the VPS's public IPv4,
plus `AAAA` if the VPS has IPv6. Caddy gets the certificate on the first
request once the record resolves.

## 6. First deploy

Push to `main`, or re-run the latest Release workflow. `deploy.sh` pulls
`ghcr.io/romandesigns/stockwatcher-server:<sha>`, starts it, and waits up to
60 s for `/api/health` to report that sha. The first deploy also creates
the `stockwatcher_data` volume.

## 7. Rolling replay archive (the `archiver` service)

The replay archive keeps **the last 21 SIP trading days** (a trading month) and moves
forward by itself: the `archiver` service (same image as the server,
`swimport maintain`) imports each new day after the session (21:15 ET), deletes
days older than the window, and refreshes `assets.json` and
`daily-closes.json`. The server picks all of that up without a restart (new days
appear in the day menu, removed ones disappear, names and previous closes reload
within a minute). The window is the 21 trading days ending at L, the latest
completed trading day: each new session brings one day in and pushes exactly one
out, so the archive stays ~13 GB. On 2026-10-18 that is 2026-09-18 ..= 2026-10-16;
the 22 days seeded by hand start at 09-17, so the first cycle prunes 09-17.
Full rules: `crates/market-providers/src/import/maintain.rs`.

Safety, in short: the day the server is replaying is never deleted; if the
server cannot be asked (down, wrong token) nothing is deleted that cycle; while
imports are failing (no keys yet, Alpaca down) old days are kept rather than
letting the window shrink; below `ARCHIVE_MIN_FREE_MB` nothing is imported; a
day being imported stays out of the day menu until its first pass is done.

Do this **after** the release that adds the archiver has deployed (the image
must already contain `/app/swimport`, or the new service fails to start):

1. Add the Alpaca keys to `.env` on the VPS, typed there and never pasted into
   a chat, a URL or a file in the repo:
   ```sh
   cd ~/apps/stockwatcher
   umask 077
   read -rsp 'ALPACA_API_KEY: ' k; echo; read -rsp 'ALPACA_API_SECRET: ' s; echo
   printf 'ALPACA_API_KEY=%s\nALPACA_API_SECRET=%s\n' "$k" "$s" >> .env
   unset k s
   ```
   (Algo Trader Plus: 10,000 requests/min; the archiver stays under 9,000.)
2. Remove `REPLAY_DAY` from `.env`, so a server restart lands on the newest
   day instead of a pinned one, and make sure `ARCHIVE_RETENTION_DAYS` is unset
   or `0` (the archiver owns retention; the server's own startup prune does not
   ask which day is replayed):
   ```sh
   sed -i '/^REPLAY_DAY=/d' .env
   grep -E '^(REPLAY_|ARCHIVE_)' .env      # expect REPLAY_START=09:30 only
   ```
3. Copy the new `infra/vps/docker-compose.yml` and `infra/vps/deploy.sh` from
   a checkout of `main` into `~/apps/stockwatcher/` (scp from your machine),
   keeping `deploy.sh` executable (`chmod 755 deploy.sh`).
4. Start it (this also recreates the server with the edited `.env`, which
   restarts the replay on the newest day and resets the paper account):
   ```sh
   docker compose --env-file image.env up -d
   docker compose --env-file image.env ps     # server healthy, archiver running
   docker compose --env-file image.env logs --tail 20 archiver
   ```
   The first cycle runs at once: it plans the window (~60 requests), finds the
   seeded days complete, fetches nothing else, and logs one `maintain ok: ...`
   line. The first real import is the next trading evening at 21:15 ET
   (a SIP day is ~6,700 symbols, roughly half an hour to an hour).

Checking on it later:

```sh
docker compose --env-file image.env logs --tail 5 archiver   # one line per cycle
docker run --rm -v stockwatcher_data:/data:ro --user 65532:65532 debian:trixie-slim \
  cat /data/archive/plans/maintainer-status.json             # last cycle, next run
```

Optional, in `.env`: `ARCHIVE_WINDOW_DAYS` (default 21), `MAINTAIN_AT`
(`21:15`, ET), `MAINTAIN_JOBS` (12), `MAINTAIN_PER_MINUTE` (9000),
`MAINTAIN_RETRIES` (3) and `MAINTAIN_RETRY_DELAY_SECS` (120, doubling) for the
archiver; for the server `ARCHIVE_META_CHECK_SECS` (60) and
`REPLAY_FOLLOW_LATEST=1`, which switches the running replay to each newly
imported day (off by default: like any day switch it resets the paper account
and moves every client to the new day, typically late in the evening).

## Operating it

```sh
cd ~/apps/stockwatcher
tail -n 50 deploy.log publish.log
docker compose --env-file image.env ps          # image.env holds the live tag
docker compose --env-file image.env logs -f server
docker compose --env-file image.env logs --tail 5 archiver
curl -s http://127.0.0.1:8790/api/health
```

- **Deploy failed:** `deploy.sh` already rolled back to the previous tag.
  The log says why, and the CI job is red.
- **Manual rollback of the server:** check out the old commit in GitHub and
  re-run its Release workflow. Or on the host, set `image.env` to an older tag
  that's still present (`docker image ls ghcr.io/romandesigns/stockwatcher-server`),
  then run `docker compose --env-file image.env up -d`.
- **Roll back desktop updates:** the previous release directory is kept.
  ```sh
  docker run --rm -v stockwatcher_data:/data --user 65532:65532 debian:trixie-slim \
    sh -c 'cd /data/updates && ls desktop-releases && readlink desktop'
  docker run --rm -v stockwatcher_data:/data --user 65532:65532 debian:trixie-slim \
    sh -c 'cd /data/updates && ln -sfn desktop-releases/<previous-id> .desktop.next && mv -T .desktop.next desktop'
  ```
- **Stopping:** `docker compose --env-file image.env down` stops the
  container. The `stockwatcher_data` volume is kept, and so is its data.
