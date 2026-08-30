# Architecture

## Product boundary

Chaos is a commerce engine operated by Users and Stores:

```text
User ── Store Membership ── Store ── Sales Channel
  │                              │
  │                              ├── Catalog and variants
  │                              ├── Channel publication
  │                              ├── Orders and fulfillment
  │                              ├── Payments and refunds
  │                              └── Publishable channel keys
  │
  └── OAuth 2.1 + PKCE ── MCP tools ── Store membership authorization
```

A User may create and leave Stores, while Store Owners explicitly add Users and manage their roles. A Store is the tenant, authorization boundary, and commerce-data isolation boundary. There is no merchant-account layer. A Sales Channel controls where Store products are published; it is not an ownership boundary.

Human Users authenticate through the MCP OAuth 2.1 authorization-code flow with an external identity provider. Google and Apple are the initial providers behind one application port. The provider subject, not the email address, is the durable external identity.

MCP clients authenticate through OAuth 2.1 authorization-code flow with PKCE. An
OAuth access token identifies one User for the MCP resource, while every
Store-scoped request selects a Store explicitly and rechecks the User's current
Store membership before invoking a commerce use case. For MCP, the Store UUID
is a required `store_id` field in the tool input, while `Authorization` is the
only required HTTP header; Store scope is not carried in a custom header.

The MCP HTTP transport is stateless. Protocol context and authentication are supplied on each request, allowing any API replica to handle it without local session affinity.

Stores issue public Storefront Keys for Sales Channels. Each key is bound to one
active Sales Channel at creation; it cannot invoke trusted-client or
administration use cases.

Commerce administration is exposed through MCP rather than an Admin HTTP API. HTTP remains for the MCP OAuth protocol, storefront and channel traffic, provider webhooks, and health checks.

## Code structure

Chaos remains a modular monolith with inward dependencies:

```text
chaos-api -------------> chaos-core -> chaos-domain
chaos-worker ----------> chaos-core -> chaos-domain
```

- `chaos-domain` contains business types and rules without HTTP, SQL, cache, or serialization dependencies.
- `chaos-core` contains the use cases, PostgreSQL repositories, runtime code, security, storage, and Provider adapters for each bounded context.
- `chaos-api` owns HTTP, MCP, DTOs, extractors, routing, and API dependency composition.
- `chaos-worker` owns Worker dependency composition and durable background consumers independently of API replicas.

External boundaries that genuinely need replacement live in `chaos-core::contracts`; their concrete adapters live beside the use cases under `integrations`, `repositories`, `security`, and `storage`. The API and
Worker compose separate runtime dependency sets; starting a Worker does not
construct HTTP routes, MCP state, OIDC verification, or OAuth services.

Bounded contexts may depend on another context only through a small core-level interface when there is a real external or test seam. HTTP and MCP handlers do not execute SQL. Database records do not become domain entities.

## Public contract boundary

`packages/js` is the source of truth for the public channel HTTP contract.
It is the only place where storefront request paths, wire DTOs, response
envelopes, and browser/server checkout bridges are defined. A consuming
storefront must use those exported SDK resources and helpers rather than
recreating a transport client, copying DTOs, or calling a Chaos Storefront path
directly for a capability already exposed by the SDK.

The boundary has two independent safeguards:

- TypeScript annotations such as `request<T>()` describe expected data but do
  not validate JSON at runtime. SDK resource methods must validate response
  envelopes before reading fields that control commerce or payment behavior.
- Production integration code must not use `as any`, `as unknown as`, or an
  equivalent cast to force an external response into a local contract. A cast
  is acceptable only in an isolated test double or infrastructure adapter with
  an explicit reason; the actual JSON shape must still be covered by a test.

A wire-contract change is a vertical change. Update the API DTO/handler, the
`chaos-js` type and runtime validator, the SDK tests/fixtures, and every
consumer's locked dependency in the same release sequence. The consumer must
be checked against the published SDK version that the deployment will run;
building against a locally available or semver-compatible package is not proof
that the deployed API and storefront agree.

## Identity

Identity owns:

- Users;
- external Provider identities;
- external identity verification;
- MCP OAuth client registration, authorization-code flow, and token rotation.

