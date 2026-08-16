#!/bin/sh
# Initialize Vault once, idempotently.
#
# Full-automation model: the deploy runs this on every rollout. It is a no-op
# once Vault is initialized, so first and subsequent deploys take the same path.
#
# `vault operator init` can only ever run once and it GENERATES the unseal
# shares + initial root token. To keep first deploys hands-off, that output is
# written to a git-ignored host file (default: ./.vault/init.json, 0600). The
# unseal shares therefore live on the deploy host next to the Raft data volume.
# This is an accepted trade-off of unattended Shamir unsealing; use a KMS/HSM
# auto-unseal seal if that co-location is unacceptable.
set -eu

VAULT_STATE_DIR="${VAULT_STATE_DIR:-.vault}"
VAULT_INIT_OUTPUT="${VAULT_INIT_OUTPUT:-$VAULT_STATE_DIR/init.json}"
COMPOSE="docker compose -f compose.yaml -f compose.ha.yaml"

vault_status() {
    $COMPOSE exec -T \
        -e VAULT_ADDR=https://127.0.0.1:8200 \
        -e VAULT_CACERT=/vault/tls/ca.pem \
        vault vault status -format=json 2>/dev/null || true
}

status="$(vault_status)"
if printf '%s' "$status" | jq -e '.initialized == true' >/dev/null 2>&1; then
    echo "Vault is already initialized; nothing to do."
    exit 0
fi

if [ -e "$VAULT_INIT_OUTPUT" ]; then
    echo "ERROR: Vault reports uninitialized but $VAULT_INIT_OUTPUT already exists." >&2
    echo "Refusing to overwrite existing unseal material. Inspect the host state." >&2
    exit 1
fi

mkdir -p "$VAULT_STATE_DIR"
umask 077
$COMPOSE exec -T \
    -e VAULT_ADDR=https://127.0.0.1:8200 \
    -e VAULT_CACERT=/vault/tls/ca.pem \
    vault vault operator init -key-shares=5 -key-threshold=3 -format=json > "$VAULT_INIT_OUTPUT"
chmod 600 "$VAULT_INIT_OUTPUT"

echo "Vault initialized. Unseal material written to $VAULT_INIT_OUTPUT (0600)."
echo "Back this file up to secure offline storage; losing it makes Vault data unrecoverable."
