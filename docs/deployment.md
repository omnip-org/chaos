# Production Deployment

## Topology

Cloudflare terminates public TLS and proxies to the host gateway. Caddy listens on HTTP port 8080 and load-balances the blue and green API replicas; it does not issue a public certificate. Restrict origin access to Cloudflare or use Cloudflare Tunnel. PostgreSQL, Redis, Mailpit, metrics, and Vault must not be publicly reachable.

## Host bootstrap

Install Docker Engine, Docker Compose, Git, OpenSSL, `jq`, and the host backup tooling. Clone the repository to `/opt/chaos`, authenticate Docker to the image registry, and create the protected data volumes:

```bash
docker volume create chaos-postgres-data
docker volume create chaos-redis-data
docker volume create chaos-vault-data
```

Copy `.env.production.example` to `.env`, set mode `0600`, and fill every required value. `POSTGRES_PASSWORD` is the password of the `chaos` login role. Both database URLs use that login; the application assumes the migration-created `chaos_runtime` and `chaos_control_plane` NOLOGIN roles after connecting.

Configure Cloudflare's public origin as the external `AUTH_PUBLIC_BASE_URL` and `WEBAUTHN_RP_ORIGIN`. Both use `https://` even though the Cloudflare-to-Caddy hop is HTTP.

## Self-hosted Vault KV v2

Vault runs as a non-public Compose service with integrated Raft storage and internal TLS. Generate the internal CA and server certificate before the first start:

```bash
./scripts/vault-generate-tls.sh
docker volume create chaos-vault-data
docker compose -f compose.yaml -f compose.ha.yaml up -d --wait vault
```

Initialize and unseal it once. The output path must be outside the repository and backed by secure offline storage:

```bash
VAULT_INIT_OUTPUT=/secure/chaos-vault-init.json ./scripts/vault-initialize.sh
```

The initialization script enables KV v2, installs the bounded `chaos-api` policy, and prints a one-year orphan service token. Put that token in `VAULT_TOKEN`, then remove the initial root token from routine operations. With Shamir sealing, Vault must be unsealed after a restart:

```bash
VAULT_INIT_OUTPUT=/secure/chaos-vault-init.json ./scripts/vault-unseal.sh
```

Use a supported KMS/HSM seal when automatic unseal is required. Never store unseal shares beside the Raft volume.

The application token can create, update, and read only `secret/data/chaos/stores/*`. Supply it through `VAULT_TOKEN`; do not use a root token.

Operators may still write directly with the Vault CLI. Use a new path and CAS zero so a typo cannot overwrite an existing value:

```bash
vault kv put -cas=0 -mount=secret \
  chaos/stores/ACCOUNT_ID/STORE_ID/payment-credential/UNIQUE_ID \
  value='{"secret_key":"sk_live_xxx","publishable_key":"pk_live_xxx"}'

vault kv put -cas=0 -mount=secret \
  chaos/stores/ACCOUNT_ID/STORE_ID/payment-webhook/UNIQUE_ID \
  value='whsec_xxx'

vault kv put -cas=0 -mount=secret \
  chaos/stores/ACCOUNT_ID/STORE_ID/shipping-credential/UNIQUE_ID \
  value='EZAK_live_xxx'
```

The corresponding Admin API inputs are references only:

```text
vault://secret/chaos/stores/ACCOUNT_ID/STORE_ID/payment-credential/UNIQUE_ID
vault://secret/chaos/stores/ACCOUNT_ID/STORE_ID/payment-webhook/UNIQUE_ID
vault://secret/chaos/stores/ACCOUNT_ID/STORE_ID/shipping-credential/UNIQUE_ID
```

Normal Store onboarding does not need Vault CLI access. An owner or administrator uploads a secret through the Admin API:

```bash
curl --fail-with-body \
  -X POST "$BASE_URL/admin/v1/merchant-accounts/$ACCOUNT_ID/stores/$STORE_ID/provider-secrets" \
  -H "Authorization: Bearer $SESSION_TOKEN" \
  -H "Content-Type: application/json" \
  --data '{"kind":"payment_webhook","value":"whsec_xxx"}'
```

The response contains a newly generated `vault://secret/chaos/stores/...` reference. The plaintext is not returned or stored in PostgreSQL. Use that reference in the existing Provider account create/update request.

Adding or changing a Vault value takes effect on the next Provider operation on both replicas. It does not require a deployment or restart. Create a new path and update the Provider account reference when the 24-hour application-level overlap is required; updating a value in place is immediate but bypasses that overlap evidence.

## First deployment

Set a version-pinned image and run the deployment script:

```bash
export CHAOS_IMAGE=ghcr.io/OWNER/chaos:VERSION
./scripts/deploy-remote.sh
```

The script creates the external volumes if absent, starts the repository-managed Vault, refuses to continue while Vault is uninitialized or sealed, pulls the release image, starts PostgreSQL and Redis, applies migrations once, rolls blue and green independently, starts the gateway, and runs health probes.

Verify the origin and Cloudflare paths:

```bash
curl --fail http://127.0.0.1:8080/health/live
curl --fail https://api.example.com/health/ready
```

## Store onboarding without a restart

1. Authenticate through the Admin email-link or passkey flow.
2. Create the Merchant Account and Store through the Admin API.
3. Create Store-scoped publishable and secret API keys. Preserve each plaintext key returned once; these keys do not belong in the Chaos API environment.
4. Upload third-party credentials through `POST .../provider-secrets`; direct Vault access is not required for Store administrators.
5. Create disabled Payment, Shipping, or Analytics Provider configuration with the `vault://` references.
6. Complete external Provider onboarding, request enablement, and confirm readiness.
7. Activate the Store and exercise a non-destructive quote or test transaction.

## Deployment secrets and rotation

`PRODUCTION_ENV` contains platform bootstrap configuration only: database access, token signing, email, media storage, and Vault access. Store-specific Provider values live in Vault and are not copied into GitHub Actions secrets or `.env`.

Changing `PRODUCTION_ENV` causes a normal blue/green rollout. Changing or adding a Vault Provider secret does not.
