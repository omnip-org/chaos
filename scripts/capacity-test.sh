#!/bin/sh
set -eu

: "${BASE_URL:?BASE_URL is required}"
: "${METRICS_URL:?METRICS_URL is required}"
: "${PUBLISHABLE_KEY:?PUBLISHABLE_KEY is required}"
: "${PRODUCT_VARIANT_ID:?PRODUCT_VARIANT_ID is required}"
: "${SHIPPING_SERVICE_ID:?SHIPPING_SERVICE_ID is required}"
: "${DEPLOYMENT_REVISION:?DEPLOYMENT_REVISION is required}"
: "${DATASET_SIZE:?DATASET_SIZE is required}"
: "${API_INSTANCES:?API_INSTANCES is required}"

CAPACITY_VUS="${CAPACITY_VUS:-50}"
CAPACITY_WARMUP="${CAPACITY_WARMUP:-2m}"
CAPACITY_DURATION="${CAPACITY_DURATION:-10m}"
OUTPUT_DIR="${OUTPUT_DIR:-capacity-results/$(date -u +%Y%m%dT%H%M%SZ)}"

command -v k6 >/dev/null 2>&1 || {
    echo "k6 is required" >&2
    exit 1
}
command -v curl >/dev/null 2>&1 || {
    echo "curl is required" >&2
    exit 1
}
command -v jq >/dev/null 2>&1 || {
    echo "jq is required" >&2
    exit 1
}

mkdir -p "$OUTPUT_DIR"
curl --fail --silent --show-error "$METRICS_URL/metrics" > "$OUTPUT_DIR/metrics-before.prom"

k6 run \
    -e BASE_URL="$BASE_URL" \
    -e PUBLISHABLE_KEY="$PUBLISHABLE_KEY" \
    -e PRODUCT_VARIANT_ID="$PRODUCT_VARIANT_ID" \
    -e SHIPPING_SERVICE_ID="$SHIPPING_SERVICE_ID" \
    -e CAPACITY_VUS="$CAPACITY_VUS" \
    -e CAPACITY_WARMUP="$CAPACITY_WARMUP" \
    -e CAPACITY_DURATION="$CAPACITY_DURATION" \
    --summary-export "$OUTPUT_DIR/k6-summary.json" \
    scripts/capacity.js

curl --fail --silent --show-error "$METRICS_URL/metrics" > "$OUTPUT_DIR/metrics-after.prom"
jq -n \
    --arg recorded_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg base_url "$BASE_URL" \
    --arg metrics_url "$METRICS_URL" \
    --arg deployment_revision "$DEPLOYMENT_REVISION" \
    --argjson dataset_size "$DATASET_SIZE" \
    --argjson api_instances "$API_INSTANCES" \
    --argjson virtual_users "$CAPACITY_VUS" \
    --arg warmup "$CAPACITY_WARMUP" \
    --arg duration "$CAPACITY_DURATION" \
    '{recorded_at: $recorded_at, base_url: $base_url, metrics_url: $metrics_url,
      deployment_revision: $deployment_revision, dataset_size: $dataset_size,
      api_instances: $api_instances, virtual_users: $virtual_users,
      warmup: $warmup, duration: $duration}' > "$OUTPUT_DIR/environment.json"

printf 'Capacity evidence written to %s\n' "$OUTPUT_DIR"
