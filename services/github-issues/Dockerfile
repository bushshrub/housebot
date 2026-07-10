FROM debian:bookworm AS builder
RUN apt-get update && apt-get install -y \
    cmake build-essential libcurl4-openssl-dev libssl-dev curl xz-utils \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY CMakeLists.txt main.cpp .
RUN cmake -B build -DCMAKE_BUILD_TYPE=Release && cmake --build build --parallel

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libcurl4 libssl3 ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/build/github-issues /usr/local/bin/github-issues
EXPOSE 3005
CMD ["github-issues"]
