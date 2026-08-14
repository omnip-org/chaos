# ADR 0004: Store-Scoped Machine Credentials and MCP

- Status: Accepted
- Date: 2026-08-14

## Context

Chaos Commerce exposes an Admin API to people and will expose Store APIs to storefronts, integrations, automation, and AI clients through the Model Context Protocol. Human sessions and machine credentials have different ownership, lifetime, rotation, audit, and least-privilege requirements. Treating both as interchangeable bearer tokens would allow credentials to cross trust boundaries and make revocation difficult to reason about.

## Decision

Human Admin API access uses passwordless user sessions. External Store API and MCP access uses Store-scoped API keys. A key belongs to exactly one merchant account and one Store. It may optionally belong to one sales channel when the channel aggregate is introduced.

There are two machine-credential classes:

- Publishable keys identify a Store or sales channel and authorize only explicitly public read operations. They are safe to embed in a browser but are still revocable and rate limited.
- Secret keys authorize server-side operations permitted by their scopes. They must never be embedded in browsers, URLs, logs, telemetry attributes, or MCP configuration committed to source control.

Keys use a self-identifying versioned format with an environment and class prefix, a searchable non-secret key identifier, and a random secret. The initial format is `cc_v1_live_secret_<key_id>_<secret>`. The exact encoder is owned by the credential infrastructure adapter and may evolve by version.

PostgreSQL stores the key identifier and a SHA-256 digest of the complete presented key, never the plaintext secret. A narrowly granted verifier function authenticates a key after indexed high-entropy identifier lookup without exposing stored digests to the runtime role. Plaintext is returned exactly once at creation. Losing it requires rotation, not recovery.

Each key records:

- `merchant_account_id`, `store_id`, and optional future `sales_channel_id`;
- key class, display name, environment, and an explicit set of scopes;
- `created_by_user_id`, creation time, optional expiration, last-used time, and revocation time;
- a short display suffix that is not usable for authentication.

Scopes use stable capability names such as `catalog:read`, `cart:write`, `orders:read`, and `mcp:tools`. A key must satisfy both its declared scopes and the authorization policy of the requested operation. Wildcard scopes are not issued by normal Admin API workflows. Secret-key creation and revocation require an owner, administrator, or developer membership and produce immutable audit events.

The Store API resolves `merchant_account_id` and `store_id` exclusively from the authenticated key. Client-supplied account or Store identifiers cannot broaden that context. Every database transaction establishes both RLS context values before accessing Store-owned data.

MCP is an adapter over application use cases, not a separate business-logic implementation. MCP tools expose curated, task-oriented capabilities with JSON Schema inputs and stable names. An MCP connection authenticates with the same scoped machine-credential verifier, but MCP access additionally requires `mcp:tools` and the scope required by each tool. Destructive or financially consequential tools require explicit confirmation semantics and idempotency keys. Tool results use structured application DTOs and do not expose internal SQL, provider payloads, or secrets.

API keys can authenticate the initial MCP HTTP connection. A future OAuth 2.1 authorization-code flow with PKCE may be added for third-party hosted AI clients that need delegated user consent. Adding OAuth does not change the Store-scoped authorization model; grants resolve to the same merchant account, Store, and scopes.

Rate limits are keyed by credential identifier and operation class. Revocation is authoritative in PostgreSQL. A short-lived Redis cache may reduce lookup load, but cache entries must expire quickly and revocation must invalidate them. Authentication failures do not reveal whether a key identifier exists.

## Consequences

- Admin sessions cannot call Store or MCP endpoints.
- One compromised key is contained to one Store and its explicit scopes.
- Rotation can overlap old and new keys without downtime, then revoke the old key.
- The database must retain revoked credential metadata for audit while permanently discarding plaintext secrets.
- Admin, Store, and MCP contracts evolve independently even when they invoke the same application use cases.
- MCP transport concerns stay outside domain and application layers.

## Rejected alternatives

### Reuse human sessions

Sessions represent a person and inherit membership changes. They are unsuitable for unattended clients, rotation, and Store-scoped least privilege.

### Store recoverable encrypted secrets

Recovery adds encryption-key custody and disclosure risk without operational value. One-time display plus rotation is simpler and safer.

### Encode authorization claims in self-contained JWT keys

Long-lived self-contained claims make immediate revocation and scope changes harder. Opaque keys backed by authoritative server-side state match the existing revocable-session design and provide straightforward auditability.
