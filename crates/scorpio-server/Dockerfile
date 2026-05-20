# syntax=docker/dockerfile:1
FROM rust:1.93-slim AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --locked --release -p scorpio-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --user-group --create-home --home-dir /var/lib/scorpio-server scorpio
# Loco resolves `config/<env>.yaml` relative to the current working directory,
# so we copy the crate's config tree into the runtime image and run from there.
WORKDIR /var/lib/scorpio-server
COPY --from=builder /app/target/release/scorpio-server /usr/local/bin/scorpio-server
COPY --from=builder /app/crates/scorpio-server/config ./config
ENV LOCO_ENV=production
USER scorpio:scorpio
EXPOSE 8088
ENTRYPOINT ["scorpio-server", "start"]
