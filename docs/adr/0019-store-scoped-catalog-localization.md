# ADR 0019: Store-Scoped Catalog Localization

- Status: Accepted
- Date: 2026-08-16

## Decision

Every Store has one canonical BCP 47 default Locale and an explicit set of additional enabled Locales. Locale tags are parsed and canonicalized in the domain. Extensions and private-use tags are excluded from public commerce contracts so cache keys, SDK values, and fallback behavior remain bounded and portable.

Canonical Catalog fields remain the final fallback and stable handles, identifiers, SKUs, money, inventory, and lifecycle state are never translated. Typed translation tables store Product title and description, Variant title, Collection title and description, and Media alternative text. Every translation retains Store ownership, a real resource foreign key, author and timestamps. Translation and Store Locale mutations append immutable typed audit events.

Storefront Catalog requests accept an explicit `locale` query parameter. An omitted Locale resolves to the Store default. An explicit Locale must be enabled for that Store. Resolution uses exact Locale, then its primary language when that language is enabled, then canonical content. The response reports the resolved Locale. This deterministic input is part of the URL and avoids implicit `Accept-Language` cache variation.

Cart creation accepts the same optional Locale and freezes the resolved value. Product and Variant text copied into Cart lines uses that Locale and remains immutable through Checkout and Order snapshots. Later translation changes cannot rewrite an existing shopper's commercial history.

## Consequences

- Storefront clients can select language independently from settlement currency and destination country.
- Typed tables and composite foreign keys preserve account, Store, Product, Variant, Collection, and Media ownership without polymorphic JSON.
- Canonical content guarantees a complete response even while translations are incrementally authored.
- Changing the Store default affects only future requests and Carts that omit a Locale.
- Search remains canonical in the first localization release. Locale-specific indexing is a separate capacity and relevance decision; localized text is still returned for products found through canonical search.

## Rejected alternatives

### Infer Locale only from `Accept-Language`

Implicit negotiation complicates CDN cache keys, generated clients, replay, and test determinism. An explicit query value is easier to audit and compose.

### Localize handles and SKUs

Localized routing aliases introduce canonical URL, redirect, uniqueness, and historical-link concerns. Handles and SKUs remain stable identifiers.

### Store all translations in one polymorphic JSON table

That design cannot express resource ownership with ordinary foreign keys, weakens field constraints, and makes Store-isolated queries harder to index and audit.
