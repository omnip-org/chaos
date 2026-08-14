# Database Conventions

## 1. Schema ownership

PostgreSQL schemas represent bounded-context ownership. They do not represent individual tenants. Tenant isolation uses `tenant_id`, composite foreign keys, transaction-scoped tenant context, and RLS.

Reserved schema map:

| Schema | Ownership |
|---|---|
| `identity` | Users, credentials, service accounts, and sessions |
| `tenancy` | Tenants, memberships, roles, stores, channels, and domains |
| `catalog` | Products, variants, options, collections, and media |
| `pricing` | Money metadata, price lists, prices, promotions, and tax classes |
| `inventory` | Locations, stock items, reservations, and adjustments |
| `sales` | Carts, checkouts, orders, returns, and exchanges |
| `payments` | Provider accounts, payment intents, captures, and refunds |
| `fulfillment` | Shipments, packages, and fulfillment state |
| `integration` | Webhook inboxes, outbox delivery, and external mappings |
| `audit` | Immutable security and administrative audit records |
| `extensions` | Objects owned by PostgreSQL extensions |
| `public` | SQLx migration metadata only; no business tables |

Create a bounded-context schema only when its first object is introduced. Cross-schema foreign keys are allowed only when the ownership direction is explicit and stable. Cross-context writes should normally be coordinated by an application use case and transactional outbox rather than by database triggers.

Application SQL always schema-qualifies tables, types, functions, and sequences. Do not rely on `search_path`.

## 2. Identifier naming

- Use lowercase `snake_case` ASCII identifiers.
- Use plural table names: `tenants`, `price_lists`, `inventory_reservations`.
- Use singular enum and domain-type names: `tenant_status`, `payment_state`.
- Use `id` for a table primary key and `<entity>_id` for references.
- Prefer domain language over abbreviations. Use `quantity`, not `qty`; use `currency`, not `ccy`.
- Avoid PostgreSQL keywords and vague names such as `data`, `value`, `type`, or `status` without domain context.
- Keep identifiers below PostgreSQL's 63-byte limit. Choose concise names instead of relying on silent truncation.

## 3. Constraint and index naming

Use PostgreSQL-compatible names that are predictable in logs and migration errors:

| Object | Pattern | Example |
|---|---|---|
| Primary key | `<table>_pkey` | `orders_pkey` |
| Unique constraint | `<table>_<columns>_key` | `stores_tenant_id_code_key` |
| Foreign key | `<table>_<columns>_fkey` | `stores_tenant_id_fkey` |
| Check constraint | `<table>_<rule>_check` | `stores_currency_format_check` |
| Non-unique index | `<table>_<purpose>_idx` | `orders_tenant_created_idx` |
| Exclusion constraint | `<table>_<rule>_excl` | `reservations_no_overlap_excl` |

Name semantic check constraints explicitly. Column lists are acceptable for primary, unique, and foreign-key constraints. Index names describe the query purpose rather than repeating every expression.

Indexes on tenant-owned tables normally begin with `tenant_id`. Every foreign-key access path must be evaluated for an index; PostgreSQL does not create foreign-key indexes automatically. Do not add speculative indexes without a query pattern.

## 4. Column conventions

- Primary and foreign IDs use UUIDv7 and PostgreSQL `uuid`.
- Timestamps use `timestamptz`, are stored in UTC, and end in `_at`: `created_at`, `captured_at`.
- Calendar dates use `date` and end in `_on` when that improves clarity.
- Boolean names describe a positive state: `enabled`, `tax_inclusive`, `requires_shipping`.
- Mutable tables normally include `created_at` and `updated_at`.
- Soft deletion is not a default. Use `deleted_at` only when recovery or legal history requires it.
- External identifiers include the provider or scope when ambiguous: `stripe_payment_intent_id`.
- JSONB is reserved for provider payload snapshots, flexible metadata with defined limits, or data that is genuinely schemaless. Core searchable fields require typed columns.

## 5. Money, quantity, and numeric precision

- Money uses `<name>_amount_minor bigint` plus `<name>_currency char(3)` when a row may contain more than one monetary value.
- A row with one obvious currency context may use `amount_minor` and `currency`.
- Never use `real` or `double precision` for money, tax, rates, or quantities that require exact arithmetic.
- Exchange rates and measured quantities use explicitly bounded `numeric(precision, scale)` values.
- Check constraints reject negative amounts or quantities unless the domain explicitly permits them.

## 6. Tenant isolation

- Every tenant-owned table contains a non-null `tenant_id`.
- Tenant-owned relationships use composite `(tenant_id, id)` foreign keys where practical.
- Every tenant transaction calls `set_config('app.tenant_id', ..., true)` before accessing tenant data.
- RLS policies use the transaction-local `app.tenant_id` value.
- Runtime connections use a non-owner role without `BYPASSRLS`.
- Control-plane operations use a separate privileged port and explicit audit trail.
- Cross-tenant isolation tests are mandatory for every new tenant-owned aggregate.

## 7. Migration rules

- Migration names use zero-padded sequence numbers and concise English descriptions: `0004_create_identity_schema.sql`.
- Applied migrations are immutable. Fix forward with a new migration.
- Production application startup never runs migrations. A separate release job runs them once.
- Deployments follow expand/migrate/contract so adjacent application versions remain compatible.
- Destructive operations, column rewrites, large backfills, and blocking index creation require an explicit rollout plan.
- Large production indexes use `CREATE INDEX CONCURRENTLY` in a non-transactional migration when supported by the migration tooling.
- Every migration is tested from an empty database and from the previous released schema.

## 8. SQL style

- Use uppercase SQL keywords and lowercase qualified identifiers.
- List columns explicitly in application queries and inserts. Avoid `SELECT *`.
- Bind every value. Dynamic identifiers require strict allowlisting and explicit safety review.
- Keep transactions short and avoid network calls while holding database locks.
- Add comments only when they explain a non-obvious invariant or operational constraint.
