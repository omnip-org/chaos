# HTTP API Contract

## OpenAPI

The versioned Admin API source of truth is `openapi/admin-v1.json`. A running API instance serves the same embedded contract from `GET /openapi/admin-v1.json` with the `application/vnd.oai.openapi+json` media type. Publishing the embedded artifact guarantees that the contract belongs to the exact application build serving it.

Every operation has a stable and unique `operationId`. Additive fields remain optional. Removing or renaming a route, field, operation, error code, or enum value requires a compatibility plan or a new API version. Contract tests validate the OpenAPI version, operation identifiers, authentication declarations, and shared response envelopes.

Admin, Store, webhook, and future MCP surfaces have separate contracts and authentication boundaries. The Admin API uses human sessions. Store and MCP access never accepts a human session as a substitute for a scoped machine credential.

The versioned Store API source of truth is `openapi/store-v1.json`, served from `GET /openapi/store-v1.json`. Storefront operations are rooted at `/store/v1` and require a publishable bearer key with the declared public scope.

## Operational endpoints

`GET /health/live` reports process liveness and `GET /health/ready` verifies that the instance is accepting traffic and can reach PostgreSQL and Redis. `GET /metrics` exposes Prometheus text format directly from an API instance. It records request totals by method, matched route, and status, plus request latency histograms by method and matched route. Raw paths, query strings, tenant identifiers, credentials, and request bodies are never metric labels.

The Compose gateway does not proxy `/metrics`; collectors must scrape API instances over the internal service network. This keeps operational data outside the public HTTP surface while retaining a standard scrape contract.

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

`GET` and `PUT` on `/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}` read and replace Store configuration. Changing the default currency enables that currency for the Store atomically. `POST` actions beneath `/activate` and `/archive` perform explicit lifecycle transitions. Activation requires the default currency to be enabled and an active default Sales Channel. Only owners and administrators may change Store configuration or lifecycle.

Sales Channels are administered beneath `/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/sales-channels`. The collection supports cursor-paginated reads and idempotent creation; the item path supports detail and full update. Explicit `/activate` and `/archive` actions control lifecycle. A Store's automatically provisioned default Web channel cannot be archived, preserving a stable Storefront routing target. Owners and administrators may mutate Channels; all merchant members may read them.

Both list endpoints accept `limit` from 1 to 100, defaulting to 20, and an opaque `cursor`. Results use ascending UUIDv7 keyset pagination. When more results exist, `meta.page.has_more` is true and `meta.page.next_cursor` contains the cursor for the next request. Clients must not parse or construct cursors.

## Catalog

`POST /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/products` creates a draft Product aggregate atomically, including its Options, Option Values, Variants, and selected combinations. Owners, administrators, developers, and managers may author Catalog data; support members are read-only. The request requires `Idempotency-Key`, and its fingerprint includes the Store ID so a key cannot be replayed against another Store.

Draft Products may omit Options and Variants while content is being authored. Product creation never publishes implicitly. Prices, inventory quantities, and Sales Channel publication remain separate domain operations.

`GET /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/products` returns lightweight Product summaries using the same opaque UUIDv7 cursor contract as other collection endpoints. Summary rows include `variant_count` but do not expand Options or Variants.

`GET /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/products/{product_id}` returns the complete Product aggregate from one consistent database snapshot. Selected options include both stable identifiers and display values so Admin clients do not need to reconstruct combinations. A Product cannot be resolved through another Store's path.

`PUT /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/products/{product_id}` replaces Product content using the same handle, title, and description validation as creation. `POST` actions beneath `/activate` and `/archive` perform explicit lifecycle transitions. Activation requires at least one Variant.

`PUT` and `DELETE` on `/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/products/{product_id}/publications/{sales_channel_id}` publish and unpublish a Product. Publication requires both an active Product and an active Sales Channel in the same Store. Archiving makes a Product ineligible for Storefront serving without destroying its publication assignments, so deliberate reactivation can restore the prior channel topology.

Every Product write requires `Idempotency-Key`. Owners, administrators, developers, and managers may mutate Catalog data; support members remain read-only.

## Pricing

