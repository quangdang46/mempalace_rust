#!/bin/bash
# MEMPALACE PRE-COMPACT HOOK — Emergency save before compaction
#
# Rust wrapper around `mpr hook run --hook precompact`.
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
exec "$BIN" hook run --hook precompact --harness "$HARNESS"
