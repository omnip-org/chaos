# Product Model

## Goal

Chaos Commerce is a headless commerce foundation for independent online stores. A person creates one user identity, can own or join multiple merchant accounts, and can create multiple independent stores inside each merchant account. The current platform provides a verified commerce kernel through Admin and Store APIs; customer, checkout, provider, and ecosystem capabilities continue through the delivery roadmap.

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
            │    ├─ Payment Configuration
            │    └─ Analytics and Destinations
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

### Customer

A Store-owned shopping profile linked to one verified global User. Customers hold reusable contact data and saved addresses. Immutable shopper links recover Order history across devices without replacing possession-bound guest ownership. The same User has independent Customer profiles in different Stores.

### PaymentProviderAccount

A Store-owned connection to one external payment provider account. Its provider identity and external mapping are immutable, while administrators may replace write-only secret-manager references and request enablement for new payment creation. Enablement is effective only while a provider capability and responsibility assessment is ready and unexpired; otherwise the account remains disabled with bounded remediation codes. Raw provider credentials and provider identity data are never part of the product model or HTTP response.

## Isolation rules

- `merchant_account_id` is the primary RLS and authorization boundary.
- Store-owned commerce data also carries `store_id`.
- The authenticated user selects a merchant account only through a verified membership.
- Storefront commerce requests derive merchant account, Store, and Channel from a publishable key. A separate public bootstrap lookup resolves only DNS-verified custom hostnames bound to an active Store and active Web Sales Channel; it does not replace authentication or possession credentials.
- Integration requests derive merchant account and Store from a scoped secret key. Future MCP tools will reuse that boundary with an additional tool scope.
- Webhooks derive merchant account and store from a verified local provider mapping.
- Internal events carry both identifiers whenever the event belongs to a store.
- No request is authorized by trusting a client-supplied identifier alone.

## Capability status

### Current

- Passwordless identity, merchant accounts, memberships, Stores, and Sales Channels.
- Product, Variant, publication, Price List, Inventory, Customer, saved address, Cart, Checkout, and Order capabilities.
- Provider-neutral Payment Attempt and Refund state machines, Store-owned Provider administration, and a sandbox adapter.
- Fulfillment, Return, search, idempotency, inbox, outbox, RLS, and versioned HTTP contracts.
- First-party behavior collection, active-engagement sessions, Store Analytics Policy, consent-aware identity links, retention, data-subject erasure, and typed trusted commerce facts.

### Next

- Customer-specific pricing and segmentation; Store Promotions, deterministic discount allocation, shipping options, destination Tax Rules, and complete Checkout totals are implemented.
- Stripe payment, Resend notification, and shipping provider adapters with Store-owned configuration.
- Consent-aware attribution, isolated reporting read models, and conversion exports.

### Reserved

- Collections, media, localization, customer segments, exchanges, and advanced logistics.
- MCP tools, outbound integration webhooks, generated SDKs, and third-party application workflows.
- Advanced experimentation, recommendations, customer scoring, and cross-Store analytics.
