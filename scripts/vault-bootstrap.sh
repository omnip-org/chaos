#!/bin/sh
# Bootstrap the Chaos secret engine + policy + application token, idempotently.
#
# Reads the initial root token from the host init file (written by
# vault-initialize.sh), enables KV v2, installs the bounded chaos-api policy,
# and mints the long-lived application token into a git-ignored host file
# (default: ./.vault/token, 0600). deploy-remote.sh injects that file as the
# runtime VAULT_TOKEN, so it never has to travel through CI/PRODUCTION_ENV.
#
# Safe to re-run: the engine/policy steps converge, and the token is only
# minted when the host token file is absent or its token is no longer valid.
set -eu

VAULT_STATE_DIR="${VAULT_STATE_DIR:-.vault}"
VAULT_INIT_OUTPUT="${VAULT_INIT_OUTPUT:-$VAULT_STATE_DIR/init.json}"
VAULT_TOKEN_OUTPUT="${VAULT_TOKEN_OUTPUT:-$VAULT_STATE_DIR/token}"
COMPOSE="docker compose -f compose.yaml -f compose.ha.yaml"

if [ -n "${VAULT_ROOT_TOKEN:-}" ]; then
    root_token="$VAULT_ROOT_TOKEN"
elif [ -f "$VAULT_INIT_OUTPUT" ]; then
    root_token="$(jq -r '.root_token' "$VAULT_INIT_OUTPUT")"
else
    echo "ERROR: no VAULT_ROOT_TOKEN and $VAULT_INIT_OUTPUT is missing." >&2
    exit 1
fi

# --- Enable KV v2 at secret/ (idempotent) ------------------------------------
if ! $COMPOSE exec -T \
        -e VAULT_ADDR=https://127.0.0.1:8200 \
        -e VAULT_CACERT=/vault/tls/ca.pem \
        -e VAULT_TOKEN="$root_token" \
        vault vault secrets list -format=json | jq -e 'has("secret/")' >/dev/null; then
    $COMPOSE exec -T \
        -e VAULT_ADDR=https://127.0.0.1:8200 \
        -e VAULT_CACERT=/vault/tls/ca.pem \
        -e VAULT_TOKEN="$root_token" \
        vault vault secrets enable -path=secret -version=2 kv
fi

# --- Install the bounded chaos-api policy (idempotent write) -----------------
$COMPOSE exec -T \
    -e VAULT_ADDR=https://127.0.0.1:8200 \
    -e VAULT_CACERT=/vault/tls/ca.pem \
    -e VAULT_TOKEN="$root_token" \
    vault vault policy write chaos-api - <<'EOF'
path "secret/data/chaos/stores/*" {
  capabilities = ["create", "update", "read"]
}
EOF

# --- Reuse an existing valid application token if present ---------------------
if [ -f "$VAULT_TOKEN_OUTPUT" ]; then
    existing="$(cat "$VAULT_TOKEN_OUTPUT")"
    if printf '%s' "$existing" | grep -q .; then
        if $COMPOSE exec -T \
                -e VAULT_ADDR=https://127.0.0.1:8200 \
                -e VAULT_CACERT=/vault/tls/ca.pem \
                -e VAULT_TOKEN="$existing" \
                vault vault token lookup >/dev/null 2>&1; then
            echo "Existing chaos-api token is still valid; reusing $VAULT_TOKEN_OUTPUT."
            exit 0
        fi
    fi
fi

# --- Mint a fresh bounded application token ----------------------------------
mkdir -p "$VAULT_STATE_DIR"
umask 077
app_token="$($COMPOSE exec -T \
    -e VAULT_ADDR=https://127.0.0.1:8200 \
    -e VAULT_CACERT=/vault/tls/ca.pem \
    -e VAULT_TOKEN="$root_token" \
    vault vault token create \
        -policy=chaos-api \
        -no-default-policy \
        -orphan \
        -ttl=8760h \
        -display-name=chaos-api \
        -field=token)"

printf '%s' "$app_token" > "$VAULT_TOKEN_OUTPUT"
chmod 600 "$VAULT_TOKEN_OUTPUT"
echo "Minted chaos-api token into $VAULT_TOKEN_OUTPUT (0600)."
