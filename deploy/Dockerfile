# Stage 1: Build web frontend
FROM oven/bun:1.1 AS web-builder
WORKDIR /app/web
COPY web/package.json ./
RUN bun install
COPY web/ ./
RUN bun run build

# Stage 2: Build server binary
FROM rust:1.82-bookworm AS rust-builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release --bin oxidited

# Stage 3: Runtime image
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    tmux \
    openssh-client \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=rust-builder /app/target/release/oxidited /usr/local/bin/oxidited
COPY --from=web-builder /app/web/dist /app/web/dist

ENV OXIDITE_WEB_DIR=/app/web/dist
EXPOSE 3000

ENTRYPOINT ["oxidited"]
