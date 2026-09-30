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
    coreutils \
    python3 \
    py3-pip \
    git \
    curl \
    ca-certificates \
    build-base \
    findutils \
    ripgrep \
    tzdata

# Install Node.js (LTS) for JavaScript/TypeScript projects
RUN apk add --no-cache nodejs npm

RUN adduser -D -s /bin/bash sandbox \
    && mkdir -p /workspace /home/sandbox \
    && chown -R sandbox:sandbox /workspace /home/sandbox

# The toolchain lives outside /home/sandbox: sandboxd mounts an empty tmpfs
# there, which would hide anything the image put in it.
ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- \
    -y --default-toolchain stable --profile minimal --no-modify-path 2>/dev/null \
    && rm -rf /opt/rustup/downloads /opt/rustup/tmp /opt/cargo/registry /opt/cargo/git

# /workspace is the only writable path that allows execution, so package caches
# and installs go there: native Python extensions and cargo build scripts must
# be loadable, and the other tmpfs mounts are too small to hold them. Outside a
# venv, pip falls back to a user install because /usr is read-only; the
# externally-managed guard is lifted so it gets that far.
ENV CARGO_HOME=/workspace/.cargo \
    PYTHONUSERBASE=/workspace/.local \
    PIP_BREAK_SYSTEM_PACKAGES=1 \
    PIP_CACHE_DIR=/workspace/.cache/pip \
    NPM_CONFIG_PREFIX=/workspace/.npm-global \
    NPM_CONFIG_CACHE=/workspace/.cache/npm \
    PATH="/workspace/.cargo/bin:/workspace/.local/bin:/workspace/.npm-global/bin:/opt/cargo/bin:${PATH}"

# Create the entrypoint
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

USER sandbox
WORKDIR /workspace

CMD ["/entrypoint.sh"]
