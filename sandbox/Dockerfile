FROM node:22-bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 \
    git \
    curl \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g @anthropic-ai/claude-code opencode-ai

# Claude Code refuses --dangerously-skip-permissions when running as root.
RUN useradd -m -s /bin/bash sandbox \
    && mkdir -p /workspace \
    && chown -R sandbox:sandbox /workspace

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

USER sandbox
WORKDIR /workspace

CMD ["/entrypoint.sh"]
