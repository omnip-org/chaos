# Database Conventions

## Schema ownership

PostgreSQL schemas represent data ownership, not individual users, Stores, Rust modules, or deployment units. Current business schemas are `identity`, `commerce`, and `integration`. Utility extension objects live in `extensions`; `public` contains no business tables.

`commerce` owns Stores, Store memberships, Sales Channels, Store locales, public Storefront Keys, catalogs, pricing, inventory, sales, Stripe payment account configuration, payment readiness, payment commands, and the verified Stripe webhook inbox. There is no merchant-account schema or aggregate. A User-owned trusted-client credential is stored in `identity.access_keys`; a Storefront public key is stored as plaintext in `commerce.store_publishable_keys` because it is intentionally safe to embed in frontend code.

Do not create a schema merely because a Rust module exists. A new schema requires a distinct data owner, security boundary, or operational lifecycle. `commerce` contains all Store-owned catalog, inventory, sales, payment, and rebuildable Storefront read models. `integration` contains generic idempotency, outbox/event routing, and analytical processing state.

Application SQL always schema-qualifies objects. Cross-schema foreign keys are allowed only for stable ownership references. Cross-context behavior is coordinated by application use cases and durable events, not database triggers.

Durable integration work uses PGMQ for message visibility and retry attempts. An
authoritative integration row stores the bounded business payload and outcome;
its insertion trigger enqueues only the row identifier and records the returned
PGMQ message identifier. Runtime code accesses queues through schema-qualified,
security-definer routines rather than receiving direct access to extension-owned
tables. Scheduled scans derived from current business state are not queues and
may use short, recoverable row leases.

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

The bootstrap uses one file for identity and multiple capability files for
commerce: `0002_identity.sql`, `0003_commerce.sql`,
`0004_commerce_catalog.sql`, `0005_commerce_pricing.sql`, and
`0006_commerce_sales.sql`. Integration follows them as
`0007_integration.sql` and `0008_integration_analytics.sql`, with the Stripe
payment capability in `0009_commerce_payments.sql`.
Catalog, pricing, inventory, and sales capability files use the existing
`commerce` schema. Within
each file, define objects in dependency order: types, tables, indexes, routines,
triggers, row-level security, policies, and grants.

Production startup never runs migrations. Releases use a separate migration job and expand/migrate/contract changes when adjacent application versions may overlap. Destructive operations, table rewrites, large backfills, and blocking indexes require an explicit rollout plan.

## SQL style

- Use uppercase SQL keywords and built-in data types.
- List columns explicitly; never use `SELECT *` in application SQL.
- Bind every value. Dynamic identifiers require strict allowlisting.
- Evaluate an index for every foreign-key access path; PostgreSQL does not create one automatically.
- Keep transactions short and never perform network calls while holding database locks.
- Add comments only for non-obvious invariants or operational constraints.
