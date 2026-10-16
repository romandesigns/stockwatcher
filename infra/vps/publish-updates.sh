#!/usr/bin/env bash
# Publishes desktop update artifacts (installers, .sig files, latest.json)
# into the server's /updates/desktop/. Runs on the VPS as user `wavystack`,
# ONLY as the SSH forced command of the CI publish key (see SETUP.md):
#
#   tar -czf - -C out desktop | ssh wavystack@vps "<optional label, e.g. 0.1.42>"
#
# stdin: a gzipped tar whose only members are `desktop/` and regular files
# `desktop/<name>.{json,exe,msi,sig}`, including desktop/latest.json.
#
# Where it lands: the named volume `stockwatcher_data` (mounted at /app/data
# in the server), which `wavystack` can't touch directly (it's root-owned
# under /var/lib/docker). So the swap runs in a throwaway container with
# that volume mounted. Layout inside the volume:
#
#   updates/desktop-releases/<id>/...           one directory per publish
#   updates/desktop -> desktop-releases/<id>    symlink, swapped by rename(2)
#
# The symlink swap is atomic: a client sees the old set or the new set,
# never a half-written one. The previous release directory is kept for
# rollback (see SETUP.md); older ones are deleted.
set -Eeuo pipefail
umask 077

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG="$APP_DIR/publish.log"
# Overridable only for local testing; SSH does not forward env to a forced command.
VOLUME="${STOCKWATCHER_VOLUME:-stockwatcher_data}"
# Any image with GNU tar + coreutils (mv -T). Pinned tag, pulled once.
HELPER_IMAGE="debian:trixie-slim"
MAX_ARCHIVE_BYTES=$((200 * 1024 * 1024))
MAX_UNPACKED_BYTES=$((400 * 1024 * 1024))
MAX_ENTRIES=50

exec > >(tee -a "$LOG") 2>&1
log() { printf '%s [publish] %s\n' "$(date -u +%FT%TZ)" "$*"; }
die() {
  log "REJECTED: $*"
  exit 1
}
trap 'log "FAILED: command on line $LINENO exited non-zero"' ERR

label="${SSH_ORIGINAL_COMMAND:-${1:-}}"
[[ -z "$label" || "$label" =~ ^[0-9A-Za-z.+-]{1,40}$ ]] || die "label must match [0-9A-Za-z.+-]{1,40}"
id="$(date -u +%Y%m%dT%H%M%SZ)${label:+-$label}"

exec 9>"$APP_DIR/.publish.lock"
flock -w 600 9 || die "another publish held the lock for 10 minutes"

docker volume inspect "$VOLUME" >/dev/null 2>&1 || die "volume $VOLUME does not exist yet; deploy the server first"

work="$(mktemp -d "${TMPDIR:-/tmp}/stockwatcher-publish.XXXXXX")"
trap 'rm -rf "$work"' EXIT
archive="$work/upload.tar.gz"

# Read at most cap+1 bytes, so an oversized upload is detected without
# ever being stored in full.
head -c $((MAX_ARCHIVE_BYTES + 1)) >"$archive"
size="$(stat -c %s "$archive")"
((size > 0)) || die "empty upload"
((size <= MAX_ARCHIVE_BYTES)) || die "upload exceeds $MAX_ARCHIVE_BYTES bytes"

# --- validate every member before anything is extracted ---------------------
tar -tzf "$archive" >"$work/names" 2>/dev/null || die "not a gzipped tar"
tar -tvzf "$archive" >"$work/long" 2>/dev/null || die "not a gzipped tar"
entries="$(wc -l <"$work/names")"
((entries > 0 && entries <= MAX_ENTRIES)) || die "expected 1..$MAX_ENTRIES members, got $entries"

name_re='^desktop/([A-Za-z0-9][A-Za-z0-9._+-]{0,127}\.(json|exe|msi|sig))?$'
while IFS= read -r name; do
  [[ "$name" =~ $name_re ]] || die "disallowed member name: ${name:0:100}"
  [[ "$name" != *..* ]] || die "'..' in member name: ${name:0:100}"
done <"$work/names"
grep -qx 'desktop/latest.json' "$work/names" || die "desktop/latest.json is missing"

# Long listing: first char is the type. Only directories and regular files;
# no symlinks, hardlinks or devices. Field 3 is the size (GNU tar).
total=0
while read -r mode _owner bytes _rest; do
  case "${mode:0:1}" in
    -) total=$((total + bytes)) ;;
    d) ;;
    *) die "member of type '${mode:0:1}' not allowed (regular files only)" ;;
  esac
done <"$work/long"
((total <= MAX_UNPACKED_BYTES)) || die "unpacked size $total exceeds $MAX_UNPACKED_BYTES"

# --- extract to staging inside the volume, then swap the symlink ------------
# Runs as the server's uid (65532) so the files match the rest of the volume.
docker run --rm -i --network none --user 65532:65532 \
  -e RELEASE_ID="$id" -v "$VOLUME:/data" "$HELPER_IMAGE" \
  bash -Eeuo pipefail -c '
    cd /data
    mkdir -p updates/desktop-releases
    cd updates
    staging="desktop-releases/.incoming-$RELEASE_ID"
    rm -rf "$staging"
    mkdir "$staging"
    tar -xzf - -C "$staging" --no-same-owner --no-same-permissions
    test -s "$staging/desktop/latest.json"
    mv "$staging/desktop" "desktop-releases/$RELEASE_ID"
    rmdir "$staging"
    chmod -R a+rX "desktop-releases/$RELEASE_ID"

    previous=""
    if [ -L desktop ]; then
      previous="$(readlink desktop)"
    elif [ -d desktop ]; then
      # A real directory from before this script existed: keep it as a
      # release so the swap below can replace the path with a symlink.
      mv desktop "desktop-releases/legacy-$RELEASE_ID"
      previous="desktop-releases/legacy-$RELEASE_ID"
    fi

    ln -sfn "desktop-releases/$RELEASE_ID" .desktop.next
    mv -T .desktop.next desktop

    # Keep the new release and the one it replaced; drop the rest.
    for dir in desktop-releases/*; do
      [ "$dir" = "desktop-releases/$RELEASE_ID" ] && continue
      [ -n "$previous" ] && [ "$dir" = "$previous" ] && continue
      rm -rf "$dir"
    done
    echo "previous=${previous:-none}"
  ' <"$archive" | while IFS= read -r line; do log "$line"; done

log "OK published $id ($entries members, $total bytes)"
