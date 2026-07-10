# Stage 1: build the jellyfin-mcp binary.
FROM golang:1.25-bookworm AS jellyfin-builder
RUN go install github.com/jaredtrent/jellyfin-mcp@latest

# Stage 2: provide the Docker CLI without pulling the Docker daemon/runtime.
FROM docker:27-cli AS docker-cli

# Stage 3: build the DuckDuckGo MCP tool against the same Python runtime used
# by the final image. This keeps the tool from downloading its own Python.
FROM python:3.13-slim-bookworm AS mcp-builder
RUN pip install --no-cache-dir uv \
    && uv tool install duckduckgo-mcp-server

# Stage 4: build the Rust bot binary against the same Debian release as the
# runtime image, so the binary does not require a newer glibc.
FROM rust:1-bookworm AS rust-builder
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

# Stage 5: runtime image
FROM python:3.13-slim-bookworm

# Only the Docker client is needed; the daemon is provided by the host socket.
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker

# jellyfin-mcp (stdio MCP server)
COPY --from=jellyfin-builder /go/bin/jellyfin-mcp /usr/local/bin/jellyfin-mcp

# DuckDuckGo MCP server (stdio). The wrapper and its venv both use the
# Python interpreter already present in this image.
COPY --from=mcp-builder /root/.local/bin/duckduckgo-mcp-server /root/.local/bin/duckduckgo-mcp-server
COPY --from=mcp-builder /root/.local/share/uv/tools /root/.local/share/uv/tools
ENV PATH="/root/.local/bin:$PATH"

WORKDIR /app
COPY --from=rust-builder /app/target/release/housebot /usr/local/bin/housebot

RUN mkdir -p data/history data/memories

CMD ["housebot"]
