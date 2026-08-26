#!/bin/sh
set -eu

if [ -z "${CHAOS_IMAGE:-}" ]; then
    echo "ERROR: CHAOS_IMAGE must be set to a version-pinned image (tag or digest)." >&2
    exit 1
fi
case "$CHAOS_IMAGE" in
    *:latest)
        echo "ERROR: CHAOS_IMAGE must not use the mutable :latest tag." >&2
        exit 1
        ;;
esac
export ACTIVE_UPSTREAM_FILE="${ACTIVE_UPSTREAM_FILE:-nginx/conf.d/active-upstream.conf}"

if [ ! -f .env ]; then
    echo "ERROR: .env is missing; start from .env.example." >&2
    exit 1
fi
if grep -vE '^\s*#' .env | grep -q 'CHANGE_ME'; then
    echo "ERROR: .env still contains CHANGE_ME placeholders." >&2
    exit 1
fi

COMPOSE="docker compose -f docker-compose.yaml"
ORIGIN_HOST="${ORIGIN_HOST:-chaos.omnip.org}"
HEALTH_URL="${HEALTH_URL:-https://${ORIGIN_HOST}/health/ready}"
ACTIVE_COLOR_FILE="${ACTIVE_COLOR_FILE:-.active-api}"

if [ ! -d "$(dirname "$ACTIVE_UPSTREAM_FILE")" ]; then
    echo "ERROR: directory for ACTIVE_UPSTREAM_FILE does not exist: ${ACTIVE_UPSTREAM_FILE}" >&2
    exit 1
fi

$COMPOSE config --quiet

echo "Deploying image: ${CHAOS_IMAGE}"

docker volume create chaos-postgres-data >/dev/null
docker volume create chaos-redis-data >/dev/null

$COMPOSE pull migrate api-blue api-green worker
$COMPOSE up -d --wait postgres redis
$COMPOSE run --rm migrate

write_upstream() {
    replica="$1"
    temporary_file="${ACTIVE_UPSTREAM_FILE}.tmp.$$"
    {
        printf '%s\n' 'upstream chaos_api {'
        printf '%s\n' '    zone chaos_api 64k;'
        printf '    server %s:8080 resolve;\n' "$replica"
        printf '%s\n' '    keepalive 32;'
        printf '%s\n' '}'
    } >"$temporary_file"
    mv "$temporary_file" "$ACTIVE_UPSTREAM_FILE"
}

write_active_color() {
    color="$1"
    temporary_file="${ACTIVE_COLOR_FILE}.tmp.$$"
    printf '%s\n' "$color" >"$temporary_file"
    mv "$temporary_file" "$ACTIVE_COLOR_FILE"
}

roll() {
    replica="$1"
    echo "Starting ${replica} ..."
    if ! $COMPOSE up -d --no-deps --wait --wait-timeout 90 "$replica"; then
        echo "ERROR: ${replica} did not become healthy; aborting rollout." >&2
        $COMPOSE logs --tail 50 "$replica" >&2 || true
        exit 1
    fi
}

start_or_reload_gateway() {
    if ! $COMPOSE up -d --no-deps --wait --wait-timeout 30 nginx; then
        echo "ERROR: NGINX did not start." >&2
        $COMPOSE logs --tail 50 nginx >&2 || true
        return 1
    fi
    if ! $COMPOSE exec -T nginx nginx -t; then
        echo "ERROR: NGINX rejected the active upstream configuration." >&2
        return 1
    fi
    if ! $COMPOSE exec -T nginx nginx -s reload; then
        echo "ERROR: NGINX failed to reload the active upstream configuration." >&2
        return 1
    fi
}

activate() {
    new_replica="$1"
    old_replica="${2:-}"
    write_upstream "$new_replica"
    write_active_color "${new_replica#api-}"

    if start_or_reload_gateway; then
        return 0
    fi

    if [ -n "$old_replica" ]; then
        write_upstream "$old_replica"
        write_active_color "${old_replica#api-}"
        $COMPOSE exec -T nginx nginx -s reload >/dev/null 2>&1 || true
    else
        rm -f "$ACTIVE_COLOR_FILE"
    fi
    return 1
}

active_color=""
if [ -f "$ACTIVE_COLOR_FILE" ]; then
    active_color="$(sed -n '1p' "$ACTIVE_COLOR_FILE")"
    case "$active_color" in
        blue|green) ;;
        *)
            echo "ERROR: ${ACTIVE_COLOR_FILE} must contain blue or green." >&2
            exit 1
            ;;
    esac
fi

if [ -z "$active_color" ]; then
    # Bootstrap with blue, expose it, then warm the other color for the next
    # release. The gateway never points at an unready replica.
    roll api-blue
    if ! activate api-blue; then
        echo "ERROR: failed to activate api-blue." >&2
        exit 1
    fi
    roll api-green
else
    if [ "$active_color" = blue ]; then
        target_replica=api-green
    else
        target_replica=api-blue
    fi
    active_replica="api-${active_color}"

    roll "$target_replica"
    if ! activate "$target_replica" "$active_replica"; then
        echo "ERROR: failed to switch traffic to ${target_replica}; ${active_replica} remains active." >&2
        exit 1
    fi

    echo "Stopping ${active_replica} after traffic switch ..."
    if ! $COMPOSE stop --timeout 45 "$active_replica"; then
        echo "ERROR: ${active_replica} did not stop cleanly; traffic remains on ${target_replica}." >&2
        exit 1
    fi
fi

if ! $COMPOSE up -d --no-deps --wait --wait-timeout 30 worker; then
    echo "ERROR: worker did not become healthy." >&2
    $COMPOSE logs --tail 50 worker >&2 || true
    exit 1
fi

if ! curl --insecure --fail --silent --show-error \
    --header "Host: ${ORIGIN_HOST}" "${HEALTH_URL}" >/dev/null; then
    echo "ERROR: public readiness probe failed: ${HEALTH_URL}" >&2
    $COMPOSE logs --tail 50 nginx >&2 || true
    exit 1
fi

$COMPOSE ps
docker image prune -f >/dev/null

echo "Deploy complete; active API color: $(sed -n '1p' "$ACTIVE_COLOR_FILE")"
