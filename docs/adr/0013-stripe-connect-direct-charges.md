# ADR 0013: Stripe Connect Direct Charges

- Status: Superseded
- Date: 2026-08-16
- Superseded by: ADR 0024 and the current Store-owned Provider account model

## Context

An earlier design evaluated Stripe Connect direct charges for Stores. That
model would have made each Store a connected account and would have required
Connect-specific routing, fee ownership, loss liability, and onboarding
decisions.

## Decision

This design is not implemented. The current initial integration uses a direct
Stripe account configured on the Store's typed `stripe` Provider account and
Stripe Embedded Checkout. The checkout request selects the provider enum value;
the backend resolves the Store's configured account and binds its UUID to the
Order. It does not use Stripe Connect, a `Stripe-Account` header, or a
provider-health gate.

Provider credentials and webhook signing secrets remain opaque references
resolved by infrastructure. Webhook signatures are verified against the raw
request before the event is inserted into the durable inbox. Provider health,
Connect onboarding, disputes, and payout operations remain future work.

## Consequences

- Stripe-specific HTTP details remain in the Stripe adapter.
- Orders and payment commands retain the resolved Provider account UUID, so a
  later account configuration change cannot redirect an existing Order.
- If Stripe Connect is introduced later, it must be a deliberate new ADR and
  provider-account model rather than an implicit extension of this design.
