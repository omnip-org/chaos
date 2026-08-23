# Production Deployment

## Topology

Cloudflare terminates public TLS and proxies to the host gateway. NGINX sends
traffic to one active API color while the other color is started and checked
for readiness. The gateway is reloaded to switch colors without restarting
NGINX, then the old color is gracefully stopped. One independently restartable
Worker service runs background consumers and has no public listener. Restrict
origin access to Cloudflare or use Cloudflare Tunnel. PostgreSQL and Redis
must not be publicly reachable.

API and Worker capacity are independent. API replicas never poll durable queues. The default Compose topology starts one Worker for cost efficiency; production may scale it to multiple replicas because PGMQ visibility timeouts, retry counters, and idempotent consumers coordinate concurrent claims.

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

Copy `.env.example` to `.env`, set mode `0600`, and replace every `CHANGE_ME_*` value — this is the same template used for local development, so nothing distinguishes a production `.env` except the values you put in it. `POSTGRES_PASSWORD` is the password of the `chaos` login role. Both database URLs use that login; the application assumes the migration-created `chaos_runtime` and `chaos_identity` NOLOGIN roles after connecting.

`DATABASE_IDENTITY_URL` may be omitted when Identity uses the same PostgreSQL login and endpoint as `DATABASE_URL`; keeping it explicit allows the Identity pool to move independently later.

The repository includes the local self-signed origin certificate used behind Cloudflare at `deploy/nginx/certs/omnip.org.crt` and `deploy/nginx/certs/omnip.org.key`. Replace the certificate paths and `server_name` in the NGINX configuration together if a different origin name is used.

Set `AUTH_JWT_ISSUER` to the public HTTPS API origin, use a deployment-specific `AUTH_JWT_AUDIENCE`, generate `AUTH_JWT_SECRET` with at least 32 random bytes, and configure at least one of `GOOGLE_CLIENT_ID` or `APPLE_CLIENT_ID`.

Set `MCP_ALLOWED_HOSTS` to the comma-separated public Host authorities accepted by the MCP endpoint. The MCP transport is stateless, so a color switch does not require sticky sessions.

## Provider secret encryption

Payment, shipping, and analytics Provider Key secrets are AES-256-GCM encrypted and stored directly in PostgreSQL as an opaque `enc://<base64>` reference. There is no separate secret-manager service to run. `CHAOS_PROVIDER_SECRET_KEY` in the host's `.env` is the only key material: exactly 32 raw bytes, base64-encoded (`openssl rand -base64 32`), read once at startup.

**Back this key up like you would the database itself.** There is no rotation or re-encryption tooling — losing the key makes every previously stored Provider Key permanently unrecoverable, and rotating it requires an owner to re-submit every Provider Key for every Store through MCP.

An owner uploads a Provider Key with the `create_provider_secret` MCP tool. The MCP connection uses the User's private Key and selects the target Store with `X-Chaos-Store-Id`.

The response contains a newly generated `enc://...` reference. The plaintext is not returned again or stored anywhere in plaintext. Use that reference in the existing Provider account create/update request.

Adding or changing an encrypted Provider Key takes effect on the next Provider operation. It does not require a deployment or restart. Submit a new value through MCP and update the Provider account reference when the 24-hour application-level overlap is required.

## Deployment

The pre-`1.0` bootstrap migrations describe the current clean database model
and may be rewritten while deployed data remains disposable. A release that
changes an already applied migration checksum must not run the normal rollout
against that database. Back up any required Provider configuration, recreate
the PostgreSQL data volume, and run the migration job before starting API or
Worker containers. Once production data becomes non-disposable, migrations are
immutable and every change must fix forward.

The image build is automated; the rollout is not. Pushing a `v*` tag runs the Release workflow, which builds and publishes `ghcr.io/OWNER/chaos:VERSION` (and `:latest`). Nothing in CI has network access to the deploy host — there is no SSH step, and no deploy-related secret (database URL, `CHAOS_PROVIDER_SECRET_KEY`, etc.) is ever held by GitHub Actions. Rolling the new image out to the host is a deliberate, manual action taken by an operator with host access.

To deploy, on the host itself (`/opt/chaos` by convention), pull the latest compose files and scripts, then run the rollout script:

```bash
git pull --ff-only
cd deploy
./deploy.sh
```

If the NGINX `server_name` is not `chaos.omnip.org`, pass the matching value as `ORIGIN_HOST` when invoking the script so its final public gateway health probe uses the correct host.

With no `CHAOS_IMAGE` set, this deploys `ghcr.io/omnip-org/chaos:latest` — whatever Release most recently published. Pin a version for reproducible releases and rollbacks. The same command serves the first deploy and every subsequent one — there is no separate manual first-deploy procedure. `deploy.sh` creates the external volumes if absent; pulls the image (never builds locally — `docker-compose.yaml` has no `build:` stanza for the API image); applies migrations; starts the inactive API color; waits for its container readiness check; writes the active upstream fragment inside the mounted `nginx/conf.d` directory; validates and reloads NGINX to switch traffic; stops the old color; starts the Worker with a process health check; and probes public `/health/ready`. If the inactive color fails, the active color is left serving. Deployment-local state is stored in the ignored `deploy/.active-api` file and ignored `deploy/nginx/conf.d/active-upstream.conf` fragment.

Rollback to a specific version:

```bash
cd deploy
CHAOS_IMAGE=ghcr.io/omnip-org/chaos:0.1.0 ./deploy.sh
```

Release tags every image with both `:VERSION` and `:latest`, and `deploy.sh` never removes other locally-cached same-repo tags (only dangling layers) — a pinned rollback is usually a re-pull of a tag Docker already has cached, not a cold fetch.

Verify the Cloudflare gateway and configured public API paths:

```bash
curl --fail https://chaos.omnip.org/health/live
curl --fail https://chaos.omnip.org/health/ready
```

## Identity and MCP bootstrap

1. Exchange a Google or Apple identity token at `POST /identity/v1/auth/external`; retain the returned User ID for explicit Store membership management.
2. Create a User-owned Access Key at `POST /identity/v1/access-keys` with the JWT. Preserve the plaintext returned once.
3. Configure the client with `Authorization: Bearer <access-key>` and `X-Chaos-Store-Id: <store-id>` when using Store-scoped operations.
4. Create or administer the Store through MCP tools. Membership is checked for every tool call.
5. Create only public Storefront Keys for storefront or Sales Channel clients. The returned plaintext has the form `public_<identifier>_<secret>` and must be treated as a client credential.
6. Upload third-party credentials and configure Providers through MCP tools.
7. Activate the Store and exercise a non-destructive quote or test transaction.

## Deployment secrets and rotation

The host's `.env` (mode `0600`, git-ignored, never leaves the host) contains platform bootstrap configuration only: database access, token signing, media storage, and `CHAOS_PROVIDER_SECRET_KEY`. Identity does not use email delivery, and Chaos currently exposes no Resend configuration or notification persistence. The file is created once from `.env.example` during host bootstrap and edited by hand thereafter; nothing writes to it automatically. Store-specific Provider Key plaintext is never copied into `.env` or anywhere else — only the encryption key that seals it in PostgreSQL lives there.

Editing `.env` and re-running `./deploy.sh` performs a normal blue/green API rollout and restarts the Worker with the same image. Changing or adding an encrypted Provider secret through MCP does not require a deploy.
