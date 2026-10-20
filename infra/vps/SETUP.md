# VPS setup: one time

Host `stockspotter-vps` (srv1170872), user `wavystack` (docker group, no
passwordless sudo). The steps marked **sudo** need Roman. Everything else
runs as `wavystack`. Other tenants on this box (stockspotter, Convex,
scout-ntfy) are unrelated and are not touched.

After this setup a push to `main` deploys with no manual step.

## 1. App directory

```sh
mkdir -p ~/apps/stockwatcher && cd ~/apps/stockwatcher
# From a checkout of this repo, copy the infra/vps files:
#   docker-compose.yml  deploy.sh  publish-updates.sh  (Caddyfile.snippet is for step 4;
#   keep SETUP.md for reference)
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
  printf 'MARKET_DATA_MODE=replay\n'     # or live: see "Market data mode" below
  printf 'REPLAY_START=09:30\n'          # the server's default is 10:40
} > .env
```

Every variable the server and the archiver read is listed in
`docs/reference/configuration.md`.

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
out, so the archive stays ~13 GB. (When this was set up on 2026-10-18 the window
was 2026-09-18 ..= 2026-10-16; the 22 days seeded by hand started at 09-17, so the
first cycle pruned 09-17.) Full rules: `crates/market-providers/src/import/maintain.rs`.

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
`PRICE_BAND_MIN` / `PRICE_BAND_MAX` (`0.10` / `25.00`) set the candidate band for
both: the server's search and chart candidates, and the archiver's import.

## 8. Market data mode (replay or live)

Since 2026-10-18 the server runs **both**: replay on the archive, and live
(Alpaca SIP) whenever the Alpaca keys from step 7 are in `.env`. The dashboard
reads live and the chart page replay, each by asking for it. **Alpaca allows one
stream per account on most plans, so put the keys on one host only**: any server
that has them now streams live.

`MARKET_DATA_MODE` is only the default for a client that names no source (apps
built before the split). Changing it recreates the server, so every client
reconnects and both paper accounts reset:

```sh
cd ~/apps/stockwatcher
cp .env .env.bak-$(date -u +%Y%m%dT%H%M%SZ)
sed -i 's/^MARKET_DATA_MODE=.*/MARKET_DATA_MODE=live/' .env     # or =replay
docker compose --env-file image.env up -d     # plain `up` fails: the image tag is in image.env
docker logs --since 1m stockwatcher-server-1 2>&1 | grep 'market data provider started'
```

Two log lines, one per market, name the mode and source (`alpaca sip stream`,
`archive <day>`); without keys the live one says `not_configured`.
A client that stayed connected across the switch may keep the old mode's clock
until it is reopened (seen 2026-10-18).

