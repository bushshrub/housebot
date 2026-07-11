FROM node:22-bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 \
    git \
    curl \
    ca-certificates \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g --no-fund --no-audit opencode-ai \
    && npm cache clean --force

RUN useradd -m -s /bin/bash sandbox \
    && mkdir -p /workspace \
    && chown -R sandbox:sandbox /workspace

# Install Rust toolchain for the sandbox user
USER sandbox
# The default rustup profile includes documentation and other components that
# are not needed to compile projects. The minimal profile cuts hundreds of MB.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- \
    -y --default-toolchain stable --profile minimal
ENV PATH="/home/sandbox/.cargo/bin:${PATH}"
USER root

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

USER sandbox
WORKDIR /workspace

CMD ["/entrypoint.sh"]
