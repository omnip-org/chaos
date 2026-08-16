#!/bin/sh
set -eu

: "${VAULT_INIT_OUTPUT:?Set VAULT_INIT_OUTPUT to an absolute path outside the repository}"
case "$VAULT_INIT_OUTPUT" in
    /*) ;;
    *) echo "ERROR: VAULT_INIT_OUTPUT must be an absolute path." >&2; exit 1 ;;
esac
if [ -e "$VAULT_INIT_OUTPUT" ]; then
    echo "ERROR: $VAULT_INIT_OUTPUT already exists; refusing to overwrite it." >&2
    exit 1
fi

COMPOSE="docker compose -f compose.yaml -f compose.ha.yaml"
umask 077
$COMPOSE exec -T \
    -e VAULT_ADDR=https://127.0.0.1:8200 \
    -e VAULT_CACERT=/vault/tls/ca.pem \
    vault vault operator init -key-shares=5 -key-threshold=3 -format=json > "$VAULT_INIT_OUTPUT"
chmod 600 "$VAULT_INIT_OUTPUT"

for index in 0 1 2; do
    unseal_key="$(jq -r ".unseal_keys_b64[$index]" "$VAULT_INIT_OUTPUT")"
    $COMPOSE exec -T \
        -e VAULT_ADDR=https://127.0.0.1:8200 \
        -e VAULT_CACERT=/vault/tls/ca.pem \
        vault vault operator unseal "$unseal_key" >/dev/null
done

echo "Vault initialized and unsealed. Initialization material is at $VAULT_INIT_OUTPUT."
echo "Move its five unseal shares and initial root token to separate secure custody."

root_token="$(jq -r '.root_token' "$VAULT_INIT_OUTPUT")"
VAULT_ROOT_TOKEN="$root_token" ./scripts/vault-bootstrap.sh
