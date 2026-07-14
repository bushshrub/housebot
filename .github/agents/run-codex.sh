#!/usr/bin/env bash
# .github/agents/run-codex.sh — Codex coding-agent adapter.
#
# Usage: run-codex.sh <prompt_file> <model> <effort>
#
# Authentication: Codex uses an OAuth session stored on the runner.
# No API key is read or exposed by this script.
#
# The prompt is read from a file and passed to codex via stdin to avoid
# expanding issue content in shell argument position.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=common.sh
source "$SCRIPT_DIR/common.sh"

PROMPT_FILE="$1"
MODEL="$2"
EFFORT="$3"

if [[ ! -f "$PROMPT_FILE" ]]; then
    log "ERROR: Prompt file not found: $PROMPT_FILE"
    exit 1
fi

log "Starting Codex (model=$MODEL, effort=$EFFORT)"

# Codex only exposes one effort level ("default") which maps to the account's
# configured reasoning setting — no CLI flag is needed.
codex --full-auto < "$PROMPT_FILE"

log "Codex run complete."
