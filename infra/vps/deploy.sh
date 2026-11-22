#!/usr/bin/env bash
# stockwatcher server deploy. Runs on the VPS as user `wavystack`, ONLY as
# the SSH forced command of the CI deploy key (see SETUP.md):
#
#   echo "$GITHUB_TOKEN" | ssh wavystack@vps "<40-hex commit sha>"
#
# - The image tag arrives in SSH_ORIGINAL_COMMAND and must be a full sha.
# - The GHCR token arrives on stdin, never in argv, so it can't show up in
#   `ps`, shell history or this log. It is a short-lived GITHUB_TOKEN.
# - Success means /api/health answers "ok" with commit == that sha within
#   180 s. Anything else rolls back to the previous tag and exits non-zero,
#   which fails the CI job.
set -Eeuo pipefail
umask 077

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE="ghcr.io/romandesigns/stockwatcher-server"
HEALTH_URL="http://127.0.0.1:8790/api/health"
HEALTH_TIMEOUT=180
KEEP_IMAGES=3
LOG="$APP_DIR/deploy.log"
TAG_FILE="$APP_DIR/image.env"

cd "$APP_DIR"
exec > >(tee -a "$LOG") 2>&1
log() { printf '%s [deploy] %s\n' "$(date -u +%FT%TZ)" "$*"; }
die() {
  log "FAILED: $*"
  exit 1
}
trap 'log "FAILED: command on line $LINENO exited non-zero"' ERR

tag="${SSH_ORIGINAL_COMMAND:-${1:-}}"
[[ "$tag" =~ ^[0-9a-f]{40}$ ]] || die "expected a 40-hex commit sha as the command, got '${tag:0:60}'"

ghcr_token=""
IFS= read -r -t 30 ghcr_token || true
[[ -n "$ghcr_token" ]] || die "no registry token on stdin"

# One deploy at a time; a second one waits rather than interleaving.
exec 9>"$APP_DIR/.deploy.lock"
flock -w 600 9 || die "another deploy held the lock for 10 minutes"

[[ -f "$APP_DIR/.env" ]] || die "$APP_DIR/.env is missing (see SETUP.md)"
command -v curl >/dev/null || die "curl is required on the host"

logged_in=0
cleanup() {
  if ((logged_in)); then docker logout ghcr.io >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT

compose() {
  docker compose --project-directory "$APP_DIR" -f "$APP_DIR/docker-compose.yml" --env-file "$TAG_FILE" "$@"
}

set_tag() {
  printf 'STOCKWATCHER_IMAGE_TAG=%s\n' "$1" >"$TAG_FILE.tmp"
  mv -f "$TAG_FILE.tmp" "$TAG_FILE"
}

wait_healthy() {
  local want="$1" deadline=$((SECONDS + HEALTH_TIMEOUT)) body
  while ((SECONDS < deadline)); do
    body="$(curl -fsS --max-time 3 "$HEALTH_URL" 2>/dev/null || true)"
    if [[ "$body" == *'"status":"ok"'* && "$body" == *"\"commit\":\"$want\""* ]]; then
      return 0
    fi
    sleep 2
  done
  log "health never reported commit $want; last response: ${body:-<none>}"
  return 1
}

prev=""
if [[ -f "$TAG_FILE" ]]; then
  prev="$(sed -n 's/^STOCKWATCHER_IMAGE_TAG=//p' "$TAG_FILE")"
fi
log "deploying $tag (previous: ${prev:-none})"

printf '%s' "$ghcr_token" | docker login ghcr.io -u romandesigns --password-stdin >/dev/null
logged_in=1
unset ghcr_token
docker pull --quiet "$IMAGE:$tag" >/dev/null || die "pull of $IMAGE:$tag failed"
docker logout ghcr.io >/dev/null 2>&1 || true
logged_in=0

set_tag "$tag"
# Both services (server, archiver) take their tag from image.env, so this one
# `up` moves them together, and a rollback below moves them back together.
# Only the server gates the deploy: the archiver is a background job with no
# port, and a broken one is reported here and in its own log rather than
# taking the app down with it.
if compose up -d --remove-orphans && wait_healthy "$tag"; then
  log "OK $tag is live"
  archiver_id="$(compose ps -q archiver 2>/dev/null || true)"
  archiver_state=""
  if [[ -n "$archiver_id" ]]; then
    archiver_state="$(docker inspect -f '{{.State.Status}} {{.Config.Image}}' "$archiver_id" 2>/dev/null || true)"
  fi
  if [[ "$archiver_state" == "running $IMAGE:$tag" ]]; then
    log "archiver running on $tag"
  else
    log "WARNING: archiver is not running on $tag (${archiver_state:-no container}); see: docker compose --env-file image.env logs archiver"
  fi
else
  compose logs --no-color --tail 50 server || true
  if [[ -n "$prev" && "$prev" != "$tag" ]]; then
    log "rolling back to $prev"
    set_tag "$prev"
    if compose up -d --remove-orphans && wait_healthy "$prev"; then
      log "rolled back to $prev"
    else
      log "ROLLBACK FAILED: $prev is not healthy either; manual attention needed"
    fi
  fi
  die "$tag did not become healthy"
fi

# Keep the newest $KEEP_IMAGES tags (plus whatever is running); image ls
# lists newest first. A tag still used by a container just fails to delete.
mapfile -t tags < <(docker image ls "$IMAGE" --format '{{.Tag}}' | grep -E '^[0-9a-f]{40}$' || true)
for ((i = KEEP_IMAGES; i < ${#tags[@]}; i++)); do
  [[ "${tags[i]}" == "$tag" ]] && continue
  docker image rm "$IMAGE:${tags[i]}" >/dev/null 2>&1 && log "pruned ${tags[i]}" || true
done
docker image prune -f --filter "label=org.opencontainers.image.title=stockwatcher-server" >/dev/null 2>&1 || true
