#!/usr/bin/env bash
# Post-deploy health check: detects crash loops and unexpected restarts.
# Required env: DEPLOY_PATH
# Optional env: HEALTH_PROBE_DELAY (default 30s), MAX_RESTARTS (default 2)
set -euo pipefail

COMPOSE_DIR="${DEPLOY_PATH:?DEPLOY_PATH must be set}"
PROBE_DELAY="${HEALTH_PROBE_DELAY:-30}"
MAX_RESTARTS="${MAX_RESTARTS:-2}"

cd "$COMPOSE_DIR"

printf '=== Crash-loop probe: waiting %ss ===\n' "$PROBE_DELAY"
sleep "$PROBE_DELAY"

STATUS=$(docker compose ps --format '{{.Status}}' 2>/dev/null | head -1 || echo "unknown")
printf 'Container status: %s\n' "$STATUS"

if printf '%s' "$STATUS" | grep -qiE 'restarting|exited|dead|error'; then
    printf 'ERROR: Unhealthy container state: %s\n' "$STATUS" >&2
    docker compose logs --tail=100 >&2
    exit 1
fi

CONTAINER=$(docker compose ps --format '{{.Name}}' 2>/dev/null | head -1 || true)
if [ -n "$CONTAINER" ]; then
    RESTARTS=$(docker inspect --format='{{.RestartCount}}' "$CONTAINER" 2>/dev/null || echo "0")
    printf 'Restart count: %s\n' "$RESTARTS"
    if [ "$RESTARTS" -gt "$MAX_RESTARTS" ]; then
        printf 'ERROR: Container restarted %s times (limit %s) — crash loop detected\n' \
            "$RESTARTS" "$MAX_RESTARTS" >&2
        docker compose logs --tail=100 >&2
        exit 1
    fi
fi

echo "=== Health check passed ==="
