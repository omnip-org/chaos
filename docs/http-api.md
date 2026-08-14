# HTTP API Contract

## OpenAPI

The versioned Admin API source of truth is `openapi/admin-v1.json`. A running API instance serves the same embedded contract from `GET /openapi/admin-v1.json` with the `application/vnd.oai.openapi+json` media type. Publishing the embedded artifact guarantees that the contract belongs to the exact application build serving it.

Every operation has a stable and unique `operationId`. Additive fields remain optional. Removing or renaming a route, field, operation, error code, or enum value requires a compatibility plan or a new API version. Contract tests validate the OpenAPI version, operation identifiers, authentication declarations, and shared response envelopes.

Admin, Store, webhook, and future MCP surfaces have separate contracts and authentication boundaries. The Admin API uses human sessions. Store and MCP access never accepts a human session as a substitute for a scoped machine credential.

Credential-issuing operations are a security-specific exception to response replay. They still require `Idempotency-Key`, but a repeated request never returns the plaintext secret again. If the original one-time response is lost, the client must create a replacement key and revoke the inaccessible key.

## Success responses

Objects and collections are wrapped in `data`. The optional `meta` object is emitted only when pagination or other metadata exists.

```json
{
  "data": {
    "id": "019c...",
    "name": "ACME"
  }
}
```

Cursor pagination uses the following shape:

```json
{
  "data": [],
  "meta": {
    "page": {
      "has_more": false,
      "next_cursor": null
    }
  }
}
```

Successful creation returns HTTP 201. A synchronous deletion with no response body returns HTTP 204. Never replace meaningful HTTP statuses with `HTTP 200 + a custom numeric status in the body`.

## Error responses

```json
{
  "error": {
    "code": "validation_failed",
    "message": "one or more fields are invalid",
    "details": [
      { "field": "slug", "reason": "must be lowercase" }
    ]
  }
}
```

- `code` is a stable machine-readable identifier. It must not be renamed after release without a compatibility plan.
- `message` is safe and human-readable but is not a stable client contract. Clients localize errors by `code`.
- `details` is included only when structured context exists. Empty arrays are omitted.
- Internal errors, SQL, stack traces, provider payloads, and secrets never appear in responses. They are recorded only in server-side telemetry.
- `x-request-id` correlates client reports, logs, and traces. The service generates or propagates it.
- Malformed JSON, unsupported content types, and oversized bodies use the same error envelope with `invalid_json`, `unsupported_media_type`, or `payload_too_large`.

## Error mapping

| Application error | HTTP | Default code |
|---|---:|---|
| Validation | 422 | `validation_failed` |
| Unauthorized | 401 | `unauthorized` |
| Forbidden | 403 | `forbidden` |
| NotFound | 404 | `not_found` |
| Conflict | 409 | A stable use-case-specific code |
| Unavailable | 503 | `service_unavailable` |
| Unexpected | 500 | `internal_error` |

The domain layer emits domain errors. The application layer translates them into use-case semantics. The HTTP layer is the only layer that chooses HTTP statuses and JSON representations.

## Passwordless authentication

Human accounts do not have passwords. Email links provide initial sign-in and recovery; passkeys provide phishing-resistant authentication after enrollment.

| Method | Path | Authentication | Purpose |
|---|---|---|---|
| `POST` | `/admin/v1/auth/email-links` | None | Send a single-use sign-in link |
| `POST` | `/admin/v1/auth/email-links/verify` | None | Consume a link token and create a session |
| `DELETE` | `/admin/v1/auth/session` | Bearer session | Revoke the current session |
| `POST` | `/admin/v1/auth/passkeys/registration/options` | Bearer session | Start passkey enrollment |
| `POST` | `/admin/v1/auth/passkeys/registration/verify` | Bearer session | Verify and persist a passkey |
| `POST` | `/admin/v1/auth/passkeys/authentication/options` | None | Start passkey authentication for an email |
| `POST` | `/admin/v1/auth/passkeys/authentication/verify` | None | Verify an assertion and create a session |

Requesting an email link always returns HTTP 202 after the delivery request is accepted. The emailed token expires after 15 minutes and is consumed exactly once. Session tokens expire after 30 days and are sent as `Authorization: Bearer <token>` when enrolling a passkey.

Email-link delivery is limited to three requests per normalized email address per 15-minute window. Requests beyond the limit still return HTTP 202 without sending another message, which avoids exposing rate-limit or account state. Passkey authentication options are limited to ten requests per normalized email address per five-minute window. Rate-limit counters live in Redis and are therefore shared by every API instance.

Registration and authentication option responses contain `ceremony_id` and `public_key`. The browser passes `public_key` to `navigator.credentials.create()` or `navigator.credentials.get()`, then submits the returned credential with the same ceremony ID. Ceremony state expires after five minutes and is atomically removed from Redis before verification.

