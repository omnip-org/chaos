#!/bin/sh
set -eu

TLS_DIR="${VAULT_TLS_DIR:-deploy/vault/tls}"
if [ -e "$TLS_DIR/ca.pem" ] || [ -e "$TLS_DIR/vault.pem" ] || [ -e "$TLS_DIR/vault-key.pem" ]; then
    echo "ERROR: Vault TLS material already exists; refusing to overwrite it." >&2
    exit 1
fi

temporary_directory="$(mktemp -d)"
trap 'rm -rf "$temporary_directory"' EXIT HUP INT TERM
umask 077

openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out "$temporary_directory/ca-key.pem"
openssl req -x509 -new -sha256 -days 3650 \
    -key "$temporary_directory/ca-key.pem" \
    -subj "/CN=Chaos Vault Internal CA" \
    -out "$TLS_DIR/ca.pem"

openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out "$TLS_DIR/vault-key.pem"
openssl req -new -sha256 \
    -key "$TLS_DIR/vault-key.pem" \
    -subj "/CN=vault" \
    -out "$temporary_directory/vault.csr"

{
    echo "subjectAltName=DNS:vault,DNS:localhost,IP:127.0.0.1"
    echo "extendedKeyUsage=serverAuth"
    echo "keyUsage=digitalSignature,keyEncipherment"
} > "$temporary_directory/vault.ext"

openssl x509 -req -sha256 -days 825 \
    -in "$temporary_directory/vault.csr" \
    -CA "$TLS_DIR/ca.pem" \
    -CAkey "$temporary_directory/ca-key.pem" \
    -CAcreateserial \
    -extfile "$temporary_directory/vault.ext" \
    -out "$TLS_DIR/vault.pem"

chmod 600 "$TLS_DIR/vault-key.pem"
chmod 644 "$TLS_DIR/ca.pem" "$TLS_DIR/vault.pem"
echo "Vault TLS material created in $TLS_DIR."

