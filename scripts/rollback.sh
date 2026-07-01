#!/usr/bin/env bash
# Roll back housebot to a previous image on the homelab server.
# Required env: DEPLOY_PATH
# Optional env: ROLLBACK_TAG (specific tag, e.g. sha-abc1234 or v1.2.3)
#               If unset, uses the checkpoint saved by deploy.sh.
set -euo pipefail

COMPOSE_DIR="${DEPLOY_PATH:?DEPLOY_PATH must be set}"
IMAGE_BASE="ghcr.io/bushshrub/housebot"
TAG="${ROLLBACK_TAG:-}"

cd "$COMPOSE_DIR"

if [ -n "$TAG" ]; then
    TARGET="${IMAGE_BASE}:${TAG}"
    printf '=== Rolling back to explicit tag: %s ===\n' "$TARGET"
    docker pull "$TARGET"
    docker tag "$TARGET" "${IMAGE_BASE}:latest"
else
    CHECKPOINT=$(cat .prev_image_digest 2>/dev/null || echo "")
    if [ -z "$CHECKPOINT" ] || [ "$CHECKPOINT" = "none" ]; then
        echo "ERROR: No saved checkpoint and no ROLLBACK_TAG provided" >&2
        exit 1
    fi
    printf '=== Rolling back to checkpoint: %s ===\n' "$CHECKPOINT"
    docker pull "$CHECKPOINT"
    docker tag "$CHECKPOINT" "${IMAGE_BASE}:latest"
fi

echo "=== Restarting service ==="
docker compose up -d --remove-orphans
sleep 15

if ! docker compose ps | grep -q "Up"; then
    echo "ERROR: Container not running after rollback" >&2
    docker compose logs --tail=50 >&2
    exit 1
fi

docker compose ps
echo "=== Rollback complete ==="
