FROM rust:1.80-slim AS builder
WORKDIR /app
COPY Cargo.toml .
RUN mkdir src && echo 'fn main(){}' > src/main.rs && cargo build --release && rm src/main.rs
COPY src/ src/
RUN touch src/main.rs && cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/llm-client /usr/local/bin/llm-client
EXPOSE 3002
CMD ["llm-client"]
