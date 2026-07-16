#!/usr/bin/env sh
# Start the dev compose stack. The image copies a prebuilt dist/housebot,
# so the binary must be compiled before docker compose builds the image.
set -eu

cd "$(dirname "$0")/.."

scripts/build-binary.sh
docker compose -f docker-compose.dev.yml up --build "$@"
