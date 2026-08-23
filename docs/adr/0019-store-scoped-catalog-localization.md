# ADR 0019: Store-scoped Catalog Localization

- Status: Superseded
- Date: 2026-08-16

## Superseding decision

Chaos currently exposes an English-only catalog and storefront contract. A Store has one currency in `commerce.stores.currency` and does not configure locales. Locale query parameters, locale snapshots, and translation tables are not part of the current commerce model.

If localization is needed later, it must be introduced by a new ADR that defines the public API, persistence model, and migration strategy together. This record is retained only to explain why the earlier localization design is no longer active.
