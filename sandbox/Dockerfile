# Legacy sandbox image — the code-inspection sandbox image has moved to
# crates/sandbox/docker/Dockerfile.  CI now builds from that path.
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
    sudo nodejs npm

RUN adduser -D -s /bin/bash sandbox \
    && mkdir -p /workspace /home/sandbox \
    && chown -R sandbox:sandbox /workspace /home/sandbox

USER sandbox
USER root

COPY crates/sandbox/docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

USER sandbox
WORKDIR /workspace

CMD ["/entrypoint.sh"]