An account may contain one or more passkeys. There is no minimum-two-passkey rule because email-link authentication remains available as the recovery path.

## Merchant accounts

`POST /admin/v1/merchant-accounts` creates a merchant account and its owner membership atomically. It requires a valid bearer session and an `Idempotency-Key` header containing 1-255 bytes. The new merchant account ID is generated before the transaction begins and becomes the transaction-local RLS context, so initial provisioning does not bypass tenant isolation.

Idempotency records are scoped to the authenticated user for account creation. Repeating the same key and request returns the original HTTP 201 response without creating another account. Reusing the key with a different request returns HTTP 409 with `idempotency_key_reused`. The request fingerprint, business writes, and response snapshot commit in one PostgreSQL transaction.

`GET /admin/v1/merchant-accounts` returns only merchant accounts where the authenticated user has a membership. Each item includes `id`, `slug`, `display_name`, `status`, and the caller's `role`.

## Stores

`POST /admin/v1/merchant-accounts/{merchant_account_id}/stores` creates a draft store and enables its default currency atomically. It requires a bearer session and an `Idempotency-Key` header. Only active merchant accounts and members with the `owner` or `administrator` role may create stores. A missing account and an unauthorized membership both return HTTP 403 to avoid account enumeration.

Store creation idempotency is scoped to the merchant account. The request body requires `code` and `name`; `default_region` and `default_currency` are optional and default to `US` and `USD`. Regions use uppercase ISO 3166-1 alpha-2 codes and currencies use uppercase three-letter ISO 4217 codes. Store codes are unique within a merchant account; a conflict returns `store_code_taken`.

The default region is the store's initial operating configuration. It does not restrict future markets, shipping zones, tax registrations, localized catalogs, or enabled currencies.

`GET /admin/v1/merchant-accounts/{merchant_account_id}/stores` returns stores only after resolving the authenticated user's merchant membership. Each item includes the Store identity, code, name, defaults, and status.

Both list endpoints accept `limit` from 1 to 100, defaulting to 20, and an opaque `cursor`. Results use ascending UUIDv7 keyset pagination. When more results exist, `meta.page.has_more` is true and `meta.page.next_cursor` contains the cursor for the next request. Clients must not parse or construct cursors.

## Catalog

`POST /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/products` creates a draft Product aggregate atomically, including its Options, Option Values, Variants, and selected combinations. Owners, administrators, developers, and managers may author Catalog data; support members are read-only. The request requires `Idempotency-Key`, and its fingerprint includes the Store ID so a key cannot be replayed against another Store.

Draft Products may omit Options and Variants while content is being authored. Product creation never publishes implicitly. Prices, inventory quantities, and Sales Channel publication remain separate domain operations.

`GET /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/products` returns lightweight Product summaries using the same opaque UUIDv7 cursor contract as other collection endpoints. Summary rows include `variant_count` but do not expand Options or Variants.

`GET /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/products/{product_id}` returns the complete Product aggregate from one consistent database snapshot. Selected options include both stable identifiers and display values so Admin clients do not need to reconstruct combinations. A Product cannot be resolved through another Store's path.

## Pricing

`POST /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/price-lists` creates a Price List and its Variant prices atomically. The request requires `Idempotency-Key`. A Price List fixes one enabled Store currency, tax-inclusive behavior, and an optional RFC 3339 activation window. Each Variant appears at most once in a list and every amount is a non-negative integer in minor currency units.

Draft lists may be empty. Creating an active list requires at least one Price and every referenced Variant must be active in the same Store. Owners, administrators, developers, and managers may create Price Lists; support members are read-only.

## API keys

Store API keys are managed beneath `/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/api-keys`. Only owners, administrators, and developers may create, list, or revoke them. Every key is bound to the Store in its path and cannot authorize a different Store.

`POST` requires an `Idempotency-Key` and returns the plaintext `secret` exactly once. The stored record contains only a searchable random identifier, SHA-256 digest, and four-character display suffix. Replaying the same creation request returns HTTP 409 with `api_key_secret_already_issued`; it never returns the secret again. Losing the response requires creating a replacement key and revoking the inaccessible key.

Keys have an explicit `test` or `live` mode and are either `publishable` or `secret`. Publishable keys may contain only public read scopes. Secret keys can receive server-side scopes such as `orders:read` and `mcp:tools`. The exact scope allowlist is part of the versioned API contract.

`GET` returns metadata for active and revoked keys but never secret material or digests. `DELETE` requires an `Idempotency-Key`, records the revoking user, and is safely replayable. Revocation is authoritative in PostgreSQL and immediately causes machine authentication to fail.
