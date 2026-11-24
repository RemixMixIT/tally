#!/usr/bin/env bash
# Nightly Tally unique-state dump → RemixMix OneDrive (rclone remote remixmix:).
# Design: CBC Design pack. Do not print .env contents.
set -euo pipefail

COMPOSE_DIR="${COMPOSE_DIR:-/home/john/apps/tally}"
VOLUME="${VOLUME:-tally_tally-data}"
REMOTE="${REMOTE:-remixmix:Backups/Speed/tally}"
STAGING="${STAGING:-/tmp/tally-unique-state}"
ENV_FILE="${ENV_FILE:-$HOME/.config/tally-backup.env}"

if [[ -f "$ENV_FILE" ]]; then
  # shellcheck disable=SC1090
  source "$ENV_FILE"
fi

ping_check() {
  local url="${1:-}"
  if [[ -n "$url" ]]; then
    curl -fsS -m 15 "$url" >/dev/null || true
  fi
}

fail_check() {
  local url="${1:-}"
  if [[ -n "$url" ]]; then
    curl -fsS -m 15 "${url%/}/fail" >/dev/null || curl -fsS -m 15 "${url}?fail" >/dev/null || true
  fi
}

on_err() {
  fail_check "${HC_TALLY_COPY_URL:-}"
  if [[ "${WAS_RUNNING:-0}" == "1" ]]; then
    docker compose -f "$COMPOSE_DIR/docker-compose.yml" start tally || true
  fi
  rm -rf "$STAGING"
}
trap on_err ERR

command -v rclone >/dev/null
command -v docker >/dev/null
rclone listremotes | grep -qx 'remixmix:' || {
  echo "rclone remote remixmix: is missing. Add OneDrive Business (RemixMix) then retry." >&2
  exit 1
}

mkdir -p "$STAGING"
chmod 700 "$STAGING"
WAS_RUNNING=0
if docker ps --format '{{.Names}}' | grep -qx tally; then
  WAS_RUNNING=1
  docker compose -f "$COMPOSE_DIR/docker-compose.yml" stop tally
fi

docker run --rm \
  -v "${VOLUME}:/data:ro" \
  -v "${STAGING}:/out" \
  alpine:3.20 \
  tar czf /out/tally-data.tar.gz -C /data .

cp -a "$COMPOSE_DIR/.env" "$STAGING/env"
cp -a "$COMPOSE_DIR/docker-compose.yml" "$STAGING/docker-compose.yml"
if [[ -f "$HOME/.config/rclone/rclone.conf" ]]; then
  cp -a "$HOME/.config/rclone/rclone.conf" "$STAGING/rclone.conf"
fi

if [[ "$WAS_RUNNING" == "1" ]]; then
  docker compose -f "$COMPOSE_DIR/docker-compose.yml" start tally
  WAS_RUNNING=0
fi

rclone copy "$STAGING" "$REMOTE" --checksum -q
ping_check "${HC_TALLY_COPY_URL:-}"
rm -rf "$STAGING"
echo "unique-state copy ok → $REMOTE"
