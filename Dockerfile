FROM rust:1.94-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --locked --release --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations
COPY openapi ./openapi
RUN cargo build --locked --release -p chaos-api --bins

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/chaos /usr/local/bin/chaos
COPY --from=builder /app/target/release/chaos-migrate /usr/local/bin/chaos-migrate
COPY --from=builder /app/target/release/chaos-worker /usr/local/bin/chaos-worker

USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/chaos"]
