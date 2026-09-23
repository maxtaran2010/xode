#!/usr/bin/env bash
# Sync the working tree to the Windows test box (no git needed there).
set -euo pipefail
HOST="${XODE_WIN_HOST:-max@192.168.2.87}"
DEST="${XODE_WIN_DIR:-C:/src/xode}"
cd "$(dirname "$0")/.."
ssh "$HOST" "if not exist \"${DEST//\//\\}\" mkdir \"${DEST//\//\\}\""
COPYFILE_DISABLE=1 tar --no-mac-metadata -czf - \
  --exclude ./target --exclude node_modules --exclude .git --exclude '._*' --exclude .DS_Store \
  --exclude ./apps/desktop/src-tauri/target . \
  | ssh "$HOST" "tar -xzf - -C $DEST"
