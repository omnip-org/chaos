# ADR 0023: Product Reviews

- Status: Amended by ADR 0025 and ADR 0028
- Date: 2026-08-17

## Context

A Storefront client needs customer product reviews: a public, unauthenticated submission that starts invisible, a staff moderation queue that can approve or reject it, staff replies, and a public read of only what staff approved. This capability did not previously exist anywhere in Chaos.

The critical evidence rule carried over from the prior integration this replaces: a review being visible on the Storefront must never imply a purchase was verified unless a human moderator actually checked that. The submission and read paths must not derive or infer a "verified buyer" signal from any automated check — it is a fact only a human moderator asserts, deliberately, at approval time.

## Decision

`commerce.reviews` and `commerce.review_events` (defined in `migrations/0003_commerce.sql`) hold reviews as a Product-owned resource alongside Collections and Media. A review is either a customer-submitted top-level review (`is_staff_reply = false`, `rating` 1-5, no parent) or a staff reply (`is_staff_reply = true`, no rating, parent required) — a `CHECK` constraint enforces this shape at the database layer in addition to the domain layer's own validation, and a matching `CHECK` ties `approved_at`/`approved_by_user_id` presence exactly to `status = 'approved'`.

**`verified_buyer` is a plain boolean set only by the approving moderator, in the same request as approval.** It is never derived from an Order, Customer, or payment lookup — the MCP `approve_review` tool requires the field explicitly (`{"verified_buyer": true|false}`), forcing a conscious choice rather than defaulting to either value. Chaos does not attempt to automatically match a reviewer to a completed Order; that matching, if a Store wants it, remains a human moderation step outside Chaos, identical in spirit to how this capability worked in the prior integration.

Storefront submission (`POST /store/v1/products/{product_id}/reviews`) requires a Publishable Key and an `Idempotency-Key`, and needs no Shopper credential. A submission always lands `pending` and is invisible to `GET /store/v1/products/{product_id}/reviews` until an administrator approves it. That read endpoint requires the same Publishable Key as every other Store API operation and returns approved top-level reviews newest-first with their approved staff replies nested underneath. ADR 0028 removed the originally introduced Publishable Key scopes.

Review moderation is exposed through MCP tools authenticated with a User-owned Access Key and authorized through current Store membership. Approval and rejection are terminal from `pending` only; a moderation mistake requires a new review, matching other terminal commerce transitions.

## Consequences

- The Storefront client's existing review data shape (id, product_id, parent_id, author_name, author_email, rating, title, content, images, status, is_staff_reply, created_at, updated_at, replies) is preserved field-for-field, with one deliberate addition: **`verified_buyer` is now a real field in the response** rather than a badge the client renders unconditionally. A client that previously assumed every returned review was verified must now read this field and gate the badge on it — a small, intentional change that makes the "Verified Buyer" claim strictly more honest than before, not less.
- Review-photo uploads are out of scope for this release; `images` is always `[]`. Chaos already has a direct-upload Media Asset mechanism (ADR 0018) that a future increment can reuse for review photos rather than building a second upload path.
- MCP tools expose review listing, approval, rejection, and staff replies.
- There is no rate limiting specific to review submission beyond the general request path; a Store that needs abuse resistance beyond `Idempotency-Key` replay protection would need it added as a follow-up, the same gap that exists for every other Storefront write endpoint today.

## Rejected alternatives

### Derive `verified_buyer` automatically from Order history

Matching the reviewer's email or Customer identity against a completed Order would remove the manual step, but it would also silently change what the badge *means* — from "a human checked" to "our matching heuristic didn't find a mismatch," which is a materially weaker and more error-prone claim (a shopper with a different email between checkout and review, or an order placed by someone else in the household, would produce a false positive). Keeping this a deliberate human action preserves the evidence guarantee exactly.

### Allow re-approval or un-approval

A mutable moderation status would let a Store fix mistakes without creating a new review, but it would also mean a review's visible history (and any analytics or exports built on top of it) could silently change after the fact. Terminal moderation, consistent with how Orders and Collections already treat their own terminal transitions, keeps the audit trail (`commerce.review_events`) a true record of what happened rather than a log that needs reconciling against current mutable state.
