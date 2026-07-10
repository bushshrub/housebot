# Stage 1: build the jellyfin-mcp binary
FROM golang:1.25-bookworm AS jellyfin-builder
RUN go install github.com/jaredtrent/jellyfin-mcp@latest

# Stage 2: build the Rust bot binary
FROM rust:1.87-bookworm AS rust-builder
WORKDIR /app
# Prime the dependency cache with a stub crate.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && echo '' > src/lib.rs \
    && cargo build --release --locked || true
# Build the real sources.
COPY src/ src/
RUN touch src/main.rs src/lib.rs && cargo build --release --locked

# Stage 3: runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    git \
    curl \
    ca-certificates \
    docker.io \
    && rm -rf /var/lib/apt/lists/*

# jellyfin-mcp (stdio MCP server)
COPY --from=jellyfin-builder /go/bin/jellyfin-mcp /usr/local/bin/jellyfin-mcp

# uv + the DuckDuckGo MCP server (stdio)
RUN curl -LsSf https://astral.sh/uv/install.sh | sh
ENV PATH="/root/.local/bin:$PATH"
RUN uv tool install duckduckgo-mcp-server

WORKDIR /app
COPY --from=rust-builder /app/target/release/housebot /usr/local/bin/housebot

RUN mkdir -p data/history data/memories

CMD ["housebot"]
