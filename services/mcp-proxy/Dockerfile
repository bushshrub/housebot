FROM debian:bookworm AS builder
RUN apt-get update && apt-get install -y gcc make && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY mcp_proxy.c .
RUN gcc -O2 -Wall -o mcp-proxy mcp_proxy.c -lpthread

FROM debian:bookworm-slim
COPY --from=builder /app/mcp-proxy /usr/local/bin/mcp-proxy
EXPOSE 3006
CMD ["mcp-proxy"]
