# Production Deployment

## Topology

Cloudflare terminates public TLS and proxies to the host gateway. Caddy listens on HTTP port 8080 and load-balances the blue and green API replicas; it does not issue a public certificate. Restrict origin access to Cloudflare or use Cloudflare Tunnel. PostgreSQL, Redis, Mailpit, and metrics must not be publicly reachable.

## Host bootstrap

Install Docker Engine, Docker Compose, Git, OpenSSL, `jq`, and the host backup tooling. Clone the repository to `/opt/chaos`, authenticate Docker to the image registry, and create the protected data volumes:

```bash
docker volume create chaos-postgres-data
docker volume create chaos-redis-data
```

Copy `.env.production.example` to `.env`, set mode `0600`, and fill every required value. `POSTGRES_PASSWORD` is the password of the `chaos` login role. Both database URLs use that login; the application assumes the migration-created `chaos_runtime` and `chaos_control_plane` NOLOGIN roles after connecting.

Configure Cloudflare's public origin as the external `AUTH_PUBLIC_BASE_URL` and `WEBAUTHN_RP_ORIGIN`. Both use `https://` even though the Cloudflare-to-Caddy hop is HTTP.

## Provider secret encryption

Payment, shipping, and analytics Provider Key secrets are AES-256-GCM encrypted and stored directly in PostgreSQL as an opaque `enc://<base64>` reference. There is no separate secret-manager service to run. `CHAOS_PROVIDER_SECRET_KEY` in `PRODUCTION_ENV` is the only key material: exactly 32 raw bytes, base64-encoded (`openssl rand -base64 32`), read once at startup.

**Back this key up like you would the database itself.** There is no rotation or re-encryption tooling — losing the key makes every previously stored Provider Key permanently unrecoverable, and rotating it requires an owner/administrator to re-submit every Provider Key for every Store through the Admin API.

An owner or administrator uploads a Provider Key through the Admin API:

```bash
curl --fail-with-body \
  -X POST "$BASE_URL/admin/v1/merchant-accounts/$ACCOUNT_ID/stores/$STORE_ID/provider-secrets" \
  -H "Authorization: Bearer $SESSION_TOKEN" \
  -H "Content-Type: application/json" \
  --data '{"kind":"payment_webhook","value":"whsec_xxx"}'
```

The response contains a newly generated `enc://...` reference. The plaintext is not returned again or stored anywhere in plaintext. Use that reference in the existing Provider account create/update request.

Adding or changing an encrypted Provider Key takes effect on the next Provider operation on both replicas. It does not require a deployment or restart. Submit a new value and update the Provider account reference when the 24-hour application-level overlap is required.

## Deployment

Deploys are fully automated through GitHub Actions: pushing a `v*` tag builds and publishes the image (Release), which triggers Deploy to roll it out over SSH. `deploy.yml` writes `PRODUCTION_ENV` to `/opt/chaos/.env` and runs `deploy-remote.sh`. The same flow serves the first deploy and every subsequent one — there is no separate manual first-deploy procedure.

To roll out manually (or from a fresh checkout on the host):

```bash
export CHAOS_IMAGE=ghcr.io/OWNER/chaos:VERSION
./scripts/deploy-remote.sh
```

The script creates the external volumes if absent; pulls the release image; starts PostgreSQL and Redis; applies migrations once; rolls blue and green independently; starts the gateway; and runs health probes. If a replica fails to become healthy the rollout aborts before touching the second replica, so the previous version keeps serving.

Verify the origin and Cloudflare paths:

```bash
curl --fail http://127.0.0.1:8080/health/live
curl --fail https://api.example.com/health/ready
```

## Store onboarding without a restart

1. Authenticate through the Admin email-link or passkey flow.
2. Create the Merchant Account and Store through the Admin API.
3. Create Store-scoped publishable and secret API keys. Preserve each plaintext key returned once; these keys do not belong in the Chaos API environment.
4. Upload third-party credentials through `POST .../provider-secrets`.
5. Create disabled Payment, Shipping, or Analytics Provider configuration with the returned `enc://` references.
6. Complete external Provider onboarding, request enablement, and confirm readiness.
7. Activate the Store and exercise a non-destructive quote or test transaction.

## Deployment secrets and rotation

`PRODUCTION_ENV` contains platform bootstrap configuration only: database access, token signing, email, media storage, and `CHAOS_PROVIDER_SECRET_KEY`. Store-specific Provider Key plaintext is never copied into CI secrets or `.env` — only the encryption key that seals it in PostgreSQL lives there.

Changing `PRODUCTION_ENV` causes a normal blue/green rollout. Changing or adding an encrypted Provider secret through the Admin API does not.
