#!/bin/sh
set -eu

docker compose -f compose.yaml -f compose.ha.yaml up -d --no-deps --wait --wait-timeout 30 api-blue api-green
docker compose -f compose.yaml -f compose.ha.yaml build api-blue
docker compose -f compose.yaml -f compose.ha.yaml run --rm migrate
docker compose -f compose.yaml -f compose.ha.yaml up -d --no-deps --wait --wait-timeout 90 api-blue
docker compose -f compose.yaml -f compose.ha.yaml up -d --no-deps --wait --wait-timeout 90 api-green

docker compose -f compose.yaml -f compose.ha.yaml ps