`POST /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/price-lists` creates a Price List and its Variant prices atomically. The request requires `Idempotency-Key`. A Price List fixes one enabled Store currency, tax-inclusive behavior, and an optional RFC 3339 activation window. Each Variant appears at most once in a list and every amount is a non-negative integer in minor currency units.

Draft lists may be empty. Creating an active list requires at least one Price and every referenced Variant must be active in the same Store. Owners, administrators, developers, and managers may create Price Lists; support members are read-only.

`GET /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/price-lists` returns cursor-paginated Price List summaries. `GET` on the corresponding `/{price_list_id}` path returns the complete list with its Variant prices. Price Lists are resolved through both merchant-account and Store boundaries.

`PUT` on `/{price_list_id}` atomically replaces the Price List configuration and all Variant prices while preserving its lifecycle status. Updating an active list revalidates that every replacement Price references an active Variant. `POST` actions beneath `/activate` and `/archive` perform explicit lifecycle transitions. Every Price List mutation requires `Idempotency-Key`; activation requires at least one Price and only active Variants in the same Store.

## Inventory

Inventory is location-aware. `GET` and `POST` on `/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/inventory-locations` list and create active locations. `GET /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/inventory-items` returns location-and-Variant-specific on-hand, reserved, and available quantities. Both collection endpoints use opaque cursor pagination.

`POST /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/inventory-adjustments` applies an idempotent, non-zero on-hand delta and requires a human-readable reason. The location must be active, the Variant must belong to the Store and track inventory, and an adjustment cannot reduce on-hand quantity below the currently reserved quantity. Owners, administrators, developers, and managers may mutate inventory; support members have read-only access.

Every balance mutation appends an immutable ledger entry in the same PostgreSQL transaction. Reservations lock stock items in stable identifier order, reject quantities above current availability, and record reserved deltas in that ledger. Release and expiration restore availability; consumption reduces both on-hand and reserved quantities. Expiration compares an explicit timestamp against `expires_at`, and concurrent workers claim due reservations with `FOR UPDATE SKIP LOCKED`.

## API keys

Store API keys are managed beneath `/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/api-keys`. Only owners, administrators, and developers may create, list, or revoke them. Every key is bound to the Store in its path and cannot authorize a different Store.

`POST` requires an `Idempotency-Key` and returns the plaintext `secret` exactly once. The stored record contains only a searchable random identifier, SHA-256 digest, and four-character display suffix. Replaying the same creation request returns HTTP 409 with `api_key_secret_already_issued`; it never returns the secret again. Losing the response requires creating a replacement key and revoking the inaccessible key.

Keys have an explicit `test` or `live` mode and are either `publishable` or `secret`. Publishable keys may contain only Storefront scopes: `catalog:read`, `carts:write`, and `checkout:write`. Secret keys can receive server-side scopes such as `orders:read` and `mcp:tools`. The exact scope allowlist is part of the versioned API contract.

`GET` returns metadata for active and revoked keys but never secret material or digests. `DELETE` requires an `Idempotency-Key`, records the revoking user, and is safely replayable. Revocation is authoritative in PostgreSQL and immediately causes machine authentication to fail.

## Storefront Catalog

`GET /store/v1/products` and `GET /store/v1/products/{handle}` authenticate a publishable API key with `catalog:read`. The credential resolves the merchant account, Store, Sales Channel, mode, and scopes; these identifiers are never accepted from path or query input.

The optional `currency` query parameter selects an enabled Store currency and defaults to the Store default currency. Pricing uses one currently active Price List selected deterministically for that currency. Results include only active Stores, Sales Channels, Products, Variants, publications, enabled currencies, active Price Lists, and explicit Variant Prices. Products without at least one currently priced active Variant are omitted and are indistinguishable from unavailable Products on the detail endpoint.

Storefront responses deliberately omit lifecycle status, drafts, archived records, unpublished Products, inventory cost, API key metadata, secret material, and merchant-account identifiers. Collection pagination uses the same opaque cursor behavior as the Admin API.

## Storefront Carts and Checkout

`POST /store/v1/shopper-sessions` creates an anonymous signed possession credential for a publishable key with `carts:write`. Clients persist `data.shopper_token` outside URLs and send it in `x-chaos-shopper-token` when creating or accessing a Cart and its descendant Checkout, Order, and Payment Attempt resources. Cart creation echoes the same credential. The publishable key continues to resolve the Store and Sales Channel; the shopper credential proves possession of the resource lineage. A missing or invalid credential is unauthorized, while a valid credential for another lineage receives the same not-found response as an unknown resource. Active and previous HMAC keys support overlap during signing-key rotation.

