FROM rust:1.97.1-slim-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY migrations ./migrations
COPY src ./src
RUN cargo build --locked --release --bins

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home --shell /usr/sbin/nologin alpha

COPY --from=builder /build/target/release/api /usr/local/bin/api
COPY --from=builder /build/target/release/judge_worker /usr/local/bin/judge_worker
COPY --from=builder /build/target/release/metadata_worker /usr/local/bin/metadata_worker
COPY --from=builder /build/target/release/migrate /usr/local/bin/migrate

ENV BIND_ADDRESS=0.0.0.0:8080
EXPOSE 8080
USER alpha
CMD ["api"]

FROM docker:29.6.1-cli AS docker-cli

FROM runtime AS worker
USER root
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker
CMD ["judge_worker"]
