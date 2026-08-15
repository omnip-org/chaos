#!/bin/sh
set -eu

: "${BASE_URL:?BASE_URL is required}"
: "${PUBLISHABLE_KEY:?PUBLISHABLE_KEY is required}"
: "${PRODUCT_VARIANT_ID:?PRODUCT_VARIANT_ID is required}"

command -v k6 >/dev/null 2>&1 || {
    echo "k6 is required" >&2
    exit 1
}

k6 run \
    -e BASE_URL="$BASE_URL" \
    -e PUBLISHABLE_KEY="$PUBLISHABLE_KEY" \
    -e PRODUCT_VARIANT_ID="$PRODUCT_VARIANT_ID" \
    --vus 50 \
    --duration 10m \
    --summary-export capacity-summary.json \
    scripts/capacity.js