Authenticated Customer endpoints use the Store publishable key together with a verified User session in `x-chaos-customer-session`. `POST /store/v1/customer/associate` also requires the shopper token and idempotently links that anonymous lineage to the Store-owned Customer. `GET` and `PUT /store/v1/customer` read the profile and update its optional E.164 phone. `POST /store/v1/customer/addresses` and `DELETE /store/v1/customer/addresses/{address_id}` manage reusable saved addresses. Saved-address changes never mutate Checkout or Order snapshots.

`GET /store/v1/customer/orders` requires `checkout:write`, the Customer session, and no shopper token. It returns opaque-cursor Order history for every shopper lineage linked to that Customer in the current Store and Sales Channel, including guest Orders created before association. The association is additive and immutable; possession-bound guest access remains valid. See ADR 0011.

`POST /store/v1/carts` creates an active Cart for the publishable key's Store and Sales Channel. The optional currency must be enabled for that Store and defaults to its default currency. Cart creation selects one currently active Price List deterministically. `GET /store/v1/carts/{cart_id}` reads the current Cart, while `PUT` and `DELETE` on `/store/v1/carts/{cart_id}/lines/{product_variant_id}` add, replace, or remove a line. Cart operations require `carts:write`; mutations also require `Idempotency-Key`.

A Cart line can reference only an active Variant of an active Product published to the credential's active Sales Channel with a price in the Cart's Price List. Each line records customer-facing Product and Variant text, shipping and inventory behavior, quantity, unit price, tax inclusion, and subtotal. Mutations serialize through a Cart row lock, increment its version, and reject terminal Carts.

`POST /store/v1/carts/{cart_id}/shipping-options` requires `checkout:write` and a possession-bound shopper token. It returns active Store Shipping Services that match the Cart settlement currency and destination country. Admin operators configure these services under `/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/shipping-services`; create and lifecycle mutations are idempotent.

Admin operators configure one active Tax Rule per Store and destination country under `/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/tax-rules`. Rates are integer basis points from 0 through 10000. Country codes are canonicalized to uppercase ISO 3166-1 alpha-2 values. A zero-rate rule is explicit configuration; a missing active rule prevents Checkout creation.

Admin operators configure Store Promotions under `/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/promotions`. Promotions are either automatic or code-triggered and use a percentage or fixed amount in one settlement currency. They may define a minimum subtotal, a percentage cap, priority, and a half-open activation window. Redemption codes are canonicalized to uppercase. Create, list, activate, and archive operations are idempotent; rule handles are unique within a Store, and only one active rule may own a redemption code. Archiving an old rule before creating its replacement preserves history while allowing code reuse.

`POST /store/v1/carts/{cart_id}/checkout` requires `checkout:write`, `Idempotency-Key`, a validated guest contact, and a billing address. A shipping address and `shipping_service_id` are required when any Cart line requires shipping, and both are forbidden as a selection when no line requires shipping. An optional `promotion_code` requests a code-triggered Promotion; an invalid or ineligible submitted code rejects the operation. Email is canonicalized, optional phone numbers use E.164, country codes use ISO 3166-1 alpha-2, and address text is bounded. The operation locks the Cart, revalidates every publication, price, eligible Promotion, active destination Tax Rule, and selected Shipping Service, refreshes commercial snapshots, chooses one best discount, allocates it before calculating tax, locks tracked stock in stable order, and creates the Checkout, immutable contact/address/promotion/tax/shipping/line snapshots, inventory reservation, ledger entries, and Cart completion in one PostgreSQL transaction. Automatic and submitted-code Promotions compete by greatest discount, then lowest priority and stable ID. Tax-inclusive prices expose the extracted tax component without adding it again; tax-exclusive prices add tax to the total. The client never supplies authoritative discount, tax, shipping amount, or totals. A Checkout expires after 15 minutes. Digital or otherwise untracked Carts do not create an inventory reservation. Full recalculation and immutability rules are recorded in ADR 0010.

