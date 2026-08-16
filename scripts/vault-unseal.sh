#!/bin/sh
set -eu

: "${VAULT_INIT_OUTPUT:?Set VAULT_INIT_OUTPUT to the secure initialization JSON path}"
COMPOSE="docker compose -f compose.yaml -f compose.ha.yaml"

for index in 0 1 2; do
    unseal_key="$(jq -r ".unseal_keys_b64[$index]" "$VAULT_INIT_OUTPUT")"
    $COMPOSE exec -T \
        -e VAULT_ADDR=https://127.0.0.1:8200 \
        -e VAULT_CACERT=/vault/tls/ca.pem \
        vault vault operator unseal "$unseal_key" >/dev/null
done

echo "Vault is unsealed."

