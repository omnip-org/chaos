# ADR 0016: Resolve Store Context Only from Verified Domain Bindings

- Status: Accepted
- Date: 2026-08-16

## Context

A custom hostname is public routing input, not authentication. Resolving it through tenant-scoped tables before a merchant context exists requires a deliberately narrow cross-tenant boundary. Treating `Host`, `Forwarded`, DNS answers, or a client-supplied Store identifier as authority could select the wrong Store or Sales Channel.

## Decision

Each active hostname binds exactly one Store and one active Web Sales Channel. Creation is owner/administrator-only and issues a random 256-bit DNS TXT challenge exactly once. PostgreSQL stores only its SHA-256 digest. Verification queries `_chaos-verification.<hostname>` through a timeout-bounded DNS resolver and activates the binding only when a TXT value contains the matching `chaos-domain-verification=<token>` proof.

Domain lifecycle is `pending`, `verified`, then `archived`. Every transition appends an immutable Store-owned event. Archiving immediately removes the hostname from resolution and permits a later owner to prove and claim it again through a new row and challenge.

The pre-authentication resolver is a narrowly granted `SECURITY DEFINER` function. It returns a context only when the hostname is verified and the Merchant Account, Store, and bound Web Sales Channel are all active. It performs an exact canonical-hostname lookup and cannot accept a Store or account identifier. Runtime RLS remains mandatory for administration and audit events.

The public Store endpoint reads the direct HTTP `Host` authority and does not trust forwarding headers. It is a bootstrap lookup, not a commerce credential. Publishable keys and shopper or Customer possession credentials remain required by their existing endpoints. Any future gateway that combines domain and key resolution must require exact equality between both contexts; a domain may narrow an authenticated context but can never broaden it.

## Consequences

- DNS verification avoids fetching merchant-controlled URLs and therefore does not create an HTTP SSRF primitive.
- Unicode domains must be supplied in canonical ASCII/Punycode form.
- TLS certificate issuance and edge routing are separate deployment concerns; verification alone does not mutate gateway configuration.
- DNS propagation failures remain safely pending and may be retried with the same verification operation.