`GET /store/v1/checkouts/{checkout_id}` returns the frozen Checkout calculation and its possession-protected guest contact and address snapshots. Checkout data includes subtotal, discount, tax, total, currency, expiry, and line snapshots, but never accepts or returns merchant-account or Store identifiers. Idempotent retries return the original response snapshot even after later state changes; reusing a key with a different request is a conflict.

## Orders

`POST /store/v1/checkouts/{checkout_id}/order` requires `checkout:write` and `Idempotency-Key`. It locks a pending, unexpired Checkout, copies its header, guest contact, addresses, and every line into immutable Order snapshots, records the initial `created` transition, and completes the Checkout in one transaction. A Checkout can produce at most one Order. `GET /store/v1/orders/{order_id}` returns contact and address data only to the possession-bound shopper credential in the owning Store and Sales Channel.

Merchant operators list Orders at `/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/orders` with opaque cursor pagination and optional `status`, `customer_id`, and canonical `email` filters, and read one Order at the corresponding `/{order_id}` path. Owners, administrators, and managers may call the idempotent `/confirm` and `/cancel` actions. Confirmation moves only a pending Order to `confirmed`, consumes its active inventory reservation, and records `reservation_consumed` ledger entries. Cancellation moves only a pending Order to `cancelled`, releases the reservation, and records release entries. Both actions append an immutable Order transition carrying the acting user and timestamp; terminal Orders reject further transitions.

## Payments and Refunds

`POST /store/v1/orders/{order_id}/payment-attempts` requires `checkout:write` and `Idempotency-Key`. It creates one active Payment Attempt for a pending Order and writes a `payment.create_requested` outbox event in the same transaction. The Attempt copies the Order's exact amount and settlement currency. `GET /store/v1/payment-attempts/{payment_attempt_id}` is restricted to the publishable key's Store and Sales Channel. Once provider dispatch has assigned an immutable reference, `GET /store/v1/payment-attempts/{payment_attempt_id}/client-action` retrieves provider-neutral handoff material for the same possession-bound shopper. Client tokens are fetched from the provider on demand, never persisted, and must not be logged, cached, or placed in URLs.

Payment Providers are administered under `/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/payment-provider-accounts`. Create and update requests explicitly select whether the account should be enabled. Disabled configuration is stored with `readiness_status=unchecked`. An enable request invokes the provider's live readiness check; a successful assessment enables the account and records `ready`, while an unsuccessful assessment records `action_required` plus stable blocker codes and keeps the account disabled. Stripe checks charge and payout permissions, submitted details, active card payments, outstanding requirements, fee payer, and loss liability against ADR 0013. List and detail responses expose provider identity, display name, external account reference, effective enabled state, whether both credential references are configured, readiness status and check time, blocker codes, and rotation deadlines.

Owners and administrators may create or update an account; provider identity and external account mapping are immutable after creation. Credential and webhook-secret inputs are opaque secret-manager references, are write-only, and are never returned. A changed outbound credential is activated immediately with its previous reference retained for a 24-hour rollback window. Webhook verification accepts the active and immediately previous signing secrets for 24 hours, active first; repeating an update with unchanged references does not extend the window. One provider configuration is allowed per Store. Disabling it immediately prevents new Payment Attempts, while existing dispatches, client actions, refunds, and authenticated webhooks continue so in-flight payment history can converge. See ADR 0012.

Ready assessments expose a `readiness_valid_until` deadline. The payment worker reconciles enabled accounts every six hours through recoverable leases and capped exponential dependency retry. An action-required result disables the account immediately. An assessment that cannot be refreshed within 24 hours expires closed with `readiness_expired`; Payment Attempt creation independently requires a currently valid ready assessment.

Payment provider adapters implement an application port; provider-specific request and response types remain in infrastructure. The built-in `testpay` sandbox adapter is deterministic and intended for development and integration testing. The Stripe adapter creates PaymentIntents and Refunds as Connect direct charges, pins Stripe API version `2026-02-25.clover`, scopes every call with `Stripe-Account`, and sends the outbox event ID as `Idempotency-Key`. A credential reference resolves to a secret JSON object containing `secret_key` and `publishable_key`; the environment resolver accepts only `env://CHAOS_PAYMENT_SECRET_*` references. Resolved secrets never enter events or persistence. Workers claim outbox rows through a security-definer PostgreSQL function using `FOR UPDATE SKIP LOCKED`, so multiple instances never lease the same delivery concurrently. A processing lease may be reclaimed after one minute; the provider command retains its stable idempotency key so replay converges after process termination. The returned provider reference is persisted before the outbox row is completed. Failed work receives capped exponential backoff and moves to `dead_letter` after eight attempts. Shutdown stops new claims and waits a bounded interval for the current batch before aborting it.