MCP OAuth authorization-code transactions, PKCE challenges, short-lived access
tokens, and rotated refresh tokens are also owned by the identity database. The
MCP resource server accepts OAuth access tokens scoped to its protected
resource.

The database stores no passwords, magic links, passkeys, or human sessions. MCP OAuth access and refresh tokens are stored as digests and bound to the MCP resource. Provider ID tokens are accepted only after signature, algorithm, issuer, audience, expiry, subject, and verified-email validation against cached Provider JWKS.

Identity uses a dedicated non-owner database role because sign-in and OAuth
authentication occur before any Store context exists. That role can access only
the `identity` schema.

Automatic account linking by email is not supported. A different Provider presenting an email already assigned to a User receives a conflict. Explicit Provider linking can be added later as an authenticated use case.

Identity does not send authentication email. A verified Provider email is profile and
account-conflict data only; Google or Apple remains responsible for authenticating the
address.

## Request authorization

Every Store-owned table includes `store_id`. Every Store transaction sets transaction-local `app.store_id`; User directory reads additionally set `app.user_id`. PostgreSQL RLS is defense in depth, and the runtime role neither owns tables nor bypasses RLS.

Credential resolution is intentionally asymmetric:

- an MCP OAuth access token yields `user_id`, followed by a fresh Store membership check;
- a Publishable Store Key yields `store_id` and its bound `channel_id`;
- a webhook yields `store_id` only after signature verification and Provider mapping;
- a Worker carries `store_id` in its durable job and establishes a fresh transaction context.

The MCP operation chain is `request_id -> oauth access token -> user_id -> store_id -> use case`. OAuth access tokens are short-lived, audience-bound to the MCP resource, and refresh-token rotation is enforced. Tokens do not contain a cached membership or role.

## Commerce reliability

PostgreSQL is the source of truth for catalogs, inventory, orders, payments, refunds, fulfillment, and durable jobs. Redis is limited to rate limiting and disposable coordination; losing Redis must not violate commerce invariants.

Money uses integer minor units plus an ISO currency. Orders snapshot the product, price, address, and Provider evidence required to preserve history. Stripe owns checkout tax and promotion calculation; its verified webhook writes subtotal, discount, tax, shipping, and total as Order facts. External Provider calls occur outside database transactions. Inbox and outbox records make webhook and Worker processing retryable and idempotent.

Orders use an internal UUID for joins and a client-supplied idempotency key for checkout
deduplication on the source Cart, plus a random shopper-facing
`W-YYYYMMDD-XXXXXXXX` order number for receipts, support, and MCP lookup. Guest order
tracking uses the Order's Sales Channel storefront origin with a fragment capability,
valid for 180 days from order confirmation. Only its digest is stored in the tracking
table; the plaintext is carried by the durable `order.confirmed` outbox job until the
confirmation email reaches its terminal state, and then is removed from the outbox
payload. The capability is presented directly on every tracking request rather than
exchanged for a separate session — the tracking response omits contact details and the
full postal address precisely because the link itself is treated as shareable.

The Storefront identity is a Store-scoped persisted `commerce.shoppers` row. A
website visit creates one Shopper through `/api/v1/shopper/sessions`, and the
API returns a signed possession token for that row. The Shopper does not own a
Sales Channel and does not hold contact information; channel is request context
for Shopper identity, while Carts, Orders, and Analytics events persist their
Channel binding and contact/address data is captured directly on the business
Order. Carts, Orders, Payments, and Analytics events carry the same
`shopper_id`; there is no Customer entity or visitor-to-Customer association
table. An Order-bearing Shopper is the buyer for all commerce and analytics
purposes.

Store-owned business state, including Orders, refunds, and payment/fulfillment
state transitions, remains in the `commerce` schema so Store-scoped foreign
keys and RLS stay simple. External Provider accounts, opaque credential
references, the canonical webhook inbox, and provider-independent queue
leasing live in the `integration` schema. The migrations
are organized into Store foundation, Catalog and Pricing, Integration core,
Provider accounts, Sales, Payments, Analytics, and Fulfillment. Checkout
request deduplication is owned by the Order idempotency key and the source Cart
in `commerce`. A unique `(store_id, cart_id)` constraint prevents one source
Cart from creating a second Order; the provider idempotency key is derived from
that Order ID and is not another database field.

