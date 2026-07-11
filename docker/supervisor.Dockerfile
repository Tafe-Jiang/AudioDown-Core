FROM rust:1.88-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ crates/
COPY migrations/ migrations/
COPY docker/plugin-runtime/ docker/plugin-runtime/
COPY plugin-sdk/node/ plugin-sdk/node/
RUN cargo build --locked --release -p audiodown-supervisor

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /data/plugins /run/audiodown

COPY --from=builder /build/target/release/audiodown-supervisor /usr/local/bin/audiodown-supervisor
COPY docker/plugin-runtime/node22.lock.json /opt/audiodown/plugin-runtime/node22.lock.json
COPY docker/plugin-runtime/node22-builder.Dockerfile /opt/audiodown/plugin-runtime/node22-builder.Dockerfile
COPY docker/plugin-runtime/node22-runtime.Dockerfile /opt/audiodown/plugin-runtime/node22-runtime.Dockerfile
COPY docker/plugin-runtime/node22-build-runner.js /opt/audiodown/plugin-runtime/node22-build-runner.js
COPY plugin-sdk/node/ /opt/audiodown/plugin-sdk/node/

ENV AUDIODOWN_SUPERVISOR_SOCKET=/run/audiodown/supervisor.sock \
    AUDIODOWN_PLUGIN_DATA=/data/plugins \
    AUDIODOWN_INSTALLATION_ID_FILE=/data/plugins/installation-id \
    AUDIODOWN_CORE_TOKEN_FILE=/run/audiodown/core.token

ENTRYPOINT ["/usr/local/bin/audiodown-supervisor"]
