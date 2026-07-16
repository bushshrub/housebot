# Build a musl-linked Rust binary for the Alpine runtime.
FROM rust:1-alpine AS rust-builder
ARG GIT_COMMIT
RUN apk add --no-cache musl-dev
WORKDIR /app

# Layer 1: Cargo manifests — changes infrequently, so this layer is cached.
COPY Cargo.toml Cargo.lock ./
COPY crates/common-crawl/Cargo.toml crates/common-crawl/Cargo.toml
COPY crates/deployment-bot/Cargo.toml crates/deployment-bot/Cargo.toml
COPY crates/graph-render/Cargo.toml crates/graph-render/Cargo.toml
COPY crates/llm/Cargo.toml crates/llm/Cargo.toml

# Layer 2: build dependencies only (dummy source, no real code).
# This layer is cached by Docker as long as Cargo.toml/lock stay the same,
# keeping compiled dependency artifacts even across CI runs where
# BuildKit cache mounts might not persist.
RUN mkdir -p src \
    && echo "fn main() {}" > src/main.rs \
    && echo "fn lib() {}" > src/lib.rs \
    && mkdir -p crates/common-crawl/src \
    && echo "" > crates/common-crawl/src/lib.rs \
    && mkdir -p crates/deployment-bot/src \
    && echo "fn main() {}" > crates/deployment-bot/src/main.rs \
    && mkdir -p crates/graph-render/src \
    && echo "" > crates/graph-render/src/lib.rs \
    && mkdir -p crates/llm/src \
    && echo "" > crates/llm/src/lib.rs
RUN --mount=type=cache,id=housebot-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=housebot-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=housebot-cargo-target,target=/app/target \
    cargo build --release --locked --package housebot

# Layer 3: real source files.
COPY src/ src/
COPY db/ db/
COPY crates/ crates/
COPY assets/ assets/
COPY .github/agents/catalog.json .github/agents/catalog.json

# Layer 4: build the real binary.  The cache mount reuses artifacts from
# Layer 2, so only changed source files are recompiled.
RUN --mount=type=cache,id=housebot-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=housebot-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=housebot-cargo-target,target=/app/target \
    HOUSEBOT_GIT_SHA="$GIT_COMMIT" cargo build --release --locked --package housebot \
    && cp /app/target/release/housebot /app/housebot \
    && strip /app/housebot

# Build the Jellyfin MCP server as a static Go binary for the runtime image.
# Keep this pinned so image rebuilds do not silently change the MCP tool set.
# We clone and build directly because the module's go.mod path doesn't carry
# the /v2026 suffix required by Go's module system for this version tag.
FROM golang:1.25-alpine AS jellyfin-mcp-builder
ARG JELLYFIN_MCP_VERSION=v2026.604.2
RUN apk add --no-cache git
RUN git clone --depth 1 --branch ${JELLYFIN_MCP_VERSION} https://github.com/jaredtrent/jellyfin-mcp /src
WORKDIR /src
RUN CGO_ENABLED=0 go build -o /go/bin/jellyfin-mcp .

# Minimal runtime image: Alpine plus the statically linked bot binary.
FROM alpine:3.22
WORKDIR /app
COPY --from=rust-builder /app/housebot /usr/local/bin/housebot
COPY --from=jellyfin-mcp-builder /go/bin/jellyfin-mcp /usr/local/bin/jellyfin-mcp
RUN apk add --no-cache poppler-utils
RUN test -x /usr/local/bin/jellyfin-mcp
RUN mkdir -p data/history data/memories

CMD ["housebot"]