Cart and Order have separate responsibilities. The Checkout API transaction
resolves and verifies the current cart amount, reserves tracked inventory,
creates a pending Order snapshot, and marks the source Cart `locked`. It does
not create a successor Cart inside the checkout transaction. The storefront
obtains or creates a new active Cart after the transaction. A successful
payment marks the source Cart `completed`; a failed, cancelled, or expired
payment marks it `abandoned`.
Stripe owns the checkout UI, address, shipping, tax, and payment collection;
Chaos stores only the provider-neutral `payment_client_action` needed to recover
the form and stores final provider facts on the Order after a verified webhook.

The browser may lose the response, unmount Stripe, or return from Stripe without
paying. All of those paths retry the same Cart checkout request with the same
Cart-derived idempotency key, so the pending Order and stored client action are
reused without a second Order or Provider Session. The server checkout bridge
uses the Cart cookie as the recovery key; it creates a replacement active Cart
only after the source Cart is no longer eligible for checkout. There is no
pending-order lookup or Order-ID recovery endpoint. A successful payment
confirms the Order, consumes the reservation, and clears the action; a provider
failure, cancellation, or expiry cancels the Order, releases the reservation,
and clears the action. `cart_lines` stores only Variant identity and quantity;
Order lines retain the immutable product and pricing snapshot.
There is no local checkout expiry job: the provider callback is the source of
truth. New products are added only to a separate active Cart and a later
checkout creates a new Order.

Analytics uses one append-only, Store- and Channel-scoped behavior event ledger.
The common envelope contains `store_id`, `channel_id`, `shopper_id`, `event_id`, `event_name`, normalized
`event_source`, time, nullable `session_id`, and normalized UTM columns;
event-specific values such as
product, cart, order, traffic, and money remain in bounded `properties` JSON.
`event_name` is validated only as a lowercase snake-case identifier, not as a
database enum, so new behaviors do not require a migration. Provider delivery
is an optional retryable projection of stored events through destination and
delivery records. See ADR 0026.

The `integration` schema owns `provider_accounts`, `provider_webhook_inbox`,
event routes, and event outbox routing. `provider_accounts` uses one row shape for
Email, Payment, and Shipping accounts; `capability` and `provider` select the
capability-specific adapter while `configuration` holds bounded provider
settings and the secret columns hold only opaque references. There is no
`commerce.provider_webhooks` table. Every verified provider webhook enters the
same `integration.provider_webhook_inbox`, whose unique
`(provider_account_id, provider_event_id)` key provides idempotency and whose
PGMQ envelope provides retryable delivery. The inbox stores both the raw
`provider_event_type` and an optional `normalized_event_type`; verified events
that are not understood by the running version finish as `unsupported` rather
than being rejected at ingress. The `commerce` schema owns refunds, orders,
and fulfillment state transitions, not provider transport records.

The application deliberately does not force Email, Payment, and Shipping into
one lowest-common-denominator provider interface. Each capability has its own
port (`EmailProvider`, `PaymentProvider`, and `ShippingProvider`), while the
inbox, provider-account lookup, queue leasing, retries, and secret resolution
are shared. This keeps provider wire formats out of order state and allows a
new adapter to be registered by provider name without changing the worker
loop. Business outbox routing is data-driven:
`event_routes.queue_name` points directly to the PGMQ queue, while each
capability worker owns only its payload semantics.

API replicas never start polling loops. `chaos-worker` is deployed and scaled independently. PGMQ owns durable message visibility, retry attempts, and concurrent claims; compact integration records retain the business payload and delivery outcome. Refund commands, Email `order.confirmed` notifications, Shipping `fulfillment.shipped` projections, Search events, and all provider webhook capabilities use the same leasing contract; browser checkout Session creation remains synchronous because it must return a client secret. Deployment may begin with one Worker replica for cost, but correctness does not depend on singleton execution. Adaptive polling backoff limits idle database work, while visibility timeouts, idempotent handlers, bounded retries, and bounded shutdown provide crash recovery. Scheduled reconciliation derived from authoritative rows continues to use short database leases because it is not an event queue. See ADR 0029.

## Incremental refactoring rule

Refactoring proceeds one bounded context at a time. A slice is complete only when its old code, configuration, schema objects, API contract, and stale documentation are removed and the required repository checks pass. Transitional naming in untouched contexts does not define the target model; this document does.
