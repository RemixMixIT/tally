#!/usr/bin/env bash
# Copy Batty's published Tally valuation CSV from Google Drive onto Speed.
# Tally reads data/inbound/tally_valuations.csv inside the container.
set -euo pipefail
DEST="${DEST:-/home/john/apps/tally/data/inbound}"
mkdir -p "$DEST"
rclone copy "gdrive:_outbound/tally" "$DEST" --include "tally_valuations.csv"
