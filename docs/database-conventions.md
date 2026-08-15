# Database Conventions

## 1. Schema ownership

PostgreSQL schemas represent bounded-context ownership. They do not represent individual merchant accounts or stores. Account isolation uses `merchant_account_id`, composite foreign keys, transaction-scoped account context, and RLS.

Reserved schema map:

| Schema | Ownership |
|---|---|
| `identity` | Users, credentials, service accounts, and sessions |
| `merchant` | Merchant accounts, memberships, roles, stores, channels, and domains |
| `catalog` | Products, variants, options, collections, and media |
| `pricing` | Money metadata, price lists, prices, promotions, and tax classes |
| `inventory` | Locations, stock items, reservations, and adjustments |
| `sales` | Carts, checkouts, orders, returns, and exchanges |
| `payments` | Provider accounts, payment intents, captures, and refunds |
| `fulfillment` | Shipments, packages, and fulfillment state |
| `notification` | Semantic delivery requests, templates, recipient policy, suppression, and delivery status |
| `integration` | Webhook inboxes, outbox delivery, and external mappings |
| `audit` | Immutable security and administrative audit records |
| `extensions` | Relocatable utility extension objects, currently `citext` |
| `cron` | Objects owned by the `pg_cron` extension |
| `pgmq` | Objects owned by the `pgmq` extension |
| `partman` | Objects owned by the `pg_partman` extension |
| `public` | SQLx migration metadata only; no business tables |

Create a bounded-context schema only when its first object is introduced. Cross-schema foreign keys are allowed only when the ownership direction is explicit and stable. Cross-context writes should normally be coordinated by an application use case and transactional outbox rather than by database triggers.

Application SQL always schema-qualifies tables, types, functions, and sequences. Do not rely on `search_path`.

## 2. Identifier naming

- Use lowercase `snake_case` ASCII identifiers.
- Use plural table names: `merchant_accounts`, `price_lists`, `inventory_reservations`.
- Use singular enum and domain-type names: `merchant_account_status`, `payment_state`.
- Use `id` for a table primary key and `<entity>_id` for references.
- Prefer domain language over abbreviations. Use `quantity`, not `qty`; use `currency`, not `ccy`.
- Avoid PostgreSQL keywords and vague names such as `data`, `value`, `type`, or `status` without domain context.
- Keep identifiers below PostgreSQL's 63-byte limit. Choose concise names instead of relying on silent truncation.

## 3. Constraint and index naming

Use PostgreSQL-compatible names that are predictable in logs and migration errors:

| Object | Pattern | Example |
|---|---|---|
| Primary key | `<table>_pkey` | `orders_pkey` |
| Unique constraint | `<table>_<columns>_key` | `stores_merchant_account_id_code_key` |
| Foreign key | `<table>_<columns>_fkey` | `stores_merchant_account_id_fkey` |
| Check constraint | `<table>_<rule>_check` | `stores_currency_format_check` |
| Non-unique index | `<table>_<purpose>_idx` | `orders_account_created_idx` |
| Exclusion constraint | `<table>_<rule>_excl` | `reservations_no_overlap_excl` |

Name semantic check constraints explicitly. Column lists are acceptable for primary, unique, and foreign-key constraints. Index names describe the query purpose rather than repeating every expression.

Indexes on merchant-owned tables normally begin with `merchant_account_id`; store-specific query paths usually continue with `store_id`. Every foreign-key access path must be evaluated for an index; PostgreSQL does not create foreign-key indexes automatically. Do not add speculative indexes without a query pattern.

## 4. Column conventions

- Primary and foreign IDs use UUIDv7 and PostgreSQL `uuid`.
- Timestamps use `timestamptz`, are stored in UTC, and end in `_at`: `created_at`, `captured_at`.
- Calendar dates use `date` and end in `_on` when that improves clarity.
- Boolean names describe a positive state: `enabled`, `tax_inclusive`, `requires_shipping`.
- Mutable tables normally include `created_at` and `updated_at`.
- Soft deletion is not a default. Use `deleted_at` only when recovery or legal history requires it.
- External identifiers include the provider or scope when ambiguous: `stripe_payment_intent_id`.
- Country and operating-region codes use uppercase ISO 3166-1 alpha-2 values stored as `char(2)`.
- JSONB is reserved for provider payload snapshots, versioned opaque security-library records such as WebAuthn credentials, flexible metadata with defined limits, or data that is genuinely schemaless. Core searchable fields require typed columns.

## 5. Money, quantity, and numeric precision

- Money uses `<name>_amount_minor bigint` plus `<name>_currency char(3)` when a row may contain more than one monetary value.
- A row with one obvious currency context may use `amount_minor` and `currency`.
- Never use `real` or `double precision` for money, tax, rates, or quantities that require exact arithmetic.
- Exchange rates and measured quantities use explicitly bounded `numeric(precision, scale)` values.
- Check constraints reject negative amounts or quantities unless the domain explicitly permits them.

## 6. Merchant-account and store isolation

- Every merchant-owned table contains a non-null `merchant_account_id`.
- Store-owned commerce data also contains a non-null `store_id`.
- Merchant-owned relationships use account-scoped composite foreign keys where practical.
- Every account transaction calls `set_config('app.merchant_account_id', ..., true)` before accessing merchant data.
- RLS policies use the transaction-local `app.merchant_account_id` value.
- Runtime connections use a non-owner role without `BYPASSRLS`.
- Control-plane operations use a separate privileged port and explicit audit trail.
- Identity control-plane connections assume the non-owner `chaos_control_plane` role, which can access `identity` but cannot access merchant-owned tables.
- Cross-account account-directory reads use `app.user_id` and read-only membership policies; they never broaden merchant-owned Store or commerce-data policies.
- Cross-account isolation tests are mandatory for every new merchant-owned aggregate. Store-scoped authorization tests are mandatory for store-owned aggregates.
- API key rows and their normalized scopes live in `merchant`. Machine authentication uses a narrowly granted verifier boundary to resolve account and Store context without granting pre-authentication table access.
- Catalog child tables carry `merchant_account_id`, `store_id`, and `product_id` through composite foreign keys. A Variant selection cannot reference an Option Value from another Product even when both Products belong to the same Store.

## 7. Migration rules

- Migration names use zero-padded sequence numbers and concise English descriptions: `0004_create_identity_schema.sql`.
- Before the first shared or non-disposable environment, bootstrap migrations may be squashed after every disposable database has been recreated.
- After the first shared or non-disposable environment exists, applied migrations are immutable. Fix forward with a new migration.
- Production application startup never runs migrations. A separate release job runs them once.
- Deployments follow expand/migrate/contract so adjacent application versions remain compatible.
- Destructive operations, column rewrites, large backfills, and blocking index creation require an explicit rollout plan.
- Large production indexes use `CREATE INDEX CONCURRENTLY` in a non-transactional migration when supported by the migration tooling.
- Every migration is tested from an empty database and from the previous released schema.

## 8. SQL style

- Use uppercase SQL keywords and built-in data types, and lowercase qualified identifiers.
- Align column names, data types, and nullability clauses within `CREATE TABLE` blocks when it improves scanability.
- Separate column definitions from table constraints with one blank line.
- List columns explicitly in application queries and inserts. Avoid `SELECT *`.
- Bind every value. Dynamic identifiers require strict allowlisting and explicit safety review.
- Keep transactions short and avoid network calls while holding database locks.
- Add comments only when they explain a non-obvious invariant or operational constraint.
