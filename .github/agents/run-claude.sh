#!/usr/bin/env bash
# .github/agents/run-claude.sh — Claude Code coding-agent adapter.
#
# Usage: run-claude.sh <prompt_file> <model> <effort>
#
# Authentication: Claude Code uses an OAuth session stored on the runner
# (~/.claude/ directory). No Anthropic API key is used or exposed.
#
# The prompt is read from a file and piped to claude's stdin so that issue
# content is never expanded in shell argument position.

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

MAX_TURNS="$(effort_max_turns "$EFFORT")"
log "Starting Claude Code (model=$MODEL, effort=$EFFORT, max-turns=$MAX_TURNS)"

# --dangerously-skip-permissions: required for non-interactive CI use.
# --max-turns: bounds execution depth by effort level.
# The prompt is piped via stdin (-p -) so the issue content is never visible
# in the process argument list.
claude --dangerously-skip-permissions --max-turns "$MAX_TURNS" -p "$(cat "$PROMPT_FILE")"

log "Claude Code run complete."
