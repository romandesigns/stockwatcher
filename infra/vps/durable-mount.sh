#!/usr/bin/env bash
# Run ONCE on the VPS (srv1170872) as `wavystack`; uses sudo (owner's password).
# Run infra/pi/durable-share.sh on the Pi first.
#
# Mounts the Pi's durable share (100.79.110.117:/storage/stockwatcher/durable)
# at /mnt/stockwatcher-durable, which docker-compose.yml binds into the server
# container as /durable (rslave, so a mount that arrives or returns after the
# container started is seen without a restart). LIVE_ORIGINAL_DURABLE_DIR is
# /durable/originals, a directory that exists only on the Pi: with the share
# unmounted it is missing, the recorder reports `retention_at_risk` and keeps
# its originals, and nothing is written to the VPS disk in its place.
#
# Options, and why:
#   x-systemd.automount  mounted on first access and retried on the next one
#                        after an outage, instead of once at boot
#   nofail,_netdev       a Pi that is down never blocks the VPS booting
#   soft,timeo,retrans   an unreachable Pi fails a write after ~45 s instead of
#                        hanging it; the copy runs in the recorder's housekeeping
#                        (never the market path), and a short or damaged copy
#                        fails its hash check, is not marked durable and the
#                        local original is kept
# Idempotent: re-running changes nothing.
set -euo pipefail

PI_IP=100.79.110.117
SRC="$PI_IP:/storage/stockwatcher/durable"
MNT=/mnt/stockwatcher-durable
OPTS="nfsvers=4.2,soft,timeo=150,retrans=3,noatime,nofail,_netdev,x-systemd.automount,x-systemd.mount-timeout=30"

sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nfs-common
# nfs-common pulls in rpcbind and enables it on every interface, including this host's
# public address (port 111, a classic amplification target). NFSv4 never uses it.
sudo systemctl mask --now rpcbind.socket rpcbind.service
sudo mkdir -p "$MNT"
sudo grep -qF " $MNT " /etc/fstab || echo "$SRC $MNT nfs $OPTS 0 0" | sudo tee -a /etc/fstab >/dev/null
sudo systemctl daemon-reload
sudo systemctl restart remote-fs.target

# Touching the path triggers the automount.
if [ -d "$MNT/originals" ]; then
  echo "ok: $MNT mounted from $SRC"
  df -h "$MNT" | tail -1
else
  echo "NOT mounted: $MNT/originals is missing. Check the Pi script ran, then:" >&2
  echo "  tailscale ping $PI_IP; journalctl -u 'mnt-stockwatcher\\x2ddurable.*' -n 20" >&2
  exit 1
fi
