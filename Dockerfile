# The bot binary is built OUTSIDE this Dockerfile as a statically linked
# musl executable, so CI can cache cargo artifacts between runs instead of
# recompiling every dependency inside Docker. Build it with
# scripts/build-image.sh, or manually:
#
#   rustup target add x86_64-unknown-linux-musl
#   cargo build --release --locked --target x86_64-unknown-linux-musl --package housebot
#   mkdir -p dist && cp target/x86_64-unknown-linux-musl/release/housebot dist/housebot
#
# and then docker build as usual.

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
RUN apk add --no-cache poppler-utils
RUN mkdir -p data/history data/memories
COPY --from=jellyfin-mcp-builder /go/bin/jellyfin-mcp /usr/local/bin/jellyfin-mcp
RUN test -x /usr/local/bin/jellyfin-mcp
COPY dist/housebot /usr/local/bin/housebot

CMD ["housebot"]
