# Build a musl-linked Rust binary for the Alpine runtime.
FROM rust:1-alpine AS rust-builder
RUN apk add --no-cache musl-dev
WORKDIR /app
# Prime the dependency cache with a stub crate.
COPY Cargo.toml Cargo.lock ./
COPY crates/deployment-bot/Cargo.toml crates/deployment-bot/Cargo.toml
RUN mkdir src \
    && mkdir -p crates/deployment-bot/src \
    && echo 'fn main() {}' > src/main.rs \
    && echo '' > src/lib.rs \
    && echo 'fn main() {}' > crates/deployment-bot/src/main.rs \
    && echo '' > crates/deployment-bot/src/lib.rs \
    && cargo build --release --locked --package housebot || true
# Build the real sources.
COPY src/ src/
RUN touch src/main.rs src/lib.rs && cargo build --release --locked --package housebot
RUN strip /app/target/release/housebot

# Minimal runtime image: Alpine plus the statically linked bot binary.
FROM alpine:3.22
WORKDIR /app
COPY --from=rust-builder /app/target/release/housebot /usr/local/bin/housebot
RUN mkdir -p data/history data/memories

CMD ["housebot"]
