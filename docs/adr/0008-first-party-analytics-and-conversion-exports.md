# ADR 0008: Own First-Party Analytics and Isolate Conversion Exports

- Status: Accepted
- Date: 2026-08-15

## Context

Merchants need product and funnel analytics such as page views, product views, searches, active browsing time, Cart activity, Checkout conversion, payment success, Refunds, and campaign attribution. They may also export eligible conversion events to destinations such as Meta Conversions API or Google Analytics Measurement Protocol.

Browser behavior and commerce facts have different trust levels. A browser can report visibility and interaction but cannot authoritatively declare an Order amount or successful Payment. Server-side commerce events are reliable business facts but cannot reconstruct page focus, campaign parameters, or anonymous browsing by themselves. External analytics products also have provider-specific schemas, identifiers, credentials, privacy controls, retry behavior, and retention.

## Decision

Chaos owns a provider-neutral `analytics` boundary. It owns the first-party event taxonomy, schema versions, collection validation, consent snapshot, anonymous and authenticated identity references, sessionization, attribution inputs, retention policy, derived aggregates, and export delivery state.

Analytics never owns Product, Order, Payment, Customer, or Inventory truth. It consumes identifiers and immutable fact snapshots from their owning contexts. Analytics ingestion and export are asynchronous and cannot make Checkout, Payment, Fulfillment, or authentication fail.

```text
Storefront SDK                    Commerce transaction
     |                                     |
behavior collection                    outbox fact
     +----------------+--------------------+
                      |
             canonical event stream
                      |
        +-------------+--------------+
        |                            |
first-party aggregates       conversion destinations
                                 Meta CAPI / GA4
```

### Canonical event envelope

Every event has a stable `event_id`, `event_name`, `schema_version`, `source`, `occurred_at`, `received_at`, merchant account, Store, optional Sales Channel, and a typed property object. Optional identity references include an opaque `anonymous_id`, `session_id`, and internal Customer ID. Events also carry the consent and collection-policy version applied when they were accepted.

Event names describe business meaning rather than a destination schema. Initial browser events include `page_viewed`, `product_viewed`, `search_performed`, `cart_line_added`, `checkout_started`, and engagement heartbeats. Trusted server events include `order_created`, `payment_captured`, `refund_succeeded`, `fulfillment_shipped`, and `return_completed`.

Client events are untrusted input. The collection endpoint enforces an allowlisted schema, bounded body and batch sizes, timestamp skew, Store context, rate limits, and property limits. It rejects arbitrary event names and never accepts authoritative amount, currency, Payment status, or Order status from a browser.

### Engagement time

Browsing duration represents active engagement, not the difference between page load and tab close. The Storefront SDK accumulates bounded intervals only while the document is visible and focused, reports periodic heartbeats, and flushes a final interval when possible. Missing final events are expected. The server validates non-negative intervals, caps each interval and session total, and treats the result as an estimate rather than a financial or authorization fact.

### Identity, consent, and privacy

Anonymous behavior uses randomly generated opaque identifiers. Linking anonymous activity to a Customer is an explicit event after authentication or Checkout identification and follows Store policy. Raw email addresses, phone numbers, access tokens, addresses, free-form request bodies, and payment data are excluded from the general analytics event log.

Collection, storage, identity linking, advertising export, and retention are separate policy decisions. The Storefront supplies the applicable consent state from its consent-management flow; the server records the policy snapshot and independently enforces the Store's enabled purposes and destinations. Consent revocation stops future ineligible exports. Retention expiry and data-subject deletion operate by internal identity references across raw events, aggregates, and pending exports.

Jurisdiction-specific consent text and legal basis remain merchant and product-policy responsibilities. The architecture provides enforcement points and evidence; it does not infer permission from the presence of an identifier or from use of a server-side API.

### Attribution

Eligible browser events may record bounded campaign parameters, referrer class, landing page, and provider click identifiers. Attribution models consume those inputs and produce versioned first-touch, last-touch, or other derived results. An Order may snapshot the selected attribution reference, but campaign input never changes its commercial totals or lifecycle.

### Conversion destinations

Application ports use destination-neutral batches and results. Adapters map canonical events to Meta CAPI, GA4 Measurement Protocol, or future destinations. Destination SDK types and parameter names remain in infrastructure.

The same stable `event_id` is used when equivalent browser and server events are sent to a destination that supports deduplication. Trusted server facts supply value, currency, Order identity, and status. Destination-specific customer matching fields are minimized, normalized, and transformed only inside the adapter after policy authorization. Destination credentials live in a secret manager, while PostgreSQL stores configuration references, checkpoints, and auditable delivery state.

Exports use retry with bounded backoff, recoverable leases, dead letters, and per-destination rate limits. A destination acknowledgement means only that delivery was accepted; it does not rewrite the canonical event or commerce aggregate. Backfills replay immutable canonical events through a versioned mapping and record the mapping version used.

### Storage and query isolation

OLTP PostgreSQL stores Store configuration, consent policy versions, durable commerce outbox facts, export checkpoints, and delivery state. High-volume behavior events and analytical scans use a replaceable analytics store or warehouse so reporting cannot exhaust the transactional database pool. The initial storage choice requires measured volume and query evidence; the application boundary must not expose a vendor-specific warehouse model.

Aggregates such as sessions, product views, funnel steps, conversion rates, revenue, Refunds, and cohort summaries are rebuildable read models. Financial dashboards reconcile trusted server events against the owning Payments and Sales records rather than summing browser events.

## Consequences

- Chaos retains a stable first-party event history while Meta, Google, or warehouse adapters can change independently.
- Browser engagement and server conversion facts can be joined without granting browser events business authority.
- Consent, retention, deletion, and destination eligibility are explicit and auditable.
- Analytics outages and advertising API failures do not block commerce transactions.
- Event schemas, identity linking, sessionization, and attribution require versioning and contract tests.
- A separate analytical store adds operational cost once event volume justifies it.

## Provider constraints

- Meta describes Conversions API as a server connection for website, app, offline, and messaging events and states that it does not bypass privacy or platform policies: [About Conversions API](https://www.facebook.com/business/help/AboutConversionsAPI).
- Google describes Measurement Protocol as a supplement to client tagging for server-side and offline interactions, with `client_id`, `session_id`, privacy settings, and engagement time participating in reporting: [Measurement Protocol](https://developers.google.com/analytics/devguides/collection/protocol/ga4) and [reference](https://developers.google.com/analytics/devguides/collection/protocol/ga4/reference).

## Rejected alternatives

### Send provider events directly from business use cases

This couples transaction latency and availability to advertising systems, spreads consent logic across domains, and prevents consistent replay or destination replacement.

### Treat browser purchase events as authoritative

Browser payloads are user-controlled and can be blocked, duplicated, altered, or forged. Only server-side Sales and Payments state can authoritatively report commercial outcomes.

### Store only destination-specific payloads

Provider payloads lose first-party semantics and make historical reprocessing depend on a vendor schema. Canonical immutable events preserve an internal contract and allow versioned remapping.

### Run analytical scans on OLTP tables

Large event scans and ad hoc aggregations compete with Checkout, Inventory, and Payment transactions. Analytical query load requires an isolated read model and, at sufficient volume, a dedicated store.
