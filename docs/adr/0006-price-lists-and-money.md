# ADR 0006: Price Lists Own Currency-Specific Variant Prices

- Status: Accepted
- Date: 2026-08-15

## Context

A Store may sell the same Product Variant in multiple currencies, markets, channels, and future customer segments. A single price column on a Variant cannot distinguish these contexts and would make currency conversion overwrite authoritative settlement prices. Cart and Order calculations also require exact arithmetic and stable tax semantics.

## Decision

Pricing is a bounded context with its own PostgreSQL `pricing` schema and domain module.

`Money` stores a signed 64-bit minor-unit amount and an uppercase ISO 4217 currency code. Domain operations use checked arithmetic, reject mixed currencies, and allocate remainders deterministically while preserving the exact total. Floating-point values are never accepted for monetary amounts.

A Price List belongs to exactly one merchant account and Store. It defines:

- a Store-scoped code and merchant-facing name;
- exactly one currency that must be enabled for the Store;
- whether listed amounts include tax;
- an optional half-open activation window represented by `starts_at` and `ends_at`;
- a draft, active, or archived lifecycle status.

A Price belongs to one Price List and one Product Variant in the same Store. Its `amount_minor` is non-negative. A Variant may appear at most once in a Price List and may appear in many Price Lists. Creating an active Price List requires at least one Price and all referenced Variants to be active.

Admin creation writes the Price List, all Prices, and its idempotency response in one transaction. Composite foreign keys and RLS enforce Store and merchant-account boundaries after application authorization.

## Consequences

- Explicit USD and EUR Price Lists remain separate settlement sources; exchange-rate display logic cannot mutate either list.
- Tax-inclusive and tax-exclusive amounts cannot be silently mixed in one list.
- Storefront pricing can choose one active list by Store, currency, time, and later market/channel assignments.
- Orders will snapshot the selected currency, amount, tax semantics, and Price List identity instead of reading mutable prices after creation.
- Store Tax Rules use integer basis points and one active rule per destination country. Checkout rounds the aggregate tax once and allocates it deterministically to lines. Tax-inclusive lists extract tax without increasing the total; tax-exclusive lists add tax to the total.
- Promotions and customer-specific adjustments remain separate future pricing concepts rather than destructive edits to base prices.

## Rejected alternatives

### Store price and currency on Product Variant

One Variant may have many valid prices. Variant ownership would encode a false one-to-one relationship and couple Catalog changes to Pricing.

### Store decimal major-unit amounts

Decimal input still requires currency-specific scale interpretation at every boundary. Canonical minor units make persistence and arithmetic explicit; presentation converts at the edge using versioned currency metadata.

### Convert one canonical currency at request time

Converted display amounts are not authoritative settlement prices and introduce exchange-rate timing and rounding ambiguity. Merchants define explicit prices for every supported settlement currency.
