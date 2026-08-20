#!/bin/sh
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
HEALTH_URL="${HEALTH_URL:-https://127.0.0.1/health/live}"
ORIGIN_HOST="${ORIGIN_HOST:-chaos.omnip.org}"

$COMPOSE config --quiet

echo "Deploying image: ${CHAOS_IMAGE}"

docker volume create chaos-postgres-data >/dev/null
docker volume create chaos-redis-data >/dev/null

$COMPOSE pull migrate api-blue api-green worker

$COMPOSE up -d --wait postgres redis

$COMPOSE run --rm migrate

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

$COMPOSE up -d --no-deps worker

$COMPOSE up -d --no-deps --wait --wait-timeout 30 nginx

curl --insecure --fail --silent --show-error \
    --header "Host: ${ORIGIN_HOST}" "${HEALTH_URL}" >/dev/null

$COMPOSE ps

docker image prune -f >/dev/null

echo "Deploy complete."
