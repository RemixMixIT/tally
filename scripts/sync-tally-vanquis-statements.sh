#!/usr/bin/env bash
# Copy Vanquis statement PDFs from Dropbox onto Speed inbound, ask Tally to
# import, then file successful drops onto OneDrive (canonical archive name).
#
# Drop (Windows): Dropbox\!Inbox\Documents\Vanquis Statements\  (folder root; Dropbox is the inbox)
# Archive (Windows): OneDrive\Documents\Financial\Vanquis Statements\
# Archive name: YYYYMM -MonthlyStatement.pdf  (space before the hyphen)
# Do not create a nested Inbox under the drop folder.
set -euo pipefail

DROP="${VANQUIS_DROP:-dropbox:!Inbox/Documents/Vanquis Statements}"
ARCHIVE="${VANQUIS_ARCHIVE:-onedrive:Documents/Financial/Vanquis Statements}"
INBOUND="${INBOUND:-/home/john/apps/tally/data/inbound/vanquis}"
LOG="${LOG:-/home/john/apps/rclone-tally-vanquis.log}"

mkdir -p "$INBOUND/inbox" "$INBOUND/archive"

# PDFs in the Dropbox drop folder root only. Do not mkdir or copy a nested Inbox.
rclone copy "$DROP" "$INBOUND/inbox" \
  --filter "+ *.pdf" \
  --filter "- *" \
  --max-depth 1 \
  --log-file="$LOG" --log-level=ERROR

REPORT="$(mktemp)"
trap 'rm -f "$REPORT"' EXIT

if ! docker exec tally curl -fsS -X POST http://127.0.0.1:3001/internal/vanquis/import >"$REPORT"; then
  echo "tally vanquis import POST failed" >&2
  exit 1
fi

python3 - "$REPORT" "$INBOUND" "$DROP" "$ARCHIVE" <<'PY'
import json, os, subprocess, sys

report_path, inbound, drop, archive = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
with open(report_path, encoding="utf-8") as f:
    data = json.load(f)
report = data.get("report") or data
inbox_root = os.path.join(inbound, "inbox")

def rclone(*args):
    r = subprocess.run(["rclone", *args], capture_output=True, text=True)
    if r.returncode != 0:
        sys.stderr.write(r.stderr or r.stdout or f"rclone {' '.join(args)} failed\n")
    return r.returncode == 0

def dropbox_gone(name):
    rclone("deletefile", f"{drop}/{name}")

for fr in report.get("files") or []:
    if fr.get("source") != "inbox" or not fr.get("ok"):
        continue
    src = fr.get("path") or ""
    name = fr.get("original_name") or os.path.basename(src)
    dest_name = fr.get("archive_name")
    if not dest_name:
        print(f"skip filing {name}: no archive_name", file=sys.stderr)
        continue
    local = src if os.path.isfile(src) else os.path.join(inbox_root, name)
    if not os.path.isfile(local):
        print(f"skip filing {name}: local copy missing", file=sys.stderr)
        continue
    dest = f"{archive}/{dest_name}"
    exists = subprocess.run(
        ["rclone", "lsf", dest], capture_output=True, text=True
    )
    if exists.returncode == 0 and exists.stdout.strip():
        dropbox_gone(name)
        try:
            os.remove(local)
        except FileNotFoundError:
            pass
        print(f"filed skip (exists): {name} -> {dest}")
        continue
    if not rclone("copyto", local, dest):
        print(f"failed to copy {name} to archive as {dest_name}", file=sys.stderr)
        continue
    dropbox_gone(name)
    try:
        os.remove(local)
    except FileNotFoundError:
        pass
    print(f"filed: {name} -> {dest}")
PY
