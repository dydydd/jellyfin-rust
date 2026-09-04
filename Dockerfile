ARG BUILDER_IMAGE=rust:1.88-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim

FROM ${BUILDER_IMAGE} AS builder
WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release --locked -p jellyfin-server

FROM ${RUNTIME_IMAGE} AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --system --create-home --home-dir /app --uid 10001 jellyfin
WORKDIR /app

COPY --from=builder /build/target/release/jellyfin-server /usr/local/bin/jellyfin-server

RUN mkdir -p cache metadata programdata logs web \
    && chown -R jellyfin:jellyfin /app

USER jellyfin
EXPOSE 8096

ENV JELLYFIN_BIND_ADDRESS=0.0.0.0:8096 \
    JELLYFIN_LOG_DIR=/app/logs \
    JELLYFIN_WEB_DIR=/app/web

CMD ["jellyfin-server"]
