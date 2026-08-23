# ADR 0007: Isolate Payment and Shipping Providers

- Status: Accepted
- Date: 2026-08-15

## Context

Chaos Commerce must integrate with external payment processors and shipping carriers or aggregators. Stripe and carrier APIs expose different resource models, retry behavior, credentials, webhook formats, and availability characteristics. Allowing those provider types to become domain entities would couple commercial invariants to one vendor and make provider replacement, multi-provider routing, testing, and failure recovery unnecessarily expensive. Notification delivery is intentionally deferred; the former Resend design was removed by the integration slimming migration.

The two capabilities do not have identical business ownership:

- payment authorization, capture, and refund change financial state;
- fulfillment allocation, shipment, delivery, and return change commerce state;
- payment and fulfillment have different state ownership even though both depend on external providers.

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

The initial adapter maps provider-neutral payment commands to Stripe Embedded Checkout Sessions, supplies the Order request UUID as Stripe's provider idempotency key, and maps Checkout Session outcomes into the existing payment state machines. Checkout creation is synchronous because the browser needs the Session client secret immediately; refunds and webhook processing remain durable Worker jobs. Raw Stripe webhook bodies are verified against the exact endpoint account before Store resolution, stored in the durable inbox, deduplicated by Provider Account and provider event identity, and processed without assuming event order. Refund-provider support is a later payment-integration increment.

Stripe Connect is not supported by the initial adapter. Each Store configures the direct Stripe account that owns its API keys; the Chaos Provider Account UUID, not a Stripe account label, routes webhooks. Provider credentials are stored only as opaque encrypted references; PostgreSQL never stores recoverable plaintext credentials.

### Shipping and logistics

`fulfillment` remains the business bounded context. It owns allocations, Fulfillments, shipments, delivery, Returns, and the rules that constrain quantities and state transitions. An application `ShippingProvider` port may provide capabilities such as rate quotation, label purchase, shipment cancellation, and tracking refresh.

Store-configured manual Shipping Services are the provider-neutral baseline. Fulfillment owns their service identity, rate, settlement currency, destination countries, lifecycle, and delivery estimate. Storefront quotation reads this capability through an application port. Sales accepts only a service identifier, revalidates it against the Cart currency and shipping destination in the Checkout transaction, and freezes the selected values. This baseline does not impersonate a carrier quote and remains usable when no external provider is configured.

Carrier names, service codes, label formats, customs payloads, tracking event names, and provider errors remain adapter concerns. Purchased label and tracking snapshots are persisted as fulfillment evidence after a successful provider command. Provider callbacks enter a signed, deduplicated inbox and advance fulfillment state only through application use cases.

A separate `logistics` bounded context is deferred. It becomes justified only when Chaos owns multi-leg routing, warehouse selection, split shipment planning, cross-dock or transfer workflows, or carrier procurement independently of one Order Fulfillment. Until then, a new top-level context would duplicate Fulfillment ownership.

### Reliability and worker ownership

Each worker claims only event types owned by its consumer. Claim leases have an expiry and can be recovered after process termination. A worker stops accepting claims during shutdown, completes or safely releases in-flight work, and never relies on process-local ownership for correctness.

Checkout uses the Order request UUID for the database uniqueness boundary and as Stripe's provider idempotency key. Other external calls rely on their durable business state and provider keys where available. Webhook handlers acknowledge only after authenticated durable receipt, return quickly, tolerate duplicates and out-of-order delivery, and keep provider API versions explicit. Retry policy distinguishes transient failures from permanent validation or configuration failures. Dead-letter replay is audited and does not mutate the original payload.

## Consequences

- Stripe and carrier integrations can evolve without changing core domain types or public API contracts.
- Payment and Fulfillment retain their business invariants instead of becoming thin wrappers around providers.
- Provider onboarding and secret rotation require explicit Store administration workflows.
- More mapping code and integration contract tests are required at each adapter boundary.
- A dedicated logistics context may still be introduced later when routing and warehouse orchestration become independent business capabilities.

## Provider constraints

- Stripe recommends idempotency keys for safely retrying writes and documents signed raw-body webhooks, retries, duplicate delivery, and unordered events: [idempotent requests](https://docs.stripe.com/api/idempotent_requests) and [webhooks](https://docs.stripe.com/webhooks).
- Stripe Connect configuration determines account responsibilities and funds flow, so it requires a product decision before implementation: [Connect](https://docs.stripe.com/connect).

## Rejected alternatives

### Put Stripe, Resend, and carrier SDK types in application services

This reverses the dependency boundary and makes use cases, tests, and persistence contracts vendor-specific.

### Create one generic provider interface

Payments and shipping have different commands, invariants, failure semantics, and security requirements. A lowest-common-denominator interface would hide important behavior and encourage invalid cross-capability abstractions. The later analytics destination abstraction is intentionally scoped to analytics-event delivery and is not a cross-capability provider interface.

### Create a logistics bounded context immediately

The current system owns Order Fulfillments and Returns, not an independent transportation network. A provider port inside the Fulfillment boundary is sufficient until routing and warehouse orchestration acquire their own lifecycle and invariants.
