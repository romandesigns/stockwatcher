#!/usr/bin/env bash
# Run ONCE on the Pi (stockspotter-pi, tailnet 100.79.110.117) as `pi`; uses sudo.
#
# Exports /storage/stockwatcher/durable over NFSv4 to the VPS only (srv1170872,
# tailnet 100.88.87.41). The VPS recorder copies each sealed preserved original
# into durable/originals/<day>/ and deletes its own copy only once that one is
# verified (PR #52, LIVE_ORIGINAL_DURABLE_DIR; crates/market-archive/src/retention.rs).
#
# `originals/` must exist ONLY here, never on the VPS: an unmounted mount point
# is an empty directory, and the recorder treats "LIVE_ORIGINAL_DURABLE_DIR is
# a directory" as "available". Pointing it at durable/originals makes an
# unmounted share read as unavailable (originals kept on the VPS) instead of
# silently filling the VPS disk.
#
# Traffic stays inside the tailnet (WireGuard). all_squash maps every write to
# `pi`, so the container's uid (65532) does not need to exist here.
# Idempotent: re-running changes nothing.
set -euo pipefail

VPS_IP=100.88.87.41
DIR=/storage/stockwatcher/durable
U=$(id -u pi)
G=$(id -g pi)

sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nfs-kernel-server

mkdir -p "$DIR/originals"
cat > "$DIR/README.txt" <<'EOF'
Durable copies of StockWatcher's preserved original recordings (PR #52), written by
the VPS (srv1170872) over NFSv4 through the tailnet. Each sealed trading day lands in
originals/<day>/ with a verified durable-copy.json; only then does the VPS delete its
local copy (past LIVE_ORIGINAL_DAYS). Once released, this is the ONLY copy: never
delete or edit anything under originals/. Setup: stockwatcher infra/pi/durable-share.sh.
EOF

# NFSv4 only: v4 needs neither rpcbind nor mountd on the wire.
if ! sudo grep -q '^vers3=n' /etc/nfs.conf; then
  if sudo grep -q '^\[nfsd\]' /etc/nfs.conf; then
    sudo sed -i '/^\[nfsd\]/a vers3=n' /etc/nfs.conf
  else
    printf '[nfsd]\nvers3=n\n' | sudo tee -a /etc/nfs.conf >/dev/null
  fi
fi

LINE="$DIR $VPS_IP(rw,sync,no_subtree_check,all_squash,anonuid=$U,anongid=$G)"
sudo grep -qF "$DIR " /etc/exports || echo "$LINE" | sudo tee -a /etc/exports >/dev/null

sudo systemctl enable nfs-server >/dev/null
sudo systemctl restart nfs-server
sudo exportfs -ra
sudo exportfs -v | grep -F "$DIR"
echo "ok: $DIR exported to $VPS_IP (NFSv4). Free: $(df -h --output=avail "$DIR" | tail -1)"
