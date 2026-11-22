# syntax=docker/dockerfile:1.7
# Builds the headless iptv-server (HDHomeRun tuner + web UI + Plex sync).
#
#   docker build -t iptv-server .
#   docker run -p 8080:8080 -p 5004:5004 -v iptv-data:/data \
#     -e IPTV_M3U_URL=... -e PLEX_URL=... -e PLEX_TOKEN=... iptv-server
#
# Set --build-arg WITH_FFMPEG=false for a smaller image if you don't use
# IPTV_BUFFER=ffmpeg.

FROM rust:1.93-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p iptv-server \
 && cp target/release/iptv-server /usr/local/bin/iptv-server

FROM debian:bookworm-slim
ARG WITH_FFMPEG=true
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && if [ "$WITH_FFMPEG" = "true" ]; then apt-get install -y --no-install-recommends ffmpeg; fi \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home-dir /data iptv \
 && mkdir -p /data && chown iptv /data

COPY --from=build /usr/local/bin/iptv-server /usr/local/bin/iptv-server

USER 10001
ENV IPTV_DATA_DIR=/data \
    IPTV_BIND=0.0.0.0 \
    IPTV_UI_PORT=8080 \
    IPTV_HDHR_PORT=5004
VOLUME ["/data"]
# 8080: web UI + API (/healthz for probes); 5004: HDHomeRun tuner for Plex
EXPOSE 8080 5004
ENTRYPOINT ["/usr/local/bin/iptv-server"]
