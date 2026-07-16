#!/usr/bin/env sh
# Compile the statically linked musl bot binary into dist/housebot.
# Docker builds copy this artifact instead of compiling inside the image,
# so cargo's incremental cache is reused between builds.
set -eu

cd "$(dirname "$0")/.."

TARGET=x86_64-unknown-linux-musl

rustup target add "$TARGET"
HOUSEBOT_GIT_SHA="$(git rev-parse HEAD)" \
    cargo build --release --locked --target "$TARGET" --package housebot
mkdir -p dist
cp "target/$TARGET/release/housebot" dist/housebot
strip dist/housebot || true
