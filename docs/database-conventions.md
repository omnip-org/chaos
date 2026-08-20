# Database Conventions

## Schema ownership

PostgreSQL schemas represent bounded-context ownership, not individual users or Stores. Current business schemas are `identity`, `merchant`, `catalog`, `pricing`, `inventory`, `sales`, `payments`, `fulfillment`, `notification`, `analytics`, and `integration`. Utility extension objects live in `extensions`; `public` contains no business tables.

The legacy schema name `merchant` currently owns Stores, Store memberships, Sales Channels, Store locales, and Store API keys. Its name may be changed in a later Store-focused slice; it does not imply a merchant-account aggregate.

Application SQL always schema-qualifies objects. Cross-schema foreign keys are allowed only for stable ownership references. Cross-context behavior is coordinated by application use cases and durable events, not database triggers.

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

## Identity

`identity.users` owns the internal user identifier and current verified email. `identity.external_identities` maps a provider subject to a User. Provider subjects are opaque, case-sensitive strings and form a composite key with provider.

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

Migration files use zero-padded sequence numbers and concise English names. Before the first shared or non-disposable environment, bootstrap migrations may be rewritten and every disposable database must be recreated. After that point, applied migrations are immutable and changes fix forward.

Production startup never runs migrations. Releases use a separate migration job and expand/migrate/contract changes when adjacent application versions may overlap. Destructive operations, table rewrites, large backfills, and blocking indexes require an explicit rollout plan.

## SQL style

- Use uppercase SQL keywords and built-in data types.
- List columns explicitly; never use `SELECT *` in application SQL.
- Bind every value. Dynamic identifiers require strict allowlisting.
- Evaluate an index for every foreign-key access path; PostgreSQL does not create one automatically.
- Keep transactions short and never perform network calls while holding database locks.
- Add comments only for non-obvious invariants or operational constraints.
