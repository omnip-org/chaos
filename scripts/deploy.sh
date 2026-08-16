#!/bin/sh
# Zero-downtime blue/green deploy from a pre-built registry image.
#
# Run this ON THE DEPLOY HOST, by hand, after a Release build has published a
# new image tag to the registry (see the Release GitHub Actions workflow).
# There is no CI-to-host automation: nothing outside this host can SSH in and
# run this script, so a compromised or malicious CI run cannot reach
# production on its own.
#
# This never builds an image locally — it only pulls CHAOS_IMAGE from the
# registry and rolls the api-blue / api-green replicas one at a time behind
# the Caddy gateway. If a replica fails to become healthy, the rollout aborts
# before touching the second replica, so the previous version keeps serving.
#
# NOTE: zero downtime holds only when migrations are backward compatible, i.e.
# the currently-running (old) code must keep working against the new schema.
# Use expand/contract: ship additive migrations now, drop/rename in a later
# release once no old code references them.
#
# Usage (from /opt/chaos, or wherever this repo is checked out on the host):
#   git pull --ff-only
#   ./scripts/deploy.sh                                  # deploys :latest
#   CHAOS_IMAGE=ghcr.io/omnip-org/chaos:0.1.0 ./scripts/deploy.sh   # pinned/rollback
set -eu

export CHAOS_IMAGE="${CHAOS_IMAGE:-ghcr.io/omnip-org/chaos:latest}"

if [ ! -f .env ]; then
    echo "ERROR: .env is missing; start from .env.example." >&2
    exit 1
fi
if grep -vE '^\s*#' .env | grep -q 'CHANGE_ME'; then
    echo "ERROR: .env still contains CHANGE_ME placeholders." >&2
    exit 1
fi

COMPOSE="docker compose -f docker-compose.yaml"
HEALTH_URL="https://127.0.0.1:${HTTP_PORT:-443}/health/live"

# Fail before touching running services when interpolation or Compose structure
# is invalid.
$COMPOSE config --quiet

echo "Deploying image: ${CHAOS_IMAGE}"

# Ensure the external data volumes exist. They are declared external in
# docker-compose.yaml so compose never creates or deletes them (a stray
# `down -v` can't wipe production data). `volume create` is idempotent.
docker volume create chaos-postgres-data >/dev/null
docker volume create chaos-redis-data >/dev/null

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

# Reclaim disk from dangling layers. Deliberately does not remove other
# same-repo tags: with CHAOS_IMAGE defaulting to :latest, older version-pinned
# tags (e.g. :0.1.0) are kept locally so a rollback doesn't have to re-pull.
docker image prune -f >/dev/null

echo "Deploy complete."
