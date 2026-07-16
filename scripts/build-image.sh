#!/usr/bin/env sh
# Build the housebot Docker image. The Rust binary is compiled outside
# Docker (statically linked against musl) so cargo's incremental cache is
# reused between builds; the Dockerfile only copies the finished binary in.
set -eu

cd "$(dirname "$0")/.."

TARGET=x86_64-unknown-linux-musl
IMAGE_TAG="${IMAGE_TAG:-housebot:local}"

rustup target add "$TARGET"
HOUSEBOT_GIT_SHA="$(git rev-parse HEAD)" \
    cargo build --release --locked --target "$TARGET" --package housebot
mkdir -p dist
cp "target/$TARGET/release/housebot" dist/housebot
strip dist/housebot || true

docker build -t "$IMAGE_TAG" .
