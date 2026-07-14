#!/usr/bin/env bash
# .github/agents/common.sh — shared utilities for coding-agent adapters.
# Source this file; do not execute it directly.
#
# Usage: source "$(dirname "$0")/common.sh"

set -euo pipefail

# Print a labelled line to stderr.
log() { printf '[housebot-agent] %s\n' "$*" >&2; }

# validate_catalog <agent> <model> <effort> <expected_catalog_revision>
#
# Parses .github/agents/catalog.json and verifies that:
#   1. The catalog revision matches what was recorded at dispatch time.
#   2. The agent, model, and effort are all present.
#
# Returns 0 on success, 1 on any mismatch (prints reason to stderr).
validate_catalog() {
    local agent="$1" model="$2" effort="$3" expected_revision="$4"
    local catalog=".github/agents/catalog.json"

    if [[ ! -f "$catalog" ]]; then
        log "ERROR: catalog not found at $catalog"
        return 1
    fi

    # Use argv to pass values into Python — never interpolate into -c strings.
    local result
    result=$(python3 - "$agent" "$model" "$effort" "$expected_revision" "$catalog" << 'PYEOF'
import json, sys

agent, model, effort, expected_rev, catalog_path = sys.argv[1:]

with open(catalog_path) as f:
    c = json.load(f)

actual_rev = c.get('catalog_revision', '')
if actual_rev != expected_rev:
    print(f'revision-mismatch:{actual_rev}')
    sys.exit(0)

agents_map = c.get('agents', {})
if agent not in agents_map:
    print('invalid-agent')
    sys.exit(0)

models = agents_map[agent].get('models', [])
model_obj = next((m for m in models if m['id'] == model), None)
if model_obj is None:
    print('invalid-model')
    sys.exit(0)

efforts = model_obj.get('efforts', [])
effort_obj = next((e for e in efforts if e['id'] == effort), None)
if effort_obj is None:
    print('invalid-effort')
    sys.exit(0)

print('ok')
PYEOF
    )

    case "$result" in
        ok)
            log "Catalog validation OK: agent=$agent model=$model effort=$effort"
            ;;
        revision-mismatch:*)
            local actual_rev="${result#revision-mismatch:}"
            log "ERROR: Catalog revision mismatch."
            log "  Dispatched with: '$expected_revision'"
            log "  Repo now has:    '$actual_rev'"
            log "  Re-dispatch the job to pick up the current catalog."
            return 1
            ;;
        invalid-agent)
            log "ERROR: Agent '$agent' is not in the catalog."
            return 1
            ;;
        invalid-model)
            log "ERROR: Model '$model' is not listed for agent '$agent'."
            return 1
            ;;
        invalid-effort)
            log "ERROR: Effort '$effort' is not valid for model '$model' on agent '$agent'."
            return 1
            ;;
        *)
            log "ERROR: Unexpected validation result: '$result'"
            return 1
            ;;
    esac
}

# effort_max_turns <effort>  →  prints an integer
#
# Maps a named effort level to a turn limit for agents that support --max-turns.
effort_max_turns() {
    case "$1" in
        low)     echo 15  ;;
        medium)  echo 40  ;;
        high)    echo 100 ;;
        default) echo 50  ;;
        *)       echo 50  ;;
    esac
}

# effort_timeout_secs <effort>  →  prints an integer (seconds)
#
# Maps a named effort level to a wall-clock timeout for execution-budget agents.
effort_timeout_secs() {
    case "$1" in
        low)     echo 900   ;;   # 15 min
        medium)  echo 1800  ;;   # 30 min
        high)    echo 3600  ;;   # 60 min
        default) echo 1800  ;;
        *)       echo 1800  ;;
    esac
}
