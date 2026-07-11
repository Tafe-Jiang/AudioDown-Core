FROM rust:1.88-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ crates/
COPY migrations/ migrations/
RUN cargo build --locked --release -p audiodown-supervisor

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /data/plugins /run/audiodown

COPY --from=builder /build/target/release/audiodown-supervisor /usr/local/bin/audiodown-supervisor

ENV AUDIODOWN_SUPERVISOR_SOCKET=/run/audiodown/supervisor.sock \
    AUDIODOWN_PLUGIN_DATA=/data/plugins \
    AUDIODOWN_INSTALLATION_ID_FILE=/data/plugins/installation-id \
    AUDIODOWN_CORE_TOKEN_FILE=/run/audiodown/core.token

ENTRYPOINT ["/usr/local/bin/audiodown-supervisor"]
