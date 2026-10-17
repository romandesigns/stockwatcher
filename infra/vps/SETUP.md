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

## Operating it

```sh
cd ~/apps/stockwatcher
tail -n 50 deploy.log publish.log
docker compose --env-file image.env ps          # image.env holds the live tag
docker compose --env-file image.env logs -f server
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
