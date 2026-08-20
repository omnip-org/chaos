# Chaos Commerce

A headless commerce engine where Users create, join, and leave independent Stores. Each Store owns its channels, catalog, variants, publication state, orders, payments, refunds, fulfillment, and Publishable Keys. Users operate their Stores through AI MCP with User-owned private Keys. Chaos is built with Rust, Axum, PostgreSQL 18, and Redis 8.

## Local development

Requirements: Docker and Docker Compose. `deploy/docker-compose.yaml` defines PostgreSQL, Redis, the migration job, blue and green API replicas, an independent Worker, and the gateway. API replicas never run background polling loops. Migrations run through the `migrate` service, not application startup.

Its data volumes are `external: true` everywhere, so create them once:

```bash
docker volume create chaos-postgres-data
docker volume create chaos-redis-data
```

Build a local image and bring the whole stack up:

```bash
docker build -t chaos-api:local .
cp deploy/.env.example deploy/.env
# deploy/.env.example is the same template used in production: replace every
# CHANGE_ME_* value (openssl rand -base64 32/48 as noted inline) before
# the stack will boot. There is no weaker "just for local dev" shortcut.
export CHAOS_IMAGE=chaos-api:local
docker compose -f deploy/docker-compose.yaml up -d --wait
```

The repository includes the local self-signed origin certificate used behind Cloudflare.

After changing code, rebuild and roll both replicas (see [Production Deployment](docs/deployment.md) for the health-gated, one-at-a-time version — for local iteration a plain rebuild + `up -d` is usually enough):

```bash
docker build -t chaos-api:local .
docker compose -f deploy/docker-compose.yaml up -d --no-deps migrate api-blue api-green worker
```

Verify the service:

```bash
curl --insecure --header 'Host: chaos.omnip.org' https://localhost/health/live
curl --insecure --header 'Host: chaos.omnip.org' https://localhost/health/ready
```

Users sign in with a configured Google or Apple identity token. Chaos validates the provider token and issues a short-lived JWT; it stores no passwords or human sessions. Email configuration is used only for commerce notifications.

Stop the stack:

```bash
docker compose -f deploy/docker-compose.yaml down
```

`deploy/.env` and `CHAOS_IMAGE` must still be present/set for `down` (and every other compose command) to work — Compose parses the whole file, including every `${VAR:?...}`, before it can act. If you've deleted `deploy/.env` or unset `CHAOS_IMAGE` first, fall back to `docker rm -f` on the `chaos-*` containers.

The data volumes are `external: true`, so `down -v` cannot delete them — use `docker volume rm chaos-postgres-data chaos-redis-data` if you deliberately want a clean slate.

The custom PostgreSQL 18 image includes `pg_cron`, `pgmq`, and `pg_partman`. The initial migration activates them with isolated extension-owned schemas. See [PostgreSQL extensions](docs/postgresql-extensions.md) for lifecycle and security requirements.

For the production rollout procedure (registry image, zero-downtime `deploy/deploy.sh`) see [Production Deployment](docs/deployment.md).

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
  cargo test -p chaos-infrastructure rls_hides_other_stores_rows -- --ignored
```

Start with the [Repository Guide](docs/README.md) to locate a product area, then
see [Product Model](docs/product-model.md), [Architecture](docs/architecture.md),
[Database Conventions](docs/database-conventions.md), the OpenAPI contracts in
[`openapi/`](openapi), and [Contributing](CONTRIBUTING.md).

For the production topology, first-host bootstrap, encrypted dynamic Store Provider secrets, and release procedure, see [Production Deployment](docs/deployment.md).