`POST /webhooks/v1/payments/{provider}` uses provider-specific authentication. `testpay` requires `x-payment-signature`, a base64 HMAC-SHA256 over the exact request body using `PAYMENT_WEBHOOK_SECRET`. Stripe requires its timestamped `Stripe-Signature`; the adapter verifies every `v1` candidate over the untouched body and rejects timestamps outside five minutes. Stripe's untrusted top-level Connect account identifier is used only to select the active and, during its bounded rotation window, previous webhook-secret references through a narrow security-definer lookup. Merchant and Store context is resolved only after verification. Verified events are durably inserted into the inbox with a unique `(provider, provider_event_id)` key; duplicates return HTTP 202 with `accepted: false`. Inbox workers use the same lease, retry, and dead-letter mechanics as outbox workers.

`POST /webhooks/v1/notifications/resend` requires `svix-id`, `svix-timestamp`, and `svix-signature`. The adapter authenticates the exact raw body before parsing, rejects timestamps outside five minutes, and caps the body at 64 KiB. A verified event is mapped to a previously sent delivery by its Resend email identifier and durably deduplicated. Delivery, hard-bounce, complaint, and provider-suppression events update only notification-owned state. Hard bounces, complaints, and provider suppressions create a Store-isolated recipient suppression and never reverse an Order, Payment, Refund, Fulfillment, or Return.

Payment Attempts transition from `pending` to `authorized`, then to `captured`; failure and cancellation are terminal. An automatic-capture provider event performs the authorization and capture transitions atomically when no separate authorization event exists. Duplicate state events are no-ops, provider references become immutable when first assigned, and conflicting references are rejected. A capture confirms the Order and consumes its inventory reservation in the same transaction. `POST /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/payment-attempts/{payment_attempt_id}/refunds` creates a currency-safe Refund against a captured Attempt. Pending and successful refunds together cannot exceed the captured amount. Refund completion is driven only by a verified provider event.

The webhook contract is versioned separately in `openapi/webhooks-v1.json` and served from `GET /openapi/webhooks-v1.json`.

## Fulfillment, Returns, and Search

Admin operators create partial Fulfillments under a confirmed Order, then use the `ship`, `deliver`, or `cancel` operation. Shipping requires a carrier and tracking number. A recoverable fulfillment worker consumes these transactional events and recalculates the Order's `fulfillment_status` and `delivery_status` from authoritative Fulfillment rows. Duplicate, stale-lease, and out-of-order delivery converges without duplicating transition history.

Owners and administrators configure one external Shipping Provider Account per Provider and Store under `/admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/shipping-provider-accounts`. Create and update operations require idempotency keys. The configuration owns the default origin address and an opaque credential reference; responses expose only `credentials_configured`. Replacing the reference atomically retains the prior reference for a 24-hour rollback window without returning either reference through list or detail APIs. An enabled account must name a Provider adapter available in the deployment. `EASYPOST_API_BASE_URL` defaults to EasyPost's HTTPS API; credential references resolve only from constrained `env://CHAOS_SHIPPING_SECRET_*` names.

Delivered quantities bound Return requests. Refund amounts are allocated from immutable Order-line totals using minor-unit arithmetic; the last accepted quantity receives any rounding remainder. Returns proceed through authorization, receipt with a per-line `restock` or `discard` disposition, and completion. Completion coordinates exactly one Refund against a captured Payment Attempt and emits `refund.create_requested` in the same transaction. `GET /admin/v1/merchant-accounts/{merchant_account_id}/stores/{store_id}/returns/{return_id}` exposes the planned amount and eventual `refund_id`. Every mutation requires an idempotency key.

Storefront product listing accepts `q` for Store-isolated full-text search. Catalog writes refresh the rebuildable search read model and publish duplicate-tolerant change events in the same transaction.
