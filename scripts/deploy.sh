#!/usr/bin/env bash
# Deploy housebot on the homelab server.
# Called by the GitHub Actions deploy-homelab workflow via SSH.
# Required env: DEPLOY_PATH
set -euo pipefail

COMPOSE_DIR="${DEPLOY_PATH:?DEPLOY_PATH must be set}"
IMAGE="ghcr.io/bushshrub/housebot:latest"

cd "$COMPOSE_DIR"

echo "=== [1/5] Saving rollback checkpoint ==="
docker inspect \
    --format='{{index .RepoDigests 0}}' \
    "$IMAGE" \
    2>/dev/null > .prev_image_digest \
    || printf 'none' > .prev_image_digest
printf 'Previous image: %s\n' "$(cat .prev_image_digest)"

echo "=== [2/5] Pulling new images ==="
docker compose pull

echo "=== [3/5] Starting containers ==="
docker compose up -d --remove-orphans

echo "=== [4/5] Waiting 15s for startup ==="
sleep 15

echo "=== [5/5] Verifying containers ==="
if ! docker compose ps | grep -q "Up"; then
    echo "ERROR: No containers running after deployment" >&2
    docker compose logs --tail=100 >&2
    exit 1
fi

docker compose ps
echo "=== Deploy complete ==="
