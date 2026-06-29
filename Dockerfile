# Stage 1: build jellyfin-mcp binary
FROM golang:1.25-bookworm AS jellyfin-builder
RUN go install github.com/jaredtrent/jellyfin-mcp@latest

# Stage 2: main bot image
FROM python:3.12-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    git \
    curl \
    ca-certificates \
    docker.io \
    && rm -rf /var/lib/apt/lists/*

# Copy jellyfin-mcp binary from builder stage
COPY --from=jellyfin-builder /go/bin/jellyfin-mcp /usr/local/bin/jellyfin-mcp

# Install uv and the DuckDuckGo MCP server as an installed binary
RUN curl -LsSf https://astral.sh/uv/install.sh | sh
ENV PATH="/root/.local/bin:$PATH"
RUN uv tool install duckduckgo-mcp-server

WORKDIR /app

COPY pyproject.toml .
COPY src/ src/
COPY main.py .
RUN uv pip install --system .

RUN mkdir -p data/history data/memories

CMD ["python", "main.py"]
