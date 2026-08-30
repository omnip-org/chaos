# Database Conventions

## Schema ownership

PostgreSQL schemas represent data ownership, not individual users, Stores, Rust modules, or deployment units. Current business schemas are `identity`, `commerce`, and `integration`. Utility extension objects live in `extensions`; `public` contains no business tables.

`commerce` owns Stores, Store memberships, Sales Channels, public Storefront Keys, catalogs, pricing, inventory, sales, payment state, refunds, and fulfillment state. `integration` owns the shared Provider account registry, canonical verified webhook inbox, and generic event routing. There is no merchant-account schema or aggregate. A User-owned trusted-client credential is stored in `identity.access_keys`; a Storefront public key is stored as plaintext in `commerce.store_publishable_keys` because it is intentionally safe to embed in frontend code.

Do not create a schema merely because a Rust module exists. A new schema requires a distinct data owner, security boundary, or operational lifecycle. `commerce` contains Store-owned catalog, inventory, sales, payment state, fulfillment state, and rebuildable Storefront read models. `integration` contains external Provider account configuration, capability/provider identity, the webhook inbox, outbox/event routing, and analytical processing state.

Application SQL always schema-qualifies objects. Cross-schema foreign keys are allowed only for stable ownership references. Cross-context business behavior is coordinated by application use cases and durable events. Database triggers are limited to atomic integration capture and queue-enqueue mechanics; they must not implement business workflows.

Durable integration work uses PGMQ for message visibility and retry attempts. An
authoritative integration row stores the bounded business payload and outcome;
its insertion trigger enqueues only the row identifier and records the returned
PGMQ message identifier. Runtime code accesses queues through schema-qualified,
security-definer routines rather than receiving direct access to extension-owned
tables. Scheduled scans derived from current business state are not queues and
may use short, recoverable row leases.

The Worker runs bounded retention for expired OAuth requests, codes, bearer
tokens, refresh tokens, Order tracking capabilities, and terminal integration
outbox/webhook rows. Pending media uploads are not deleted by database
maintenance because their corresponding object-store object must be removed
through the storage provider first.

## Names and identifiers

- Use lowercase `snake_case` ASCII identifiers and plural table names.
- Use `id` for a primary key and `<entity>_id` for a foreign key.
- Use UUIDv7 values for application-created primary and foreign IDs.
- Use domain language rather than abbreviations or vague names.
- Keep identifiers below PostgreSQL's 63-byte limit.

Constraints follow `<table>_<columns-or-rule>_<kind>`, where kind is `pkey`, `key`, `fkey`, `check`, or `excl`. Index names end in `_idx` and describe the supported query path.

## Columns

- Use `timestamptz` for instants, stored in UTC, with names ending in `_at`.
- Use `date` for calendar dates and `_on` when it improves meaning.
- Name booleans as positive states such as `enabled`.
- Mutable tables normally have `created_at` and `updated_at`.
- Soft deletion is not a default.
- Use typed columns for searchable fields. JSONB is reserved for bounded provider snapshots, versioned opaque security records, and genuinely schemaless metadata.
- Country codes use uppercase ISO 3166-1 alpha-2 `char(2)` values.
- Currency codes use uppercase ISO 4217 `char(3)` values.

Money uses integer minor units and a currency. Never use floating point for money, rates, tax, or exact quantities. Negative values require an explicit domain reason and constraint.

Persisted instants use PostgreSQL `timestamptz` and Rust
`time::OffsetDateTime`. PostgreSQL generates persistence metadata such as
`created_at`; business decision times enter domain and application operations
explicitly through the application `Clock` port. Truncate production instants
to PostgreSQL microsecond precision, serialize public timestamps with the
shared RFC 3339 HTTP type, and use `std::time::Instant` for elapsed process
time. Local system time must not directly drive persistence, comparisons,
retries, expiry, or signatures. Introduce calendar or named-time-zone types
only for explicit Store-local rules.

## Identity

`identity.users` owns the internal user identifier and current verified email. `identity.credentials` maps a provider subject to a User. Provider subjects are opaque, case-sensitive strings and form a composite key with provider.

Identity access uses the non-owner `chaos_identity` role. It can access only the `identity` schema. It must not gain access to Store-owned tables merely to simplify a query.

## Store isolation

- Every Store-owned row contains non-null `store_id`.
- Store-owned relationships use Store-scoped composite foreign keys when they prevent cross-Store references.
- Store query indexes normally begin with `store_id`.
- Every Store transaction calls `set_config('app.store_id', ..., true)` before accessing Store data.
- Human directory reads set transaction-local `app.user_id` and expose only Stores with a matching membership.
- RLS policies use these transaction-local values.
- Runtime connections use a non-owner role without `BYPASSRLS`.
- Publishable Store Key authentication is exposed through a narrowly granted verifier function; pre-authentication connections never receive direct table access.
- Cross-Store isolation and Store-scoped authorization tests are required for each Store-owned aggregate.

## Migrations

Migration files use zero-padded sequence numbers and concise English names. Before `1.0`, bootstrap migrations may be rewritten only when every environment using them contains disposable data and the release includes a coordinated database recreation or migration-history reset. Otherwise, applied migrations are immutable and changes fix forward.

