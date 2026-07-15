# Build a musl-linked Rust binary for the Alpine runtime.
FROM rust:1-alpine AS rust-builder
ARG GIT_COMMIT
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/deployment-bot/Cargo.toml crates/deployment-bot/Cargo.toml
COPY crates/common-crawl/Cargo.toml crates/common-crawl/Cargo.toml
COPY src/ src/
COPY crates/ crates/
COPY assets/ assets/
COPY .github/agents/catalog.json .github/agents/catalog.json
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
