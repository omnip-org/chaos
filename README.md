# Chaos Commerce

A modern headless commerce backend where one user can operate multiple merchant accounts and each merchant account can run multiple independent storefronts. It provides isolated catalog, pricing, inventory, cart, order, payment, fulfillment, and multi-currency capabilities with Rust, Axum, PostgreSQL 18, and Redis 8.

## Local development

Requirements: Docker and Docker Compose. `docker-compose.yaml` is the one and only topology — local and production run the identical blue/green setup (`postgres`, `redis`, `migrate`, `api-blue`, `api-green`, `gateway`), differing only by environment variables. There is no `cargo run` shortcut: application code always runs inside the same image production uses, and migrations run through the `migrate` service, not `cargo sqlx migrate run`. Rust and the SQLx CLI are only needed for compiling/testing the code itself (see [Development commands](#development-commands)), not for running the stack.

Its data volumes are `external: true` everywhere, so create them once:

```bash
docker volume create chaos-postgres-data
docker volume create chaos-redis-data
```

Build a local image and bring the whole stack up:

```bash
docker build -t chaos-api:local .
cp .env.example .env
# .env.example is the same template used in production: replace every
# CHANGE_ME_* value (openssl rand -base64 32/48 as noted inline) before
# the stack will boot. There is no weaker "just for local dev" shortcut.
export CHAOS_IMAGE=chaos-api:local
docker compose -f docker-compose.yaml up -d --wait
```

After changing code, rebuild and roll both replicas (see [Production Deployment](docs/deployment.md) for the health-gated, one-at-a-time version — for local iteration a plain rebuild + `up -d` is usually enough):

```bash
docker build -t chaos-api:local .
docker compose -f docker-compose.yaml up -d --no-deps migrate api-blue api-green
```

Verify the service:

```bash
curl http://localhost:8080/health/live
curl http://localhost:8080/health/ready
```

The backend supports one-time email links and WebAuthn passkeys; it never stores account passwords. There is no local mail catcher — set `RESEND_API_KEY`/`RESEND_WEBHOOK_SECRET` in `.env` to actually receive sign-in emails locally (see `.env.example`). Authentication tokens are sent directly from process memory and are never written to the general notification outbox. Ordinary commerce messages use durable, versioned notification requests.

Stop the stack:

```bash
docker compose -f docker-compose.yaml down
```

`.env` and `CHAOS_IMAGE` must still be present/set for `down` (and every other compose command) to work — compose parses the whole file, including every `${VAR:?...}`, before it can act. If you've deleted `.env` or unset `CHAOS_IMAGE` first, fall back to `docker rm -f` on the `chaos-*` containers.

The data volumes are `external: true`, so `down -v` cannot delete them — use `docker volume rm chaos-postgres-data chaos-redis-data` if you deliberately want a clean slate.

The custom PostgreSQL 18 image includes `pg_cron`, `pgmq`, and `pg_partman`. The initial migration activates them with isolated extension-owned schemas. See [PostgreSQL extensions](docs/postgresql-extensions.md) for lifecycle and security requirements.

For the production rollout procedure (registry image, zero-downtime `scripts/deploy.sh`) see [Production Deployment](docs/deployment.md).

## Development commands

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
npm test --prefix packages/js
npm run build --prefix packages/storefront-template
./scripts/check-language.sh
```

The PostgreSQL RLS integration test is ignored by default and can be run explicitly:

```bash
TEST_DATABASE_URL=postgres://chaos:chaos@localhost:55432/chaos \
  cargo test -p chaos-infrastructure rls_hides_other_merchant_accounts_rows -- --ignored
```

See [Product Model](docs/product-model.md), [System Architecture](docs/architecture.md), [Delivery Roadmap](docs/delivery-roadmap.md), [Database Conventions](docs/database-conventions.md), [HTTP API Contract](docs/http-api.md), and [Contributing](CONTRIBUTING.md).

For the Cloudflare-to-Caddy production topology, first-host bootstrap, encrypted dynamic Store Provider secrets, and release procedure, see [Production Deployment](docs/deployment.md).
