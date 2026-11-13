FROM --platform=$BUILDPLATFORM node:22-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:1.99-slim-bookworm AS build
ARG TARGETARCH
RUN test "$(dpkg --print-architecture)" = "$TARGETARCH" \
    && apt-get update && apt-get install -y --no-install-recommends build-essential cmake pkg-config ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
# 依赖产物保留在构建分层中，GitHub Actions 缓存可跨 runner 复用。
RUN mkdir src && printf 'fn main() {}\n' > src/main.rs && touch src/lib.rs \
    && cargo build --release --locked --jobs "$(nproc)" \
    && rm -rf src
COPY migrations/ ./migrations/
COPY src/ ./src/
# 仅清理占位项目的产物，确保始终编译真实源码并保留已构建的依赖。
RUN cargo clean --release --package tv-rust \
    && cargo build --release --locked --jobs "$(nproc)" \
    && cp target/release/tv-rust /usr/local/bin/tv-rust

FROM debian:bookworm-slim AS runtime
ARG TARGETARCH
RUN test "$(dpkg --print-architecture)" = "$TARGETARCH" \
    && apt-get update && apt-get install -y --no-install-recommends ca-certificates ffmpeg curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 tv && useradd --uid 10001 --gid tv --no-create-home tv \
    && mkdir -p /data /app/web && chown tv:tv /data
COPY --from=build /usr/local/bin/tv-rust /usr/local/bin/tv-rust
COPY --from=web /web/dist/ /app/web/
COPY LICENSE /usr/share/doc/tv-rust/LICENSE
USER tv
WORKDIR /app
ENV TV_DATA_DIR=/data TV_WEB_DIR=/app/web TV_BIND=0.0.0.0:3000 RUST_LOG=tv_rust=info
EXPOSE 3000
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 CMD curl --fail --silent http://127.0.0.1:3000/api/health || exit 1
ENTRYPOINT ["/usr/local/bin/tv-rust"]
