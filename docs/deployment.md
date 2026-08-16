# Production Deployment

## Topology

Cloudflare terminates public TLS and proxies to the host gateway. Caddy listens on HTTP port 8080 and load-balances the blue and green API replicas; it does not issue a public certificate. Restrict origin access to Cloudflare or use Cloudflare Tunnel. PostgreSQL, Redis, and metrics must not be publicly reachable.

## Host bootstrap

Install Docker Engine, Docker Compose, Git, OpenSSL, `jq`, and the host backup tooling. Clone the repository to `/opt/chaos`, then create the protected data volumes:

```bash
docker volume create chaos-postgres-data
docker volume create chaos-redis-data
```

If the `ghcr.io/omnip-org/chaos` package is private, authenticate Docker to GHCR once with a GitHub PAT that has `read:packages` scope (nothing in `deploy.sh` does this for you — there is no CI-to-host automation):

```bash
echo "$GHCR_READ_TOKEN" | docker login ghcr.io -u YOUR_GITHUB_USERNAME --password-stdin
```

Copy `.env.example` to `.env`, set mode `0600`, and replace every `CHANGE_ME_*` value — this is the same template used for local development, so nothing distinguishes a production `.env` except the values you put in it. `POSTGRES_PASSWORD` is the password of the `chaos` login role. Both database URLs use that login; the application assumes the migration-created `chaos_runtime` and `chaos_control_plane` NOLOGIN roles after connecting.

Configure Cloudflare's public origin as the external `AUTH_PUBLIC_BASE_URL` and `WEBAUTHN_RP_ORIGIN`. Both use `https://` even though the Cloudflare-to-Caddy hop is HTTP.

## Provider secret encryption

Payment, shipping, and analytics Provider Key secrets are AES-256-GCM encrypted and stored directly in PostgreSQL as an opaque `enc://<base64>` reference. There is no separate secret-manager service to run. `CHAOS_PROVIDER_SECRET_KEY` in the host's `.env` is the only key material: exactly 32 raw bytes, base64-encoded (`openssl rand -base64 32`), read once at startup.

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

The image build is automated; the rollout is not. Pushing a `v*` tag runs the Release workflow, which builds and publishes `ghcr.io/OWNER/chaos:VERSION` (and `:latest`). Nothing in CI has network access to the deploy host — there is no SSH step, and no deploy-related secret (database URL, `CHAOS_PROVIDER_SECRET_KEY`, etc.) is ever held by GitHub Actions. Rolling the new image out to the host is a deliberate, manual action taken by an operator with host access.

To deploy, on the host itself (`/opt/chaos` by convention), pull the latest compose files and scripts, then run the rollout script:

```bash
git pull --ff-only
./scripts/deploy.sh
```

With no `CHAOS_IMAGE` set, this deploys `ghcr.io/omnip-org/chaos:latest` — whatever Release most recently published. The same command serves the first deploy and every subsequent one — there is no separate manual first-deploy procedure. `deploy.sh` creates the external volumes if absent; pulls the image (never builds locally — `docker-compose.yaml` has no `build:` stanza for the API image); starts PostgreSQL and Redis; applies migrations once; rolls blue and green independently; starts the gateway; and runs health probes. If a replica fails to become healthy the rollout aborts before touching the second replica, so the previous version keeps serving.

Rollback to a specific version:

```bash
CHAOS_IMAGE=ghcr.io/omnip-org/chaos:0.1.0 ./scripts/deploy.sh
```

Release tags every image with both `:VERSION` and `:latest`, and `deploy.sh` never removes other locally-cached same-repo tags (only dangling layers) — a pinned rollback is usually a re-pull of a tag Docker already has cached, not a cold fetch.

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

The host's `.env` (mode `0600`, git-ignored, never leaves the host) contains platform bootstrap configuration only: database access, token signing, email, media storage, and `CHAOS_PROVIDER_SECRET_KEY`. It is created once from `.env.example` during host bootstrap and edited by hand thereafter; nothing writes to it automatically. Store-specific Provider Key plaintext is never copied into `.env` or anywhere else — only the encryption key that seals it in PostgreSQL lives there.

Editing `.env` and re-running `./scripts/deploy.sh` performs a normal blue/green rollout, since both replicas are restarted with the new environment. Changing or adding an encrypted Provider secret through the Admin API does not require a deploy.
