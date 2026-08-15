# ADR 0013: Use Stripe Connect Direct Charges

- Status: Accepted
- Date: 2026-08-16

## Context

The production payment adapter needs an explicit merchant-of-record and funds-flow model. Stripe Connect supports direct charges, destination charges, and separate charges and transfers. Choosing only an HTTP shape without choosing the commercial model would leave fee ownership, refunds, disputes, payouts, and tenant isolation ambiguous.

Chaos is a headless commerce software platform. Each Store merchant sells directly to its shopper and owns its Stripe connected account. That relationship fits direct charges better than a marketplace model in which Chaos receives the shopper funds before transferring them.

## Decision

Use Stripe Connect direct charges for the first production adapter:

- the connected Store merchant is the merchant of record;
- PaymentIntents and Refunds are created on the connected account with the `Stripe-Account` header;
- sale proceeds and payouts belong to the connected account;
- refunds debit the connected account balance;
- the initial model does not collect an application fee;
- live onboarding targets accounts whose Stripe configuration makes the connected account responsible for payment losses and fees;
- onboarding automation must verify capabilities, charge permissions, payout readiness, fee payer, and loss-liability configuration before the Provider account may be enabled for live traffic.

The adapter pins a tested Stripe API version and uses the transactional outbox event ID as Stripe's idempotency key. Payment Attempt and Refund identifiers are sent only as Stripe metadata for webhook correlation. Provider references are stored immediately after a successful command and are immutable.

The PaymentIntent client secret is never persisted. A possession-bound shopper requests a provider-neutral client action after asynchronous dispatch; the adapter retrieves the current PaymentIntent and returns its publishable key, client token, and connected-account reference. These values are returned only to that shopper and must not enter logs, URLs, caches, or analytics.

Provider credentials remain external to PostgreSQL. The current environment-backed secret adapter resolves only references beginning with `env://CHAOS_PAYMENT_SECRET_`. A Stripe credential secret is a JSON object with `secret_key` and `publishable_key`. A webhook reference resolves to the raw Stripe endpoint signing secret. The secret resolver is an application port so Vault, AWS Secrets Manager, or another deployment adapter can replace environment lookup without changing payment use cases.

Stripe webhook verification uses the exact request bytes and accepts a timestamp only within five minutes. The untrusted Connect account identifier selects only an opaque webhook-secret reference. Signature verification completes before merchant and Store context is resolved or the event is inserted into the durable inbox.

## Consequences

- Stripe-specific wire types and authentication remain in infrastructure; sales and payment domain models stay provider-neutral.
- Direct-charge objects are isolated in each connected account and every read or write must retain the connected-account header.
- Chaos does not become the default holder of shopper funds in this model.
- Account configuration can change fee and negative-balance responsibility, so live enablement remains gated on the Phase 8 onboarding verifier rather than inferred from account type.
- Dual-reference credential rotation, onboarding automation, periodic reconciliation, dispute synchronization, and payout visibility remain Phase 8 work.