The bootstrap uses `0001_platform.sql`, `0002_identity.sql`,
`0003_commerce.sql`, `0004_integration.sql`,
`0005_commerce_products.sql`, `0006_commerce_orders.sql`, and
`0007_integration_analytics.sql`, followed by `0008_identity_oauth.sql`,
`0009_checkout_lifecycle.sql`, `0010_checkout_attempts.sql`,
`0011_order_centric_checkout.sql`, and `0012_publishable_key_channel_binding.sql`.
The last migration binds legacy Publishable Keys to each Store's active default
Sales Channel, makes the binding required, and contracts authentication to the
bound active channel. Migration `0011` contracts the temporary
Checkout Attempt implementation into an Order-centric checkout: Cart status is
`active | locked | abandoned`, and the private payment-form handoff lives in
`commerce.carts.payment_client_action`.
Catalog media attachments and manual-review provenance are part of
`0005_commerce_products.sql`. Release-hardening constraints, capability checks,
and cleanup routines are defined in the migration that creates each dependent
object. `0004_integration.sql` creates the shared
`integration.provider_accounts` and `integration.provider_webhook_inbox` structures;
Commerce references those account IDs while retaining payment and fulfillment
state transitions. Within each file, define objects in dependency order:
types, tables, indexes, routines, triggers, row-level security, policies, and
grants.

Production startup never runs migrations. Releases use a separate migration job and expand/migrate/contract changes when adjacent application versions may overlap. Destructive operations, table rewrites, large backfills, and blocking indexes require an explicit rollout plan.

Migration `0011_order_centric_checkout.sql` is the contract step for the
temporary Checkout Attempt schema: it drops the old table and status types.
The deployment must drain the old API before applying it, then start the
Order-centric API; the old and new checkout implementations must not serve
traffic against the same database concurrently.

Migration `0012_publishable_key_channel_binding.sql` is a fix-forward data
contract. It must complete the legacy NULL backfill before setting
`commerce.store_publishable_keys.sales_channel_id` to `NOT NULL`; a key without
an active default channel intentionally blocks the migration for operator repair.

## SQL style

- Use uppercase SQL keywords and built-in data types.
- List columns explicitly; never use `SELECT *` in application SQL.
- Bind every value. Dynamic identifiers require strict allowlisting.
- Evaluate an index for every foreign-key access path; PostgreSQL does not create one automatically.
- Keep transactions short and never perform network calls while holding database locks.
- Add comments only for non-obvious invariants or operational constraints.

### Named SQL row contracts

`sqlx::query_as` with a named `FromRow` struct maps result columns by their
runtime names. Rust field names do not repair a missing or changed SQL column,
and expressions can receive a database-generated name that is not the Rust
field name. Therefore every named-row projection must give every expression,
cast, `CASE`, `COALESCE`, `NULLIF`, and joined/qualified value an explicit
`AS <rust_field_name>` alias. Do not rely on positional order or an inferred
expression label for a named row.

Prefer SQLx compile-time query macros when the repository's build has the
migrated schema or offline metadata available. When runtime `query_as` is
required, the owning repository and its integration test must be updated in
the same change. A migration or query projection is not complete until a
fresh migrated database exercises the affected read/write path.

### PostgreSQL enum and status columns

PostgreSQL enum columns are typed boundaries. Application SQL must not rely on
implicit conversion from `text`, especially when an expression produces a
status value.

- Cast bound status values at the point of use, using the enum owned by the
  column:
  `SET payment_status = $3::commerce.order_payment_status`.
- Cast the complete result of `CASE`, `COALESCE`, `NULLIF`, `UNION`, or
  `VALUES` expressions:
  `(CASE WHEN payment_status = 'paid' THEN 'paid' ELSE 'pending' END)::commerce.order_payment_status`.
- Use an explicit schema-qualified cast for enum literals in status writes,
  even when PostgreSQL could infer the target type:
  `SET status = 'cancelled'::commerce.order_status`.
- Bound values in enum predicates must use the corresponding cast, for example
  `WHERE status = $1::commerce.order_status`.
- Read enum columns as text at the repository boundary (`status::text`) and
  parse them into the Rust domain enum. Do not make the domain layer depend on
  SQL types.
- Keep allowed transitions in the domain/application layer. The PostgreSQL
  enum and relational constraints remain the storage boundary; a `CHECK`
  constraint or trigger does not replace explicit expression casts.

Every new enum write path requires a PostgreSQL integration test against the
migrated schema. Static SQL should use SQLx compile-time query checking where
the build pipeline provides the migrated database or offline query metadata.

### Checkout lifecycle invariants

- The checkout transaction locks one active Cart, creates one pending Order,
  snapshots its lines, and reserves inventory. It never calls a Provider and
  never creates a successor Cart.
- `(store_id, cart_id)` is unique on Orders. A locked Cart is immutable and
  cannot start a second checkout; the storefront obtains or creates the next
  active Cart after the transaction.
- `commerce.carts.payment_client_action` is private provider-form recovery
  state. It is allowed only on a locked Cart, is returned only by checkout
  handoff endpoints, and is cleared by every terminal payment or Order path.
- A pending Order is the resume identity. If the action exists, resume must
  return it without a Provider call. If it does not exist, a retry may create
  the Provider form with the Order-derived idempotency key; the retry must
  provide a return URL because it is intentionally not persisted.
- Provider callbacks, not a local timer, decide payment expiry. Callback
  handling is responsible for clearing the action and releasing inventory on
  failure or cancellation.
