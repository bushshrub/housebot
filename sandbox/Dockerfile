FROM node:22-alpine

RUN apk add --no-cache \
    bash \
    python3 \
    git \
    curl \
    ca-certificates \
    build-base

RUN npm install -g --no-fund --no-audit opencode-ai \
    && npm cache clean --force

RUN adduser -D -s /bin/bash sandbox \
    && mkdir -p /workspace \
    && chown -R sandbox:sandbox /workspace

# Install Rust toolchain for the sandbox user
USER sandbox
# The default rustup profile includes documentation and other components that
# are not needed to compile projects. The minimal profile cuts hundreds of MB.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- \
    -y --default-toolchain stable --profile minimal \
    && rm -rf /home/sandbox/.rustup/downloads \
        /home/sandbox/.rustup/tmp \
        /home/sandbox/.cargo/registry \
        /home/sandbox/.cargo/git
ENV PATH="/home/sandbox/.cargo/bin:${PATH}"
USER root

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

USER sandbox
WORKDIR /workspace

CMD ["/entrypoint.sh"]
