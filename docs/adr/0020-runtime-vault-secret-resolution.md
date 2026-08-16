# ADR 0020: Resolve Store Provider Secrets from Vault at Runtime

- Status: Accepted
- Date: 2026-08-16

## Context

Payment, shipping, and analytics Provider accounts are created after the platform is deployed. The initial environment-variable adapter requires every new secret value to be added to both API containers, which forces a rolling restart and makes Store onboarding depend on a platform release operation.

Provider credentials must remain outside PostgreSQL. The existing Provider administration contracts accept only opaque references and intentionally never return raw credentials.

Cloudflare terminates public TLS and proxies requests to the deployment gateway. Cloudflare Secrets Store currently exposes secret values to supported Cloudflare service bindings, rather than as a general-purpose value-reading API for an origin application, so it is not the runtime store for the Compose-hosted Rust process.

## Decision

The infrastructure layer supports two reference schemes behind the existing capability-specific secret resolver ports:

- `env://CHAOS_*_SECRET_*` preserves bootstrap and shared-platform compatibility;
- `vault://<kv-v2-mount>/<path>` resolves the current `value` field from HashiCorp Vault KV v2 for every Provider operation.

Vault connectivity uses `VAULT_ADDR`, a narrowly scoped `VAULT_TOKEN`, and an optional `VAULT_NAMESPACE`. Vault must use HTTPS except for loopback tests. References allow only bounded ASCII path segments and reject empty segments and traversal. Responses, errors, logs, persistence, and telemetry never contain the Vault token or resolved value. Only the secret-creation response exposes its newly generated reference; existing Provider read responses continue to omit references.

The token policy grants create, update, and read access only to the Chaos Store Provider-secret prefix. An owner/administrator-only Admin endpoint may receive one bounded secret over authenticated HTTPS, write it directly to a new Vault path with KV v2 check-and-set zero, and return the generated reference once with `Cache-Control: no-store`. The plaintext exists only in request/process memory and Vault; it is never written to PostgreSQL, idempotency snapshots, logs, events, metrics, or responses. Provider administration continues to persist only write-only references and rotation deadlines. This narrow upload operation supersedes ADR 0012 only where that ADR said raw credentials are never accepted by any Admin API.

The resolver does not cache values. A newly created or updated Vault value is therefore available to both API replicas on their next Provider operation without a process restart. Existing 24-hour reference-overlap rules remain unchanged.

## Consequences

- New Stores and Provider accounts can be onboarded without changing application environment variables or restarting API replicas.
- Admin clients do not require direct Vault credentials to create Store Provider secrets.
- Vault availability becomes a runtime dependency for Provider operations that use `vault://`; unrelated commerce operations remain available.
- One platform bootstrap credential is still required to authenticate Chaos to Vault.
- Environment references continue to require a rolling restart when their values change.
- Vault policy, audit logging, high availability, backup, unsealing, and token renewal are deployment responsibilities.
