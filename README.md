# Chaos Commerce

A modern headless commerce backend where one user can operate multiple merchant accounts and each merchant account can run multiple independent storefronts. It provides isolated catalog, pricing, inventory, cart, order, payment, fulfillment, and multi-currency capabilities with Rust, Axum, PostgreSQL 18, and Redis 8.

## Local development

Requirements: Rust 1.94+, Docker, Docker Compose, and SQLx CLI. Run `cargo sqlx --version` to verify the CLI installation.

```bash
docker compose up -d
cp .env.example .env
set -a && source .env && set +a
cargo sqlx migrate run
cargo run -p chaos-api --bin chaos
```

Verify the service:

```bash
curl http://localhost:8080/health/live
curl http://localhost:8080/health/ready
```

Development sign-in emails are captured by Mailpit at `http://localhost:58025`. The backend supports one-time email links and WebAuthn passkeys; it never stores account passwords.

Stop the dependencies:

```bash
docker compose down
```

`docker compose down -v` also deletes the local PostgreSQL and Redis volumes. Use it with care.

## Dual-instance Compose deployment

Start Caddy, two blue/green API instances, the migration job, PostgreSQL, Redis, and the development mail catcher:

```bash
docker compose -f compose.yaml -f compose.ha.yaml up -d --build --wait
curl http://localhost:8080/health/ready
```

The custom PostgreSQL 18 image includes `pg_cron`, `pgmq`, and `pg_partman`. The initial migration activates them with isolated extension-owned schemas. See [PostgreSQL extensions](docs/postgresql-extensions.md) for lifecycle and security requirements.

Replace both API instances sequentially after a code or image update:

```bash
./scripts/rolling-update.sh
```

The script updates blue and waits for readiness before updating green, so an old or new instance remains available throughout the deployment. Both instances must be healthy before an update. Database migrations must remain compatible with both adjacent application versions.

`/health/gateway` checks the stable gateway, `/health/live` checks an API process, and `/health/ready` checks whether one API instance can accept new traffic. A draining instance returns 503 from its readiness endpoint by design; this does not mean that business traffic through the gateway is interrupted.

Stop the complete stack without deleting data:

```bash
docker compose -f compose.yaml -f compose.ha.yaml down
```

## Development commands

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
./scripts/check-language.sh
```

The PostgreSQL RLS integration test is ignored by default and can be run explicitly:

```bash
TEST_DATABASE_URL=postgres://chaos:chaos@localhost:55432/chaos \
  cargo test -p chaos-infrastructure rls_hides_other_merchant_accounts_rows -- --ignored
```

See [Product Model](docs/product-model.md), [System Architecture](docs/architecture.md), [Delivery Roadmap](docs/delivery-roadmap.md), [Database Conventions](docs/database-conventions.md), [HTTP API Contract](docs/http-api.md), and [Contributing](CONTRIBUTING.md).
