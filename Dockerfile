# The bot binary is built OUTSIDE this Dockerfile as a statically linked
# musl executable, so CI can cache cargo artifacts between runs instead of
# recompiling every dependency inside Docker. Build it with
# scripts/build-image.sh (or scripts/build-binary.sh followed by
# docker build --platform linux/amd64, since the staged binary is amd64).
# For the dev compose stack use scripts/dev-up.sh.

# Minimal runtime image: Alpine plus the statically linked bot binary.
FROM alpine:3.22
WORKDIR /app
RUN apk add --no-cache poppler-utils ffmpeg
RUN mkdir -p data/history data/memories
COPY dist/housebot /usr/local/bin/housebot

CMD ["housebot"]
