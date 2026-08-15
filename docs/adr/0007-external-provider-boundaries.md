# ADR 0007: Isolate Payment, Shipping, and Notification Providers

- Status: Accepted
- Date: 2026-08-15

## Context

Chaos Commerce must integrate with external payment processors, shipping carriers or aggregators, and notification delivery services. Stripe, carrier APIs, and Resend expose different resource models, retry behavior, credentials, webhook formats, and availability characteristics. Allowing those provider types to become domain entities would couple commercial invariants to one vendor and make provider replacement, multi-provider routing, testing, and failure recovery unnecessarily expensive.

The three capabilities do not have identical business ownership:

- payment authorization, capture, and refund change financial state;
- fulfillment allocation, shipment, delivery, and return change commerce state;
- email, SMS, and push delivery communicate state but do not own the underlying business decision.

## Decision

Provider integrations follow the existing dependency direction:

```text
HTTP / webhook / worker adapters
              |
application use cases and provider ports
              |
domain state machines and semantic events
              ^
infrastructure provider adapters
```

Provider SDK types, error types, event names, credentials, and payloads remain in `chaos-infrastructure`. Domain and application packages use provider-neutral commands, results, identifiers, and errors. Provider selection is resolved from Store-owned configuration before a command is dispatched.

### Payments

`payments` remains a bounded context because Payment Attempts, captures, Refunds, settlement currency, and reconciliation are business records. A Stripe adapter implements payment application ports; Stripe does not become a domain module.

The adapter maps provider-neutral commands to Stripe Payment Intents and Refunds, supplies a stable idempotency key derived from the Chaos operation or outbox event, and maps provider outcomes into the existing payment state machines. Raw Stripe webhook bodies are verified before parsing or tenant resolution, stored in the durable inbox, deduplicated by provider event identity, and processed without assuming event order.

Stripe Connect is a separate product decision, not an adapter detail. Before implementing onboarding, the product must decide the merchant of record, charge flow, fee payer, negative-balance liability, dispute ownership, and payout model. Store configuration records only external account references and capability state. API keys and webhook secrets live in a secret manager; PostgreSQL stores secret references and rotation metadata, never recoverable credentials.

### Shipping and logistics

`fulfillment` remains the business bounded context. It owns allocations, Fulfillments, shipments, delivery, Returns, and the rules that constrain quantities and state transitions. An application `ShippingProvider` port may provide capabilities such as rate quotation, label purchase, shipment cancellation, and tracking refresh.

Carrier names, service codes, label formats, customs payloads, tracking event names, and provider errors remain adapter concerns. Purchased label and tracking snapshots are persisted as fulfillment evidence after a successful provider command. Provider callbacks enter a signed, deduplicated inbox and advance fulfillment state only through application use cases.

A separate `logistics` bounded context is deferred. It becomes justified only when Chaos owns multi-leg routing, warehouse selection, split shipment planning, cross-dock or transfer workflows, or carrier procurement independently of one Order Fulfillment. Until then, a new top-level context would duplicate Fulfillment ownership.

### Notifications

Notifications are an integration capability, not the source of truth for authentication, Orders, Payments, or Fulfillments. Domain and application workflows emit semantic events such as `identity.sign_in_link_requested`, `order.confirmed`, `fulfillment.shipped`, or `refund.succeeded`. Notification policy decides whether an event produces email, SMS, push, or no delivery.

The notification application boundary owns delivery requests, templates and template versions, recipient policy, suppression state, and delivery status. An `EmailProvider` port is implemented by a Resend adapter. Ordinary business transactions write notification requests or semantic events atomically to an outbox; notification workers render an approved template and send with a stable provider idempotency key.

Authentication email is a dedicated security flow. A raw sign-in token may be passed directly to the provider adapter while it is in process memory, or referenced through encrypted short-lived storage if asynchronous delivery is later required. The general notification outbox contains only a non-secret semantic reference and never persists a reusable sign-in token in plaintext.

Resend webhook requests are verified from the raw body, deduplicated by delivery identity, and may update delivery status or suppression records. Delivery, bounce, and complaint events never reverse the business transaction that requested the message. Authentication secrets and one-time links must not be written to logs, metrics, general event payloads, or reusable templates.

### Reliability and worker ownership

Each worker claims only event types owned by its consumer. Claim leases have an expiry and can be recovered after process termination. A worker stops accepting claims during shutdown, completes or safely releases in-flight work, and never relies on process-local ownership for correctness.

External calls use both Chaos-side deduplication and provider idempotency where available. Webhook handlers acknowledge only after authenticated durable receipt, return quickly, tolerate duplicates and out-of-order delivery, and keep provider API versions explicit. Retry policy distinguishes transient failures from permanent validation or configuration failures. Dead-letter replay is audited and does not mutate the original payload.

## Consequences

- Stripe, Resend, and carrier integrations can evolve without changing core domain types or public API contracts.
- Payment and Fulfillment retain their business invariants instead of becoming thin wrappers around providers.
- Notification failures do not roll back successful commerce transactions.
- Provider onboarding and secret rotation require explicit Store administration workflows.
- More mapping code and integration contract tests are required at each adapter boundary.
- A dedicated logistics context may still be introduced later when routing and warehouse orchestration become independent business capabilities.

## Provider constraints

- Stripe recommends idempotency keys for safely retrying writes and documents signed raw-body webhooks, retries, duplicate delivery, and unordered events: [idempotent requests](https://docs.stripe.com/api/idempotent_requests) and [webhooks](https://docs.stripe.com/webhooks).
- Stripe Connect configuration determines account responsibilities and funds flow, so it requires a product decision before implementation: [Connect](https://docs.stripe.com/connect).
- Resend supports send idempotency keys and documents signed, at-least-once, unordered webhooks: [idempotency keys](https://resend.com/docs/dashboard/emails/idempotency-keys), [webhooks](https://resend.com/docs/webhooks/introduction), and [webhook verification](https://resend.com/docs/webhooks/verify-webhooks-requests).

## Rejected alternatives

### Put Stripe, Resend, and carrier SDK types in application services

This reverses the dependency boundary and makes use cases, tests, and persistence contracts vendor-specific.

### Create one generic provider interface

Payments, shipping, and notifications have different commands, invariants, failure semantics, and security requirements. A lowest-common-denominator interface would hide important behavior and encourage invalid cross-capability abstractions.

### Create a logistics bounded context immediately

The current system owns Order Fulfillments and Returns, not an independent transportation network. A provider port inside the Fulfillment boundary is sufficient until routing and warehouse orchestration acquire their own lifecycle and invariants.
