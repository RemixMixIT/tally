#!/usr/bin/env bash
# Ping only if Tally /healthz succeeds. Silence or /fail = Tally down.
set -euo pipefail
ENV_FILE="${ENV_FILE:-$HOME/.config/tally-backup.env}"
[[ -f "$ENV_FILE" ]] && source "$ENV_FILE"
url="${HC_TALLY_URL:-}"
[[ -n "$url" ]] || exit 0
if curl -fsS -m 10 http://127.0.0.1:3001/healthz >/dev/null; then
  curl -fsS -m 15 "$url" >/dev/null
else
  curl -fsS -m 15 "${url%/}/fail" >/dev/null || true
  exit 1
fi
