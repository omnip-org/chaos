#!/bin/sh
# Unseal Vault, idempotently, from the host-stored init material.
#
# Shamir sealing re-seals Vault on every restart, so the deploy runs this on
# every rollout. It is a no-op when Vault is already unsealed.
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
if ! printf '%s' "$status" | jq -e '.initialized == true' >/dev/null 2>&1; then
    echo "ERROR: Vault is not initialized; run scripts/vault-initialize.sh first." >&2
    exit 1
fi
if printf '%s' "$status" | jq -e '.sealed == false' >/dev/null 2>&1; then
    echo "Vault is already unsealed; nothing to do."
    exit 0
fi
if [ ! -f "$VAULT_INIT_OUTPUT" ]; then
    echo "ERROR: $VAULT_INIT_OUTPUT is missing; cannot unseal without the shares." >&2
    exit 1
fi

# key-threshold is 3; apply shares until Vault reports unsealed.
for index in 0 1 2; do
    unseal_key="$(jq -r ".unseal_keys_b64[$index]" "$VAULT_INIT_OUTPUT")"
    $COMPOSE exec -T \
        -e VAULT_ADDR=https://127.0.0.1:8200 \
        -e VAULT_CACERT=/vault/tls/ca.pem \
        vault vault operator unseal "$unseal_key" >/dev/null
done

echo "Vault is unsealed."
