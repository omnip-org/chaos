# ADR 0021: Bounded Catalog Metadata

- Status: Accepted
- Date: 2026-08-17

## Context

Storefront clients need to attach merchandising content to a Product, ProductVariant, or Collection that has no fixed shape and changes independently of a release: taglines, hero imagery references, comparison tables, size guides, and similar content authored per catalog item. This content has no canonical searchable meaning to Chaos itself and does not participate in pricing, inventory, or publication rules. `docs/database-conventions.md` already reserves JSONB for exactly this case ("flexible metadata with defined limits, or data that is genuinely schemaless"), distinct from core fields that require typed columns.

## Decision

Product, ProductVariant, and Collection each gain a nullable `metadata JSONB` column. The domain layer treats metadata as an opaque, bounded value rather than a structured type: `CatalogMetadata` (`chaos-domain/src/catalog/metadata.rs`) wraps canonical JSON text and enforces only a 32 KiB upper bound, matching the order of magnitude of other bounded payloads in this codebase (e.g. the analytics event body limit). Structural JSON validity is guaranteed upstream by the API layer's typed `serde_json::Value` deserialization before a `CatalogMetadata` is ever constructed, so the domain never needs a JSON parser dependency.

Metadata is set at write time through the existing Product, Variant, and Collection create/update operations — there is no separate metadata endpoint. `ProductContent` and `CollectionContent` carry an optional `CatalogMetadata` alongside their existing title/description validation; `ProductVariant` carries its own, set at variant-creation time (variants have no standalone update endpoint in this release, matching their existing create-only lifecycle). A 32 KiB PostgreSQL `CHECK` constraint on each table backstops the domain bound.

Admin Product/Variant/Collection detail responses and Storefront Product/Variant/Collection responses both include `metadata` when present; admin list responses omit it, matching the existing convention that list rows exclude `description`. The field is deliberately schema-agnostic on the wire: Chaos stores and returns exactly the JSON object a client sent, with no interpretation, translation, or validation of its internal shape.

## Consequences

- Storefront clients can carry rich, per-item merchandising content without a schema migration for every new content shape, at the cost of Chaos being unable to search, validate, or localize anything inside it.
- The 32 KiB bound is enforced identically in the domain (before any write) and in PostgreSQL (as defense in depth against a future write path that bypasses the domain type), so a stored value is never larger than what the domain already accepted.
- `chaos-mcp`'s `create_product`/`update_product`/`create_collection`/`update_collection` tools do not yet expose a `metadata` parameter; the Admin HTTP API is the only way to set it in this release.
- Metadata is not translated by the Store-scoped Catalog localization introduced in ADR 0019. A client that needs localized merchandising content must currently encode that inside the JSON value itself.

## Rejected alternatives

### Model metadata as a typed domain aggregate

Modeling the known content shapes (taglines, comparison tables, size guides, and so on) as typed fields would let Chaos validate and evolve them safely, but the shapes are owned and iterated on by each Storefront client independently of the Chaos release cycle. A typed model would require a Chaos migration for every new merchandising layout a Store wants to try.

### Give metadata its own PUT endpoint separate from content update

A dedicated endpoint would let a client update metadata without resending title/description, but it would also let a client observe or create a Product/Collection state with content and metadata written by two non-atomic requests. Folding metadata into the existing content update keeps one write, one idempotency key, and one audit event.
