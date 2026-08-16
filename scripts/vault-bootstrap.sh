#!/bin/sh
set -eu

: "${VAULT_ROOT_TOKEN:?Set VAULT_ROOT_TOKEN for this one-time bootstrap operation}"
COMPOSE="docker compose -f compose.yaml -f compose.ha.yaml"
VAULT_TOKEN="$VAULT_ROOT_TOKEN"
export VAULT_TOKEN

vault_exec() {
    $COMPOSE exec -T \
        -e VAULT_ADDR=https://127.0.0.1:8200 \
        -e VAULT_CACERT=/vault/tls/ca.pem \
        -e VAULT_TOKEN \
        vault vault "$@"
}

if ! vault_exec secrets list -format=json | jq -e 'has("secret/")' >/dev/null; then
    vault_exec secrets enable -path=secret -version=2 kv
fi

$COMPOSE exec -T \
    -e VAULT_ADDR=https://127.0.0.1:8200 \
    -e VAULT_CACERT=/vault/tls/ca.pem \
    -e VAULT_TOKEN \
    vault vault policy write chaos-api - <<'EOF'
path "secret/data/chaos/stores/*" {
  capabilities = ["create", "update", "read"]
}
EOF

echo "Store the following token as VAULT_TOKEN in the production environment:"
vault_exec token create \
    -policy=chaos-api \
    -no-default-policy \
    -orphan \
    -ttl=8760h \
    -display-name=chaos-api \
    -field=token
