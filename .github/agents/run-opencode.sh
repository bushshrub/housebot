#!/usr/bin/env bash
# .github/agents/run-opencode.sh — OpenCode + NVIDIA NIM adapter.
#
# Usage: run-opencode.sh <prompt_file> <model> <effort>
#
# Authentication: requires NVIDIA_API_KEY in the environment, injected by the
# workflow as a GitHub secret. It is never printed or logged.
#
# Effort is handled as an execution budget: a wall-clock timeout wraps the
# opencode process.

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

# NVIDIA NIM models require an API key; free-tier models (opencode/* prefix) do not.
if [[ "$MODEL" == nvidia/* ]] && [[ -z "${NVIDIA_API_KEY:-}" ]]; then
    log "ERROR: NVIDIA_API_KEY is required for NVIDIA NIM models but is not set."
    log "  Configure it as a repository secret, or choose a free-tier model."
    exit 1
fi

TIMEOUT_SECS="$(effort_timeout_secs "$EFFORT")"
log "Starting OpenCode (model=$MODEL, effort=$EFFORT, timeout=${TIMEOUT_SECS}s)"

# Write an opencode.json config in the current working directory (the repo root).
# Cleaned up after the run regardless of outcome.
CONFIG_FILE="opencode.json"
printf '{"model":"%s"}\n' "$MODEL" > "$CONFIG_FILE"
trap 'rm -f "$CONFIG_FILE"' EXIT

# Run opencode under a wall-clock timeout.  The prompt is passed as a
# positional argument — single-quoted command substitution so issue content is
# not re-evaluated by the shell.
timeout "$TIMEOUT_SECS" opencode run "$(cat "$PROMPT_FILE")"
EXIT_CODE=$?

if [[ $EXIT_CODE -eq 124 ]]; then
    log "ERROR: OpenCode exceeded the execution budget ($TIMEOUT_SECS s) for effort level '$EFFORT'."
    exit 1
fi

log "OpenCode run complete (exit=$EXIT_CODE)."
exit $EXIT_CODE