**Live recording (restarts keep the day's live charts).** The live market
records what it receives into the archive itself, as a provisional day:
`/app/data/archive/<trading date>/live/` (a directory per day, one file per
symbol). After a restart the dashboard is up at once and loads each symbol's
day back from there (charts on request, the rest in the background); on a
weekend it shows the last session, labelled with its date. The archiver's
nightly import of the same day **replaces** the recording: while it imports,
the day's `.importing` marker also stops the recorder from writing to it, and
when it clears the marker it deletes `live/`. Recordings no import replaced
are kept for 5 days (`LIVE_RECORD_DAYS`); a whole-market day is roughly
1 GB (see `docs/architecture/simulator-and-chart.md` "Live recording"). To
check it:

```sh
docker run --rm -v stockwatcher_data:/data:ro --user 65532:65532 debian:trixie-slim \
  sh -c 'du -sh /data/archive/*/live'
docker logs --since 10m stockwatcher-server-1 2>&1 | grep -E 'live (recorder|history)'
```

`LIVE_RECORD=0` in `.env` stops recording (what is already in the archive is
still loaded). The day being written must not be deleted by hand while the
server runs; any other `live/` directory may be.

## 9. Durable copy of preserved originals (the Pi)

The live recorder keeps each day's untouched original recording (PR #52) in
`archive/live-original/`, ~0.8 GB per whole-market day. It keeps the last 5
trading days (`LIVE_ORIGINAL_DAYS`) and deletes an older one only once a
verified copy exists on the Pi: copied, flushed, read back and hash-checked
there, then re-checked at release. With no copy nothing is deleted, so without
this step the VPS disk grows by that much every trading day.

The Pi (`stockspotter-pi`) exports `/storage/stockwatcher/durable` over NFSv4,
to this host's tailnet address only. The host mounts it at
`/mnt/stockwatcher-durable`, the server sees it as `/durable`, and
`LIVE_ORIGINAL_DURABLE_DIR=/durable/originals` (set in `docker-compose.yml`).
`originals/` exists only on the Pi, so when the share is not mounted the path
is missing: the recorder reports `retention_at_risk` and keeps its originals,
rather than "copying" them onto this disk.

1. On the Pi, from a checkout of `main`: `bash infra/pi/durable-share.sh`
   (uses the Pi's sudo).
2. Here, from a checkout of `main`: `bash infra/vps/durable-mount.sh` (**sudo**).
   It ends with `ok: ... mounted`.
3. Copy the new `infra/vps/docker-compose.yml` into `~/apps/stockwatcher/` and
   recreate the server (this restarts it, like any `.env` change):
   ```sh
   docker compose --env-file image.env up -d
   ```

Checking on it (status is rewritten at startup and at every day change):

```sh
docker run --rm -v stockwatcher_data:/data:ro --user 65532:65532 debian:trixie-slim \
  cat /data/archive/plans/retention-status.json      # "status": "ok" or "retention_at_risk"
ls /mnt/stockwatcher-durable/originals                # one directory per copied day
```

The Pi's `/storage` is two NVMe drives in RAID 0: one drive failing loses the
copies there, but the VPS still holds the newest 5 days, and the copies are a
second machine rather than the only one. Never delete under `originals/` on
the Pi: past the window it is the only copy.

## 10. Ross Panel (stock context)

The Ross Panel lists stocks passing all five pillars (price, day gain, relative volume, float,
news; `docs/stock-context/`). The server only **reads** evidence from the archive volume; it never
calls a provider. Three things feed it:

- the `archiver` (step 7) already stores the daily bars, the nightly split-adjusted set and the
  corporate-action checks (03:30 ET, then every 30 min) that give the gain and RVOL pillars a
  checked previous close and a 30-session baseline;
- the new `evidence` service (`swimport evidence`, same image) stores float (FMP, once per
  trading day at 07:00 ET, the whole market in ~10 bulk requests: bulk is the default, nothing to
  set) and news (Alpaca/Benzinga, every 60 s from 04:00 to 20:00 ET);
- `STOCK_CONTEXT=1` turns the panel on in the server, which then picks up new evidence every
  15 s (`CONTEXT_EVIDENCE_CHECK_SECS`).

Do this **after** the release that adds the `evidence` service to the compose file has deployed.

1. Add the FMP key to `.env` on the VPS, typed there and never pasted into a chat, a URL or a
   file in the repo. The Alpaca keys from step 7 are reused for news; check they are there
   without printing them:
   ```sh
   cd ~/apps/stockwatcher
   umask 077
   read -rsp 'FMP_API_KEY: ' f; echo
   printf 'FMP_API_KEY=%s\n' "$f" >> .env
   unset f
   grep -cE '^(FMP_API_KEY|ALPACA_API_KEY|ALPACA_API_SECRET)=' .env   # expect 3
   ```
2. Copy the new `infra/vps/docker-compose.yml` from a checkout of `main` into
   `~/apps/stockwatcher/` (scp from your machine) and start the collector. The `.env` change also
   recreates the server and the archiver (same `.env`), which restarts the replay and resets the
   paper accounts:
   ```sh
   docker compose --env-file image.env up -d
   docker compose --env-file image.env ps                    # evidence running
   docker compose --env-file image.env logs --tail 20 evidence
   ```
   The first line must say `Alpaca keys set, FMP key set`. The first news search covers the last
   25 h at once; on a trading day after 07:00 ET the float refresh runs right away, otherwise at
   the next 07:00 ET.
3. Turn the panel on and recreate:
   ```sh
   cp .env .env.bak-$(date -u +%Y%m%dT%H%M%SZ)
   printf 'STOCK_CONTEXT=1\n' >> .env
   docker compose --env-file image.env up -d
   docker logs --since 2m stockwatcher-server-1 2>&1 | grep -E 'stock context evidence|STOCK_CONTEXT'
   ```
   Expect one `stock context evidence loaded` line per market (live and replay), with record
   counts and, for the baseline rebuild, the number of symbols it looked at (`universe`). A line
   containing `STOCK_CONTEXT:` and `context disabled` means `PRICE_BAND_*` is not $0.10–$25:
   remove the override or the panel stays off.

Checking on it later:

```sh
docker compose --env-file image.env logs --since 30m evidence     # one line per tick with requests
docker logs --since 30m stockwatcher-server-1 2>&1 | grep 'stock context evidence'
docker run --rm -v stockwatcher_data:/data:ro --user 65532:65532 debian:trixie-slim \
  sh -c 'ls -l /data/archive/evidence/*/; tail -n 1 /data/archive/evidence/news-coverage/*.jsonl'
docker run --rm -v stockwatcher_data:/data:ro --user 65532:65532 debian:trixie-slim \
  sh -c 'ls /data/archive/corporate-actions/$(TZ=America/New_York date +%F) | tail -n 3'
```

The last news-coverage line's `observed_at` (UTC ns) should be under two minutes old during
04:00–20:00 ET; older than that and every "no news" pillar turns UNKNOWN instead of FAIL. A
trading day without a `corporate-actions/<today>` directory (the archiver's 03:30 ET check)
leaves every gain UNKNOWN, so nothing reaches 5/5: check the archiver's log and
`plans/maintainer-status.json` (step 7). Before 07:00 ET floats are the previous day's and turn
stale after 24 h, so an early premarket mover can lack its float until the refresh.

To turn the panel off: remove the `STOCK_CONTEXT=1` line and `up -d` again. The collector can
keep running (it only appends to the volume) or be stopped with
`docker compose --env-file image.env stop evidence`; with `restart: unless-stopped` a stopped
service stays stopped until the next `up -d`.

## Operating it

```sh
cd ~/apps/stockwatcher
tail -n 50 deploy.log publish.log
docker compose --env-file image.env ps          # image.env holds the live tag
docker compose --env-file image.env logs -f server
docker compose --env-file image.env logs --tail 5 archiver
docker compose --env-file image.env logs --tail 5 evidence
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
