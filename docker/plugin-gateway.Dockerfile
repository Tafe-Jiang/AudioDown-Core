FROM rust:1.88-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ crates/
RUN cargo build --locked --release -p audiodown-proxy-gateway

FROM debian:bookworm-slim AS runtime

RUN useradd --uid 10003 --create-home --shell /usr/sbin/nologin audiodown-gateway

COPY --from=builder /build/target/release/audiodown-proxy-gateway /usr/local/bin/audiodown-proxy-gateway

USER 10003:10003
EXPOSE 18081

ENTRYPOINT ["/usr/local/bin/audiodown-proxy-gateway"]
