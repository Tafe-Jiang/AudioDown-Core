FROM node:22-bookworm-slim AS web-builder

WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:1.88-bookworm AS rust-builder

WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ crates/
COPY migrations/ migrations/
COPY --from=web-builder /build/web/dist web/dist/
RUN cargo build --locked --release -p audiodown-server

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates gosu \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home --shell /usr/sbin/nologin audiodown \
    && mkdir -p /data /run/audiodown \
    && chown -R 10001:10001 /data

COPY --from=rust-builder /build/target/release/audiodown-server /usr/local/bin/audiodown-server

ENV AUDIODOWN_BIND=0.0.0.0:18080 \
    AUDIODOWN_DATA_DIR=/data \
    AUDIODOWN_DATABASE_URL=sqlite:///data/audiodown.db \
    AUDIODOWN_SUPERVISOR_SOCKET=/run/audiodown/supervisor.sock \
    AUDIODOWN_CORE_TOKEN_FILE=/run/audiodown/core.token \
    AUDIODOWN_LOG=info

EXPOSE 18080

ENTRYPOINT ["sh", "-c", "set -eu; chown -R 10001:10001 /data; attempts=0; while [ ! -f /run/audiodown/core.token ]; do attempts=$((attempts + 1)); if [ \"$attempts\" -ge 60 ]; then echo 'Supervisor token was not created' >&2; exit 1; fi; sleep 1; done; chown 10001:10001 /run/audiodown/core.token; chmod 600 /run/audiodown/core.token; exec gosu audiodown /usr/local/bin/audiodown-server"]
