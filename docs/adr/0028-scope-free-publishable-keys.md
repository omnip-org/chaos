# ADR 0028: Keep Publishable Keys Scope-Free

- Status: Accepted
- Date: 2026-08-20

## Context

A Publishable Key is deliberately embedded in an untrusted Storefront client. The
previous model assigned endpoint scopes such as `catalog:read`, `checkout:write`,
and `analytics:write` to that public credential. Those scopes did not establish a
meaningful trust boundary, but they multiplied configuration, persistence,
authentication, transport, and testing states.

Sensitive operations already require stronger possession or authority: Shopper
credentials bind carts, Orders, and Payment Attempts; guest tracking capabilities bind
Order tracking; verified Provider webhooks confirm payments; and User Access Keys
plus current Store membership authorize MCP administration.

## Decision

A Publishable Key identifies one Store and resolves one active Sales Channel. Every
active key may enter the complete Store API. Storefront operations continue to
enforce resource ownership, Shopper or tracking credentials, idempotency, business
state, validation, and rate limits where applicable.

Publishable Keys have no scope enum, scope collection, or configurable scope input.
The Storefront SDK exposes the API-key authentication model without OAuth-style
scope values.

## Consequences

Storefront setup and key rotation have one valid configuration instead of many
partially functional combinations. Revocation, Store status, Sales Channel status,
and operation-specific credentials remain the effective authorization boundaries.
Future privileged browser capabilities must introduce a real possession or identity
boundary rather than adding a label to a public key.
