# Sandbox execution image for the code-inspection sandbox.
#
# This is a general-purpose image containing tools needed to clone,
# inspect, and run short commands on public repositories.
#
# It does NOT auto-execute any agent or workflow.  It starts an inert
# sleep process and waits for sandboxd to invoke commands via docker exec.

FROM alpine:3.20

RUN apk add --no-cache \
    bash \
    python3 \
    py3-pip \
    git \
    curl \
    ca-certificates \
    build-base \
    ripgrep \
    sudo

# Install Node.js (LTS) for JavaScript/TypeScript projects
RUN apk add --no-cache nodejs npm

# Install Rust toolchain via rustup
RUN adduser -D -s /bin/bash sandbox \
    && mkdir -p /workspace /home/sandbox \
    && chown -R sandbox:sandbox /workspace /home/sandbox

USER sandbox
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- \
    -y --default-toolchain stable --profile minimal 2>/dev/null \
    && rm -rf /home/sandbox/.rustup/downloads \
        /home/sandbox/.rustup/tmp \
        /home/sandbox/.cargo/registry \
        /home/sandbox/.cargo/git
ENV PATH="/home/sandbox/.cargo/bin:${PATH}"
USER root

# Create the entrypoint
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

USER sandbox
WORKDIR /workspace

CMD ["/entrypoint.sh"]
