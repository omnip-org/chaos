# Product Model

## Goal

Chaos Commerce is the backend operating system for independent online stores. A person creates one user identity, can own or join multiple merchant accounts, and can create multiple independent stores inside each merchant account. The platform provides all commerce backend capabilities through headless Admin and Storefront APIs.

The product is multi-account in the same way that one Stripe user can access multiple isolated business accounts. `Tenant` is an architectural property, not a term exposed in the product model or public API.

## Core hierarchy

```text
User
  └─ MerchantAccountMembership
       └─ MerchantAccount
            ├─ Store
            │    ├─ Domain
            │    ├─ SalesChannel
            │    ├─ Catalog
            │    ├─ PriceLists and Currencies
            │    ├─ Inventory
            │    ├─ Customers and Carts
            │    ├─ Orders and Fulfillment
            │    └─ Payment Configuration
            └─ Store
```

## Ubiquitous language

### User

A global login identity for a person. A user does not own commerce data directly. Access is granted through merchant-account memberships.

### MerchantAccount

An isolated merchant workspace and the primary authorization, billing, and data-isolation boundary. A merchant account represents one business operator and can own multiple stores. One user can own or join multiple merchant accounts.

### MerchantAccountMembership

The relationship between a user and a merchant account. It carries roles and permissions such as owner, administrator, developer, catalog manager, and support agent.

### Store

An independent online storefront operated by a merchant account. A store owns its domains, sales channels, catalog visibility, currencies, pricing, inventory rules, customers, carts, orders, payment configuration, and fulfillment settings.

Stores are isolated by default. Sharing products, customers, inventory, or price lists across stores requires an explicit future domain feature rather than an implicit database join.

### SalesChannel

A publication and access surface within a store, such as Web, mobile, POS, or marketplace. It controls publishable keys, product visibility, inventory selection, and channel-specific behavior.

Every Store starts with an active default Web channel. Additional channels are explicit Store-owned resources. Product visibility is granted per channel rather than through one global published flag.

### Product

The Store-owned aggregate for merchant-authored catalog content. A Product has a stable handle, title, description, lifecycle status, Options, Option Values, and Variants. Draft Products may be incomplete and are never served merely because they exist.

### ProductVariant

The actual sellable unit of a Product. A Variant may have a Store-unique SKU and selects exactly one value for every Product Option. It carries behavioral flags such as whether shipping and inventory tracking are required, but it does not own prices or stock quantities.

### ProductPublication

The explicit relationship that makes an active Product visible through a Sales Channel. Publication does not override inactive Store, Channel, Product, or Variant state.

### PriceList

A Store-owned pricing context with one enabled currency, tax semantics, an optional activation window, and explicit Product Variant prices. A Price List is independent from Product content and Sales Channel publication. The same Variant may have different authoritative prices in different Price Lists.

### Money

An exact amount in integer minor units paired with one ISO 4217 currency. Arithmetic is checked and never mixes currencies implicitly. Display conversion and exchange-rate calculations do not overwrite authoritative Money values.

### ApiKey

A revocable machine credential bound to one Store and an explicit set of capabilities. Publishable keys identify public Store API clients. Secret keys authenticate trusted integrations and MCP clients. Plaintext secrets are visible only when a key is created.

## Isolation rules

- `merchant_account_id` is the primary RLS and authorization boundary.
- Store-owned commerce data also carries `store_id`.
- The authenticated user selects a merchant account only through a verified membership.
- Storefront requests derive merchant account and store from a publishable key or verified domain.
- Integration and MCP requests derive merchant account and store from a scoped secret key.
- Webhooks derive merchant account and store from a verified local provider mapping.
- Internal events carry both identifiers whenever the event belongs to a store.
- No request is authorized by trusting a client-supplied identifier alone.
