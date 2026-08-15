# ADR 0011: Store Customer Association

- Status: Accepted
- Date: 2026-08-16

## Context

A global User identity may shop at multiple independent Stores. Anonymous browsing and Cart ownership already use a possession-bound shopper credential, while authenticated customers need reusable profiles, saved addresses, and Order history on a new device. Replacing guest ownership when a User signs in would break existing credentials, audit history, and retry semantics.

## Decision

A Customer is Store-owned and references one global `identity.users` record. The same User receives a distinct Customer identity in every Store. Customer email is initialized and refreshed from the verified User identity; optional Customer phone and saved addresses are Store-local profile data.

Storefront Customer operations require two independent credentials:

- the Store publishable key in `Authorization`, which resolves merchant account, Store, Sales Channel, and capability scope;
- the verified User session in `x-chaos-customer-session`, which resolves the global User.

Association additionally requires the possession-bound `x-chaos-shopper-token`. It appends an immutable Customer-to-shopper link and never rewrites the shopper identity on an existing Cart, Checkout, Order, or Payment Attempt. One shopper credential can be associated with only one Customer in a Store. A transaction-scoped advisory lock serializes competing association attempts without granting update permission on the immutable link table.

New Carts record the current association when available. Checkout creation resolves it again so a shopper may create a Cart anonymously and authenticate before Checkout. Checkout and Order snapshot the resulting optional Customer ID. Customer Order history also follows immutable shopper links, so Orders created before association remain recoverable after verified sign-in and from a new device.

Saved addresses are reusable profile data, not commercial evidence. Checkout always copies submitted contact and address values into immutable snapshots. Updating or deleting a saved address never changes an existing Checkout or Order.

## Consequences

- Customer and Order history remain isolated by merchant account and Store, while history is also restricted to the current Sales Channel at the Store API boundary.
- Possession-bound guest access continues to work after association.
- Account recovery does not require retaining the original shopper token.
- Customer deletion, consent, retention, and analytics identity unlinking require an explicit privacy workflow in a later phase; deleting profile data must not erase legally required Order snapshots.
- The custom Customer session header is an explicit dual-credential contract. A future first-party Storefront backend may exchange it for a narrower Store Customer token without changing the application boundary.
