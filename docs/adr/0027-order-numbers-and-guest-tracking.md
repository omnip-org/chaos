# ADR 0027: Separate Order Identity, Display, and Guest Tracking

- Status: Accepted
- Date: 2026-08-20

## Decision

Orders retain UUIDv7 primary keys for internal relationships, API operations,
and Analytics deduplication. Every Order also receives one immutable,
Store-scoped display number in `W-YYYYMMDD-XXXXXXXX` format. The date is UTC
and the suffix uses cryptographically secure Crockford Base32 characters.
There is no sequential component. A unique database constraint rejects the
extremely unlikely collision.

Guest tracking no longer treats the Order UUID as a credential. Confirmation
creates a 256-bit `otk_` tracking capability whose long-lived table stores only
its SHA-256 digest. The emailed URL places the capability in the Fragment so
it is not sent in HTTP request targets or Referrer headers. The Storefront
exchanges it in a POST body for a random `ots_` session valid for 30 minutes,
then removes the Fragment from browser history. Session access is read-only,
bound to one Store, Sales Channel, tracking key, and Order. Tracking keys
expire after 180 days and can be revoked.

An external notification channel may carry the plaintext tracking capability
because the recipient must receive it. Its payload is
cleared after successful delivery. Failed jobs retain it only while retries
remain possible.

The public Order read endpoint again requires the possession-bound Shopper
credential. An active Publishable Key authorizes the Store API entry point;
possession of the tracking capability or session is still required to read a
guest Order.

Carrier tracking URLs remain data shown inside the stable Chaos tracking page.
Emails link to Chaos rather than making a carrier URL the primary customer
entry point.
