#!/bin/bash
# MEMPALACE GENERIC HOOK — dispatch any lifecycle hook kind to the Rust CLI.
#
# Usage: mempal_hook.sh <kind> [harness]
#   <kind>: session-start | session-end | stop | precompact | post-tool-use |
#           post-tool-failure | prompt-submit | notification |
#           subagent-start | subagent-stop | task-completed
#
# Reads hook JSON from stdin and emits JSON to stdout, mirroring mempalace's
# per-hook scripts. The named wrappers (mempal_save_hook.sh / mempal_precompact_hook.sh)
# remain for back-compat; this generic form covers the remaining kinds.

set -euo pipefail

HOOK="${1:?usage: mempal_hook.sh <kind> [harness]}"
HARNESS="${2:-${MEMPALACE_HOOK_HARNESS:-claude-code}}"
case "$HARNESS" in
  claude-code|codex) ;;
  *)
    echo "Unsupported harness: $HARNESS" >&2
    exit 1
    ;;
esac

BIN="${MEMPALACE_BIN:-mpr}"
exec "$BIN" hook run --hook "$HOOK" --harness "$HARNESS"
