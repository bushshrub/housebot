FROM haskell:9.6 AS builder
WORKDIR /app
COPY agent.cabal .
RUN cabal update && cabal build --only-dependencies
COPY src/ src/
RUN cabal build && cp $(cabal exec which agent) /usr/local/bin/agent-exe

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates libgmp10 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/bin/agent-exe /usr/local/bin/agent-exe
EXPOSE 3003
CMD ["agent-exe"]
