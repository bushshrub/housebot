# Build a musl-linked Rust binary for the Alpine runtime.
FROM rust:1-alpine AS rust-builder
RUN apk add --no-cache musl-dev
WORKDIR /app
# Prime the dependency cache with a stub crate.
COPY Cargo.toml Cargo.lock ./
COPY crates/deployment-bot/Cargo.toml crates/deployment-bot/Cargo.toml
COPY crates/common-crawl/Cargo.toml crates/common-crawl/Cargo.toml
RUN mkdir src \
    && mkdir -p crates/deployment-bot/src \
    && mkdir -p crates/common-crawl/src \
    && echo 'fn main() {}' > src/main.rs \
    && echo '' > src/lib.rs \
    && echo 'fn main() {}' > crates/deployment-bot/src/main.rs \
    && echo '' > crates/deployment-bot/src/lib.rs \
    && echo '' > crates/common-crawl/src/lib.rs \
    && cargo build --release --locked --package housebot || true
# Build the real sources.
COPY src/ src/
RUN touch src/main.rs src/lib.rs && cargo build --release --locked --package housebot
RUN strip /app/target/release/housebot

# Build the Jellyfin MCP server as a static Go binary for the runtime image.
# Keep this pinned so image rebuilds do not silently change the MCP tool set.
FROM golang:1.25-alpine AS jellyfin-mcp-builder
ARG JELLYFIN_MCP_VERSION=v2026.604.2
RUN apk add --no-cache git
RUN CGO_ENABLED=0 go install github.com/jaredtrent/jellyfin-mcp/v2026@${JELLYFIN_MCP_VERSION}

# Minimal runtime image: Alpine plus the statically linked bot binary.
FROM alpine:3.22
WORKDIR /app
COPY --from=rust-builder /app/target/release/housebot /usr/local/bin/housebot
COPY --from=jellyfin-mcp-builder /go/bin/jellyfin-mcp /usr/local/bin/jellyfin-mcp
RUN test -x /usr/local/bin/jellyfin-mcp
RUN mkdir -p data/history data/memories

CMD ["housebot"]
