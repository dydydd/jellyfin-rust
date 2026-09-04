ARG BUILDER_IMAGE=rust:1.88-bookworm
ARG WEB_BUILDER_IMAGE=node:24-bookworm-slim
ARG RUNTIME_IMAGE=debian:bookworm-slim

FROM ${WEB_BUILDER_IMAGE} AS web-builder
WORKDIR /build

COPY jellyfin-web/package.json jellyfin-web/package-lock.json ./
RUN npm ci

COPY jellyfin-web ./
RUN npm run build:production

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
COPY --from=web-builder /build/dist /app/web

RUN mkdir -p cache metadata programdata logs \
    && chown -R jellyfin:jellyfin /app

USER jellyfin
EXPOSE 8096

ENV JELLYFIN_BIND_ADDRESS=0.0.0.0:8096 \
    JELLYFIN_LOG_DIR=/app/logs \
    JELLYFIN_WEB_DIR=/app/web

CMD ["jellyfin-server"]
