FROM rust:1.97.1-slim-bookworm@sha256:99e09cb2284e2ddbb73a995deee3e91783fd04d177602ccf6eab326d778ee777 AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY migrations ./migrations
COPY src ./src
RUN cargo build --locked --release --bins

FROM debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818 AS runtime

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home --shell /usr/sbin/nologin alpha

COPY --from=builder /build/target/release/api /usr/local/bin/api
COPY --from=builder /build/target/release/judge_worker /usr/local/bin/judge_worker
COPY --from=builder /build/target/release/metadata_worker /usr/local/bin/metadata_worker
COPY --from=builder /build/target/release/content_worker /usr/local/bin/content_worker
COPY --from=builder /build/target/release/migrate /usr/local/bin/migrate

ENV BIND_ADDRESS=0.0.0.0:8080
EXPOSE 8080
USER alpha
CMD ["api"]

FROM docker:29.6.1-cli@sha256:862099ada15c669000bef53aa4cb9d821262829f45b0dda2159ccb276443043b AS docker-cli

FROM runtime AS worker
USER root
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker
CMD ["judge_worker"]
