# ADR 0017: Channel-Published Catalog Collections

- Status: Accepted
- Date: 2026-08-16

## Context

Merchants need curated groups such as featured products, seasonal edits, and landing-page assortments. A Collection cannot be a loose tag on Product: merchandising order, lifecycle, channel visibility, auditability, and future localized content belong to the Collection itself. Product visibility must remain authoritative so that adding a draft or unpublished Product to a public Collection never exposes it.

## Decision

Catalog owns manual Collections. A Collection belongs to exactly one Store and has a canonical handle, title, description, and `draft`, `active`, or terminal `archived` lifecycle. Its complete Product membership is replaced atomically as an ordered list of at most 1,000 same-Store Product IDs. Composite foreign keys prevent cross-account and cross-Store membership.

Collection publication is an explicit relation to a Sales Channel. Only an active Collection can be published to an active Channel in the same Store. Storefront Collection reads require the Store, Channel, Collection, and Collection publication to be active. A Product in that Collection is visible and counted only when that Product is independently active and published to the same Channel. Product listing with a Collection handle preserves the manual membership position across cursor pages.

Owner, administrator, developer, and manager memberships may mutate Collections. Mutations are idempotent and append typed immutable audit events for creation, content updates, lifecycle transitions, membership replacement, publication, and unpublication. Runtime privileges prohibit deleting Collection roots or changing audit events; RLS protects all Collection tables.

## Consequences

- Collection membership cannot bypass Product publication or lifecycle rules.
- The same Collection may be visible on one Sales Channel and absent on another.
- Replacing membership is atomic, deterministic, and safe to retry.
- Archived Collections retain their identity and audit evidence and cannot be reactivated.
- Automated rule-based Collections remain a future aggregate behavior rather than hidden query syntax in the manual model.

## Rejected alternatives

### Store Collection IDs as a Product array

Arrays cannot enforce Store ownership, stable unique positions, or efficient reverse lookup with the same integrity as normalized membership rows.

### Treat Collection membership as publication

That would let merchandising accidentally expose draft or channel-unpublished Products. Collection and Product publication remain independent gates.

### Order Collection results by Product ID

UUID ordering is stable but does not represent merchant intent. Cursor traversal resolves the anchor membership position and continues in manual order.
