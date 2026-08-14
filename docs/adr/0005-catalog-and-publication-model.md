# ADR 0005: Catalog Aggregates and Explicit Channel Publication

- Status: Accepted
- Date: 2026-08-15

## Context

A headless commerce catalog must represent products consistently across Admin, Storefront, marketplace, mobile, POS, and MCP clients. Combining product content, prices, inventory, and publication in one record creates unclear ownership and makes multi-currency pricing and inventory reservations difficult to evolve independently.

Products may be incomplete while a merchant edits them. A purchasable item may also have combinations such as Color and Size. The database must prevent option values or variants from being connected across products, Stores, or merchant accounts even if an application defect supplies incorrect identifiers.

## Decision

Catalog is a bounded context with its own PostgreSQL `catalog` schema and domain module. Its primary aggregate is Product.

A Product owns:

- stable identity, Store-scoped handle, title, description, and lifecycle status;
- zero to ten ordered Product Options;
- ordered Option Values owned by exactly one Option;
- Product Variants, which are the actual sellable units;
- one selected Option Value per Option for every Variant when Options exist.

A draft Product may be incomplete. Activating a Product requires at least one Variant. The application rejects duplicate Option names, duplicate values within an Option, duplicate Variant combinations, and duplicate SKUs within the aggregate. PostgreSQL additionally enforces Store-wide case-insensitive SKU uniqueness.

Normalized composite foreign keys carry `merchant_account_id`, `store_id`, and `product_id` through Option, Option Value, Variant, and selected-value tables. This prevents a Variant from selecting a value owned by another Product or Store. All Catalog tables use merchant-account RLS.

Sales Channel belongs to the Merchant context because it is a Store access and publication surface rather than product content. Every newly provisioned Store receives one active default Web channel in the same transaction as the Store and default currency.

Product visibility uses an explicit `catalog.product_publications` relation between Product and Sales Channel. Product status and publication answer different questions:

- Product status determines whether the product content is operationally active.
- Publication determines whether an active Product is visible through a particular channel.
- Store status and Sales Channel status remain additional serving-time gates.

Prices do not belong to Product or Variant rows. Pricing will own currency-specific Price Lists and amounts. Inventory will own locations, stock, availability, and reservations. Catalog stores only `track_inventory` and `requires_shipping` behavior flags on Variant.

## Consequences

- The same Product can be published to one channel without leaking into another.
- Admin workflows can safely persist incomplete drafts.
- Storefront queries must join only active Store, Channel, Product, publication, and Variant records.
- Product creation will be transactional across Product, Options, Option Values, Variants, and selected values.
- Cross-context pricing and inventory reads require application composition rather than Catalog table columns.
- Sharing products between Stores remains an explicit future capability instead of an accidental cross-Store relationship.

## Rejected alternatives

### Store option combinations as JSON

JSON is easy to write but cannot provide the same referential integrity, ordering constraints, or efficient option-level queries. Core catalog structure remains normalized.

### Put prices and stock on Product Variant

One Variant can have many currency, market, customer-group, and time-dependent prices, and stock can exist at many locations. Single columns would encode false one-to-one relationships.

### Use a Product `published` boolean

A boolean cannot represent independent Web, mobile, marketplace, POS, or future channel visibility. Explicit publication records preserve channel ownership and auditability.
