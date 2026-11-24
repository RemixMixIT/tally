#!/usr/bin/env bash
# Dead-man's switch: Speed is reachable (job ran).
set -euo pipefail
ENV_FILE="${ENV_FILE:-$HOME/.config/tally-backup.env}"
[[ -f "$ENV_FILE" ]] && source "$ENV_FILE"
url="${HC_HOST_URL:-}"
[[ -n "$url" ]] || exit 0
curl -fsS -m 15 "$url" >/dev/null
