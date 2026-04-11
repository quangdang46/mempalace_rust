#!/bin/bash
# MEMPALACE SAVE HOOK — Auto-save every N exchanges
#
# Rust wrapper around `mpr hook run --hook stop`.
# Reads hook JSON from stdin and emits JSON to stdout.

set -euo pipefail

HARNESS="${1:-${MEMPALACE_HOOK_HARNESS:-claude-code}}"
case "$HARNESS" in
  claude-code|codex) ;;
  *)
    echo "Unsupported harness: $HARNESS" >&2
    exit 1
    ;;
esac

BIN="${MEMPALACE_BIN:-mpr}"
exec "$BIN" hook run --hook stop --harness "$HARNESS"
