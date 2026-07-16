#!/usr/bin/env sh
# Build the housebot Docker image. The Rust binary is compiled outside
# Docker (statically linked against musl) so cargo's incremental cache is
# reused between builds; the Dockerfile only copies the finished binary in.
set -eu

cd "$(dirname "$0")/.."

IMAGE_TAG="${IMAGE_TAG:-housebot:local}"

scripts/build-binary.sh
docker build --platform linux/amd64 -t "$IMAGE_TAG" .
