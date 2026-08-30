# ADR 0023: Product Reviews

- Status: Accepted
- Date: 2026-08-17

## Context

A Storefront client needs customer product reviews: a public, unauthenticated submission that starts invisible, a staff moderation queue that can approve or reject it, staff replies, and a public read of only what staff approved. This capability did not previously exist anywhere in Chaos.

The critical evidence rule carried over from the prior integration this replaces: a review being visible on the Storefront must never imply a purchase was verified unless a human moderator actually checked that. The submission and read paths must not derive or infer a "verified buyer" signal from any automated check — it is a fact only a human moderator asserts, deliberately, at approval time.

## Decision

`commerce.reviews` holds reviews as a Product-owned resource alongside Collections and Media. A review is either a customer-submitted top-level review (`is_staff_reply = false`, `rating` 1-5, no parent) or a staff reply (`is_staff_reply = true`, no rating, parent required) — a `CHECK` constraint enforces this shape at the database layer in addition to the domain layer's own validation, and a matching `CHECK` ties `approved_at`/`approved_by_user_id` presence exactly to `status = 'approved'`. The separate `commerce.review_events` write-side ledger was removed because no runtime reader depended on it.

**`verified_buyer` is a plain boolean set only by the approving moderator, in the same request as approval.** It is never derived from an Order, Customer, or payment lookup — the MCP `approve_review` tool requires the field explicitly (`{"verified_buyer": true|false}`), forcing a conscious choice rather than defaulting to either value. Chaos does not attempt to automatically match a reviewer to a completed Order; that matching, if a Store wants it, remains a human moderation step outside Chaos, identical in spirit to how this capability worked in the prior integration.

Storefront submission (`POST /api/v1/products/{product_id}/reviews`) requires a Publishable Key and needs no Shopper credential. A submission always lands `pending` and is invisible to `GET /api/v1/products/{product_id}/reviews` until an administrator approves it. That read endpoint requires the same Publishable Key as every other public channel API operation and returns approved top-level reviews newest-first with their approved staff replies nested underneath. ADR 0028 removed the originally introduced Publishable Key scopes.

Review moderation is exposed through MCP tools authenticated with OAuth and
authorized through current Store membership. Approval and rejection are
terminal from `pending` only; a moderation mistake requires a new review,
matching other terminal commerce transitions.

## Consequences

- The Storefront review response deliberately omits the submitted `author_email`; it remains an internal moderation field and is accepted only on submission. The public shape otherwise stays stable, with one deliberate addition: **`verified_buyer` is now a real field in the response** rather than a badge the client renders unconditionally. A client that previously assumed every returned review was verified must now read this field and gate the badge on it — a small, intentional change that makes the "Verified Buyer" claim strictly more honest than before, not less.
- Review-photo uploads use the generic Media Asset lifecycle from ADR 0018. `review_media_assets` owns the Review-specific attachment, order, and alt text; Storefront returns only ready images belonging to approved reviews. The same `prepare_media_upload` / `complete_media_upload` pair is also used by Product galleries and Product metadata.
- Reviews imported from external channels are created through the authenticated MCP `create_manual_review` tool, start pending, retain an internal source channel/reference, and require the caller to obtain explicit publication consent before creation. Chaos does not persist a separate consent flag. They never become verified buyers automatically.
- MCP tools expose review listing, approval, rejection, and staff replies.
- There is no rate limiting specific to review submission beyond the general request path; a Store that needs abuse resistance would need it added as a follow-up.

## Rejected alternatives

### Derive `verified_buyer` automatically from Order history

Matching the reviewer's email or Customer identity against a completed Order would remove the manual step, but it would also silently change what the badge *means* — from "a human checked" to "our matching heuristic didn't find a mismatch," which is a materially weaker and more error-prone claim (a shopper with a different email between checkout and review, or an order placed by someone else in the household, would produce a false positive). Keeping this a deliberate human action preserves the evidence guarantee exactly.

### Allow re-approval or un-approval

A mutable moderation status would let a Store fix mistakes without creating a new review, but it would also mean a review's visible history could silently change after the fact. Terminal moderation, consistent with how Orders and Collections already treat their own terminal transitions, keeps the current review state explicit. A generic audit record can be added later when a concrete reader requires it.
