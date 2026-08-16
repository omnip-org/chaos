#!/bin/sh
# Zero-downtime blue/green deploy from a pre-built registry image.
#
# Unlike rolling-update.sh (which builds locally), this pulls CHAOS_IMAGE from
# the registry and rolls the api-blue / api-green replicas one at a time behind
# the Caddy gateway. If a replica fails to become healthy, the rollout aborts
# before touching the second replica, so the previous version keeps serving.
#
# NOTE: zero downtime holds only when migrations are backward compatible, i.e.
# the currently-running (old) code must keep working against the new schema.
# Use expand/contract: ship additive migrations now, drop/rename in a later
# release once no old code references them.
#
# Usage:
#   CHAOS_IMAGE=ghcr.io/omnip-org/chaos:0.1.0 ./scripts/deploy-remote.sh
set -eu

: "${CHAOS_IMAGE:?CHAOS_IMAGE must be set (e.g. ghcr.io/omnip-org/chaos:0.1.0)}"

if [ ! -f .env ]; then
    echo "ERROR: production .env is missing; start from .env.production.example." >&2
    exit 1
fi
if grep -q 'CHANGE_ME' .env; then
    echo "ERROR: production .env still contains CHANGE_ME placeholders." >&2
    exit 1
fi

COMPOSE="docker compose -f compose.yaml -f compose.ha.yaml"
HEALTH_URL="http://127.0.0.1:${HTTP_PORT:-8080}/health/live"

# Fail before touching running services when interpolation or Compose structure
# is invalid.
$COMPOSE config --quiet

echo "Deploying image: ${CHAOS_IMAGE}"

# Ensure the external data volumes exist. They are declared external in
# compose.ha.yaml so compose never creates or deletes them (a stray
# `down -v` can't wipe production data). `volume create` is idempotent.
docker volume create chaos-postgres-data >/dev/null
docker volume create chaos-redis-data >/dev/null
docker volume create chaos-vault-data >/dev/null

# Start the repository-managed Vault when the deployment points at its Compose
# service. A sealed Vault is process-healthy but cannot serve secrets until the
# one-time initialization/unseal procedure has been completed.
if grep -Eq '^VAULT_ADDR=https://vault:8200/?$' .env; then
    if [ ! -f deploy/vault/tls/ca.pem ] || [ ! -f deploy/vault/tls/vault.pem ] || [ ! -f deploy/vault/tls/vault-key.pem ]; then
        echo "ERROR: Vault TLS material is missing; run ./scripts/vault-generate-tls.sh first." >&2
        exit 1
    fi
    $COMPOSE up -d --wait vault

    # Compose considers a sealed Vault healthy because the server process is
    # available for operator actions. Deployment, however, must not continue
    # until the secret service can actually answer application requests.
    vault_status="$($COMPOSE exec -T \
        -e VAULT_ADDR=https://127.0.0.1:8200 \
        -e VAULT_CACERT=/vault/tls/ca.pem \
        vault vault status -format=json 2>/dev/null || true)"
    if ! printf '%s' "$vault_status" | jq -e '.initialized == true' >/dev/null 2>&1; then
        echo "ERROR: Vault is not initialized; run scripts/vault-initialize.sh as documented." >&2
        exit 1
    fi
    if printf '%s' "$vault_status" | jq -e '.sealed == true' >/dev/null 2>&1; then
        echo "ERROR: Vault is sealed; run scripts/vault-unseal.sh before deploying." >&2
        exit 1
    fi
fi

# Pull the new image up front so the swap is fast and atomic-ish.
$COMPOSE pull migrate api-blue api-green

# Bootstrap stateful dependencies before the migration. This is required on a
# fresh host because the replica updates below intentionally use --no-deps.
$COMPOSE up -d --wait postgres redis

# Apply (backward-compatible) migrations before rolling the API replicas.
$COMPOSE run --rm migrate

# Roll one replica at a time. A failed --wait aborts the script (set -e) before
# the second replica is touched, leaving the other replica on the old version.
roll() {
    replica="$1"
    echo "Rolling ${replica} ..."
    if ! $COMPOSE up -d --no-deps --wait --wait-timeout 90 "$replica"; then
        echo "ERROR: ${replica} did not become healthy; aborting rollout." >&2
        $COMPOSE logs --tail 50 "$replica" >&2 || true
        exit 1
    fi
}

roll api-blue
roll api-green

# Ensure the gateway is up (no-op if already running).
$COMPOSE up -d --no-deps --wait --wait-timeout 30 gateway

$COMPOSE ps

# Verify zero downtime held during the rollout window.
if [ -x ./scripts/smoke-zero-downtime.sh ]; then
    ./scripts/smoke-zero-downtime.sh "$HEALTH_URL" 50
fi

# Reclaim disk from superseded images now that the new one is live.
# Deploys use version-pinned tags (e.g. :0.1.0), so older tags are not
# "dangling" — remove same-repo images other than the one we just deployed,
# then clean up any leftover dangling layers.
echo "Pruning old images ..."
repo="${CHAOS_IMAGE%:*}"
docker images --format '{{.Repository}}:{{.Tag}}' \
    | grep -E "^${repo}:" \
    | grep -vx "$CHAOS_IMAGE" \
    | xargs -r docker rmi 2>/dev/null || true
docker image prune -f >/dev/null

echo "Deploy complete."
