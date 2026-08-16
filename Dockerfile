# syntax=docker/dockerfile:1

# --- Plan stage: compute a dependency recipe that changes only when deps change.
FROM rust:1.94-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

# --- Build stage: cook dependencies first (cached), then build the app.
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# This layer is cached as long as recipe.json (i.e. the dependency set) is
# unchanged, so editing application code does not recompile dependencies.
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

USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/chaos"]
